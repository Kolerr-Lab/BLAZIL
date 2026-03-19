module github.com/blazil/loadgen

go 1.25.8

require (
	github.com/blazil/banking v0.0.0
	github.com/blazil/crypto v0.0.0
	github.com/blazil/services/payments v0.0.0
	github.com/blazil/trading v0.0.0
	google.golang.org/grpc v1.79.3
)

require (
	golang.org/x/net v0.52.0 // indirect
	golang.org/x/sys v0.42.0 // indirect
	golang.org/x/text v0.35.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20260316180232-0b37fe3546d5 // indirect
	google.golang.org/protobuf v1.36.11 // indirect
)

replace (
	github.com/blazil/banking => ../../services/banking
	github.com/blazil/crypto => ../../services/crypto
	github.com/blazil/observability => ../../libs/observability
	github.com/blazil/services/payments => ../../services/payments
	github.com/blazil/trading => ../../services/trading
)
