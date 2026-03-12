#!/bin/bash
set -e

echo "🔥 Blazil Demo — Open-source financial infrastructure"
echo "======================================================"
echo ""
echo "Starting all services..."

docker compose \
  -f infra/docker/docker-compose.demo.yml \
  up --build -d

echo ""
echo "Waiting for services to be healthy..."
sleep 5

echo ""
echo "✅ Blazil is running!"
echo ""
echo "  Grafana dashboard  → http://localhost:3000"
echo "  (username: admin, password: blazil)"
echo ""
echo "  Prometheus metrics → http://localhost:9090"
echo ""
echo "  Services:"
echo "    Payments  → localhost:50051"
echo "    Banking   → localhost:50052"
echo "    Trading   → localhost:50053"
echo "    Crypto    → localhost:50054"
echo ""
echo "Press Ctrl+C to stop."
echo ""

docker compose \
  -f infra/docker/docker-compose.demo.yml \
  logs -f blazil-loadgen
