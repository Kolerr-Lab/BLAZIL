package billing

import (
	"context"
	"time"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"
)

// Store persists and retrieves invoices and Stripe customer references.
type Store interface {
	// CreateInvoice atomically persists a draft invoice with its line items.
	CreateInvoice(ctx context.Context, inv Invoice) (*Invoice, error)

	// GetInvoice returns an invoice by ID including its line items.
	// Returns pgx.ErrNoRows if not found.
	GetInvoice(ctx context.Context, id string) (*Invoice, error)

	// GetInvoiceByStripeID looks up an invoice by its Stripe invoice ID.
	// Returns pgx.ErrNoRows if not found.
	GetInvoiceByStripeID(ctx context.Context, stripeInvoiceID string) (*Invoice, error)

	// ListInvoices returns all invoices for a tenant, ordered by period_start DESC.
	// Line items are NOT populated (use GetInvoice for detail).
	ListInvoices(ctx context.Context, tenantID string) ([]Invoice, error)

	// UpdateStatus transitions an invoice's status.
	// paidAt should be non-nil iff transitioning to StatusPaid.
	UpdateStatus(ctx context.Context, id string, status InvoiceStatus, paidAt *time.Time) error

	// SetStripeCustomerID stores the Stripe customer ID on the tenant record.
	SetStripeCustomerID(ctx context.Context, tenantID, customerID string) error

	// GetStripeCustomerID returns the Stripe customer ID for a tenant.
	// Returns "" (not an error) if the tenant has not been provisioned yet.
	GetStripeCustomerID(ctx context.Context, tenantID string) (string, error)
}

// pgStore is a PostgreSQL-backed Store.
type pgStore struct {
	db *pgxpool.Pool
}

// NewStore returns a Store backed by the provided pgxpool connection pool.
func NewStore(db *pgxpool.Pool) Store {
	return &pgStore{db: db}
}

// ── CreateInvoice ─────────────────────────────────────────────────────────────

func (s *pgStore) CreateInvoice(ctx context.Context, inv Invoice) (*Invoice, error) {
	tx, err := s.db.Begin(ctx)
	if err != nil {
		return nil, err
	}
	defer tx.Rollback(ctx) //nolint:errcheck

	const insertInvoice = `
		INSERT INTO invoices
			(tenant_id, stripe_invoice_id, period_start, period_end, total_micro_usd, status)
		VALUES ($1, NULLIF($2,''), $3, $4, $5, $6)
		RETURNING id, created_at`

	var id string
	var createdAt time.Time
	if err := tx.QueryRow(ctx, insertInvoice,
		inv.TenantID,
		inv.StripeInvoiceID,
		inv.PeriodStart,
		inv.PeriodEnd,
		inv.TotalMicroUSD,
		string(inv.Status),
	).Scan(&id, &createdAt); err != nil {
		return nil, err
	}

	const insertLine = `
		INSERT INTO invoice_line_items
			(invoice_id, window_start, tx_count, price_per_tx_micro, total_micro_usd)
		VALUES ($1, $2, $3, $4, $5)
		RETURNING id`

	for i := range inv.Lines {
		if err := tx.QueryRow(ctx, insertLine,
			id,
			inv.Lines[i].WindowStart,
			inv.Lines[i].TxCount,
			inv.Lines[i].PricePerTxMicro,
			inv.Lines[i].TotalMicroUSD,
		).Scan(&inv.Lines[i].ID); err != nil {
			return nil, err
		}
		inv.Lines[i].InvoiceID = id
	}

	if err := tx.Commit(ctx); err != nil {
		return nil, err
	}

	inv.ID = id
	inv.CreatedAt = createdAt
	return &inv, nil
}

// ── GetInvoice ────────────────────────────────────────────────────────────────

