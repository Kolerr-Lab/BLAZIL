// Package db provides database utilities for the Blazil payments service.
package db

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"

	"github.com/jackc/pgx/v5"
	"github.com/jackc/pgx/v5/pgxpool"

	"github.com/blazil/services/payments/internal/domain"
)

// PostgresPaymentStore is a pgxpool-backed implementation of lifecycle.PaymentStore.
// All methods are safe for concurrent use.
type PostgresPaymentStore struct {
	pool *pgxpool.Pool
}

// NewPostgresPaymentStore constructs a store backed by the given pool.
func NewPostgresPaymentStore(pool *pgxpool.Pool) *PostgresPaymentStore {
	return &PostgresPaymentStore{pool: pool}
}

// GetByID implements lifecycle.PaymentStore.
func (s *PostgresPaymentStore) GetByID(id domain.PaymentID) (*domain.Payment, error) {
	const q = `
		SELECT id, idempotency_key, debit_account_id, credit_account_id,
		       amount_minor_units, currency_code, currency_numeric, currency_decimals,
		       ledger_id, rails, status, failure_reason, metadata, created_at, updated_at
		FROM payments
		WHERE id = $1`
	row := s.pool.QueryRow(context.Background(), q, string(id))
	p, err := scanPayment(row)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, fmt.Errorf("payment %s: %w", id, domain.ErrPaymentNotFound)
		}
		return nil, fmt.Errorf("get payment by id: %w", err)
	}
	return p, nil
}

// GetByIdempotencyKey implements lifecycle.PaymentStore.
// Returns (nil, nil) when the key is not found — a missing key is not an error.
func (s *PostgresPaymentStore) GetByIdempotencyKey(key string) (*domain.Payment, error) {
	const q = `
		SELECT id, idempotency_key, debit_account_id, credit_account_id,
		       amount_minor_units, currency_code, currency_numeric, currency_decimals,
		       ledger_id, rails, status, failure_reason, metadata, created_at, updated_at
		FROM payments
		WHERE idempotency_key = $1`
	row := s.pool.QueryRow(context.Background(), q, key)
	p, err := scanPayment(row)
	if err != nil {
		if errors.Is(err, pgx.ErrNoRows) {
			return nil, nil
		}
		return nil, fmt.Errorf("get payment by idempotency key: %w", err)
	}
	return p, nil
}

// Save implements lifecycle.PaymentStore.
// Performs an upsert on the primary key.  On conflict, only mutable fields
// (status, failure_reason, metadata, updated_at) are overwritten.
func (s *PostgresPaymentStore) Save(payment *domain.Payment) error {
	metaJSON, err := json.Marshal(payment.Metadata)
	if err != nil {
		return fmt.Errorf("marshal payment metadata: %w", err)
	}
	const q = `
		INSERT INTO payments (
			id, idempotency_key,
			debit_account_id, credit_account_id,
			amount_minor_units,
			currency_code, currency_numeric, currency_decimals,
			ledger_id, rails,
			status, failure_reason, metadata,
			created_at, updated_at
		) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)
		ON CONFLICT (id) DO UPDATE SET
			status         = EXCLUDED.status,
			failure_reason = EXCLUDED.failure_reason,
			metadata       = EXCLUDED.metadata,
			updated_at     = EXCLUDED.updated_at`
	_, err = s.pool.Exec(context.Background(), q,
		string(payment.ID),
		payment.IdempotencyKey,
		string(payment.DebitAccountID),
		string(payment.CreditAccountID),
		payment.Amount.MinorUnits,
		payment.Amount.Currency.Code,
		int32(payment.Amount.Currency.Numeric),
		int32(payment.Amount.Currency.Decimals),
		int32(payment.LedgerID),
		int16(payment.Rails),
		int16(payment.Status),
		payment.FailureReason,
		metaJSON,
		payment.CreatedAt,
		payment.UpdatedAt,
	)
	if err != nil {
		return fmt.Errorf("save payment: %w", err)
	}
	return nil
}

// scanPayment reads a single payment row into a domain.Payment.
func scanPayment(row pgx.Row) (*domain.Payment, error) {
	var (
		p            domain.Payment
		id           string
		debitAccID   string
		creditAccID  string
		currCode     string
		currNumeric  int32
		currDecimals int32
		ledgerID     int32
		rails        int16
		status       int16
		metaJSON     []byte
	)
	err := row.Scan(
		&id, &p.IdempotencyKey,
		&debitAccID, &creditAccID,
		&p.Amount.MinorUnits,
		&currCode, &currNumeric, &currDecimals,
		&ledgerID, &rails,
		&status, &p.FailureReason, &metaJSON,
		&p.CreatedAt, &p.UpdatedAt,
	)
	if err != nil {
		return nil, err
	}

	p.ID = domain.PaymentID(id)
	p.DebitAccountID = domain.AccountID(debitAccID)
	p.CreditAccountID = domain.AccountID(creditAccID)
	p.Amount.Currency = domain.Currency{
		Code:     currCode,
		Numeric:  uint16(currNumeric),
		Decimals: uint8(currDecimals),
	}
	p.LedgerID = domain.LedgerID(ledgerID)
	p.Rails = domain.PaymentRails(rails)
	p.Status = domain.PaymentStatus(status)

	if len(metaJSON) > 0 && string(metaJSON) != "null" {
		if err := json.Unmarshal(metaJSON, &p.Metadata); err != nil {
			return nil, fmt.Errorf("unmarshal payment metadata: %w", err)
		}
	}
	return &p, nil
}
