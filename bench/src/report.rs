//! The result file and the comparison with an earlier one (the performance gate).
//!
//! A run writes one JSON file of named metrics. The nightly job keeps the last good file and
//! `vellora-bench compare` fails when a tracked metric got worse by more than the allowed share.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::Result;

/// The file format version; `compare` refuses files of another one.
const SCHEMA: u32 = 1;

/// One measured number.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Metric {
    /// Median over the runs.
    pub(crate) value: f64,
    /// `ms`, `MiB`, `tiles/s`, `pages/s`, `thumbnails/s`.
    pub(crate) unit: String,
    /// A bigger value is better (throughput); otherwise a smaller one is (time, memory).
    pub(crate) higher_is_better: bool,
    /// Whether `compare` gates on it. Untracked numbers are recorded for the record only.
    pub(crate) tracked: bool,
    /// A change smaller than this (in `unit`) never fails the gate, however large the share is:
    /// a 3 ms move of a 20 ms number is the machine, not the code.
    pub(crate) floor: f64,
}

/// Everything one run measured.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct Report {
    schema: u32,
    /// `windows`, `linux` or `macos`.
    pub(crate) os: String,
    pub(crate) arch: String,
    /// Repetitions behind each median.
    pub(crate) runs: usize,
    /// How the cold-cache numbers were obtained, per document: `dropped` (the OS file cache was
    /// emptied) or `fresh-file` (it could not be, so a freshly written file was opened).
    pub(crate) cold: BTreeMap<String, String>,
    pub(crate) metrics: BTreeMap<String, Metric>,
}

impl Report {
    pub(crate) fn new(runs: usize) -> Self {
        Self {
            schema: SCHEMA,
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            runs,
            cold: BTreeMap::new(),
            metrics: BTreeMap::new(),
        }
    }

    /// Records a time in milliseconds (smaller is better).
    pub(crate) fn time_ms(&mut self, name: String, value: f64, floor: f64) {
        self.add(name, value, "ms", false, true, floor);
    }

    /// Records a throughput (bigger is better).
    pub(crate) fn rate(&mut self, name: String, value: f64, unit: &str, floor: f64) {
        self.add(name, value, unit, true, true, floor);
    }

    /// Records a memory size; `tracked` is false where the OS gives no peak.
    pub(crate) fn memory_mib(&mut self, name: String, value: f64, tracked: bool) {
        self.add(name, value, "MiB", false, tracked, 8.0);
    }

    fn add(
        &mut self,
        name: String,
        value: f64,
        unit: &str,
        higher_is_better: bool,
        tracked: bool,
        floor: f64,
    ) {
        self.metrics.insert(
            name,
            Metric {
                value,
                unit: unit.into(),
                higher_is_better,
                tracked,
                floor,
            },
        );
    }

    pub(crate) fn write(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }

    pub(crate) fn read(path: &Path) -> Result<Self> {
        let report: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if report.schema != SCHEMA {
            return Err(format!(
                "{} has schema {}, this harness reads {SCHEMA}",
                path.display(),
                report.schema
            )
            .into());
        }
        Ok(report)
    }
}

/// How a tracked metric changed.
#[derive(Debug, PartialEq)]
pub(crate) struct Change {
    pub(crate) name: String,
    pub(crate) previous: f64,
    pub(crate) current: f64,
    pub(crate) unit: String,
    /// Positive: worse. As a share of the previous value.
    pub(crate) worse_by: f64,
    pub(crate) failed: bool,
}

/// Compares `current` with `previous`. A metric fails when it is worse by more than
/// `max_regression` (a share: 0.10 is 10 %) and by more than its floor. Metrics present in only
/// one file are ignored (a new metric has nothing to be compared with).
pub(crate) fn compare(previous: &Report, current: &Report, max_regression: f64) -> Vec<Change> {
    let mut changes = Vec::new();
    for (name, now) in &current.metrics {
        let Some(before) = previous.metrics.get(name) else {
            continue;
        };
        if !now.tracked || !before.tracked || before.value <= 0.0 {
            continue;
        }
        let delta = if now.higher_is_better {
            before.value - now.value
        } else {
            now.value - before.value
        };
        let worse_by = delta / before.value;
        changes.push(Change {
            name: name.clone(),
            previous: before.value,
            current: now.value,
            unit: now.unit.clone(),
            worse_by,
            failed: worse_by > max_regression && delta > now.floor,
        });
    }
    changes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(entries: &[(&str, f64, bool, bool)]) -> Report {
        let mut report = Report::new(3);
        for &(name, value, higher, tracked) in entries {
            report.add(name.into(), value, "u", higher, tracked, 0.0);
        }
        report
    }

    fn no_names() -> Vec<String> {
        Vec::new()
    }

    fn failed(previous: &Report, current: &Report) -> Vec<String> {
        compare(previous, current, 0.10)
            .into_iter()
            .filter(|c| c.failed)
            .map(|c| c.name)
            .collect()
    }

    #[test]
    fn a_slower_time_over_the_limit_fails() {
        let before = report(&[("open", 100.0, false, true)]);
        assert_eq!(
            failed(&before, &report(&[("open", 111.0, false, true)])),
            ["open"]
        );
        assert_eq!(
            failed(&before, &report(&[("open", 110.0, false, true)])),
            no_names()
        );
        assert_eq!(
            failed(&before, &report(&[("open", 60.0, false, true)])),
            no_names()
        );
    }

    #[test]
    fn a_lower_throughput_over_the_limit_fails() {
        let before = report(&[("rate", 100.0, true, true)]);
        assert_eq!(
            failed(&before, &report(&[("rate", 89.0, true, true)])),
            ["rate"]
        );
        assert_eq!(
            failed(&before, &report(&[("rate", 91.0, true, true)])),
            no_names()
        );
        assert_eq!(
            failed(&before, &report(&[("rate", 200.0, true, true)])),
            no_names()
        );
    }

    #[test]
    fn untracked_and_new_metrics_never_fail() {
        let before = report(&[("memory", 10.0, false, false)]);
        let now = report(&[("memory", 99.0, false, false), ("new", 1.0, false, true)]);
        assert_eq!(failed(&before, &now), no_names());
    }

    #[test]
    fn a_change_inside_the_floor_never_fails() {
        let mut before = Report::new(3);
        before.time_ms("open".into(), 20.0, 5.0);
        let mut now = Report::new(3);
        now.time_ms("open".into(), 24.0, 5.0);
        assert_eq!(failed(&before, &now), no_names());
        now.time_ms("open".into(), 26.0, 5.0);
        assert_eq!(failed(&before, &now), ["open"]);
    }

    #[test]
    fn a_report_survives_the_file() {
        let mut report = Report::new(5);
        report.time_ms("a/b".into(), 12.5, 2.0);
        report.cold.insert("doc".into(), "dropped".into());
        let path = std::env::temp_dir().join(format!("vellora-bench-{}.json", std::process::id()));
        report.write(&path).expect("write");
        let back = Report::read(&path).expect("read");
        let _ = std::fs::remove_file(&path);
        assert_eq!(back.metrics, report.metrics);
        assert_eq!(back.cold, report.cold);
        assert_eq!(back.runs, 5);
    }
}
