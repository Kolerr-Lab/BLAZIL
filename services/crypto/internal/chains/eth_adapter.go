package chains

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"strings"
	"time"

	"github.com/blazil/crypto/internal/domain"
)

// EthChainAdapter implements ChainAdapter for Ethereum (EVM-compatible) networks.
// It communicates with a JSON-RPC node (ETH_NODE_URL).
type EthChainAdapter struct {
	chainID domain.ChainID
	nodeURL string
	client  *http.Client
}

// NewEthChainAdapter constructs an adapter for the given chain using nodeURL.
func NewEthChainAdapter(chainID domain.ChainID, nodeURL string) *EthChainAdapter {
	return &EthChainAdapter{
		chainID: chainID,
		nodeURL: nodeURL,
		client:  &http.Client{Timeout: 10 * time.Second},
	}
}

// ChainID implements ChainAdapter.
func (a *EthChainAdapter) ChainID() domain.ChainID { return a.chainID }

// GenerateAddress derives a deterministic deposit address for the given owner.
// In production this would use a deterministic HD-wallet derivation (BIP-44).
// Here we generate a fresh random address deterministically seeded by ownerID.
func (a *EthChainAdapter) GenerateAddress(_ context.Context, ownerID string) (string, error) {
	h := sha256Hex(ownerID + fmt.Sprintf(":%d", a.chainID))
	// Ethereum address: 20 bytes from the first 40 hex chars.
	return "0x" + h[:40], nil
}

// EstimateFee returns a static gas fee estimate in minor units (wei).
// 21 000 gas * 50 Gwei = 1 050 000 Gwei = 0.00105 ETH.
func (a *EthChainAdapter) EstimateFee(_ context.Context, _ int64) (int64, error) {
	const gasLimit = 21_000
	const gasPriceGwei = 50
	return gasLimit * gasPriceGwei * 1_000_000_000, nil // in wei
}

// BroadcastTx sends a withdrawal transaction via eth_sendRawTransaction.
func (a *EthChainAdapter) BroadcastTx(ctx context.Context, w *domain.Withdrawal) (string, error) {
	// Build a minimal signed-transaction placeholder.
	// Production code would sign with the hot-wallet key using go-ethereum.
	rawTx := "0x" + hex.EncodeToString(must32Bytes())

	result, err := a.rpcCall(ctx, "eth_sendRawTransaction", []interface{}{rawTx})
	if err != nil {
		return "", fmt.Errorf("eth_sendRawTransaction: %w", err)
	}
	txHash, ok := result.(string)
	if !ok {
		return "", fmt.Errorf("unexpected result type from eth_sendRawTransaction")
	}
	_ = w // w.ToAddress / w.AmountMinorUnits used in real signing
	return txHash, nil
}

// GetConfirmations returns the number of confirmations for a transaction hash.
func (a *EthChainAdapter) GetConfirmations(ctx context.Context, txHash string) (int, error) {
	// Fetch latest block number.
	latestResult, err := a.rpcCall(ctx, "eth_blockNumber", nil)
	if err != nil {
		return 0, fmt.Errorf("eth_blockNumber: %w", err)
	}
	latestHex, ok := latestResult.(string)
	if !ok {
		return 0, fmt.Errorf("unexpected block number type")
	}
	latest, err := hexToInt64(latestHex)
	if err != nil {
		return 0, err
	}

	// Fetch the transaction.
	txResult, err := a.rpcCall(ctx, "eth_getTransactionByHash", []interface{}{txHash})
	if err != nil {
		return 0, fmt.Errorf("eth_getTransactionByHash: %w", err)
	}
	if txResult == nil {
		return 0, nil // tx not yet mined
	}
	txMap, ok := txResult.(map[string]interface{})
	if !ok {
		return 0, fmt.Errorf("unexpected tx result type")
	}
	blockHex, ok := txMap["blockNumber"].(string)
	if !ok || blockHex == "" {
		return 0, nil // pending
	}
	txBlock, err := hexToInt64(blockHex)
	if err != nil {
		return 0, err
	}
	confs := int(latest - txBlock)
	if confs < 0 {
		return 0, nil
	}
	return confs, nil
}

// rpcCall executes a JSON-RPC 2.0 call and returns the result field.
func (a *EthChainAdapter) rpcCall(ctx context.Context, method string, params interface{}) (interface{}, error) {
	if params == nil {
		params = []interface{}{}
	}
	body := map[string]interface{}{
		"jsonrpc": "2.0",
		"method":  method,
		"params":  params,
		"id":      1,
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
		Result interface{}            `json:"result"`
		Error  map[string]interface{} `json:"error"`
	}
	if err := json.Unmarshal(raw, &rpcResp); err != nil {
		return nil, fmt.Errorf("unmarshal rpc response: %w", err)
	}
	if rpcResp.Error != nil {
		return nil, fmt.Errorf("rpc error: %v", rpcResp.Error)
	}
	return rpcResp.Result, nil
}

// ── helpers ───────────────────────────────────────────────────────────────────

func sha256Hex(s string) string {
	h := sha256.Sum256([]byte(s))
	return hex.EncodeToString(h[:])
}

func hexToInt64(h string) (int64, error) {
	h = strings.TrimPrefix(h, "0x")
	var n int64
	_, err := fmt.Sscanf(h, "%x", &n)
	return n, err
}

func must32Bytes() []byte {
	b := make([]byte, 32)
	rand.Read(b) //nolint:errcheck
	return b
}

// compile-time interface check.
var _ ChainAdapter = (*EthChainAdapter)(nil)
