// Package admin provides the internal control-plane HTTP API.
//
// All endpoints require a static bearer token (BLAZIL_ADMIN_TOKEN) validated
// with subtle.ConstantTimeCompare to prevent timing-based token oracle attacks.
//
// This interface is NOT customer-facing. It is intended for use by internal
// tooling, the Blazil operations team, and automated provisioning scripts.
// It MUST NOT be exposed on the public internet — bind it to an internal
// address or protect it with a network policy / VPN.
//
// Endpoints:
//
//	POST   /v1/admin/tenants                      — Create tenant
//	GET    /v1/admin/tenants                      — List tenants (paginated)
//	GET    /v1/admin/tenants/{id}                 — Get tenant
//	POST   /v1/admin/tenants/{id}/suspend         — Suspend tenant
//	DELETE /v1/admin/tenants/{id}/suspend         — Unsuspend tenant
//	POST   /v1/admin/tenants/{id}/api-keys        — Issue API key (returns raw once)
//	GET    /v1/admin/tenants/{id}/api-keys        — List API keys
//	DELETE /v1/admin/api-keys/{keyID}             — Revoke API key
//	GET    /v1/admin/tenants/{id}/usage           — Current-month windowed usage
//	GET    /v1/admin/tenants/{id}/invoice         — Billing preview invoice
//	GET    /health                                — Liveness probe
//	GET    /ready                                 — Readiness probe (DB ping)
package admin

import (
	"crypto/subtle"
	"encoding/json"
	"net/http"
	"strings"
	"time"

	"github.com/blazil/metering"
	"github.com/blazil/services/gateway/internal/tenant"
	"github.com/go-chi/chi/v5"
	"go.uber.org/zap"
)

// Handler is the admin control-plane HTTP handler.
type Handler struct {
	store      tenant.Store
	writer     metering.UsageWriter
	recorder   metering.Recorder
	adminToken []byte // pre-computed for subtle comparison
	logger     *zap.Logger
}

// New creates a Handler. adminToken must be non-empty; if it is, every request
// will receive 503 Service Unavailable.
func New(
	store tenant.Store,
	writer metering.UsageWriter,
	recorder metering.Recorder,
	adminToken string,
	logger *zap.Logger,
) *Handler {
	return &Handler{
		store:      store,
		writer:     writer,
		recorder:   recorder,
		adminToken: []byte(adminToken),
		logger:     logger,
	}
}

// Routes returns the chi.Router with all endpoints registered.
func (h *Handler) Routes() chi.Router {
	r := chi.NewRouter()
	r.Use(h.authenticate)

	r.Get("/health", h.health)
	r.Get("/ready", h.ready)

	r.Route("/v1/admin", func(r chi.Router) {
		r.Post("/tenants", h.createTenant)
		r.Get("/tenants", h.listTenants)

		r.Route("/tenants/{id}", func(r chi.Router) {
			r.Get("/", h.getTenant)
			r.Post("/suspend", h.suspendTenant)
			r.Delete("/suspend", h.unsuspendTenant)
			r.Post("/api-keys", h.issueAPIKey)
			r.Get("/api-keys", h.listAPIKeys)
			r.Get("/usage", h.getUsage)
			r.Get("/invoice", h.getInvoice)
		})

		r.Delete("/api-keys/{keyID}", h.revokeAPIKey)
	})

	return r
}

// ── Middleware ─────────────────────────────────────────────────────────────

// authenticate validates the Bearer token on every request using
// constant-time comparison to prevent timing-based oracle attacks.
func (h *Handler) authenticate(next http.Handler) http.Handler {
	return http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		// Skip auth for liveness/readiness probes so Kubernetes can reach them
		// without a token even when the token source is unavailable.
		if r.URL.Path == "/health" || r.URL.Path == "/ready" {
			next.ServeHTTP(w, r)
			return
		}

		if len(h.adminToken) == 0 {
			// Admin token not configured — refuse all requests until operator
			// sets BLAZIL_ADMIN_TOKEN (fail-closed).
			jsonError(w, http.StatusServiceUnavailable, "admin token not configured")
			return
		}

		authHeader := r.Header.Get("Authorization")
		const prefix = "Bearer "
		if !strings.HasPrefix(authHeader, prefix) {
			jsonError(w, http.StatusUnauthorized, "missing or invalid Authorization header")
			return
		}
		provided := []byte(strings.TrimPrefix(authHeader, prefix))

		// Use subtle.ConstantTimeCompare to prevent timing oracle attacks.
		// We must ensure both slices are the same length to avoid leaking
		// the token length through a short-circuit comparison.
		if subtle.ConstantTimeCompare(provided, h.adminToken) != 1 {
			jsonError(w, http.StatusUnauthorized, "invalid admin token")
			return
		}
		next.ServeHTTP(w, r)
	})
}

