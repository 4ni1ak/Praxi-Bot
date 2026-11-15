//! Registers this computer's screens with the live-view registry (ADR-0029:
//! the CLI only registers existing backend instances).
//!
//! Registering creates NO capture, encoder or input worker: the targets stay
//! lazy until a viewer subscribes. Opening the desktop can touch the OS (an X
//! server that accepts connections but never answers, a locked Windows
//! session), so it never runs on the daemon's start path: [`DesktopRegistrar`]
//! probes on a worker thread with a deadline, and the API is up regardless.
//!
//! A screen that cannot be offered is recorded as a fixed [`UnavailableCode`]
//! served by `GET /v1/view/availability`. The raw backend message (paths, user
//! names, display sockets, environment values) goes only to the local daemon
//! log, through the production redactor, and never to a peer. A transient
//! failure is retried: when a viewer asks (rate-limited) and on a slow
//! background timer, never in a tight loop; success clears the note in the
//! same step as it registers the target.

use std::{
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use desktop_api::{DesktopBackend, LiveViewOptions};
use live_view::{
    Reason, TargetKind, TargetRegistry, TargetUnavailable, UnavailableCode, ViewTarget,
};
use parking_lot::Mutex;

/// Host id of this computer in target metadata.
pub const LOCAL_HOST_ID: &str = "local";

/// A desktop that could not be offered: the public code and reason, plus the
/// raw message for the local log only.
#[derive(Debug, Clone)]
pub struct StartFailure {
    pub code: UnavailableCode,
    pub reason: Reason,
    pub detail: String,
}

/// Opens the desktop target. May block on the OS; only ever called off the
/// start path, by [`DesktopRegistrar`].
pub type Opener = Arc<dyn Fn() -> Result<Arc<dyn ViewTarget>, StartFailure> + Send + Sync>;
/// Sink for the local, redacted diagnostic line.
pub type LogSink = Arc<dyn Fn(&str) + Send + Sync>;

/// Codes whose cause cannot change while the daemon runs.
const fn is_permanent(code: UnavailableCode) -> bool {
    matches!(
        code,
        UnavailableCode::DesktopOff
            | UnavailableCode::UnsupportedPlatform
            | UnavailableCode::Wayland
    )
}

fn code_for(reason: &Reason) -> UnavailableCode {
    match reason {
        Reason::Unsupported => UnavailableCode::UnsupportedPlatform,
        _ => UnavailableCode::ScreenUnavailable,
    }
}

/// Timing of probes. Production values are in [`Timing::default`].
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// A probe that has not answered after this long is reported `timed_out`.
    pub deadline: Duration,
    /// Earliest a viewer request may start another probe.
    pub min_interval: Duration,
    /// Period of the slow background retry while the desktop is missing.
    pub background: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            deadline: Duration::from_secs(15),
            min_interval: Duration::from_secs(5),
            background: Duration::from_secs(60),
        }
    }
}

#[derive(Default)]
struct State {
    /// A probe is running (it may be stuck in the OS; at most one at a time).
    in_flight: bool,
    /// Registered, or failed for a permanent reason: nothing more to try.
    done: bool,
    last_start: Option<Instant>,
    /// Bumped per probe so a late deadline timer cannot overwrite a newer note.
    attempt: u64,
}

/// Registers the local desktop without ever blocking the caller.
pub struct DesktopRegistrar {
    registry: Arc<TargetRegistry>,
    opener: Opener,
    log: LogSink,
    timing: Timing,
    state: Mutex<State>,
}

impl DesktopRegistrar {
    /// Start probing in the background and wire viewer-driven re-probes. Returns
    /// at once; until the first answer the registry says `starting`. Keep the
    /// returned handle for the daemon's lifetime.
    pub fn start(
        registry: Arc<TargetRegistry>,
        opener: Opener,
        log: LogSink,
        timing: Timing,
    ) -> Arc<Self> {
        let this = Arc::new(Self {
            registry,
            opener,
            log,
            timing,
            state: Mutex::new(State::default()),
        });
        // Weak: the registry must not keep the registrar (and its registry) alive.
        let weak = Arc::downgrade(&this);
        this.registry.set_reprobe({
            let weak = weak.clone();
            move || {
                if let Some(this) = weak.upgrade() {
                    this.request();
                }
            }
        });
        this.probe(false);
        spawn_background(weak, timing.background);
        this
    }

