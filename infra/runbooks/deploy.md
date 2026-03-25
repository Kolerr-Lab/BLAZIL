# Deploy Runbook

Deploy a new Blazil version to the DO 3-node cluster.

## Zero-Downtime Rolling Deploy

TigerBeetle VSR tolerates one unavailable replica (2/3 quorum). Deploy one node
at a time, verify health before proceeding.

```bash
# 1. Get latest code on all nodes first (no restart yet)
for IP in $NODE1_IP $NODE2_IP $NODE3_IP; do
  ssh root@$IP "cd /opt/blazil && git pull origin main"
done

# 2. Roll node-3 first (furthest from node-1 which hosts Grafana/Prometheus)
ssh root@$NODE3_IP "cd /opt/blazil && \
  docker compose -f infra/docker/docker-compose.node-3.yml pull && \
  docker compose -f infra/docker/docker-compose.node-3.yml up --build -d"

# 3. Wait for node-3 VSR to re-join (watch Grafana or check TB health)
sleep 30
ssh root@$NODE3_IP "docker exec blazil-tigerbeetle-2 sh -c 'grep -q :0BBA /proc/net/tcp && echo HEALTHY'"

# 4. Roll node-2
ssh root@$NODE2_IP "cd /opt/blazil && \
  docker compose -f infra/docker/docker-compose.node-2.yml up --build -d"

sleep 30
ssh root@$NODE2_IP "docker exec blazil-tigerbeetle-1 sh -c 'grep -q :0BB9 /proc/net/tcp && echo HEALTHY'"

# 5. Roll node-1 last (it hosts Prometheus/Grafana — brief monitoring gap acceptable)
ssh root@$NODE1_IP "cd /opt/blazil && \
  docker compose -f infra/docker/docker-compose.node-1.yml up --build -d"
```

Or with Ansible:
```bash
cd infra/ansible
ansible-playbook -i inventory/production playbooks/site.yml --tags start --limit node-3
# verify node-3 healthy
ansible-playbook -i inventory/production playbooks/site.yml --tags start --limit node-2
# verify node-2 healthy
ansible-playbook -i inventory/production playbooks/site.yml --tags start --limit node-1
```

## Full Cold Deploy (new cluster)

```bash
# 1. Provision with Terraform
cd infra/terraform/digitalocean
export TF_VAR_do_token="dop_v1_..."
terraform apply

# 2. Export IPs
TB=$(terraform output -raw tb_addresses)
NODES=$(terraform output -raw blazil_nodes)

# 3. Setup + tune all nodes in parallel, then start serially
cd ../../ansible
ansible-galaxy install -r requirements.yml
ansible-playbook -i inventory/production playbooks/site.yml \
  -e "tb_addresses=$TB" \
  -e "blazil_nodes=$NODES"

# 4. Verify
ansible-playbook -i inventory/production playbooks/site.yml --tags verify
```

## Post-Deploy Verification

```bash
# Check all TigerBeetle replicas are healthy
for NODE in node-1 node-2 node-3; do
  echo "=== $NODE ==="
  ansible $NODE -i inventory/production -m shell \
    -a "docker ps --filter name=tigerbeetle --format '{{.Names}} {{.Status}}'"
done

# Quick load test (1000 transactions)
ssh root@$NODE1_IP "cd /opt/blazil && \
  ./tools/stresstest/stresstest-linux \
    -target=localhost:50051 \
    -duration=10s \
    -goroutines=1 \
    -window=256"
```

## Rollback

```bash
# Roll back to previous commit on all nodes
PREV_COMMIT=$(git rev-parse HEAD~1)
for IP in $NODE1_IP $NODE2_IP $NODE3_IP; do
  ssh root@$IP "cd /opt/blazil && git checkout $PREV_COMMIT"
done

# Restart (rolling, same procedure as above)
```
