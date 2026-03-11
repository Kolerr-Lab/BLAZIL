// Package domain contains core crypto domain types for the Blazil crypto service.
package domain

// ChainID uniquely identifies a blockchain network.
type ChainID int32

const (
	ChainBitcoin  ChainID = 1
	ChainEthereum ChainID = 2
	ChainPolygon  ChainID = 3
	ChainSolana   ChainID = 4
	ChainTron     ChainID = 5
)

// NetworkType identifies whether a chain runs on mainnet or testnet.
type NetworkType string

const (
	NetworkMainnet NetworkType = "mainnet"
	NetworkTestnet NetworkType = "testnet"
)

// Chain holds static metadata for a supported blockchain.
type Chain struct {
	ID                    ChainID
	Name                  string
	Symbol                string
	DecimalPlaces         int
	RequiredConfirmations int
	Network               NetworkType
}

// SupportedChains returns the full set of chains the crypto service handles.
func SupportedChains() []Chain {
	return []Chain{
		{
			ID:                    ChainBitcoin,
			Name:                  "Bitcoin",
			Symbol:                "BTC",
			DecimalPlaces:         8,
			RequiredConfirmations: 6,
			Network:               NetworkMainnet,
		},
		{
			ID:                    ChainEthereum,
			Name:                  "Ethereum",
			Symbol:                "ETH",
			DecimalPlaces:         18,
			RequiredConfirmations: 12,
			Network:               NetworkMainnet,
		},
		{
			ID:                    ChainPolygon,
			Name:                  "Polygon",
			Symbol:                "MATIC",
			DecimalPlaces:         18,
			RequiredConfirmations: 32,
			Network:               NetworkMainnet,
		},
		{
			ID:                    ChainSolana,
			Name:                  "Solana",
			Symbol:                "SOL",
			DecimalPlaces:         9,
			RequiredConfirmations: 32,
			Network:               NetworkMainnet,
		},
		{
			ID:                    ChainTron,
			Name:                  "Tron",
			Symbol:                "TRX",
			DecimalPlaces:         6,
			RequiredConfirmations: 32,
			Network:               NetworkMainnet,
		},
	}
}
