# Blazil Production Overlay

Kubernetes manifests for production environment.

## Configuration

- Replica counts: 5+ (with HPA)
- Resource limits: production-grade
- Storage: persistent volumes with replication
- Ingress: LoadBalancer with TLS
- Monitoring: full observability stack
- Security: NetworkPolicies, PodSecurityPolicies
- High availability: anti-affinity rules
