//! Watches the app's own memory use, so a leak shows up in the log and in the
//! window long before it takes the machine down.
//!
//! A thread samples the app's memory every few seconds. It logs a summary now
//! and then, warns when memory grows fast or passes a soft limit, and (unless
//! switched off) stops the app above a hard limit: an app that is stopped is
//! better than a Mac that runs out of memory and panics, which is what a
//! runaway dashboard did on 2026-11-24.
//!
//! On macOS the size is the physical footprint, the figure Activity Monitor
//! shows as "Memory": unlike the resident size it includes compressed
//! memory, which is where a leak goes when the machine runs short. Elsewhere
//! it is the resident size.

use std::collections::VecDeque;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::config::MemoryConfig;

const MB: u64 = 1024 * 1024;

/// How far back fast growth is measured, and how long memory has to stay
/// calm before a fast-growth warning goes away.
const GROWTH_WINDOW_SECONDS: f64 = 60.0;

/// How often a summary line goes to the log at `info`.
const REPORT_EVERY_SECONDS: f64 = 300.0;

/// The highest default stop limit.
const DEFAULT_STOP_CAP_MB: u64 = 8192;

/// The warning the window shows, if any. The window subscribes to changes.
fn alarm_channel() -> &'static watch::Sender<Option<String>> {
    static ALARM: OnceLock<watch::Sender<Option<String>>> = OnceLock::new();
    ALARM.get_or_init(|| watch::Sender::new(None))
}

/// Yields the memory warning (or `None` once it clears) each time it changes.
pub fn alarms() -> impl iced::futures::Stream<Item = Option<String>> {
    let mut changes = alarm_channel().subscribe();
    // Also yield the warning in force when the window subscribes.
    changes.mark_changed();
    iced::futures::stream::unfold(changes, |mut changes| async move {
        changes.changed().await.ok()?;
        let alarm = changes.borrow_and_update().clone();
        Some((alarm, changes))
    })
}

/// Starts the watch thread. Does nothing if `config.check_seconds` is 0.
pub fn spawn_watch(config: MemoryConfig) {
    if config.check_seconds == 0 {
        tracing::info!("memory watch is off");
        return;
    }
    let stop_above_mb = config.stop_above_mb.unwrap_or_else(|| default_stop_mb(physical_memory_mb()));
    tracing::info!(
        warn_above_mb = config.warn_above_mb,
        growth_warn_mb = config.growth_warn_mb,
        stop_above_mb,
        "memory watch started"
    );
    let spawned = std::thread::Builder::new().name("memory-watch".into()).spawn(move || {
        let interval = Duration::from_secs(config.check_seconds);
        let started = Instant::now();
        let mut watch = Watch::new(config, stop_above_mb);
        let mut sampler = Sampler::new();
        loop {
            if let Some(bytes) = sampler.sample() {
                for event in watch.observe(started.elapsed().as_secs_f64(), bytes) {
                    event.log();
                }
                alarm_channel().send_if_modified(|shown| {
                    let alarm = watch.alarm();
                    let changed = *shown != alarm;
                    *shown = alarm;
                    changed
                });
            }
            std::thread::sleep(interval);
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "memory watch is off: could not start its thread");
    }
}

/// Half the computer's memory, but no more than 8 GB, so the app stops well
/// before the computer runs short whatever its size.
fn default_stop_mb(physical_mb: Option<u64>) -> u64 {
    physical_mb.map_or(DEFAULT_STOP_CAP_MB, |mb| (mb / 2).clamp(1, DEFAULT_STOP_CAP_MB))
}

fn physical_memory_mb() -> Option<u64> {
    let mut system = sysinfo::System::new();
    system.refresh_memory();
    Some(system.total_memory() / MB).filter(|&mb| mb > 0)
}

/// Reads this process's memory use.
struct Sampler {
    #[cfg(not(target_os = "macos"))]
    system: sysinfo::System,
    #[cfg(not(target_os = "macos"))]
    pid: Option<sysinfo::Pid>,
}

impl Sampler {
    fn new() -> Sampler {
        Sampler {
            #[cfg(not(target_os = "macos"))]
            system: sysinfo::System::new(),
            #[cfg(not(target_os = "macos"))]
            pid: sysinfo::get_current_pid().ok(),
        }
    }

    /// The physical footprint: resident plus compressed memory, as Activity
    /// Monitor counts it.
    #[cfg(target_os = "macos")]
    fn sample(&mut self) -> Option<u64> {
        let mut info = std::mem::MaybeUninit::<libc::rusage_info_v2>::zeroed();
        // SAFETY: `info` is a writable rusage_info_v2, the struct the
        // RUSAGE_INFO_V2 flavor fills in; it is read only after success.
        let status = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                info.as_mut_ptr().cast::<libc::rusage_info_t>(),
            )
        };
        // SAFETY: zero-initialised and, on success, filled in by the kernel.
        (status == 0).then(|| unsafe { info.assume_init() }.ri_phys_footprint)
    }

    /// The resident size.
    #[cfg(not(target_os = "macos"))]
    fn sample(&mut self) -> Option<u64> {
        use sysinfo::{ProcessRefreshKind, ProcessesToUpdate};
        let pid = self.pid?;
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&[pid]),
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        self.system.process(pid).map(|p| p.memory())
    }
}

