//! Watches the app's own memory use, so a leak shows up in the log and in the
//! window long before it takes the machine down.
//!
//! A thread samples the resident size every few seconds. It logs a summary
//! now and then, warns when memory grows fast or passes a soft limit, and
//! (unless switched off) stops the app above a hard limit: an app that is
//! killed is better than a Mac that runs out of memory and panics, which is
//! what a runaway dashboard did on 2026-11-24.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};

use crate::config::MemoryConfig;

const MB: u64 = 1024 * 1024;

/// How far back fast growth is measured.
const GROWTH_WINDOW_SECONDS: f64 = 60.0;

/// How often a summary line goes to the log at `info`.
const REPORT_EVERY_SECONDS: f64 = 300.0;

/// The latest warning, for the window to show.
static ALARM: Mutex<Option<String>> = Mutex::new(None);

/// The memory warning to show in the window, if any.
pub fn alarm() -> Option<String> {
    ALARM.lock().ok()?.clone()
}

fn set_alarm(message: Option<String>) {
    if let Ok(mut alarm) = ALARM.lock() {
        *alarm = message;
    }
}

/// Starts the watch thread. Does nothing if `config.check_seconds` is 0.
pub fn spawn_watch(config: MemoryConfig) {
    if config.check_seconds == 0 {
        tracing::info!("memory watch is off");
        return;
    }
    let Ok(pid) = sysinfo::get_current_pid() else {
        tracing::warn!("memory watch is off: can't find this process");
        return;
    };
    let spawned = std::thread::Builder::new().name("memory-watch".into()).spawn(move || {
        let interval = Duration::from_secs(config.check_seconds);
        let started = Instant::now();
        let mut system = System::new();
        let mut watch = Watch::new(config);
        loop {
            if let Some(rss) = resident_size(&mut system, pid) {
                for event in watch.observe(started.elapsed().as_secs_f64(), rss) {
                    event.report();
                }
            }
            std::thread::sleep(interval);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "memory watch is off: could not start its thread");
    }
}

/// The resident size of process `pid`, in bytes.
fn resident_size(system: &mut System, pid: sysinfo::Pid) -> Option<u64> {
    system.refresh_processes_specifics(ProcessesToUpdate::Some(&[pid]), true, ProcessRefreshKind::nothing().with_memory());
    system.process(pid).map(|p| p.memory())
}

/// What a sample of the resident size calls for.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A routine summary.
    Report { rss_mb: u64, peak_mb: u64 },
    /// Memory grew by more than the configured amount within a minute.
    Growing { from_mb: u64, to_mb: u64, seconds: u64 },
    /// Memory passed the soft limit.
    Above { rss_mb: u64, limit_mb: u64 },
    /// Memory is back under the soft limit.
    BackUnder { rss_mb: u64 },
    /// Memory passed the hard limit; the app stops.
    Stop { rss_mb: u64, limit_mb: u64 },
}

impl Event {
    fn report(&self) {
        match *self {
            Event::Report { rss_mb, peak_mb } => tracing::info!(rss_mb, peak_mb, "memory use"),
            Event::Growing { from_mb, to_mb, seconds } => {
                tracing::warn!(from_mb, to_mb, seconds, "memory is growing fast; possible leak");
                set_alarm(Some(format!(
                    "Memory use grew from {from_mb} MB to {to_mb} MB in {seconds} s. This may be a leak; see the log."
                )));
            }
            Event::Above { rss_mb, limit_mb } => {
                tracing::warn!(rss_mb, limit_mb, "memory use is above the warning limit; possible leak");
                set_alarm(Some(format!(
                    "Memory use is {rss_mb} MB, above the {limit_mb} MB warning limit. This may be a leak; see the log."
                )));
            }
            Event::BackUnder { rss_mb } => {
                tracing::info!(rss_mb, "memory use is back under the warning limit");
                set_alarm(None);
            }
            Event::Stop { rss_mb, limit_mb } => {
                tracing::error!(
                    rss_mb,
                    limit_mb,
                    "memory use is above the stop limit; stopping the app to protect the computer \
                     (set memory.stop_above_mb = 0 in config.toml to never stop)"
                );
                // Give the log file writer a moment to write the line.
                std::thread::sleep(Duration::from_millis(500));
                std::process::exit(2);
            }
        }
    }
}

/// Decides, sample by sample, what to report. Times are seconds since the
/// watch started, sizes are bytes.
#[derive(Debug)]
pub struct Watch {
    config: MemoryConfig,
    /// Samples of the last minute, oldest first.
    recent: VecDeque<(f64, u64)>,
    peak: u64,
    above: bool,
    last_growth_warning: Option<f64>,
    last_report: Option<f64>,
}

impl Watch {
    pub fn new(config: MemoryConfig) -> Watch {
        Watch { config, recent: VecDeque::new(), peak: 0, above: false, last_growth_warning: None, last_report: None }
    }

