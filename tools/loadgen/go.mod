module github.com/blazil/loadgen

go 1.24.0

require (
	github.com/blazil/banking v0.0.0
	github.com/blazil/crypto v0.0.0
	github.com/blazil/services/payments v0.0.0
	github.com/blazil/trading v0.0.0
	google.golang.org/grpc v1.79.2
)

require (
	golang.org/x/net v0.48.0 // indirect
	golang.org/x/sys v0.40.0 // indirect
	golang.org/x/text v0.32.0 // indirect
	google.golang.org/genproto/googleapis/rpc v0.0.0-20251202230838-ff82c1b0f217 // indirect
	google.golang.org/protobuf v1.36.11 // indirect
)

replace (
	github.com/blazil/banking => ../../services/banking
	github.com/blazil/crypto => ../../services/crypto
	github.com/blazil/observability => ../../libs/observability
	github.com/blazil/services/payments => ../../services/payments
	github.com/blazil/trading => ../../services/trading
)
