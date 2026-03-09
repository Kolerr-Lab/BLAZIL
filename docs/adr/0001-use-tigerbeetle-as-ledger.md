# Architecture Decision Record: Use TigerBeetle as Ledger

## Status

Accepted

## Context

Blazil requires a high-performance, strictly consistent ledger for financial accounting. The ledger must:

1. **Guarantee strict serializability** - No possibility of inconsistent balances
2. **Support double-entry bookkeeping** - Every transaction has balanced debits and credits
3. **Handle 1M-10M TPS** - Must not be a bottleneck for transaction processing
4. **Provide microsecond latencies** - Sub-millisecond p99 latencies
5. **Ensure durability** - Zero data loss, crash-safe
6. **Scale horizontally** - Support clustering for fault tolerance

We evaluated three options:
- PostgreSQL (relational database)
- FoundationDB (distributed key-value store)
- TigerBeetle (purpose-built accounting database)

## Decision

We will use **TigerBeetle** as the foundational ledger for Blazil.

TigerBeetle is a distributed financial accounting database specifically designed for:
- High-throughput transaction processing
- Strict consistency guarantees
- Double-entry bookkeeping primitives
- Low-latency operations

### Integration Approach

1. **Direct Integration**: Use TigerBeetle's native client protocol from `blazil-ledger` crate
2. **Abstraction Layer**: Build domain-specific abstractions in `core/ledger/` that map financial concepts to TigerBeetle primitives
3. **Account Structure**: Map Blazil accounts to TigerBeetle accounts with appropriate ledger IDs
4. **Transaction Mapping**: Map Blazil transactions to TigerBeetle transfers with two-phase commit support

## Consequences

### Positive

**Performance**
- TigerBeetle can handle 1M+ TPS with single-digit millisecond latencies
- Purpose-built for financial workloads
- Minimal overhead compared to general-purpose databases

**Correctness**
- Built-in double-entry bookkeeping guarantees
- Strict serializable isolation
- Atomic multi-account transfers
- Cannot create invalid accounting states

**Operations**
- Simple deployment model (single binary)
- Efficient replication protocol
- Small memory footprint
- Predictable performance characteristics

**Development**
- Native Rust client library available
- Well-documented protocol
- Active development and community

### Negative

**Maturity**
- TigerBeetle is relatively new (v0.x releases)
- Smaller ecosystem than PostgreSQL
- Fewer third-party tools and integrations
- Less operational knowledge in the industry

**Flexibility**
- Purpose-built for accounting; not suitable for general data storage
- Requires additional databases for non-accounting data
- Limited query capabilities (get by ID, not arbitrary SQL)

**Learning Curve**
- Team needs to learn TigerBeetle's concepts and API
- Different mental model than traditional databases
- Requires thinking in terms of accounts and transfers

### Mitigation Strategies

1. **Abstraction Layer**: Build `blazil-ledger` crate to abstract TigerBeetle details
2. **Monitoring**: Implement comprehensive observability for ledger operations
3. **Testing**: Extensive integration tests and chaos testing
4. **Fallback Plan**: Design abstraction layer to allow swapping implementations if needed
5. **Community Engagement**: Contribute fixes/improvements back to TigerBeetle

## Alternatives Considered

### PostgreSQL
**Pros**:
- Mature and battle-tested
- Rich ecosystem and tooling
- Team familiarity
- Flexible query capabilities

**Cons**:
- Not designed for ultra-high-throughput transaction processing
- Requires careful schema design to enforce accounting invariants
- Lower throughput ceiling (even with optimizations)
- More complex operational overhead for HA setup
- Higher latencies at scale

**Decision**: Rejected due to throughput and latency requirements.

### FoundationDB
**Pros**:
- Excellent consistency guarantees
- High performance and scalability
- Battle-tested (Apple uses it for iCloud)
- Flexible layer architecture

**Cons**:
- No built-in accounting primitives
- Would need to implement double-entry bookkeeping ourselves
- More complex operational model
- Requires more resources (memory, CPU)
- Steeper learning curve

**Decision**: Rejected because building accounting on top of FoundationDB duplicates TigerBeetle's work.

## References

- [TigerBeetle Documentation](https://docs.tigerbeetle.com/)
- [TigerBeetle Design Document](https://github.com/tigerbeetle/tigerbeetle/blob/main/docs/DESIGN.md)
- [TigerBeetle vs Traditional Databases](https://tigerbeetle.com/blog/2023-07-11-we-put-half-a-million-dollars-on-the-line/)
