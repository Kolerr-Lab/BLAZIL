package auth

import (
	"context"
	"crypto/rsa"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"net/http"
	"os"
	"sync"
	"time"

	"github.com/golang-jwt/jwt/v5"
)

// ErrTokenInvalid is returned when the JWT fails validation.
var ErrTokenInvalid = errors.New("token invalid")

// ErrTokenExpired is returned when the JWT is expired.
var ErrTokenExpired = errors.New("token expired")

// ErrUnauthenticated is returned when no token is provided and auth is required.
var ErrUnauthenticated = errors.New("unauthenticated")

// jwksKey holds a single JWK public key entry.
type jwksKey struct {
	Kty string `json:"kty"`
	Kid string `json:"kid"`
	Use string `json:"use"`
	N   string `json:"n"`
	E   string `json:"e"`
	Alg string `json:"alg"`
}

type jwksResponse struct {
	Keys []jwksKey `json:"keys"`
}

// blazilClaims extends jwt.RegisteredClaims with Keycloak realm_access roles.
type blazilClaims struct {
	jwt.RegisteredClaims
	RealmAccess struct {
		Roles []string `json:"roles"`
	} `json:"realm_access"`
}

// JWTValidator validates Keycloak-issued JWTs using JWKS.
// The JWKS endpoint is cached for 1 hour.
type JWTValidator struct {
	keycloakURL string
	issuer      string
	httpClient  *http.Client

	mu          sync.RWMutex
	cachedKeys  map[string]*rsa.PublicKey // keyed by kid
	cacheExpiry time.Time
}

// NewJWTValidator creates a JWTValidator reading KEYCLOAK_URL from env.
// If KEYCLOAK_URL is empty, returns a MockTokenValidator (dev mode).
func NewJWTValidator() TokenValidator {
	url := os.Getenv("KEYCLOAK_URL")
	if url == "" {
		return NewMockTokenValidator()
	}
	return &JWTValidator{
		keycloakURL: url,
		issuer:      url + "/realms/blazil",
		httpClient:  &http.Client{Timeout: 10 * time.Second},
		cachedKeys:  make(map[string]*rsa.PublicKey),
	}
}

// NewJWTValidatorWithURL creates a JWTValidator with an explicit URL.
// Primarily for tests.
func NewJWTValidatorWithURL(keycloakURL string) *JWTValidator {
	return &JWTValidator{
		keycloakURL: keycloakURL,
		issuer:      keycloakURL + "/realms/blazil",
		httpClient:  &http.Client{Timeout: 10 * time.Second},
		cachedKeys:  make(map[string]*rsa.PublicKey),
	}
}

func (v *JWTValidator) ValidateToken(ctx context.Context, tokenStr string) (*Claims, error) {
	if tokenStr == "" {
		return nil, ErrUnauthenticated
	}

	keyFunc := func(token *jwt.Token) (interface{}, error) {
		if _, ok := token.Method.(*jwt.SigningMethodRSA); !ok {
			return nil, fmt.Errorf("unexpected signing method: %v", token.Header["alg"])
		}
		kid, _ := token.Header["kid"].(string)
		return v.getKey(ctx, kid)
	}

	var claims blazilClaims
	token, err := jwt.ParseWithClaims(tokenStr, &claims, keyFunc,
		jwt.WithIssuer(v.issuer),
		jwt.WithExpirationRequired(),
	)
	if err != nil {
		if errors.Is(err, jwt.ErrTokenExpired) {
			return nil, ErrTokenExpired
		}
		return nil, fmt.Errorf("%w: %v", ErrTokenInvalid, err)
	}
	if !token.Valid {
		return nil, ErrTokenInvalid
	}

	exp, _ := claims.GetExpirationTime()
	expTime := time.Time{}
	if exp != nil {
		expTime = exp.Time
	}

	return &Claims{
		Subject:   claims.Subject,
		Roles:     claims.RealmAccess.Roles,
		ExpiresAt: expTime,
		Issuer:    claims.Issuer,
	}, nil
}

// getKey returns the RSA public key for the given kid, using the JWKS cache.
func (v *JWTValidator) getKey(ctx context.Context, kid string) (*rsa.PublicKey, error) {
	v.mu.RLock()
	if time.Now().Before(v.cacheExpiry) {
		if key, ok := v.cachedKeys[kid]; ok {
			v.mu.RUnlock()
			return key, nil
		}
	}
	v.mu.RUnlock()

	// Cache miss or expired — refresh.
	if err := v.refreshJWKS(ctx); err != nil {
		return nil, fmt.Errorf("jwks refresh: %w", err)
	}

	v.mu.RLock()
	defer v.mu.RUnlock()
	key, ok := v.cachedKeys[kid]
	if !ok {
		return nil, fmt.Errorf("key id %q not found in JWKS", kid)
	}
	return key, nil
}

func (v *JWTValidator) refreshJWKS(ctx context.Context) error {
	url := fmt.Sprintf("%s/realms/blazil/protocol/openid-connect/certs", v.keycloakURL)
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, url, nil)
	if err != nil {
		return err
	}

	resp, err := v.httpClient.Do(req)
	if err != nil {
		return fmt.Errorf("fetch JWKS: %w", err)
	}
	defer resp.Body.Close()

	var jwks jwksResponse
	if err := json.NewDecoder(resp.Body).Decode(&jwks); err != nil {
		return fmt.Errorf("decode JWKS: %w", err)
	}

	keys := make(map[string]*rsa.PublicKey, len(jwks.Keys))
	for _, k := range jwks.Keys {
		if k.Kty != "RSA" || k.Use != "sig" {
			continue
		}
		pub, err := parseRSAPublicKey(k.N, k.E)
		if err != nil {
			continue // skip malformed keys
		}
		keys[k.Kid] = pub
	}

	v.mu.Lock()
	v.cachedKeys = keys
	v.cacheExpiry = time.Now().Add(time.Hour)
	v.mu.Unlock()
	return nil
}

// parseRSAPublicKey reconstructs an *rsa.PublicKey from base64url-encoded N and E.
func parseRSAPublicKey(nB64, eB64 string) (*rsa.PublicKey, error) {
	nBytes, err := base64.RawURLEncoding.DecodeString(nB64)
	if err != nil {
		return nil, fmt.Errorf("decode N: %w", err)
	}
	eBytes, err := base64.RawURLEncoding.DecodeString(eB64)
	if err != nil {
		return nil, fmt.Errorf("decode E: %w", err)
	}
	n := new(big.Int).SetBytes(nBytes)
	e := int(new(big.Int).SetBytes(eBytes).Int64())
	return &rsa.PublicKey{N: n, E: e}, nil
}
