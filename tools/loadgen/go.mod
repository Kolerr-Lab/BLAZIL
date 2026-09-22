module github.com/blazil/loadgen

go 1.25.10

require (
	github.com/blazil/banking v0.0.0
	github.com/blazil/crypto v0.0.0
	github.com/blazil/services/payments v0.0.0
	github.com/blazil/trading v0.0.0
	google.golang.org/grpc v1.83.2
)

require (
	go.opentelemetry.io/otel v1.45.0 // indirect
	go.opentelemetry.io/otel/sdk/metric v1.45.0 // indirect
	golang.org/x/net v0.58.0 // indirect
	golang.org/x/sys v0.47.0 // indirect
	golang.org/x/text v0.41.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20260803160001-6ac0973c030d // indirect
	google.golang.org/protobuf v1.36.11 // indirect
)

replace (
	github.com/blazil/banking => ../../services/banking
	github.com/blazil/crypto => ../../services/crypto
	github.com/blazil/observability => ../../libs/observability
	github.com/blazil/services/payments => ../../services/payments
	github.com/blazil/trading => ../../services/trading
)