/// What a sample of the app's memory calls for in the log.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A routine summary.
    Report { mb: u64, peak_mb: u64 },
    /// Memory grew by more than the configured amount within a minute.
    Growing { from_mb: u64, to_mb: u64, seconds: u64 },
    /// No fast growth for a minute after a fast-growth warning.
    Calm { mb: u64 },
    /// Memory passed the soft limit.
    Above { mb: u64, limit_mb: u64 },
    /// Memory is back under the soft limit.
    BackUnder { mb: u64 },
    /// Memory passed the hard limit; the app stops.
    Stop { mb: u64, limit_mb: u64 },
}

impl Event {
    fn log(&self) {
        match *self {
            Event::Report { mb, peak_mb } => tracing::info!(mb, peak_mb, "memory use"),
            Event::Growing { from_mb, to_mb, seconds } => {
                tracing::warn!(from_mb, to_mb, seconds, "memory is growing fast; possible leak")
            }
            Event::Calm { mb } => tracing::info!(mb, "memory stopped growing fast"),
            Event::Above { mb, limit_mb } => {
                tracing::warn!(mb, limit_mb, "memory use is above the warning limit; possible leak")
            }
            Event::BackUnder { mb } => tracing::info!(mb, "memory use is back under the warning limit"),
            Event::Stop { mb, limit_mb } => {
                tracing::error!(
                    mb,
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

/// Decides, sample by sample, what to log and what the window shows. Times
/// are seconds since the watch started, sizes are bytes.
#[derive(Debug)]
pub struct Watch {
    config: MemoryConfig,
    stop_above_mb: u64,
    /// Samples of the last minute, oldest first.
    recent: VecDeque<(f64, u64)>,
    peak: u64,
    /// The window's text for fast growth, while it lasts.
    growing: Option<String>,
    /// The window's text for the soft limit, while above it.
    above: Option<String>,
    last_growth_warning: Option<f64>,
    last_report: Option<f64>,
}

impl Watch {
    /// `stop_above_mb` is the hard limit in force (0 never stops).
    pub fn new(config: MemoryConfig, stop_above_mb: u64) -> Watch {
        Watch {
            config,
            stop_above_mb,
            recent: VecDeque::new(),
            peak: 0,
            growing: None,
            above: None,
            last_growth_warning: None,
            last_report: None,
        }
    }

    /// The warning the window shows now, if any.
    pub fn alarm(&self) -> Option<String> {
        match (&self.growing, &self.above) {
            (Some(g), Some(a)) => Some(format!("{a} {g}")),
            (g, a) => g.clone().or_else(|| a.clone()),
        }
    }

    pub fn observe(&mut self, at: f64, bytes: u64) -> Vec<Event> {
        let c = &self.config;
        let mut events = Vec::new();
        let mb = bytes / MB;
        self.peak = self.peak.max(bytes);
        tracing::debug!(mb, "memory sample");

        if self.stop_above_mb > 0 && mb >= self.stop_above_mb {
            events.push(Event::Stop { mb, limit_mb: self.stop_above_mb });
            return events;
        }

        while self.recent.front().is_some_and(|&(t, _)| at - t > GROWTH_WINDOW_SECONDS) {
            self.recent.pop_front();
        }
        self.recent.push_back((at, bytes));
        if c.growth_warn_mb > 0 {
            let &(since, lowest) = self.recent.iter().min_by_key(|(_, b)| *b).expect("just pushed");
            let warned_lately = self.last_growth_warning.is_some_and(|t| at - t < GROWTH_WINDOW_SECONDS);
            if bytes.saturating_sub(lowest) / MB >= c.growth_warn_mb {
                if !warned_lately {
                    let (from_mb, seconds) = (lowest / MB, (at - since).round() as u64);
                    events.push(Event::Growing { from_mb, to_mb: mb, seconds });
                    self.growing = Some(format!(
                        "Memory use grew from {from_mb} MB to {mb} MB in {seconds} s. This may be a leak; see the log."
                    ));
                    self.last_growth_warning = Some(at);
                }
            } else if self.growing.is_some() && !warned_lately {
                // A quiet minute since the last warning.
                self.growing = None;
                events.push(Event::Calm { mb });
            }
        }

        if c.warn_above_mb > 0 {
            if self.above.is_none() && mb >= c.warn_above_mb {
                events.push(Event::Above { mb, limit_mb: c.warn_above_mb });
            } else if self.above.is_some() && mb < c.warn_above_mb * 8 / 10 {
                // Some slack, so hovering around the limit doesn't flap.
                self.above = None;
                events.push(Event::BackUnder { mb });
            }
            if self.above.is_some() || mb >= c.warn_above_mb {
                self.above = Some(format!(
                    "Memory use is {mb} MB (warning limit {} MB). This may be a leak; see the log.",
                    c.warn_above_mb
                ));
            }
        }

        if self.last_report.is_none_or(|t| at - t >= REPORT_EVERY_SECONDS) {
            events.push(Event::Report { mb, peak_mb: self.peak / MB });
            self.last_report = Some(at);
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::{Event, MB, Sampler, Watch, default_stop_mb};
    use crate::config::MemoryConfig;

    fn config() -> MemoryConfig {
        MemoryConfig { check_seconds: 5, warn_above_mb: 2048, growth_warn_mb: 1024, stop_above_mb: None }
    }

    fn watch() -> Watch {
        Watch::new(config(), 8192)
    }

    fn alarms(events: Vec<Event>) -> Vec<Event> {
        events.into_iter().filter(|e| !matches!(e, Event::Report { .. })).collect()
    }

    #[test]
    fn reads_its_own_memory_use() {
        let bytes = Sampler::new().sample().unwrap();
        assert!(bytes > MB, "{bytes} bytes");
    }

    #[test]
    fn the_default_stop_limit_is_half_the_memory_up_to_8_gb() {
        assert_eq!(default_stop_mb(Some(8 * 1024)), 4096);
        assert_eq!(default_stop_mb(Some(64 * 1024)), 8192);
        assert_eq!(default_stop_mb(None), 8192);
    }

    #[test]
    fn steady_memory_only_reports() {
        let mut w = watch();
        assert_eq!(w.observe(0.0, 300 * MB), vec![Event::Report { mb: 300, peak_mb: 300 }]);
        for i in 1..60 {
            assert!(w.observe(i as f64 * 5.0, (300 + i % 3) * MB).is_empty(), "at {i}");
        }
        assert_eq!(w.observe(300.0, 300 * MB), vec![Event::Report { mb: 300, peak_mb: 302 }]);
        assert_eq!(w.alarm(), None);
    }

    #[test]
    fn fast_growth_warns_once_a_minute() {
        let mut w = watch();
        w.observe(0.0, 200 * MB);
        assert!(alarms(w.observe(5.0, 700 * MB)).is_empty());
        assert_eq!(alarms(w.observe(10.0, 1300 * MB)), vec![Event::Growing { from_mb: 200, to_mb: 1300, seconds: 10 }]);
        assert!(w.alarm().unwrap().contains("grew from 200 MB to 1300 MB"));
        // Still growing, but warned less than a minute ago.
        assert!(alarms(w.observe(15.0, 1900 * MB)).is_empty());
        // Samples older than a minute no longer count: 1300 → 1950 MB is slow
        // enough, and a minute has passed, so the warning goes away.
        assert_eq!(alarms(w.observe(70.0, 1950 * MB)), vec![Event::Calm { mb: 1950 }]);
        assert_eq!(w.alarm(), None);
        // Fast growth again warns again, together with the soft limit.
        assert_eq!(
            alarms(w.observe(80.0, 3100 * MB)),
            vec![
                Event::Growing { from_mb: 1950, to_mb: 3100, seconds: 10 },
                Event::Above { mb: 3100, limit_mb: 2048 },
            ]
        );
        let alarm = w.alarm().unwrap();
        assert!(alarm.contains("warning limit 2048 MB") && alarm.contains("grew from 1950 MB"), "{alarm}");
    }

    #[test]
    fn slow_growth_is_not_fast_growth() {
        let mut w = watch();
        // 100 MB a minute for 15 minutes: never 1 GB within a minute.
        for i in 0..180 {
            let events = alarms(w.observe(i as f64 * 5.0, (200 + i * 100 / 12) * MB));
            assert!(events.is_empty(), "at {i}: {events:?}");
        }
    }

    #[test]
    fn the_soft_limit_warns_once_until_memory_drops() {
        let mut w = watch();
        w.observe(0.0, 1900 * MB);
        assert_eq!(alarms(w.observe(60.0, 2100 * MB)), vec![Event::Above { mb: 2100, limit_mb: 2048 }]);
        assert!(alarms(w.observe(120.0, 2200 * MB)).is_empty());
        assert!(w.alarm().unwrap().contains("Memory use is 2200 MB"));
        assert!(alarms(w.observe(180.0, 1900 * MB)).is_empty());
        assert!(w.alarm().is_some());
        assert_eq!(alarms(w.observe(240.0, 1500 * MB)), vec![Event::BackUnder { mb: 1500 }]);
        assert_eq!(w.alarm(), None);
    }

    #[test]
    fn the_hard_limit_stops_and_zero_switches_it_off() {
        let mut w = watch();
        assert_eq!(w.observe(0.0, 9000 * MB), vec![Event::Stop { mb: 9000, limit_mb: 8192 }]);
        let quiet = MemoryConfig { warn_above_mb: 0, growth_warn_mb: 0, ..config() };
        let mut off = Watch::new(quiet, 0);
        assert_eq!(alarms(off.observe(0.0, 100 * MB)), vec![]);
        assert_eq!(alarms(off.observe(5.0, 90_000 * MB)), vec![]);
        assert_eq!(off.alarm(), None);
    }
}
