// Command stresstest runs a multi-scenario stress test against the Blazil
// payments service and writes a Markdown report.
//
// Usage:
//
//	go run . [flags]
//
// Flags:
//
//	--target      host:port of the payments gRPC service (default: localhost:50051)
//	--duration    sustain scenario duration (default: 60s)
//	--report      path to write the Markdown report (default: docs/stress-report.md)
//	--mode        local | cluster (default: local)
//	--nodes       comma-separated host:port list for cluster mode
package main

import (
	"flag"
	"fmt"
	"os"
	"strings"
	"time"

	"github.com/blazil/stresstest/report"
	"github.com/blazil/stresstest/scenarios"
)

func main() {
	target := flag.String("target", "localhost:50051", "host:port of the payments gRPC service")
	duration := flag.Duration("duration", 60*time.Second, "sustain scenario duration")
	reportPath := flag.String("report", "docs/stress-report.md", "output path for the Markdown report")
	mode := flag.String("mode", "local", "local | cluster")
	nodesFlag := flag.String("nodes", "", "comma-separated host:port list for cluster mode")
	flag.Parse()

	var clusterTargets []string
	if *nodesFlag != "" {
		for _, n := range strings.Split(*nodesFlag, ",") {
			n = strings.TrimSpace(n)
			if n != "" {
				clusterTargets = append(clusterTargets, n)
			}
		}
	}
	if *mode == "cluster" && len(clusterTargets) > 0 {
		*target = clusterTargets[0]
	}

	cfg := scenarios.Config{
		Target:         *target,
		Duration:       *duration,
		SampleInterval: 5 * time.Second,
	}

	fmt.Printf("Blazil Stress Test\n")
	fmt.Printf("  target:   %s\n", *target)
	fmt.Printf("  mode:     %s\n", *mode)
	fmt.Printf("  duration: %s\n", *duration)
	fmt.Printf("  report:   %s\n\n", *reportPath)

	start := time.Now()
	var results []scenarios.Result

	// ── Ramp ─────────────────────────────────────────────────────────────────
	fmt.Println("==> Scenario: ramp (1→8 goroutines, 10 s per step, 10 s warmup, 256 window each)")
	rampResult := scenarios.Ramp(cfg)
	results = append(results, rampResult)
	printResult(rampResult)

	// ── Sustain ───────────────────────────────────────────────────────────────
	fmt.Printf("==> Scenario: sustain (8 goroutines × 256 window, %s, 10 s warmup)\n", *duration)
	sustainResult := scenarios.Sustain(cfg, scenarios.DefaultSustainConfig())
	results = append(results, sustainResult)
	printResult(sustainResult)

	// ── Spike ─────────────────────────────────────────────────────────────────
	fmt.Println("==> Scenario: spike (4→8→4 goroutines × 256 window, 10 s warmup per phase)")
	spikeResult := scenarios.Spike(cfg)
	results = append(results, spikeResult)
	printResult(spikeResult)

	// ── Failover ──────────────────────────────────────────────────────────────
	fmt.Println("==> Scenario: failover")
	failoverResult := scenarios.Failover(cfg, clusterTargets)
	results = append(results, failoverResult)
	printResult(failoverResult)

	totalDur := time.Since(start)

	// Generate and write report.
	md := report.Generate(*target, results, totalDur)
	if err := os.WriteFile(*reportPath, []byte(md), 0o644); err != nil {
		fmt.Fprintf(os.Stderr, "WARNING: could not write report to %s: %v\n", *reportPath, err)
		fmt.Println("\n--- Report (stdout) ---")
		fmt.Println(md)
	} else {
		fmt.Printf("\nReport written to %s\n", *reportPath)
	}

	// Exit non-zero if any scenario failed.
	for _, r := range results {
		if !r.Passed {
			os.Exit(1)
		}
	}
}

func printResult(r scenarios.Result) {
	verdict := "PASS"
	if !r.Passed {
		verdict = "FAIL"
	}
	fmt.Printf("  [%s] %s — peak %.0f TPS, P99 %.2f ms, err %.2f%%\n",
		verdict, r.Name, r.PeakTPS, r.P99Ms, r.ErrPct)
	if r.Notes != "" {
		fmt.Printf("       note: %s\n", r.Notes)
	}
	fmt.Println()
}
