# Ansible Playbooks

This directory contains Ansible playbooks for bare metal server provisioning.

## Structure

- `playbooks/` - Ansible playbooks
- `inventory/` - Inventory files
- `roles/` - Ansible roles
- `group_vars/` - Group variables
- `host_vars/` - Host variables

## Prerequisites

- Ansible 2.15+
- SSH access to target hosts
- Python 3 on target hosts

## Usage

```bash
# Install dependencies
ansible-galaxy install -r requirements.yml

# Run playbook
ansible-playbook -i inventory/production playbooks/site.yml
```

## TODO

- [ ] Create OS hardening playbooks
- [ ] Configure kernel parameters for low latency
- [ ] Set up network tuning (io_uring support)
- [ ] Install runtime dependencies
- [ ] Configure monitoring agents
- [ ] Set up log shipping
