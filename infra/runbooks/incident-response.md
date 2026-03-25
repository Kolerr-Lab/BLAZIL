# Incident Response Runbook

## Severity Levels

| Level | Example | Response time |
|---|---|---|
| P0 | All 3 nodes down / data corruption | Immediate |
| P1 | 1 node down (VSR degraded, still operational) | < 15 min |
| P2 | High latency / single service down | < 1 hour |
| P3 | Monitoring gap / non-critical service | Next business day |

---

## Node Down (P1)

TigerBeetle VSR requires 2/3 nodes. With 1 node down the cluster continues
processing but cannot tolerate another failure.

```bash
# 1. Identify which node is down
ansible blazil_nodes -i inventory/production -m ping

# 2. Check if it's a Docker issue (most common)
ssh root@<FAILED_NODE_IP> "docker ps -a | grep -E 'Exit|Restarting'"

# 3. Restart failed containers
ssh root@<FAILED_NODE_IP> "cd /opt/blazil && \
  docker compose -f infra/docker/docker-compose.node-<N>.yml restart"

# 4. If containers won't start — check logs
ssh root@<FAILED_NODE_IP> "docker logs blazil-tigerbeetle-<N> --tail=50"
ssh root@<FAILED_NODE_IP> "docker logs blazil-engine --tail=50"

# 5. Verify VSR re-join (within 60s of restart)
ssh root@<FAILED_NODE_IP> "docker exec blazil-tigerbeetle-<N> \
  sh -c 'grep -q :0BB[89A] /proc/net/tcp && echo VSR_HEALTHY || echo VSR_NOT_READY'"
```

---

## TigerBeetle VSR Split-Brain / Crash (P0)

If TB refuses to start after a crash, it may have a corrupted data file.

```bash
# Check TB logs for error
ssh root@<NODE_IP> "docker logs blazil-tigerbeetle-<N> --tail=100"

# Common error: "data file format mismatch" → version upgrade issue
# Fix: pull matching TB image
ssh root@<NODE_IP> "docker pull ghcr.io/tigerbeetle/tigerbeetle:0.16.72"

# Common error: "address already in use" → port conflict
ssh root@<NODE_IP> "ss -tlnp | grep :300[0-2]"

# LAST RESORT: restore from backup (see backup-restore.md)
# WARNING: this will replay from the backup point — transactions since backup are lost
```

---

## OOM Kill (P1)

Symptom: containers killed with exit code 137.

```bash
# Check kernel OOM logs
ssh root@<NODE_IP> "dmesg | grep -i 'oom\|killed' | tail -20"

# Check memory usage
ssh root@<NODE_IP> "free -h && docker stats --no-stream"

# Immediate fix: restart killed container
ssh root@<NODE_IP> "docker compose -f /opt/blazil/infra/docker/docker-compose.node-<N>.yml \
  up -d blazil-engine"

# Root cause: AERON_TERM_BUFFER_LENGTH too large for available RAM
# Fix: reduce term buffer (128MB is safe on 8GB nodes)
# AERON_TERM_BUFFER_LENGTH=134217728  # 128MB
ssh root@<NODE_IP> "grep AERON_TERM /opt/blazil/.env.node"
```

---

## High Latency / Low TPS (P2)

```bash
# 1. Check engine metrics
curl -s http://<NODE_IP>:9090/metrics | grep -E "blazil_tps|blazil_p99"

# 2. Check CPU frequency (thermal throttling on non-server hardware)
ssh root@<NODE_IP> "cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_cur_freq"

# 3. Re-apply kernel tuning
ssh root@<NODE_IP> "bash -s" < scripts/do-tune.sh

# 4. Check for disk I/O saturation (TigerBeetle writes)
ssh root@<NODE_IP> "iostat -x 1 5"

# 5. Check for network congestion
ssh root@<NODE_IP> "ss -s"
```

---

## Engine Crash / Ring Buffer Full (P2)

```bash
# Check engine logs
ssh root@<NODE_IP> "docker logs blazil-engine --tail=100"

# Common: RingBufferFull — too many in-flight transactions
# Fix: reduce load gen window or increase CAPACITY in engine config
ssh root@<NODE_IP> "docker restart blazil-engine"

# Wait for engine + TB connection
sleep 15
curl -s http://<NODE_IP>:9090/metrics | grep blazil_engine_ready
```

---

## Grafana / Prometheus Down (P3)

Non-critical — cluster continues operating without monitoring.

```bash
# Restart monitoring stack (node-1 only)
ssh root@$NODE1_IP "cd /opt/blazil && \
  docker compose -f infra/docker/docker-compose.node-1.yml \
  restart prometheus grafana"

# Verify
curl -s http://$NODE1_IP:9090/-/healthy
curl -s http://$NODE1_IP:3001/api/health
```
