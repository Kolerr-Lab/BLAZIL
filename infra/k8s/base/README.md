# Kubernetes Manifests

This directory contains Kubernetes manifests for deploying Blazil in various environments.

## Structure

- `base/` - Base manifests (Kustomize base)
- `overlays/dev/` - Development overlay
- `overlays/staging/` - Staging overlay
- `overlays/prod/` - Production overlay

## Usage

```bash
# Apply dev environment
kubectl apply -k overlays/dev/

# Apply staging environment
kubectl apply -k overlays/staging/

# Apply production environment
kubectl apply -k overlays/prod/
```

## Prerequisites

- Kubernetes cluster 1.28+
- kubectl configured
- Kustomize (built into kubectl)

## TODO

- [ ] Create base manifests for core services
- [ ] Create deployment manifests for Go services
- [ ] Configure ingress and service mesh
- [ ] Set up HorizontalPodAutoscaler
- [ ] Configure PodDisruptionBudgets
- [ ] Set resource limits and requests
