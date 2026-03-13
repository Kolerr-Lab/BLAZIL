// Package sharding provides consistent hashing and shard routing for Blazil's
// distributed transaction processing layer. It enables horizontal scaling by
// mapping AccountIDs to specific shard nodes using jump consistent hash,
// ensuring the same account always routes to the same node with minimal
// remapping when the cluster topology changes.
package sharding

// ConsistentHasher maps uint64 keys to shard IDs using Google's jump
// consistent hash algorithm. It is stateless and safe for concurrent use.
type ConsistentHasher struct{}

// ShardOf returns the shard ID in [0, numShards) for the given accountID.
// Identical to calling JumpHash(accountID, numShards).
func (c ConsistentHasher) ShardOf(accountID uint64, numShards int) int {
	return JumpHash(accountID, numShards)
}

// JumpHash implements Google's jump consistent hash algorithm.
//
// It maps key to a bucket in [0, numBuckets) with O(ln n) time complexity and
// minimal key remapping when the number of buckets changes. Specifically, when
// a bucket is added, only 1/(n+1) of keys are remapped — the theoretical
// minimum.
//
// Reference: "A Fast, Minimal Memory, Consistent Hash Algorithm"
// (Lamping & Veach, 2014) — https://arxiv.org/abs/1406.2294
func JumpHash(key uint64, numBuckets int) int {
	b, j := -1, 0
	for j < numBuckets {
		b = j
		key = key*2862933555777941757 + 1
		j = int(float64(b+1) *
			(float64(int64(1)<<31) /
				float64((key>>33)+1)))
	}
	return b
}