// ── Probes ─────────────────────────────────────────────────────────────────

func (h *Handler) health(w http.ResponseWriter, _ *http.Request) {
	jsonOK(w, map[string]string{"status": "ok"})
}

func (h *Handler) ready(w http.ResponseWriter, r *http.Request) {
	if err := h.store.Ping(r.Context()); err != nil {
		h.logger.Warn("readiness check failed", zap.Error(err))
		jsonError(w, http.StatusServiceUnavailable, "database unavailable")
		return
	}
	jsonOK(w, map[string]string{"status": "ready"})
}

// ── Tenant CRUD ────────────────────────────────────────────────────────────

type createTenantRequest struct {
	Name           string `json:"name"`
	Email          string `json:"email"`
	Tier           string `json:"tier"`
	RateLimitRPS   int    `json:"rate_limit_rps"`
	RateLimitBurst int    `json:"rate_limit_burst"`
}

func (h *Handler) createTenant(w http.ResponseWriter, r *http.Request) {
	var req createTenantRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if req.Name == "" || req.Email == "" {
		jsonError(w, http.StatusBadRequest, "name and email are required")
		return
	}
	if req.Tier == "" {
		req.Tier = tenant.TierFree
	}
	if req.RateLimitRPS <= 0 {
		req.RateLimitRPS = 100
	}
	if req.RateLimitBurst <= 0 {
		req.RateLimitBurst = 200
	}

	t, err := h.store.CreateTenant(r.Context(),
		req.Name, req.Email, req.Tier, req.RateLimitRPS, req.RateLimitBurst)
	if err != nil {
		h.internalError(w, "createTenant", err)
		return
	}
	jsonStatus(w, http.StatusCreated, t)
}

func (h *Handler) listTenants(w http.ResponseWriter, r *http.Request) {
	tenants, err := h.store.ListTenants(r.Context())
	if err != nil {
		h.internalError(w, "listTenants", err)
		return
	}
	jsonOK(w, tenants)
}

func (h *Handler) getTenant(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	t, err := h.store.GetTenant(r.Context(), id)
	if err != nil {
		h.storeError(w, "getTenant", err)
		return
	}
	jsonOK(w, t)
}

