use super::{Actor, ApiError, AppState, audit, end_session, input, interrupt};
use axum::{
    Extension,
    body::Bytes,
    extract::{Path, State},
    http::StatusCode,
};
use axum::{
    body::Body,
    http::{HeaderMap, HeaderValue, header},
    response::Response,
};
use live_view::wire::Record;
use live_view::{EffectiveProfile, MEDIA_TYPE, Reason, RecordHeader, ViewState};
use std::{sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// A takeover is interrupted when no heartbeat/frame publication was seen for this long
/// (was 2 s: one slow relay round trip dropped control).
const TAKEOVER_STALE_AFTER: Duration = Duration::from_secs(5);
/// How long the producer waits for the transport to take the next chunk before it ends the
/// stream (was 2 s). Only the newest frame is kept while waiting.
const SEND_STALL: Duration = Duration::from_secs(8);
/// A frame the viewer has not acknowledged after this long halves the send rate (the stream
/// stays open: a slow link is not a reason to drop the screen).
const ACK_TIMEOUT: Duration = Duration::from_millis(1500);
/// Ack-paced send rate: this share of the measured throughput, within these bounds (bytes/s).
const RATE_SHARE: f64 = 0.7;
const RATE_FLOOR: f64 = 32.0 * 1024.0;
const RATE_CEIL: f64 = 1024.0 * 1024.0;
/// Frames smaller than this say more about latency than about throughput: no rate sample.
const RATE_SAMPLE_MIN_BYTES: usize = 16 * 1024;

/// The ack-paced sender: at most one frame is unacknowledged once the viewer has acked at all
/// (an older client that never acks keeps the plain token-bucket behaviour), and the byte rate
/// follows what the acks show the link can carry.
#[derive(Default)]
pub(super) struct AckPacer {
    /// The frame sent and not yet acked: sequence, wire bytes, time.
    inflight: Option<(u64, usize, Instant)>,
    /// Smoothed throughput, bytes/s (0 = no sample yet).
    measured: f64,
    /// The adaptive byte-rate ceiling; `None` until the first sample or timeout.
    cap: Option<f64>,
}

impl AckPacer {
    pub(super) fn sent(&mut self, seq: u64, bytes: usize, at: Instant) {
        self.inflight = Some((seq, bytes, at));
    }

    /// Fold in the viewer's newest ack (`acked`, if any) and report whether a new frame may go.
    pub(super) fn may_send(&mut self, acked: Option<(u64, Instant)>, now: Instant) -> bool {
        let Some((seq, bytes, at)) = self.inflight else {
            return true;
        };
        if let Some((_, acked_at)) = acked.filter(|(a, _)| *a >= seq) {
            self.inflight = None;
            if bytes >= RATE_SAMPLE_MIN_BYTES {
                let secs = acked_at
                    .saturating_duration_since(at)
                    .as_secs_f64()
                    .max(0.05);
                let sample = f64::from(u32::try_from(bytes).unwrap_or(u32::MAX)) / secs;
                self.measured = if self.measured > 0.0 {
                    0.5 * self.measured + 0.5 * sample
                } else {
                    sample
                };
                self.cap = Some((RATE_SHARE * self.measured).clamp(RATE_FLOOR, RATE_CEIL));
            }
            return true;
        }
        if acked.is_none() {
            // The viewer never acks: no pacing at all.
            self.inflight = None;
            return true;
        }
        if now.saturating_duration_since(at) >= ACK_TIMEOUT {
            // The ack is late: slow down, keep the stream, send the next frame.
            let current = self.cap.unwrap_or(RATE_CEIL);
            self.cap = Some((current / 2.0).max(RATE_FLOOR));
            self.measured /= 2.0;
            self.inflight = None;
            return true;
        }
        false
    }

    /// The byte rate to refill the token bucket with.
    pub(super) fn rate(&self, profile_rate: u32) -> f64 {
        let profile_rate = f64::from(profile_rate);
        self.cap.map_or(profile_rate, |c| c.min(profile_rate))
    }
}

pub(super) fn spawn_reaper(s: &AppState, id: Uuid, actor: Actor) {
    let weak = Arc::downgrade(&s.0);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let s = AppState(inner);
            if s.shutdown_token().is_cancelled() {
                let _ = end_session(&s, &actor, id, Reason::Disconnected).await;
                return;
            }
            let live = s.live_view();
            if !live.sessions.lock().contains_key(&id) {
                return;
            }
            if live.session(&actor, id).is_err() {
                let _ = end_session(&s, &actor, id, Reason::Expired).await;
                return;
            }
            // The host stops producing images for a viewer that went quiet, even
            // when its client keeps heartbeating (or has vanished with a live TTL).
            if let Some(reason) = live.limit_reached(id) {
                let _ = end_session(&s, &actor, id, reason).await;
                return;
            }
            let _executor = live.executor.lock().await;
            if let Some(t) = s.hub().takeover().filter(|t| {
                t.info.session_id == id
                    && matches!(t.info.state, ViewState::Active | ViewState::Acquiring)
            }) {
                if t.info.input_expires_in_ms == 0 || !live.allowed(&actor, true) {
                    let _ = interrupt(&s, &actor, &t, Reason::Expired).await;
                } else if t.info.state == ViewState::Active
                    && live
                        .sessions
                        .lock()
                        .get(&id)
                        .and_then(|s| s.geometry)
                        .is_some_and(|(_, _, at)| at.elapsed() > TAKEOVER_STALE_AFTER)
                {
                    let _ = interrupt(&s, &actor, &t, Reason::Stale).await;
                }
            }
        }
    });
}
#[utoipa::path(get,path="/v1/view/sessions/{id}/frames",tag="view",security(("bearer"=[])),params(("id"=Uuid,Path)),responses((status=200,content_type="application/vnd.praxi.live-view.v1",body=String),(status=409)))]
#[allow(clippy::too_many_lines)] // The producer owns cleanup and bounded body capacity in one task.
pub(crate) async fn frames(
    State(s): State<AppState>,
    Extension(actor): Extension<Actor>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if headers.get(header::ACCEPT).and_then(|h| h.to_str().ok()) != Some(MEDIA_TYPE) {
        return Err(ApiError::new(
            StatusCode::NOT_ACCEPTABLE,
            "view_media_type",
            "live-view v1 Accept required",
        ));
    }
    let live = Arc::clone(s.live_view());
    let executor = live.executor.lock().await;
    let (target, generation, profile) = live.session(&actor, id)?;
    let target_impl = live.target(target, generation)?;
    let cancel = CancellationToken::new();
    {
        let mut sessions = live.sessions.lock();
        let session = sessions
            .get_mut(&id)
            .ok_or_else(|| ApiError::not_found("view session"))?;
        if session.stream.is_some() {
            return Err(ApiError::conflict("one frame stream per session"));
        }
        session.stream = Some(cancel.clone());
    }
    let source = target_impl.frame_source();
    // A cold browser can need longer to start capture than the other backend calls.
    let subscribe = tokio::time::timeout(Duration::from_secs(6), source.subscribe(profile))
        .await
        .map_err(|_| {
            ApiError::new(
                StatusCode::GATEWAY_TIMEOUT,
                "unavailable",
                "the screen backend did not start capture in time",
            )
        })
        .and_then(|result| result.map_err(super::backend));
    let subscription = match subscribe {
        Ok(sub) => sub,
        Err(e) => {
            if let Some(session) = live.sessions.lock().get_mut(&id) {
                session.stream = None;
            }
            return Err(e);
        }
    };
    if let Err(error) = live.source_profile(target).await {
        if let Some(session) = live.sessions.lock().get_mut(&id) {
            session.stream = None;
        }
        return Err(error);
    }
    let (tx, rx) = mpsc::channel::<Result<Bytes, std::io::Error>>(1);
    let state = s.clone();
    drop(executor);
    tokio::spawn(async move {
        let mut sub = subscription;
        let mut last_status = Vec::new();
        let mut last_frame: Option<(u64, u64)> = None;
        let mut over_budget = 0u32;
        let mut publication = true;
        let mut heartbeat_at = Instant::now();
        // Heartbeat records flow every 5 s; the audit proves the trail is
        // writable at most once a minute (a failure still ends the stream).
        let mut checkpoint_at = Instant::now();
        let mut token_at = Instant::now();
        let mut tokens = f64::from(EffectiveProfile::watch().burst_bytes);
        let mut pacer = AckPacer::default();
        let ack_notify = live
            .sessions
            .lock()
            .get(&id)
            .map(|session| Arc::clone(&session.ack_notify));
        let mut last_sent = Instant::now()
            .checked_sub(Duration::from_secs(5))
            .unwrap_or_else(Instant::now);
        let shutdown = state.shutdown_token();
        loop {
            if cancel.is_cancelled() || shutdown.is_cancelled() || live.session(&actor, id).is_err()
            {
                break;
            }
            // Reserve body capacity before cloning/encoding pixels: one in flight + latest watch slot.
            let permit = tokio::select! {
                () = cancel.cancelled() => break,
                () = shutdown.cancelled() => break,
                p = tokio::time::timeout(SEND_STALL,tx.reserve()) => match p { Ok(Ok(p)) => p,_ => break },
            };
            if live.session(&actor, id).is_err() {
                break;
            }
            let Ok((_, _, profile)) = live.session(&actor, id) else {
                break;
            };
            let takeover = state.hub().takeover().filter(|t| t.info.session_id == id);
            let status = Record {
                header: RecordHeader::Status {
                    v: 1,
                    session_id: id,
                    state: takeover
                        .as_ref()
                        .map_or(ViewState::Watch, |t| t.info.state.clone()),
                    reason: takeover.as_ref().and_then(|t| t.info.reason.clone()),
                    effective_profile: profile.clone(),
                    control_epoch: takeover.as_ref().map_or(0, |t| t.info.control_epoch),
                    input_allowed: input::owner(&state, &actor, id, None),
                },
                payload: Vec::new(),
            }
            .encode();
            if let Ok(status) = status
                && status != last_status
            {
                last_status.clone_from(&status);
                permit.send(Ok(Bytes::from(status)));
                continue;
            }
            // Clone only after validating authorization; the buffer lives only in this producer/body.
            publication |= sub.frames.has_changed().unwrap_or(false);
            let frame = sub.frames.borrow_and_update().clone();
            if let Some(frame) = frame {
                let meta = &frame.metadata;
                let now = Instant::now();
                tokens = (tokens
                    + now.duration_since(token_at).as_secs_f64()
                        * pacer.rate(profile.bytes_per_second))
                .min(f64::from(profile.burst_bytes));
                token_at = now;
                let frame_key = (meta.frame_seq, meta.captured_at_ms);
                if frame.validate().is_err()
                    || meta.target_id != target
                    || meta.generation != generation
                    || live.target(target, generation).is_err()
                {
                    break;
                }
                let frame_cost = frame.payload.len().saturating_add(live_view::MAX_HEADER);
                if frame_cost > usize::try_from(profile.burst_bytes).unwrap_or(0)
                    || meta.width > profile.max_width
                    || meta.height > profile.max_height
                {
                    let header = RecordHeader::Status {
                        v: 1,
                        session_id: id,
                        state: ViewState::Unavailable,
                        reason: Some(Reason::FrameBudgetExceeded),
                        effective_profile: profile.clone(),
                        control_epoch: takeover.as_ref().map_or(0, |t| t.info.control_epoch),
                        input_allowed: false,
                    };
                    if let Ok(bytes) = (Record {
                        header,
                        payload: Vec::new(),
                    })
                    .encode()
                    {
                        permit.send(Ok(Bytes::from(bytes)));
                    } else {
                        break;
                    }
                    // Slow down instead of wrecking the picture: fewer frames, same size and
                    // quality; the source re-encodes under the current profile.
                    over_budget = over_budget.saturating_add(1);
                    let mut degraded = profile;
                    degraded.max_fps = degraded.max_fps.div_ceil(2).max(1);
                    if over_budget > 20 {
                        // Persistent: only now give up some size (the quality floor stays).
                        degraded.max_width = degraded.max_width.min(1280);
                        degraded.max_height = degraded.max_height.min(720);
                    }
                    if let Some(session) = live.sessions.lock().get_mut(&id) {
                        session.profile = degraded.clone();
                        session.geometry = None;
                        session.healthy_since = None;
                    }
                    let _ = source.set_profile(degraded).await;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                // Source publications (including repeated frames) are health evidence only once.
                if publication {
                    let mut sessions = live.sessions.lock();
                    if let Some(session) = sessions.get_mut(&id) {
                        if session
                            .geometry
                            .is_some_and(|(r, _, _)| r != meta.geometry_revision)
                        {
                            session.rendered = None;
                        }
                        let capture_recent = u64::try_from(storage::now_ms())
                            .unwrap_or(0)
                            .saturating_sub(meta.captured_at_ms)
                            <= 2000;
                        if capture_recent || meta.repeated {
                            session.geometry = Some((meta.geometry_revision, meta.frame_seq, now));
                        }
                    }
                }
                let repeated_publication = publication && meta.repeated;
                publication = false;
                let period = if profile.profile == live_view::Profile::Watch {
                    if matches!(
                        meta.cause,
                        live_view::Cause::Action | live_view::Cause::Refresh
                    ) {
                        Duration::ZERO
                    } else {
                        Duration::from_secs_f64(1.0 / f64::from(profile.max_fps.max(1)))
                    }
                } else {
                    Duration::from_secs_f64(1.0 / f64::from(profile.max_fps.max(1)))
                };
                let acked = live
                    .sessions
                    .lock()
                    .get(&id)
                    .and_then(|session| session.acked);
                let may_go = pacer.may_send(acked, now);
                if (last_frame != Some(frame_key) || repeated_publication)
                    && may_go
                    && last_sent.elapsed() >= period
                    && tokens >= f64::from(u32::try_from(frame_cost).unwrap_or(u32::MAX))
                {
                    let record = Record {
                        header: RecordHeader::Frame {
                            v: 1,
                            session_id: id,
                            sent_at_ms: u64::try_from(storage::now_ms()).unwrap_or(0),
                            frame: meta.clone(),
                        },
                        payload: frame.payload.to_vec(),
                    };
                    if let Ok(bytes) = record.encode() {
                        tokens -= f64::from(u32::try_from(bytes.len()).unwrap_or(u32::MAX));
                        pacer.sent(meta.frame_seq, bytes.len(), now);
                        permit.send(Ok(Bytes::from(bytes)));
                        last_frame = Some(frame_key);
                        over_budget = 0;
                        last_sent = now;
                        continue;
                    }
                    break;
                }
            }
            if heartbeat_at.elapsed() >= Duration::from_secs(5) {
                if checkpoint_at.elapsed() >= Duration::from_secs(60) {
                    if audit(
                        &state,
                        &actor,
                        "view.checkpoint",
                        serde_json::json!({"session_id":id,"target_id":target}),
                    )
                    .is_err()
                    {
                        break;
                    }
                    checkpoint_at = Instant::now();
                }
                if let Ok(bytes) = (Record {
                    header: RecordHeader::Heartbeat {
                        v: 1,
                        server_time_ms: u64::try_from(storage::now_ms()).unwrap_or(0),
                    },
                    payload: Vec::new(),
                })
                .encode()
                {
                    permit.send(Ok(Bytes::from(bytes)));
                    heartbeat_at = Instant::now();
                    continue;
                }
            }
            drop(permit);
            tokio::select! {
                () = cancel.cancelled() => break,
                () = shutdown.cancelled() => break,
                () = tx.closed() => break,
                () = tokio::time::sleep(Duration::from_millis(100)) => {},
                () = async {
                    match &ack_notify {
                        Some(n) => n.notified().await,
                        None => std::future::pending().await,
                    }
                } => {},
                result = sub.frames.changed() => { if result.is_err() { break; } publication = true; },
            }
        }
        drop(sub);
        drop(tx);
        let _executor = live.executor.lock().await;
        if let Some(session) = live.sessions.lock().get_mut(&id) {
            session.stream = None;
            session.geometry = None;
        }
        if let Some(t) = state.hub().takeover().filter(|t| t.info.session_id == id) {
            let _ = interrupt(&state, &actor, &t, Reason::Disconnected).await;
        }
        // Backends also have a subscription drop callback; stop clears their last frame.
        if !live
            .sessions
            .lock()
            .values()
            .any(|s| s.target == target && s.stream.is_some())
        {
            let _ = tokio::time::timeout(super::BACKEND_CALL_TIMEOUT, source.stop()).await;
        }
        let _ = audit(
            &state,
            &actor,
            "view.stream_ended",
            serde_json::json!({"session_id":id,"target_id":target}),
        );
    });
    let body_stream = futures::stream::unfold(rx, |mut rx| async {
        rx.recv().await.map(|item| (item, rx))
    });
    let mut response = Response::new(Body::from_stream(body_stream));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    Ok(response)
}

#[cfg(test)]
mod pacer_tests {
    use super::*;

    #[test]
    fn one_unacked_frame_at_most_with_a_measured_rate_and_a_halving_timeout() {
        let t0 = Instant::now();
        let ms = Duration::from_millis;
        let mut p = AckPacer::default();
        assert!(p.may_send(None, t0), "nothing in flight");

        // A client that never acks is never held back.
        p.sent(1, 128 * 1024, t0);
        assert!(p.may_send(None, t0 + ms(10)));

        // Once it acks, the next frame waits for the ack of the one in flight.
        p.sent(2, 128 * 1024, t0);
        assert!(!p.may_send(Some((1, t0 + ms(100))), t0 + ms(200)));
        // 128 KiB acked after 500 ms is 256 KiB/s; 70 % of that is the new rate.
        assert!(p.may_send(Some((2, t0 + ms(500))), t0 + ms(500)));
        let rate = p.rate(4 * 1024 * 1024);
        assert!((rate - 0.7 * 256.0 * 1024.0).abs() < 1.0, "{rate}");
        assert!(
            p.rate(64 * 1024) <= 64.0 * 1024.0,
            "never above the profile"
        );

        // No ack for 1.5 s: the rate halves and the next frame goes out anyway.
        p.sent(3, 128 * 1024, t0 + ms(600));
        assert!(!p.may_send(Some((2, t0 + ms(500))), t0 + ms(2000)));
        assert!(p.may_send(Some((2, t0 + ms(500))), t0 + ms(2100)));
        assert!((p.rate(4 * 1024 * 1024) - rate / 2.0).abs() < 1.0);

        // Repeated timeouts bottom out at 32 KiB/s.
        for n in 0..20u64 {
            p.sent(10 + n, 1000, t0);
            assert!(p.may_send(Some((2, t0)), t0 + ms(1600)));
        }
        assert!((p.rate(4 * 1024 * 1024) - 32.0 * 1024.0).abs() < 1.0);
    }
}
