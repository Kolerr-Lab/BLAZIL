#!/bin/bash
# TigerBeetle entrypoint wrapper.
# On fresh volumes the data file does not exist and TigerBeetle requires
# a one-time `format` command before `start` can be used.
# This script handles both cases idempotently.
set -e

DATA_FILE="/data/0_0.tigerbeetle"

if [ ! -f "$DATA_FILE" ]; then
  echo "Formatting TigerBeetle data file..."
  tigerbeetle format \
    --cluster=0 \
    --replica=0 \
    --replica-count=1 \
    "$DATA_FILE"
  echo "Format complete."
fi

echo "Starting TigerBeetle..."
exec tigerbeetle start \
  --addresses=0.0.0.0:3000 \
  "$DATA_FILE"
