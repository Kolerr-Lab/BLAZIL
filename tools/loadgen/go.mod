module github.com/blazil/loadgen

go 1.24.0

require (
	github.com/blazil/banking v0.0.0
	github.com/blazil/crypto v0.0.0
	github.com/blazil/services/payments v0.0.0
	github.com/blazil/trading v0.0.0
	google.golang.org/grpc v1.79.2
)

replace (
	github.com/blazil/banking => ../../services/banking
	github.com/blazil/crypto => ../../services/crypto
	github.com/blazil/observability => ../../libs/observability
	github.com/blazil/services/payments => ../../services/payments
	github.com/blazil/trading => ../../services/trading
)