func (s *pgStore) GetInvoice(ctx context.Context, id string) (*Invoice, error) {
	const q = `
		SELECT id, tenant_id, COALESCE(stripe_invoice_id,''),
		       period_start, period_end, total_micro_usd, status, created_at, paid_at
		FROM invoices
		WHERE id = $1`

	inv := &Invoice{}
	var statusStr string
	if err := s.db.QueryRow(ctx, q, id).Scan(
		&inv.ID, &inv.TenantID, &inv.StripeInvoiceID,
		&inv.PeriodStart, &inv.PeriodEnd, &inv.TotalMicroUSD,
		&statusStr, &inv.CreatedAt, &inv.PaidAt,
	); err != nil {
		return nil, err
	}
	inv.Status = InvoiceStatus(statusStr)

	lines, err := s.listLineItems(ctx, id)
	if err != nil {
		return nil, err
	}
	inv.Lines = lines
	return inv, nil
}

// ── GetInvoiceByStripeID ──────────────────────────────────────────────────────

func (s *pgStore) GetInvoiceByStripeID(ctx context.Context, stripeInvoiceID string) (*Invoice, error) {
	const q = `SELECT id FROM invoices WHERE stripe_invoice_id = $1`
	var id string
	if err := s.db.QueryRow(ctx, q, stripeInvoiceID).Scan(&id); err != nil {
		return nil, err
	}
	return s.GetInvoice(ctx, id)
}

// ── ListInvoices ──────────────────────────────────────────────────────────────

func (s *pgStore) ListInvoices(ctx context.Context, tenantID string) ([]Invoice, error) {
	const q = `
		SELECT id, tenant_id, COALESCE(stripe_invoice_id,''),
		       period_start, period_end, total_micro_usd, status, created_at, paid_at
		FROM invoices
		WHERE tenant_id = $1
		ORDER BY period_start DESC`

	rows, err := s.db.Query(ctx, q, tenantID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var result []Invoice
	for rows.Next() {
		var inv Invoice
		var statusStr string
		if err := rows.Scan(
			&inv.ID, &inv.TenantID, &inv.StripeInvoiceID,
			&inv.PeriodStart, &inv.PeriodEnd, &inv.TotalMicroUSD,
			&statusStr, &inv.CreatedAt, &inv.PaidAt,
		); err != nil {
			return nil, err
		}
		inv.Status = InvoiceStatus(statusStr)
		result = append(result, inv)
	}
	return result, rows.Err()
}

// ── UpdateStatus ──────────────────────────────────────────────────────────────

func (s *pgStore) UpdateStatus(ctx context.Context, id string, status InvoiceStatus, paidAt *time.Time) error {
	const q = `UPDATE invoices SET status = $1, paid_at = $2 WHERE id = $3`
	_, err := s.db.Exec(ctx, q, string(status), paidAt, id)
	return err
}

// ── Stripe customer ID ────────────────────────────────────────────────────────

func (s *pgStore) SetStripeCustomerID(ctx context.Context, tenantID, customerID string) error {
	const q = `UPDATE tenants SET stripe_customer_id = $1 WHERE id = $2`
	_, err := s.db.Exec(ctx, q, customerID, tenantID)
	return err
}

func (s *pgStore) GetStripeCustomerID(ctx context.Context, tenantID string) (string, error) {
	const q = `SELECT COALESCE(stripe_customer_id, '') FROM tenants WHERE id = $1`
	var id string
	if err := s.db.QueryRow(ctx, q, tenantID).Scan(&id); err != nil {
		if err == pgx.ErrNoRows {
			return "", nil
		}
		return "", err
	}
	return id, nil
}

// ── listLineItems (internal) ──────────────────────────────────────────────────

func (s *pgStore) listLineItems(ctx context.Context, invoiceID string) ([]LineItem, error) {
	const q = `
		SELECT id, invoice_id, window_start, tx_count, price_per_tx_micro, total_micro_usd
		FROM invoice_line_items
		WHERE invoice_id = $1
		ORDER BY window_start ASC`

	rows, err := s.db.Query(ctx, q, invoiceID)
	if err != nil {
		return nil, err
	}
	defer rows.Close()

	var items []LineItem
	for rows.Next() {
		var item LineItem
		if err := rows.Scan(
			&item.ID, &item.InvoiceID, &item.WindowStart,
			&item.TxCount, &item.PricePerTxMicro, &item.TotalMicroUSD,
		); err != nil {
			return nil, err
		}
		items = append(items, item)
	}
	return items, rows.Err()
}

// compile-time assertion: pgStore must satisfy Store.
var _ Store = (*pgStore)(nil)
