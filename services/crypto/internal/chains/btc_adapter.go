package chains

import (
	"bytes"
	"context"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"

	"github.com/blazil/crypto/internal/domain"
)

// BtcChainAdapter implements ChainAdapter for Bitcoin.
// It communicates with a Bitcoin Core JSON-RPC node (BTC_NODE_URL).
type BtcChainAdapter struct {
	nodeURL string
	rpcUser string
	rpcPass string
	client  *http.Client
}

// NewBtcChainAdapter constructs a BtcChainAdapter.
// nodeURL should be the full URL including credentials, e.g. "http://user:pass@127.0.0.1:8332".
// rpcUser and rpcPass override basic auth when provided.
func NewBtcChainAdapter(nodeURL, rpcUser, rpcPass string) *BtcChainAdapter {
	return &BtcChainAdapter{
		nodeURL: nodeURL,
		rpcUser: rpcUser,
		rpcPass: rpcPass,
		client:  &http.Client{Timeout: 15 * time.Second},
	}
}

// ChainID implements ChainAdapter.
func (a *BtcChainAdapter) ChainID() domain.ChainID { return domain.ChainBitcoin }

// GenerateAddress derives a deterministic P2WPKH deposit address for the owner.
// Production code would derive using BIP-84 HD-wallet paths.
// Here we use a SHA-256 hash of ownerID as a stable placeholder address.
func (a *BtcChainAdapter) GenerateAddress(_ context.Context, ownerID string) (string, error) {
	h := sha256Hex(ownerID + ":btc")
	// Bitcoin bech32 address starts with "bc1q"; use first 32 chars of hash as pubkey hash.
	return "bc1q" + h[:32], nil
}

// EstimateFee returns a static fee estimate in satoshis.
// 250 bytes * 20 sat/vbyte = 5 000 satoshis.
func (a *BtcChainAdapter) EstimateFee(_ context.Context, _ int64) (int64, error) {
	return 5_000, nil // satoshis
}

// BroadcastTx submits a signed raw transaction via sendrawtransaction.
func (a *BtcChainAdapter) BroadcastTx(ctx context.Context, w *domain.Withdrawal) (string, error) {
	// In production this would sign with the hot-wallet UTXO set.
	// Here we submit a dummy raw tx for interface compliance.
	rawTx := hex.EncodeToString(must32Bytes())
	result, err := a.rpcCall(ctx, "sendrawtransaction", []interface{}{rawTx})
	if err != nil {
		return "", fmt.Errorf("sendrawtransaction: %w", err)
	}
	txHash, ok := result.(string)
	if !ok {
		return "", fmt.Errorf("unexpected result type from sendrawtransaction")
	}
	_ = w
	return txHash, nil
}

// GetConfirmations returns the number of confirmations for a txid.
func (a *BtcChainAdapter) GetConfirmations(ctx context.Context, txHash string) (int, error) {
	result, err := a.rpcCall(ctx, "getrawtransaction", []interface{}{txHash, true})
	if err != nil {
		return 0, fmt.Errorf("getrawtransaction: %w", err)
	}
	if result == nil {
		return 0, nil
	}
	txMap, ok := result.(map[string]interface{})
	if !ok {
		return 0, fmt.Errorf("unexpected tx result type")
	}
	confs, _ := txMap["confirmations"].(float64)
	return int(confs), nil
}

// rpcCall executes a Bitcoin Core JSON-RPC 1.0 call.
func (a *BtcChainAdapter) rpcCall(ctx context.Context, method string, params interface{}) (interface{}, error) {
	if params == nil {
		params = []interface{}{}
	}
	body := map[string]interface{}{
		"jsonrpc": "1.0",
		"id":      "blazil-crypto",
		"method":  method,
		"params":  params,
	}
	data, err := json.Marshal(body)
	if err != nil {
		return nil, err
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, a.nodeURL, bytes.NewReader(data))
	if err != nil {
		return nil, err
	}
	req.Header.Set("Content-Type", "application/json")
	if a.rpcUser != "" {
		req.SetBasicAuth(a.rpcUser, a.rpcPass)
	}

	resp, err := a.client.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	raw, err := io.ReadAll(io.LimitReader(resp.Body, 1<<20))
	if err != nil {
		return nil, err
	}

	var rpcResp struct {
		Result interface{} `json:"result"`
		Error  interface{} `json:"error"`
	}
	if err := json.Unmarshal(raw, &rpcResp); err != nil {
		return nil, fmt.Errorf("unmarshal btc rpc response: %w", err)
	}
	if rpcResp.Error != nil {
		return nil, fmt.Errorf("btc rpc error: %v", rpcResp.Error)
	}
	return rpcResp.Result, nil
}

// compile-time interface check.
var _ ChainAdapter = (*BtcChainAdapter)(nil)
