//! Timestamped ping results, kept for the last 12 hours.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How far back results are kept (and the chart can show).
pub const RETAIN: Duration = Duration::from_secs(12 * 60 * 60);
/// Hard cap: 12 hours at the 250 ms minimum interval, about 4 MB.
const MAX_SAMPLES: usize = 175_000;
/// The packet-loss line is the loss over this many pings, ending at each point.
pub const LOSS_WINDOW: usize = 10;

#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub at: Instant,
    /// Round-trip time; `None` for a lost ping (timeout or error).
    pub rtt_ms: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct History {
    samples: VecDeque<Sample>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Summary {
    pub count: usize,
    pub avg_ms: Option<u32>,
    pub min_ms: Option<u32>,
    pub max_ms: Option<u32>,
    pub loss_pct: f32,
}

impl History {
    pub fn push(&mut self, rtt_ms: Option<u32>) {
        let now = Instant::now();
        self.samples.push_back(Sample { at: now, rtt_ms });
        while self.samples.len() > MAX_SAMPLES
            || self
                .samples
                .front()
                .is_some_and(|s| now.duration_since(s.at) > RETAIN)
        {
            self.samples.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn samples(&self) -> &VecDeque<Sample> {
        &self.samples
    }

    /// Stats over the most recent `last` samples (all of them for `None`).
    pub fn summary(&self, last: Option<usize>) -> Option<Summary> {
        let skip = last.map_or(0, |n| self.samples.len().saturating_sub(n));
        summarize(self.samples.iter().skip(skip))
    }
}

pub fn summarize<'a>(samples: impl Iterator<Item = &'a Sample>) -> Option<Summary> {
    let (mut count, mut replies, mut sum) = (0usize, 0usize, 0u64);
    let (mut min, mut max) = (None::<u32>, None::<u32>);
    for sample in samples {
        count += 1;
        if let Some(ms) = sample.rtt_ms {
            replies += 1;
            sum += u64::from(ms);
            min = Some(min.map_or(ms, |m| m.min(ms)));
            max = Some(max.map_or(ms, |m| m.max(ms)));
        }
    }
    (count > 0).then(|| Summary {
        count,
        avg_ms: (replies > 0).then(|| (sum / replies as u64) as u32),
        min_ms: min,
        max_ms: max,
        loss_pct: 100.0 * (count - replies) as f32 / count as f32,
    })
}

/// Rolling packet loss (percent) at each sample, over the `LOSS_WINDOW` pings
/// ending there.
pub fn rolling_loss(samples: &[Sample]) -> Vec<f32> {
    let mut lost = 0usize;
    samples
        .iter()
        .enumerate()
        .map(|(i, sample)| {
            lost += usize::from(sample.rtt_ms.is_none());
            if i >= LOSS_WINDOW {
                lost -= usize::from(samples[i - LOSS_WINDOW].rtt_ms.is_none());
            }
            100.0 * lost as f32 / (i + 1).min(LOSS_WINDOW) as f32
        })
        .collect()
}

/// One point on the chart: a sample, or the average of a run of them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point {
    pub at: Instant,
    /// Mean round-trip time of the replies; `None` if every ping was lost.
    pub rtt_ms: Option<f32>,
    /// Mean rolling packet loss, in percent.
    pub loss_pct: f32,
}

/// Reduces `samples` to at most `buckets` points by averaging them over equal
/// slices of time, so hours of fast pings draw as one point per pixel.
pub fn downsample(samples: &[Sample], buckets: usize) -> Vec<Point> {
    let loss = rolling_loss(samples);
    let (Some(first), Some(last)) = (samples.first(), samples.last()) else {
        return Vec::new();
    };
    if samples.len() <= buckets.max(1) {
        return samples
            .iter()
            .zip(loss)
            .map(|(s, loss_pct)| Point {
                at: s.at,
                rtt_ms: s.rtt_ms.map(|ms| ms as f32),
                loss_pct,
            })
            .collect();
    }

    let span = last
        .at
        .duration_since(first.at)
        .as_secs_f64()
        .max(f64::MIN_POSITIVE);
    let mut points = Vec::with_capacity(buckets);
    // (bucket, last time, rtt sum, replies, loss sum, samples)
    let mut acc: Option<(usize, Instant, f64, u32, f32, u32)> = None;
    let flush =
        |points: &mut Vec<Point>,
         (_, at, rtt, replies, loss, n): (usize, Instant, f64, u32, f32, u32)| {
            points.push(Point {
                at,
                rtt_ms: (replies > 0).then(|| (rtt / f64::from(replies)) as f32),
                loss_pct: loss / n as f32,
            });
        };
    for (sample, loss_pct) in samples.iter().zip(loss) {
        let offset = sample.at.duration_since(first.at).as_secs_f64() / span;
        let bucket = ((offset * buckets as f64) as usize).min(buckets - 1);
        if acc.is_some_and(|a| a.0 != bucket) {
            flush(&mut points, acc.take().unwrap());
        }
        let a = acc.get_or_insert((bucket, sample.at, 0.0, 0, 0.0, 0));
        a.1 = sample.at;
        if let Some(ms) = sample.rtt_ms {
            a.2 += f64::from(ms);
            a.3 += 1;
        }
        a.4 += loss_pct;
        a.5 += 1;
    }
    if let Some(a) = acc {
        flush(&mut points, a);
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples(rtts: &[Option<u32>]) -> Vec<Sample> {
        let at = Instant::now();
        rtts.iter().map(|&rtt_ms| Sample { at, rtt_ms }).collect()
    }

    #[test]
    fn summary_counts_losses_and_ignores_them_for_latency() {
        let s = samples(&[Some(10), None, Some(30), None]);
        let summary = summarize(s.iter()).unwrap();
        assert_eq!(summary.count, 4);
        assert_eq!(summary.avg_ms, Some(20));
        assert_eq!(summary.min_ms, Some(10));
        assert_eq!(summary.max_ms, Some(30));
        assert_eq!(summary.loss_pct, 50.0);
    }

    #[test]
    fn rolling_loss_uses_a_sliding_window() {
        let mut rtts = vec![None];
        rtts.extend([Some(1); LOSS_WINDOW]);
        let loss = rolling_loss(&samples(&rtts));
        assert_eq!(loss[0], 100.0);
        assert_eq!(loss[1], 50.0);
        // Window still includes the loss at index 0.
        assert_eq!(loss[LOSS_WINDOW - 1], 10.0);
        // Now it has slid past it.
        assert_eq!(loss[LOSS_WINDOW], 0.0);
    }

    #[test]
    fn downsample_averages_into_time_buckets() {
        let start = Instant::now();
        let samples: Vec<Sample> = (0..100u32)
            .map(|i| Sample {
                at: start + Duration::from_secs(u64::from(i)),
                // Every 10th ping in the first half is lost.
                rtt_ms: (i >= 50 || i % 10 != 0).then_some(i),
            })
            .collect();
        let points = downsample(&samples, 10);
        assert_eq!(points.len(), 10);
        // Bucket 9 holds samples 90..=99: mean 94.5.
        assert_eq!(points[9].rtt_ms, Some(94.5));
        assert_eq!(points[9].at, samples[99].at);
        assert_eq!(points[9].loss_pct, 0.0);
        assert!(points[0].loss_pct > 0.0);

        // Few enough samples: passed through unchanged.
        assert_eq!(downsample(&samples[..5], 10).len(), 5);
    }
}