    /// A viewer (or the background timer) asks to look again. Does nothing when
    /// the desktop is registered or permanently impossible, while a probe is
    /// running, or when the last probe started less than `min_interval` ago.
    pub fn request(self: &Arc<Self>) {
        self.probe(true);
    }

    /// Number of probes started so far (diagnostics and tests).
    #[must_use]
    pub fn attempts(&self) -> u64 {
        self.state.lock().attempt
    }

    fn probe(self: &Arc<Self>, rate_limited: bool) {
        let attempt = {
            let mut st = self.state.lock();
            if st.done || st.in_flight {
                return;
            }
            if rate_limited
                && st
                    .last_start
                    .is_some_and(|t| t.elapsed() < self.timing.min_interval)
            {
                return;
            }
            st.in_flight = true;
            st.last_start = Some(Instant::now());
            st.attempt += 1;
            st.attempt
        };
        // Say "starting" only while nothing is known; a retry keeps the last
        // reason instead of flickering the panel (and its change events).
        if self.registry.unavailable().is_empty() {
            self.note(UnavailableCode::Starting, Reason::Unavailable);
        }
        let worker = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("praxi-view-probe".into())
            .spawn(move || {
                let result = (worker.opener)();
                worker.finish(result);
            });
        if spawned.is_err() {
            self.state.lock().in_flight = false;
            self.note(UnavailableCode::StartFailed, Reason::Unavailable);
            return;
        }
        // The deadline note is separate so a probe stuck in the OS cannot keep
        // the panel on "starting"; the probe still applies its late answer.
        let weak: Weak<Self> = Arc::downgrade(self);
        let deadline = self.timing.deadline;
        let _ = std::thread::Builder::new()
            .name("praxi-view-deadline".into())
            .spawn(move || {
                std::thread::sleep(deadline);
                if let Some(this) = weak.upgrade() {
                    let still_running = {
                        let st = this.state.lock();
                        st.in_flight && st.attempt == attempt
                    };
                    if still_running {
                        this.note(UnavailableCode::TimedOut, Reason::Unavailable);
                    }
                }
            });
    }

    fn finish(&self, result: Result<Arc<dyn ViewTarget>, StartFailure>) {
        match result {
            Ok(target) => {
                // Registering clears the note for this kind and host atomically.
                self.registry.register(target);
                let mut st = self.state.lock();
                st.in_flight = false;
                st.done = true;
            }
            Err(failure) => {
                (self.log)(&format!(
                    "live view: this computer's screen is not offered ({:?}): {}",
                    failure.code, failure.detail
                ));
                {
                    let mut st = self.state.lock();
                    st.in_flight = false;
                    st.done = is_permanent(failure.code);
                }
                self.note(failure.code, failure.reason);
            }
        }
    }

    fn note(&self, code: UnavailableCode, reason: Reason) {
        self.registry.note_unavailable(TargetUnavailable {
            kind: TargetKind::Desktop,
            host_id: LOCAL_HOST_ID.to_owned(),
            reason,
            code,
        });
    }
}

fn spawn_background(weak: Weak<DesktopRegistrar>, period: Duration) {
    let _ = std::thread::Builder::new()
        .name("praxi-view-retry".into())
        .spawn(move || {
            loop {
                std::thread::sleep(period);
                let Some(this) = weak.upgrade() else { return };
                if this.state.lock().done {
                    return;
                }
                this.request();
            }
        });
}

