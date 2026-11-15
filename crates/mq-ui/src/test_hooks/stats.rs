//! Frame timing telemetry (`min`, `max`, `avg`, `p50`, `p95`, `p99`, histogram).

use std::time::Duration;

use serde_json::json;

const MAX_SAMPLES: usize = 50_000;

/// Tracks per-frame CPU render duration in milliseconds.
#[derive(Debug, Default)]
pub struct FrameStatsTracker {
    samples_ms: Vec<f64>,
    buckets: [u64; 7],
    min_ms: Option<f64>,
    max_ms: f64,
    sum_ms: f64,
    count: u64,
    over_8ms: u64,
    over_16ms: u64,
}

impl FrameStatsTracker {
    pub fn record(&mut self, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        self.count += 1;
        self.sum_ms += ms;
        self.min_ms = Some(self.min_ms.map_or(ms, |m| m.min(ms)));
        self.max_ms = self.max_ms.max(ms);

        if ms > 8.0 {
            self.over_8ms += 1;
        }
        if ms > 16.0 {
            self.over_16ms += 1;
        }

        let bucket_idx = if ms < 1.0 {
            0
        } else if ms < 2.0 {
            1
        } else if ms < 4.0 {
            2
        } else if ms < 8.0 {
            3
        } else if ms < 16.0 {
            4
        } else if ms < 33.0 {
            5
        } else {
            6
        };
        self.buckets[bucket_idx] += 1;

        if self.samples_ms.len() >= MAX_SAMPLES {
            self.samples_ms.remove(0);
        }
        self.samples_ms.push(ms);
    }

    pub fn reset(&mut self) {
        self.samples_ms.clear();
        self.buckets = [0; 7];
        self.min_ms = None;
        self.max_ms = 0.0;
        self.sum_ms = 0.0;
        self.count = 0;
        self.over_8ms = 0;
        self.over_16ms = 0;
    }

    pub fn to_json(&self) -> serde_json::Value {
        let mut sorted = self.samples_ms.clone();
        sorted.sort_by(|a, b| a.total_cmp(b));

        let percentile = |p: f64| -> f64 {
            if sorted.is_empty() {
                return 0.0;
            }
            let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
            sorted[idx.min(sorted.len() - 1)]
        };

        let avg_ms = if self.count > 0 {
            self.sum_ms / (self.count as f64)
        } else {
            0.0
        };

        json!({
            "count": self.count,
            "min_ms": self.min_ms.unwrap_or(0.0),
            "max_ms": self.max_ms,
            "avg_ms": avg_ms,
            "p50_ms": percentile(0.50),
            "p95_ms": percentile(0.95),
            "p99_ms": percentile(0.99),
            "over_8ms_count": self.over_8ms,
            "over_16ms_count": self.over_16ms,
            "histogram": {
                "<1ms": self.buckets[0],
                "1-2ms": self.buckets[1],
                "2-4ms": self.buckets[2],
                "4-8ms": self.buckets[3],
                "8-16ms": self.buckets[4],
                "16-33ms": self.buckets[5],
                ">33ms": self.buckets[6],
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::FrameStatsTracker;

    #[test]
    fn frame_stats_records_percentiles_and_histogram() {
        let mut stats = FrameStatsTracker::default();
        for ms in [0.5, 1.5, 3.0, 6.0, 12.0, 20.0, 40.0] {
            stats.record(Duration::from_secs_f64(ms / 1000.0));
        }
        let json = stats.to_json();
        assert_eq!(json["count"], 7);
        assert_eq!(json["over_8ms_count"], 3);
        assert_eq!(json["over_16ms_count"], 2);
        assert_eq!(json["histogram"]["<1ms"], 1);
        assert_eq!(json["histogram"][">33ms"], 1);
        assert_eq!(json["max_ms"], 40.0);

        stats.reset();
        let reset_json = stats.to_json();
        assert_eq!(reset_json["count"], 0);
        assert_eq!(reset_json["max_ms"], 0.0);
    }
}