func (h *Handler) suspendTenant(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if err := h.store.SuspendTenant(r.Context(), id); err != nil {
		h.storeError(w, "suspendTenant", err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

func (h *Handler) unsuspendTenant(w http.ResponseWriter, r *http.Request) {
	id := chi.URLParam(r, "id")
	if err := h.store.UnsuspendTenant(r.Context(), id); err != nil {
		h.storeError(w, "unsuspendTenant", err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// ── API Key management ─────────────────────────────────────────────────────

type issueAPIKeyRequest struct {
	Name string `json:"name"`
}

type issueAPIKeyResponse struct {
	ID        string    `json:"id"`
	RawKey    string    `json:"raw_key"` // returned once, never stored
	Prefix    string    `json:"prefix"`
	Name      string    `json:"name"`
	CreatedAt time.Time `json:"created_at"`
	Warning   string    `json:"warning"`
}

func (h *Handler) issueAPIKey(w http.ResponseWriter, r *http.Request) {
	tenantID := chi.URLParam(r, "id")

	var req issueAPIKeyRequest
	if !decodeJSON(w, r, &req) {
		return
	}
	if req.Name == "" {
		jsonError(w, http.StatusBadRequest, "name is required")
		return
	}

	key, err := h.store.IssueAPIKey(r.Context(), tenantID, req.Name)
	if err != nil {
		h.storeError(w, "issueAPIKey", err)
		return
	}

	// RawKey is returned exactly once here and never stored anywhere.
	jsonStatus(w, http.StatusCreated, issueAPIKeyResponse{
		ID:        key.ID,
		RawKey:    key.RawKey,
		Prefix:    key.Prefix,
		Name:      key.Name,
		CreatedAt: key.CreatedAt,
		Warning:   "Store this key securely — it will not be shown again.",
	})
}

func (h *Handler) listAPIKeys(w http.ResponseWriter, r *http.Request) {
	tenantID := chi.URLParam(r, "id")
	keys, err := h.store.ListAPIKeys(r.Context(), tenantID)
	if err != nil {
		h.storeError(w, "listAPIKeys", err)
		return
	}
	jsonOK(w, keys)
}

func (h *Handler) revokeAPIKey(w http.ResponseWriter, r *http.Request) {
	keyID := chi.URLParam(r, "keyID")
	if err := h.store.RevokeAPIKey(r.Context(), keyID); err != nil {
		h.storeError(w, "revokeAPIKey", err)
		return
	}
	w.WriteHeader(http.StatusNoContent)
}

// ── Usage & Billing ────────────────────────────────────────────────────────

type usageResponse struct {
	TenantID string              `json:"tenant_id"`
	Windows  []metering.UsageRow `json:"windows"`
}

func (h *Handler) getUsage(w http.ResponseWriter, r *http.Request) {
	tenantID := chi.URLParam(r, "id")
	now := time.Now().UTC()
	windows, err := h.writer.WindowedUsage(r.Context(), tenantID, now.Year(), int(now.Month()))
	if err != nil {
		h.internalError(w, "getUsage", err)
		return
	}
	jsonOK(w, usageResponse{TenantID: tenantID, Windows: windows})
}

func (h *Handler) getInvoice(w http.ResponseWriter, r *http.Request) {
	tenantID := chi.URLParam(r, "id")

	t, err := h.store.GetTenant(r.Context(), tenantID)
	if err != nil {
		h.storeError(w, "getInvoice/getTenant", err)
		return
	}

	now := time.Now().UTC()
	windows, err := h.writer.WindowedUsage(r.Context(), tenantID, now.Year(), int(now.Month()))
	if err != nil {
		h.internalError(w, "getInvoice/windowedUsage", err)
		return
	}

	counts := make([]metering.WindowCount, len(windows))
	for i, w := range windows {
		counts[i] = metering.WindowCount{
			WindowStart: w.WindowStart,
			Count:       w.TxCount,
		}
	}

	lines := metering.CalculateInvoice(tenantID, metering.Tier(t.Tier), counts)
	total := metering.TotalInvoiceMicroUSD(lines)

	jsonOK(w, map[string]interface{}{
		"tenant_id":       tenantID,
		"tier":            t.Tier,
		"lines":           lines,
		"total_micro_usd": total,
		"period": map[string]int{
			"year":  now.Year(),
			"month": int(now.Month()),
		},
	})
}

// ── Helpers ────────────────────────────────────────────────────────────────

func (h *Handler) storeError(w http.ResponseWriter, op string, err error) {
	switch {
	case err == tenant.ErrNotFound:
		jsonError(w, http.StatusNotFound, "not found")
	case err == tenant.ErrKeyRevoked:
		jsonError(w, http.StatusGone, "api key revoked")
	case err == tenant.ErrSuspended:
		jsonError(w, http.StatusForbidden, "tenant suspended")
	default:
		h.internalError(w, op, err)
	}
}

func (h *Handler) internalError(w http.ResponseWriter, op string, err error) {
	h.logger.Error("admin handler error", zap.String("op", op), zap.Error(err))
	// Never surface internal error details to the caller — log only.
	jsonError(w, http.StatusInternalServerError, "internal error")
}

func jsonOK(w http.ResponseWriter, v interface{}) {
	jsonStatus(w, http.StatusOK, v)
}

func jsonStatus(w http.ResponseWriter, code int, v interface{}) {
	w.Header().Set("Content-Type", "application/json")
	w.WriteHeader(code)
	_ = json.NewEncoder(w).Encode(v)
}

func jsonError(w http.ResponseWriter, code int, msg string) {
	jsonStatus(w, code, map[string]string{"error": msg})
}

// decodeJSON decodes r.Body into dst and writes a 400 response if it fails.
// Returns true on success.
func decodeJSON(w http.ResponseWriter, r *http.Request, dst interface{}) bool {
	dec := json.NewDecoder(r.Body)
	dec.DisallowUnknownFields()
	if err := dec.Decode(dst); err != nil {
		jsonError(w, http.StatusBadRequest, "invalid request body: "+err.Error())
		return false
	}
	return true
}