/// The production opener: `desktop` when the daemon already has a backend,
/// otherwise a fresh attempt to start one (so a locked session or a display
/// that comes up later recovers without a restart).
pub fn local_opener(desktop: Option<Arc<dyn DesktopBackend>>) -> Opener {
    let late: Mutex<Option<Arc<dyn DesktopBackend>>> = Mutex::new(desktop);
    Arc::new(move || {
        let backend = {
            let mut slot = late.lock();
            if let Some(b) = slot.as_ref() {
                Arc::clone(b)
            } else {
                if crate::host_runtime::desktop_switched_off() {
                    return Err(StartFailure {
                        code: UnavailableCode::DesktopOff,
                        reason: Reason::Unavailable,
                        detail: "desktop control is switched off (BOT_DESKTOP)".into(),
                    });
                }
                let b = crate::host_runtime::backend_checked().map_err(|f| StartFailure {
                    code: f.code,
                    reason: if is_permanent(f.code) {
                        Reason::Unsupported
                    } else {
                        Reason::Unavailable
                    },
                    detail: f.detail,
                })?;
                *slot = Some(Arc::clone(&b));
                b
            }
        };
        backend
            .live_view_target(&LiveViewOptions::new(LOCAL_HOST_ID))
            .map_err(|reason| StartFailure {
                code: code_for(&reason),
                detail: format!("the backend could not open the screen: {reason:?}"),
                reason,
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use desktop_api::{
        Action, ActionResult, Capabilities, DesktopError, Image, Quality, Region, Snapshot,
        WindowInfo,
        snapshot::{AppSpec, Pid},
    };
    use live_view::{
        Capabilities as ViewCaps, Codec, FrameSource, InputSink, SyntheticSource, TargetInfo,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use uuid::Uuid;

    struct Target {
        id: Uuid,
        source: Arc<SyntheticSource>,
    }

    #[async_trait]
    impl ViewTarget for Target {
        fn info(&self) -> TargetInfo {
            TargetInfo {
                target_id: self.id,
                kind: TargetKind::Desktop,
                host_id: LOCAL_HOST_ID.to_owned(),
                bot_id: None,
                run_id: None,
                generation: 1,
                capabilities: ViewCaps {
                    view: true,
                    input: false,
                    codecs: vec![Codec::Jpeg],
                    reason: None,
                },
                unavailable_reason: None,
            }
        }
        fn frame_source(&self) -> Arc<dyn FrameSource> {
            self.source.clone()
        }
        fn input_sink(&self) -> Option<Arc<dyn InputSink>> {
            None
        }
    }

    fn target() -> (Arc<dyn ViewTarget>, Arc<SyntheticSource>) {
        let source = Arc::new(SyntheticSource::default());
        let t: Arc<dyn ViewTarget> = Arc::new(Target {
            id: Uuid::now_v7(),
            source: Arc::clone(&source),
        });
        (t, source)
    }

    fn quiet() -> LogSink {
        Arc::new(|_| {})
    }

    fn fast() -> Timing {
        Timing {
            deadline: Duration::from_millis(150),
            min_interval: Duration::ZERO,
            background: Duration::from_secs(3600),
        }
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let start = Instant::now();
        while !ok() {
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "timed out: {what}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn codes(registry: &TargetRegistry) -> Vec<UnavailableCode> {
        registry.unavailable().iter().map(|n| n.code).collect()
    }

    fn fail(code: UnavailableCode, detail: &str) -> StartFailure {
        StartFailure {
            code,
            reason: Reason::Unavailable,
            detail: detail.to_owned(),
        }
    }

    /// 244-V #1: whatever the backend says, a peer only ever gets a fixed code.
    #[test]
    fn peers_get_reason_codes_never_paths_users_or_display_names() {
        let raw = "WAYLAND_DISPLAY=/home/alice/private/wayland-0 failed for C:\\Users\\alice\\AppData \
                   on DISPLAY=:alice-secret XDG_RUNTIME_DIR=/run/user/1000";
        let logged = Arc::new(Mutex::new(Vec::<String>::new()));
        let sink: LogSink = {
            let logged = Arc::clone(&logged);
            // Stands in for the production redactor: masks the user name.
            Arc::new(move |line: &str| logged.lock().push(line.replace("alice", "[REDACTED]")))
        };
        let registry = Arc::new(TargetRegistry::default());
        let opener: Opener = Arc::new(move || Err(fail(UnavailableCode::Wayland, raw)));
        let _r = DesktopRegistrar::start(Arc::clone(&registry), opener, sink, fast());
        wait_for("note", || codes(&registry) == [UnavailableCode::Wayland]);
        let wire = serde_json::to_string(&registry.unavailable()).unwrap();
        for secret in [
            "alice", "/home", "C:", "AppData", "secret", "1000", "DISPLAY",
        ] {
            assert!(!wire.contains(secret), "{secret} leaked into {wire}");
        }
        assert!(wire.contains("\"code\":\"wayland\""), "{wire}");
        // The detail went to the local log only, redacted.
        wait_for("log", || !logged.lock().is_empty());
        let log = logged.lock().join("\n");
        assert!(
            log.contains("[REDACTED]") && !log.contains("alice"),
            "{log}"
        );
    }

    /// 244-V #5: a probe that never returns cannot delay start or hang it.
    #[test]
    fn a_stalled_probe_never_blocks_start_and_reports_timed_out_then_recovers() {
        let (tx, rx) = std::sync::mpsc::channel::<()>();
        let rx = Mutex::new(rx);
        let (t, _source) = target();
        let registry = Arc::new(TargetRegistry::default());
        let opener: Opener = Arc::new(move || {
            // Stalled in the "OS" until the test releases it.
            let _ = rx.lock().recv();
            Ok(Arc::clone(&t))
        });
        let started = Instant::now();
        let _r = DesktopRegistrar::start(Arc::clone(&registry), opener, quiet(), fast());
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "start must not wait for the probe"
        );
        assert_eq!(codes(&registry), [UnavailableCode::Starting]);
        wait_for("deadline note", || {
            codes(&registry) == [UnavailableCode::TimedOut]
        });
        // The late answer still registers and clears the reason atomically.
        tx.send(()).unwrap();
        wait_for("late registration", || registry.targets().len() == 1);
        assert_eq!(registry.unavailable(), []);
    }

    /// 244-V #4: a locked session at start does not remove desktop live view for
    /// the daemon's lifetime; a viewer asking after unlock gets it.
    #[test]
    fn a_transient_failure_recovers_on_a_viewer_request_without_restart() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (t, source) = target();
        let registry = Arc::new(TargetRegistry::default());
        let opener: Opener = {
            let calls = Arc::clone(&calls);
            Arc::new(move || {
                if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                    Err(fail(UnavailableCode::ScreenUnavailable, "session locked"))
                } else {
                    Ok(Arc::clone(&t))
                }
            })
        };
        let r = DesktopRegistrar::start(Arc::clone(&registry), opener, quiet(), fast());
        wait_for("first failure", || {
            codes(&registry) == [UnavailableCode::ScreenUnavailable]
        });
        assert_eq!(registry.targets(), []);
        // The session is unlocked; the next viewer request (through the registry
        // hook the API handlers call) finds the screen.
        registry.request_reprobe();
        wait_for("recovery", || registry.targets().len() == 1);
        assert_eq!(
            registry.unavailable(),
            [],
            "the reason is cleared on success"
        );
        // Lazy: registering subscribed nothing.
        assert_eq!(source.subscribers(), 0);
        // Registered: further requests never probe again.
        registry.request_reprobe();
        registry.request_reprobe();
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(r.attempts(), 2);
    }

    /// 244-V #4: retries are bounded: rate-limited, and never for a permanent reason.
    #[test]
    fn retries_are_rate_limited_and_permanent_reasons_are_never_retried() {
        let calls = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(TargetRegistry::default());
        let opener: Opener = {
            let calls = Arc::clone(&calls);
            Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(fail(UnavailableCode::NoDisplay, "x"))
            })
        };
        let timing = Timing {
            min_interval: Duration::from_secs(3600),
            ..fast()
        };
        let _r = DesktopRegistrar::start(Arc::clone(&registry), opener, quiet(), timing);
        wait_for("failure", || {
            codes(&registry) == [UnavailableCode::NoDisplay]
        });
        for _ in 0..50 {
            registry.request_reprobe();
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a request storm is one probe"
        );

        let calls = Arc::new(AtomicUsize::new(0));
        let registry = Arc::new(TargetRegistry::default());
        let opener: Opener = {
            let calls = Arc::clone(&calls);
            Arc::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Err(fail(UnavailableCode::DesktopOff, "off"))
            })
        };
        let _r = DesktopRegistrar::start(Arc::clone(&registry), opener, quiet(), fast());
        wait_for("failure", || {
            codes(&registry) == [UnavailableCode::DesktopOff]
        });
        for _ in 0..10 {
            registry.request_reprobe();
        }
        std::thread::sleep(Duration::from_millis(50));
        assert_eq!(calls.load(Ordering::SeqCst), 1, "switched off stays off");
    }

    /// 244-V #4: the slow background retry alone finds a screen that appears
    /// later, and stops once it did (no tight loop).
    #[test]
    fn the_slow_background_retry_finds_a_late_screen_and_then_stops() {
        let calls = Arc::new(AtomicUsize::new(0));
        let (t, _source) = target();
        let registry = Arc::new(TargetRegistry::default());
        let opener: Opener = {
            let calls = Arc::clone(&calls);
            Arc::new(move || {
                if calls.fetch_add(1, Ordering::SeqCst) < 2 {
                    Err(fail(UnavailableCode::ScreenUnavailable, "no X yet"))
                } else {
                    Ok(Arc::clone(&t))
                }
            })
        };
        let timing = Timing {
            background: Duration::from_millis(40),
            ..fast()
        };
        let _r = DesktopRegistrar::start(Arc::clone(&registry), opener, quiet(), timing);
        wait_for("background recovery", || registry.targets().len() == 1);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(calls.load(Ordering::SeqCst), 3, "stopped after success");
        assert_eq!(registry.unavailable(), []);
    }

    /// A backend that counts how often it was asked for a view target.
    struct Fake {
        result: Result<Arc<dyn ViewTarget>, Reason>,
        asked: AtomicUsize,
    }

    #[async_trait]
    impl DesktopBackend for Fake {
        async fn snapshot(&self, _scope: Option<u64>) -> Result<Snapshot, DesktopError> {
            Err(DesktopError::Unsupported("fake".into()))
        }
        async fn act(&self, _action: Action) -> Result<ActionResult, DesktopError> {
            Err(DesktopError::Unsupported("fake".into()))
        }
        async fn screenshot(&self, _r: Region, _q: Quality) -> Result<Image, DesktopError> {
            Err(DesktopError::Unsupported("fake".into()))
        }
        async fn windows(&self) -> Result<Vec<WindowInfo>, DesktopError> {
            Ok(vec![])
        }
        async fn launch(&self, _app: AppSpec) -> Result<Pid, DesktopError> {
            Err(DesktopError::Unsupported("fake".into()))
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                platform: "fake".into(),
                accessibility_tree: false,
                screenshot: false,
                app_launch: false,
                has_display: false,
            }
        }
        fn live_view_target(
            &self,
            _options: &LiveViewOptions,
        ) -> Result<Arc<dyn ViewTarget>, Reason> {
            self.asked.fetch_add(1, Ordering::SeqCst);
            self.result.clone()
        }
    }

    #[test]
    fn the_production_opener_registers_a_lazy_target_or_maps_the_backends_reason() {
        let (t, source) = target();
        let backend = Arc::new(Fake {
            result: Ok(t),
            asked: AtomicUsize::new(0),
        });
        let dyn_backend: Arc<dyn DesktopBackend> = backend.clone();
        let registry = Arc::new(TargetRegistry::default());
        let _r = DesktopRegistrar::start(
            Arc::clone(&registry),
            local_opener(Some(dyn_backend)),
            quiet(),
            fast(),
        );
        wait_for("registered", || registry.targets().len() == 1);
        assert_eq!(registry.targets()[0].kind, TargetKind::Desktop);
        assert_eq!(backend.asked.load(Ordering::SeqCst), 1);
        assert_eq!(source.subscribers(), 0, "lazy: no capture before a viewer");

        for (reason, code) in [
            (Reason::Unavailable, UnavailableCode::ScreenUnavailable),
            (Reason::Unsupported, UnavailableCode::UnsupportedPlatform),
        ] {
            let backend: Arc<dyn DesktopBackend> = Arc::new(Fake {
                result: Err(reason),
                asked: AtomicUsize::new(0),
            });
            let registry = Arc::new(TargetRegistry::default());
            let _r = DesktopRegistrar::start(
                Arc::clone(&registry),
                local_opener(Some(backend)),
                quiet(),
                fast(),
            );
            wait_for("mapped", || codes(&registry) == [code]);
            assert_eq!(registry.targets(), []);
        }
    }
}
