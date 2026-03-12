// Blazil loadgen fires continuous mixed load against all 4 services.
// Load profile: 50 payments + 20 banking + 30 trading + 10 crypto per second.
// All goroutines run concurrently; errors are logged, never fatal.
package main

import (
	"context"
	"fmt"
	"log"
	"os"
	"sync/atomic"
	"time"

	"google.golang.org/grpc"
	"google.golang.org/grpc/credentials/insecure"

	bankingv1 "github.com/blazil/banking/api/proto/banking/v1"
	cryptov1 "github.com/blazil/crypto/api/proto/crypto/v1"
	paymentsv1 "github.com/blazil/services/payments/api/proto/payments/v1"
	tradingv1 "github.com/blazil/trading/api/proto/trading/v1"
)

func main() {
	paymentsAddr := envOr("PAYMENTS_ADDR", "localhost:50051")
	bankingAddr := envOr("BANKING_ADDR", "localhost:50052")
	tradingAddr := envOr("TRADING_ADDR", "localhost:50053")
	cryptoAddr := envOr("CRYPTO_ADDR", "localhost:50054")

	paymentsConn := dialWithRetry(paymentsAddr)
	bankingConn := dialWithRetry(bankingAddr)
	tradingConn := dialWithRetry(tradingAddr)
	cryptoConn := dialWithRetry(cryptoAddr)
	defer paymentsConn.Close()
	defer bankingConn.Close()
	defer tradingConn.Close()
	defer cryptoConn.Close()

	paymentsClient := paymentsv1.NewPaymentsServiceClient(paymentsConn)
	bankingClient := bankingv1.NewBankingServiceClient(bankingConn)
	tradingClient := tradingv1.NewTradingServiceClient(tradingConn)
	cryptoClient := cryptov1.NewCryptoServiceClient(cryptoConn)

	// Seed demo accounts on startup (best-effort, errors ignored)
	seedBankingAccounts(bankingClient)
	seedCryptoWallets(cryptoClient)

	var (
		paymentsOps atomic.Int64
		bankingOps  atomic.Int64
		tradingOps  atomic.Int64
		cryptoOps   atomic.Int64
		errorCount  atomic.Int64
	)

	log.Printf("🚀 [loadgen] starting — payments=%s banking=%s trading=%s crypto=%s",
		paymentsAddr, bankingAddr, tradingAddr, cryptoAddr)

	// Report every 5 seconds
	go func() {
		ticker := time.NewTicker(5 * time.Second)
		defer ticker.Stop()
		for range ticker.C {
			p := paymentsOps.Swap(0)
			b := bankingOps.Swap(0)
			t := tradingOps.Swap(0)
			c := cryptoOps.Swap(0)
			e := errorCount.Swap(0)
			log.Printf("📊 [loadgen] payments=%d/5s banking=%d/5s trading=%d/5s crypto=%d/5s errors=%d",
				p, b, t, c, e)
		}
	}()

	// Main load loop — one tick per second
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()

	for range ticker.C {
		// 50 payment requests
		for i := 0; i < 50; i++ {
			go func(n int) {
				ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				defer cancel()
				_, err := paymentsClient.ProcessPayment(ctx, &paymentsv1.ProcessPaymentRequest{
					IdempotencyKey:  fmt.Sprintf("loadgen-pay-%d-%d", time.Now().UnixNano(), n),
					DebitAccountId:  "loadgen-debit-01",
					CreditAccountId: "loadgen-credit-01",
					AmountMinorUnits: 100,
					CurrencyCode:    "USD",
					LedgerId:        1,
				})
				if err != nil {
					errorCount.Add(1)
				} else {
					paymentsOps.Add(1)
				}
			}(i)
		}

		// 20 banking credit/debit ops (alternate)
		for i := 0; i < 20; i++ {
			go func(n int) {
				ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				defer cancel()
				_, err := bankingClient.GetBalance(ctx, &bankingv1.GetBalanceRequest{
					AccountId: "loadgen-banking-01",
				})
				if err != nil {
					errorCount.Add(1)
				} else {
					bankingOps.Add(1)
				}
			}(i)
		}

		// 30 order placements (mix of buy/sell, BTC-USD)
		for i := 0; i < 30; i++ {
			go func(n int) {
				ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				defer cancel()
				side := "buy"
				if n%2 == 0 {
					side = "sell"
				}
				_, err := tradingClient.PlaceOrder(ctx, &tradingv1.PlaceOrderRequest{
					OrderId:              fmt.Sprintf("loadgen-order-%d-%d", time.Now().UnixNano(), n),
					InstrumentId:         "BTC-USD",
					OwnerId:              fmt.Sprintf("loadgen-trader-%d", n%5),
					Side:                 side,
					LimitPriceMinorUnits: 6_500_000 + int64(n*1000),
					QuantityUnits:        1,
				})
				if err != nil {
					errorCount.Add(1)
				} else {
					tradingOps.Add(1)
				}
			}(i)
		}

		// 10 internal transfers via crypto service
		for i := 0; i < 10; i++ {
			go func(n int) {
				ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
				defer cancel()
				_, err := cryptoClient.InternalTransfer(ctx, &cryptov1.InternalTransferRequest{
					TransferId:      fmt.Sprintf("loadgen-xfer-%d-%d", time.Now().UnixNano(), n),
					FromWalletId:    "loadgen-wallet-src",
					ToWalletId:      "loadgen-wallet-dst",
					FromAccountId:   "loadgen-from-acct",
					ToAccountId:     "loadgen-to-acct",
					AmountMinorUnits: 50,
				})
				if err != nil {
					errorCount.Add(1)
				} else {
					cryptoOps.Add(1)
				}
			}(i)
		}
	}
}

// dialWithRetry connects to addr with exponential backoff (non-blocking).
func dialWithRetry(addr string) *grpc.ClientConn {
	opts := []grpc.DialOption{
		grpc.WithTransportCredentials(insecure.NewCredentials()),
		grpc.WithBlock(),
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()

	conn, err := grpc.DialContext(ctx, addr, opts...) //nolint:staticcheck
	if err != nil {
		log.Printf("⚠️  [loadgen] could not connect to %s: %v — continuing anyway", addr, err)
		// Return a lazy connection that will retry on use
		conn, _ = grpc.Dial(addr, grpc.WithTransportCredentials(insecure.NewCredentials())) //nolint:staticcheck
	}
	return conn
}

// seedBankingAccounts creates demo accounts (best-effort).
func seedBankingAccounts(client bankingv1.BankingServiceClient) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	for _, id := range []string{"loadgen-banking-01"} {
		_, _ = client.CreateAccount(ctx, &bankingv1.CreateAccountRequest{
			AccountId:                id,
			OwnerId:                  "loadgen",
			AccountType:              "checking",
			CurrencyCode:             "USD",
			InitialBalanceMinorUnits: 1_000_000_00,
		})
	}
}

// seedCryptoWallets creates demo wallets (best-effort).
func seedCryptoWallets(client cryptov1.CryptoServiceClient) {
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	for _, id := range []string{"loadgen-wallet-src", "loadgen-wallet-dst"} {
		_, _ = client.CreateWallet(ctx, &cryptov1.CreateWalletRequest{
			WalletId:   id,
			OwnerId:    "loadgen",
			ChainId:    1,
			WalletType: "hot",
		})
	}
}

func envOr(key, def string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return def
}
