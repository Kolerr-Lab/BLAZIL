package scenarios

import (
	"context"
	"fmt"
	"time"

	"github.com/blazil/stresstest/metrics"
)

// SustainConfig controls the sustained load parameters.
type SustainConfig struct {
	Goroutines int
	// MinTPS is the minimum acceptable sustained transactions per second.
	MinTPS float64
	// MaxP99Ms is the maximum acceptable P99 latency in milliseconds.
	MaxP99Ms float64
	// MaxErrPct is the maximum acceptable error percentage.
	MaxErrPct float64
}

// DefaultSustainConfig returns production-grade SLO parameters.
func DefaultSustainConfig() SustainConfig {
	return SustainConfig{
		Goroutines: 500,
		MinTPS:     10_000,
		MaxP99Ms:   10.0,
		MaxErrPct:  0.1,
	}
}

// Sustain runs a fixed number of concurrent workers for the full duration and
// samples metrics every SampleInterval. A run passes when:
//   - Mean sustained TPS > MinTPS
//   - P99 latency < MaxP99Ms
//   - Error rate < MaxErrPct
func Sustain(cfg Config, sc SustainConfig) Result {
	conn, err := dial(cfg.Target)
	if err != nil {
		return Result{Name: "sustain", Notes: fmt.Sprintf("dial error: %v", err)}
	}
	defer conn.Close()

	col, stopCol := metrics.NewCollector()
	defer stopCol()

	interval := cfg.SampleInterval
	if interval == 0 {
		interval = 5 * time.Second
	}

	ctx, cancel := context.WithTimeout(context.Background(), cfg.Duration)
	defer cancel()

	for i := 0; i < sc.Goroutines; i++ {
		go worker(ctx, conn, col, int64(i))
	}

	var samples []metrics.Sample
	elapsed := time.Duration(0)
	ticker := time.NewTicker(interval)
	defer ticker.Stop()
	start := time.Now()

	for {
		select {
		case <-ctx.Done():
			goto done
		case <-ticker.C:
			elapsed = time.Since(start)
			total, success, failed, p50, p99 := col.Snapshot()
			secs := interval.Seconds()
			tps := float64(success) / secs
			errPct := 0.0
			if total > 0 {
				errPct = float64(failed) / float64(total) * 100
			}
			s := metrics.Sample{
				Elapsed: elapsed,
				TPS:     tps,
				P50Ms:   p50,
				P99Ms:   p99,
				ErrPct:  errPct,
			}
			samples = append(samples, s)
			fmt.Printf("  sustain +%.0fs → %9.0f TPS  P50 %.2f ms  P99 %.2f ms  err %.2f%%\n",
				elapsed.Seconds(), tps, p50, p99, errPct)
		}
	}

done:
	// Final snapshot for any residual counts.
	{
		total, success, failed, p50, p99 := col.Snapshot()
		remaining := time.Since(start) - elapsed
		if remaining > 0 && (total > 0) {
			secs := remaining.Seconds()
			tps := float64(success) / secs
			errPct := 0.0
			if total > 0 {
				errPct = float64(failed) / float64(total) * 100
			}
			samples = append(samples, metrics.Sample{
				Elapsed: time.Since(start),
				TPS:     tps,
				P50Ms:   p50,
				P99Ms:   p99,
				ErrPct:  errPct,
			})
		}
	}

	// Aggregate metrics over steady-state samples (skip first sample = warm-up).
	var (
		sumTPS  float64
		maxP99  float64
		maxErr  float64
		counted int
		skip    = 1
	)
	for _, s := range samples {
		if skip > 0 {
			skip--
			continue
		}
		sumTPS += s.TPS
		if s.P99Ms > maxP99 {
			maxP99 = s.P99Ms
		}
		if s.ErrPct > maxErr {
			maxErr = s.ErrPct
		}
		counted++
	}
	var meanTPS float64
	if counted > 0 {
		meanTPS = sumTPS / float64(counted)
	} else if len(samples) > 0 {
		// Only one sample (short run) — use it directly.
		meanTPS = samples[0].TPS
		maxP99 = samples[0].P99Ms
		maxErr = samples[0].ErrPct
	}

	passed := meanTPS >= sc.MinTPS && maxP99 <= sc.MaxP99Ms && maxErr <= sc.MaxErrPct
	notes := ""
	if !passed {
		notes = fmt.Sprintf("SLO breach — TPS %.0f (need ≥ %.0f), P99 %.2f ms (need ≤ %.0f ms), err %.2f%% (need ≤ %.1f%%)",
			meanTPS, sc.MinTPS, maxP99, sc.MaxP99Ms, maxErr, sc.MaxErrPct)
	}

	// Accumulate totals from all sample windows.
	var peakTPS float64
	for _, s := range samples {
		if s.TPS > peakTPS {
			peakTPS = s.TPS
		}
	}

	return Result{
		Name:       "sustain",
		Passed:     passed,
		SustainTPS: meanTPS,
		PeakTPS:    peakTPS,
		P99Ms:      maxP99,
		ErrPct:     maxErr,
		Samples:    samples,
		Notes:      notes,
	}
}
