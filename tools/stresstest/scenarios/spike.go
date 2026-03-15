package scenarios

import (
	"context"
	"fmt"
	"sync"
	"time"

	"github.com/blazil/stresstest/metrics"
)

// Spike tests recovery from a sudden 10× concurrency spike.
// Phases: baseline (30 s) → spike (10 s) → recovery (30 s).
// Pass criteria:
//   - Error rate during spike < 1 %
//   - TPS recovers to ≥ 90 % of baseline TPS within one sample interval after spike ends.
func Spike(cfg Config) Result {
	pool, err := dialPool(cfg.Target)
	if err != nil {
		return Result{Name: "spike", Notes: fmt.Sprintf("dial error: %v", err)}
	}
	defer pool.close()

	col, stopCol := metrics.NewCollector()
	defer stopCol()

	// FIX 2: baseline 4 workers, spike to 8 workers
	// 4×256 = 1,024 concurrent → 8×256 = 2,048 concurrent
	const (
		baselineWorkers = 4
		spikeWorkers    = 8
		baselineDur     = 30 * time.Second
		spikeDur        = 10 * time.Second
		recoveryDur     = 30 * time.Second
	)

	interval := cfg.SampleInterval
	if interval == 0 {
		interval = 5 * time.Second
	}

	type phaseResult struct {
		name    string
		tps     float64
		p99Ms   float64
		errPct  float64
	}

	runPhase := func(name string, workers int, dur time.Duration) phaseResult {
		col.Reset()
		const warmup = 10 * time.Second
		ctx, cancel := context.WithTimeout(context.Background(), dur+warmup)
		defer cancel()

		var wg sync.WaitGroup
		for i := 0; i < workers; i++ {
			wg.Add(1)
			wIdx := i
			go func() {
				defer wg.Done()
				worker(ctx, pool.get(wIdx), col, int64(wIdx+workers*10000))
			}()
		}

		// 10 s warmup: discard metrics before the measurement window.
		time.Sleep(warmup)
		col.Reset()

		// Measurement window: let workers run for dur, then cancel.
		time.Sleep(dur)
		cancel()
		wg.Wait()

		total, success, failed, _, p99 := col.SnapshotDelta()
		tps := float64(success) / dur.Seconds()
		errPct := 0.0
		if total > 0 {
			errPct = float64(failed) / float64(total) * 100
		}
		fmt.Printf("  spike %-10s %4d workers → %9.0f TPS  P99 %.2f ms  err %.2f%%\n",
			name, workers, tps, p99, errPct)
		return phaseResult{name: name, tps: tps, p99Ms: p99, errPct: errPct}
	}

	baseline := runPhase("baseline", baselineWorkers, baselineDur)
	spike := runPhase("spike", spikeWorkers, spikeDur)
	recovery := runPhase("recovery", baselineWorkers, recoveryDur)

	spikeErrOK := spike.errPct < 1.0
	recoveryOK := baseline.tps == 0 || recovery.tps >= baseline.tps*0.90

	passed := spikeErrOK && recoveryOK
	notes := ""
	if !passed {
		notes = fmt.Sprintf("spike err %.2f%% (need <1%%), recovery TPS %.0f (need ≥ %.0f)",
			spike.errPct, recovery.tps, baseline.tps*0.90)
	}

	return Result{
		Name:    "spike",
		Passed:  passed,
		PeakTPS: spike.tps,
		P99Ms:   spike.p99Ms,
		ErrPct:  spike.errPct,
		Samples: []metrics.Sample{
			{Elapsed: baselineDur, TPS: baseline.tps, P99Ms: baseline.p99Ms, ErrPct: baseline.errPct},
			{Elapsed: baselineDur + spikeDur, TPS: spike.tps, P99Ms: spike.p99Ms, ErrPct: spike.errPct},
			{Elapsed: baselineDur + spikeDur + recoveryDur, TPS: recovery.tps, P99Ms: recovery.p99Ms, ErrPct: recovery.errPct},
		},
		Notes: notes,
	}
}
