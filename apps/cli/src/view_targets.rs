//! Registers this computer's screens with the live-view registry at daemon
//! start (ADR-0029: the CLI only registers existing backend instances).
//!
//! Registering creates NO capture, encoder or input worker: the targets stay
//! lazy until a viewer subscribes. When a screen cannot be offered the reason
//! is recorded and served by `GET /v1/view/availability`, so the panel can say
//! why instead of showing an empty box.

use std::sync::Arc;

use desktop_api::{DesktopBackend, LiveViewOptions};
use live_view::{Reason, TargetKind, TargetRegistry, TargetUnavailable};

/// Host id of this computer in target metadata.
pub const LOCAL_HOST_ID: &str = "local";

/// Longest `detail` kept (a sentence, never a log).
const MAX_DETAIL: usize = 200;

fn clean(detail: &str) -> String {
    detail
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_DETAIL)
        .collect()
}

fn reason_detail(reason: &Reason) -> &'static str {
    match reason {
        Reason::Unsupported => "this platform has no screen capture backend",
        Reason::Unavailable => {
            "the screen could not be opened (no active display, a locked session or missing permission)"
        }
        _ => "the screen is not available",
    }
}

/// Register the local desktop as a view target when the backend can offer one,
/// otherwise record why not. `why_no_backend` explains an absent backend (it is
/// only called then).
pub fn register_local_desktop(
    registry: &TargetRegistry,
    desktop: Option<&Arc<dyn DesktopBackend>>,
    why_no_backend: impl FnOnce() -> String,
) {
    let note = |reason: Reason, detail: String| TargetUnavailable {
        kind: TargetKind::Desktop,
        host_id: LOCAL_HOST_ID.to_owned(),
        reason,
        detail: clean(&detail),
    };
    match desktop {
        None => registry.note_unavailable(note(Reason::Unavailable, why_no_backend())),
        Some(backend) => match backend.live_view_target(&LiveViewOptions::new(LOCAL_HOST_ID)) {
            Ok(target) => registry.register(target),
            Err(reason) => {
                let detail = reason_detail(&reason).to_owned();
                registry.note_unavailable(note(reason, detail));
            }
        },
    }
}

/// Why there is no desktop backend, in one sentence for the user.
#[must_use]
pub fn no_backend_detail() -> String {
    if matches!(
        std::env::var("BOT_DESKTOP").ok().as_deref(),
        Some("off" | "0" | "false")
    ) {
        return "desktop control is switched off (BOT_DESKTOP)".to_owned();
    }
    match crate::host_runtime::backend() {
        Err(error) => format!("desktop control could not start: {error}"),
        Ok(_) => "desktop control is not running".to_owned(),
    }
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
        ViewTarget,
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

    fn fake(result: Result<Arc<dyn ViewTarget>, Reason>) -> Arc<Fake> {
        Arc::new(Fake {
            result,
            asked: AtomicUsize::new(0),
        })
    }

    #[test]
    fn a_working_backend_registers_one_lazy_desktop_target() {
        let source = Arc::new(SyntheticSource::default());
        let target: Arc<dyn ViewTarget> = Arc::new(Target {
            id: Uuid::now_v7(),
            source: Arc::clone(&source),
        });
        let backend = fake(Ok(target));
        let dyn_backend: Arc<dyn DesktopBackend> = backend.clone();
        let registry = TargetRegistry::default();
        register_local_desktop(&registry, Some(&dyn_backend), || unreachable!());
        let targets = registry.targets();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].kind, TargetKind::Desktop);
        assert_eq!(targets[0].host_id, LOCAL_HOST_ID);
        assert_eq!(registry.unavailable(), []);
        assert_eq!(backend.asked.load(Ordering::SeqCst), 1);
        // Lazy: registering subscribed nothing, so no capture was started.
        assert_eq!(source.subscribers(), 0);
    }

    #[test]
    fn a_backend_that_cannot_open_the_screen_records_a_precise_reason() {
        for (reason, expect) in [
            (Reason::Unavailable, "no active display"),
            (Reason::Unsupported, "no screen capture backend"),
        ] {
            let backend: Arc<dyn DesktopBackend> = fake(Err(reason.clone()));
            let registry = TargetRegistry::default();
            register_local_desktop(&registry, Some(&backend), || unreachable!());
            assert_eq!(registry.targets(), []);
            let notes = registry.unavailable();
            assert_eq!(notes.len(), 1);
            assert_eq!(notes[0].kind, TargetKind::Desktop);
            assert_eq!(notes[0].reason, reason);
            assert!(notes[0].detail.contains(expect), "{}", notes[0].detail);
        }
    }

    #[test]
    fn no_backend_at_all_says_why_and_never_floods() {
        let registry = TargetRegistry::default();
        register_local_desktop(&registry, None, || {
            format!("desktop control is switched off\n{}", "x".repeat(500))
        });
        // Registering again replaces the note of the same kind and host.
        register_local_desktop(&registry, None, || "second\u{7}".to_owned());
        let notes = registry.unavailable();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].detail, "second");
        assert_eq!(registry.targets(), []);
        register_local_desktop(&registry, None, || "y".repeat(500));
        assert!(registry.unavailable()[0].detail.len() <= MAX_DETAIL);
    }
}
