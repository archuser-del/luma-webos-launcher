//! Presentation pacing is measured only in forced-redraw profiling mode. Idle
//! wakeups are not frames, and CPU submission time is not a GPU timestamp.
use serde::Serialize;
use std::time::Instant;

const WARMUP: usize = 10;

#[derive(Default)]
pub struct Profile {
    presented: usize,
    previous: Option<Instant>,
    intervals: Vec<f64>,
    cpu: Vec<f64>,
}
#[derive(Serialize)]
pub struct Timing {
    pub samples: usize,
    pub mean_ms: f64,
    pub p95_ms: f64,
    pub max_ms: f64,
}
#[derive(Serialize)]
pub struct Summary {
    pub presented: usize,
    pub warmup_frames: usize,
    pub pacing: Option<Timing>,
    pub cpu_submission: Option<Timing>,
}
impl Profile {
    pub fn record(&mut self, now: Instant, cpu_ms: f64, exclude: bool) {
        self.presented += 1;
        if self.presented > WARMUP
            && !exclude
            && let Some(previous) = self.previous
        {
            self.intervals
                .push(now.duration_since(previous).as_secs_f64() * 1000.);
            self.cpu.push(cpu_ms);
        }
        self.previous = Some(now);
    }
    pub fn pause(&mut self) {
        self.previous = None;
    }
    pub fn summary(&self) -> Summary {
        Summary {
            presented: self.presented,
            warmup_frames: WARMUP,
            pacing: summarize(&self.intervals),
            cpu_submission: summarize(&self.cpu),
        }
    }
}
fn summarize(samples: &[f64]) -> Option<Timing> {
    if samples.is_empty() {
        return None;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(f64::total_cmp);
    Some(Timing {
        samples: sorted.len(),
        mean_ms: sorted.iter().sum::<f64>() / sorted.len() as f64,
        p95_ms: sorted[(sorted.len() * 95).div_ceil(100) - 1],
        max_ms: *sorted.last()?,
    })
}

/// Auto UI resolution's view of how fast animations really present.
///
/// Remote presses animate for well under a second, so frames are gathered
/// across many animations over the last minute. The first frame of each
/// animation measures the idle wait before it and is left out.
#[derive(Default)]
pub struct AutoPacing {
    samples: std::collections::VecDeque<(Instant, f64)>,
    prior: Option<(Instant, bool)>,
}

/// A judgement on enough frames: (p95 ms, mean ms, too slow for 60 fps).
pub type Verdict = (f64, f64, bool);

impl AutoPacing {
    /// Frames needed before a judgement.
    pub const SAMPLES: usize = 90;

    /// One presented frame; `measured` when it was an animation frame.
    /// Returns a judgement once enough frames are in, and starts over.
    pub fn frame(&mut self, at: Instant, measured: bool) -> Option<Verdict> {
        if let Some((previous, was_measured)) = self.prior
            && measured
            && was_measured
        {
            let delta = at.duration_since(previous).as_secs_f64() * 1000.;
            if (0.0..250.0).contains(&delta) {
                self.samples.push_back((at, delta));
            }
        }
        self.prior = Some((at, measured));
        while self
            .samples
            .front()
            .is_some_and(|(t, _)| at.duration_since(*t).as_secs() > 60)
        {
            self.samples.pop_front();
        }
        if self.samples.len() < Self::SAMPLES {
            return None;
        }
        let gaps: Vec<f64> = self.samples.drain(..).map(|(_, d)| d).collect();
        let timing = summarize(&gaps)?;
        // A 60 Hz frame is 16.7 ms; a TV that misses it often is too slow.
        Some((
            timing.p95_ms,
            timing.mean_ms,
            timing.p95_ms > 18.5 && timing.mean_ms > 17.5,
        ))
    }

    /// Forget frames from before a change of drawing size.
    pub fn clear(&mut self) {
        self.samples.clear();
        self.prior = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn profiling_excludes_warmup_and_capture_stalls() {
        let mut profile = Profile::default();
        let start = Instant::now();
        for index in 0..20 {
            profile.record(start + Duration::from_millis(index * 16), 2., index == 19);
        }
        let summary = profile.summary();
        let pacing = summary.pacing.unwrap();
        assert_eq!(pacing.samples, 9);
        assert_eq!(pacing.mean_ms, 16.);
        assert_eq!(pacing.p95_ms, 16.);
        assert_eq!(summary.cpu_submission.unwrap().mean_ms, 2.);
    }

    /// Short animations, each separated by an idle pause, as remote presses
    /// give: `burst` frames `gap` ms apart, then 400 ms of nothing.
    fn presses(
        pacing: &mut AutoPacing,
        start: Instant,
        burst: u32,
        gap: u64,
        n: u32,
    ) -> Option<Verdict> {
        let mut at = start;
        let mut verdict = None;
        for _ in 0..n {
            for _ in 0..burst {
                at += Duration::from_millis(gap);
                verdict = verdict.or(pacing.frame(at, true));
            }
            at += Duration::from_millis(400);
            verdict = verdict.or(pacing.frame(at, false));
        }
        verdict
    }

    #[test]
    fn short_slow_animations_add_up_to_a_verdict() {
        // Your TV at 4K: half-second animations at about 20 ms a frame.
        let mut pacing = AutoPacing::default();
        let (p95, mean, slow) = presses(&mut pacing, Instant::now(), 25, 20, 10).unwrap();
        assert!(slow, "p95 {p95} mean {mean}");
        assert!(
            (mean - 20.).abs() < 0.01,
            "idle pauses are not frames: {mean}"
        );
    }

    #[test]
    fn smooth_animations_keep_the_resolution() {
        let mut pacing = AutoPacing::default();
        let (_, _, slow) = presses(&mut pacing, Instant::now(), 25, 16, 10).unwrap();
        assert!(!slow);
    }

    #[test]
    fn a_few_presses_are_not_enough_to_judge() {
        let mut pacing = AutoPacing::default();
        assert!(presses(&mut pacing, Instant::now(), 20, 25, 3).is_none());
    }
}
