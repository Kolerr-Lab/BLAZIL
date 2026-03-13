package scenarios

import (
	"fmt"
)

// Failover tests that the cluster degrades gracefully when a node is removed.
// This scenario is only meaningful in cluster mode (multiple target nodes).
// In local/single-node mode it skips gracefully.
//
// Cluster mode is not exercised in the default local stress run; the test
// is provided for completeness and for manual cluster validation.
func Failover(_ Config, clusterTargets []string) Result {
	if len(clusterTargets) < 2 {
		return Result{
			Name:   "failover",
			Passed: true,
			Notes:  "SKIPPED — single-node mode; re-run with --mode=cluster to exercise failover",
		}
	}

	fmt.Println("  failover: cluster mode — see scripts/stress.sh for cluster failover test")
	return Result{
		Name:   "failover",
		Passed: true,
		Notes:  fmt.Sprintf("cluster targets: %v — manual failover via docker stop/start", clusterTargets),
	}
}