    pub fn observe(&mut self, at: f64, rss: u64) -> Vec<Event> {
        let c = &self.config;
        let mut events = Vec::new();
        let rss_mb = rss / MB;
        self.peak = self.peak.max(rss);
        tracing::debug!(rss_mb, "memory sample");

        if c.stop_above_mb > 0 && rss_mb >= c.stop_above_mb {
            events.push(Event::Stop { rss_mb, limit_mb: c.stop_above_mb });
            return events;
        }

        while self.recent.front().is_some_and(|&(t, _)| at - t > GROWTH_WINDOW_SECONDS) {
            self.recent.pop_front();
        }
        self.recent.push_back((at, rss));
        if c.growth_warn_mb > 0 {
            let &(since, lowest) = self.recent.iter().min_by_key(|(_, bytes)| *bytes).expect("just pushed");
            let quiet = self.last_growth_warning.is_none_or(|t| at - t >= GROWTH_WINDOW_SECONDS);
            if rss.saturating_sub(lowest) / MB >= c.growth_warn_mb && quiet {
                events.push(Event::Growing { from_mb: lowest / MB, to_mb: rss_mb, seconds: (at - since).round() as u64 });
                self.last_growth_warning = Some(at);
            }
        }

        if c.warn_above_mb > 0 {
            if !self.above && rss_mb >= c.warn_above_mb {
                self.above = true;
                events.push(Event::Above { rss_mb, limit_mb: c.warn_above_mb });
            } else if self.above && rss_mb < c.warn_above_mb * 8 / 10 {
                // Some slack, so hovering around the limit doesn't flap.
                self.above = false;
                events.push(Event::BackUnder { rss_mb });
            }
        }

        if self.last_report.is_none_or(|t| at - t >= REPORT_EVERY_SECONDS) {
            events.push(Event::Report { rss_mb, peak_mb: self.peak / MB });
            self.last_report = Some(at);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, MB, Watch};
    use crate::config::MemoryConfig;

    fn watch() -> Watch {
        Watch::new(MemoryConfig { check_seconds: 5, warn_above_mb: 2048, growth_warn_mb: 1024, stop_above_mb: 8192 })
    }

    fn alarms(events: Vec<Event>) -> Vec<Event> {
        events.into_iter().filter(|e| !matches!(e, Event::Report { .. })).collect()
    }

    #[test]
    fn reads_its_own_resident_size() {
        let pid = sysinfo::get_current_pid().unwrap();
        let rss = super::resident_size(&mut sysinfo::System::new(), pid).unwrap();
        assert!(rss > MB, "{rss} bytes");
    }

    #[test]
    fn steady_memory_only_reports() {
        let mut w = watch();
        assert_eq!(w.observe(0.0, 300 * MB), vec![Event::Report { rss_mb: 300, peak_mb: 300 }]);
        for i in 1..60 {
            assert!(w.observe(i as f64 * 5.0, (300 + i % 3) * MB).is_empty(), "at {i}");
        }
        assert_eq!(w.observe(300.0, 300 * MB), vec![Event::Report { rss_mb: 300, peak_mb: 302 }]);
    }

    #[test]
    fn fast_growth_warns_once_a_minute() {
        let mut w = watch();
        w.observe(0.0, 200 * MB);
        assert!(alarms(w.observe(5.0, 700 * MB)).is_empty());
        assert_eq!(alarms(w.observe(10.0, 1300 * MB)), vec![Event::Growing { from_mb: 200, to_mb: 1300, seconds: 10 }]);
        // Still growing, but warned less than a minute ago.
        assert!(alarms(w.observe(15.0, 1900 * MB)).is_empty());
        // Samples older than a minute no longer count: 1300 → 1950 MB is slow enough.
        assert!(alarms(w.observe(70.0, 1950 * MB)).is_empty());
        // A minute after the last warning, fast growth warns again.
        assert_eq!(
            alarms(w.observe(80.0, 3100 * MB)),
            vec![
                Event::Growing { from_mb: 1950, to_mb: 3100, seconds: 10 },
                Event::Above { rss_mb: 3100, limit_mb: 2048 },
            ]
        );
    }

    #[test]
    fn slow_growth_is_not_fast_growth() {
        let mut w = watch();
        // 100 MB a minute for 15 minutes: never 1 GB within a minute.
        for i in 0..180 {
            let events = alarms(w.observe(i as f64 * 5.0, (200 + i * 100 / 12) * MB));
            assert!(events.iter().all(|e| !matches!(e, Event::Growing { .. })), "at {i}: {events:?}");
        }
    }

    #[test]
    fn the_soft_limit_warns_once_until_memory_drops() {
        let mut w = watch();
        w.observe(0.0, 1900 * MB);
        assert_eq!(alarms(w.observe(60.0, 2100 * MB)), vec![Event::Above { rss_mb: 2100, limit_mb: 2048 }]);
        assert!(alarms(w.observe(120.0, 2200 * MB)).is_empty());
        assert!(alarms(w.observe(180.0, 1900 * MB)).is_empty());
        assert_eq!(alarms(w.observe(240.0, 1500 * MB)), vec![Event::BackUnder { rss_mb: 1500 }]);
    }

    #[test]
    fn the_hard_limit_stops_and_zero_switches_it_off() {
        let mut w = watch();
        assert_eq!(w.observe(0.0, 9000 * MB), vec![Event::Stop { rss_mb: 9000, limit_mb: 8192 }]);
        let mut off = Watch::new(MemoryConfig { stop_above_mb: 0, warn_above_mb: 0, growth_warn_mb: 0, check_seconds: 5 });
        assert_eq!(alarms(off.observe(0.0, 100 * MB)), vec![]);
        assert_eq!(alarms(off.observe(5.0, 90_000 * MB)), vec![]);
    }
}
