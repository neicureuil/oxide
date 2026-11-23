//! Two synthetic engines call each other through the Teams NGC SDP conversion, as the core does.
use std::sync::Arc;
use std::time::Duration;

use teams_calling::sdp::{self, Kind};
use teams_media::{MediaEngine, MediaEvent, PrepareOptions, VideoFrame, VideoSlot};
use tokio::sync::mpsc;

struct Side {
    engine: MediaEngine,
    events: mpsc::UnboundedReceiver<MediaEvent>,
}

fn side() -> Side {
    let (tx, events) = mpsc::unbounded_channel();
    let engine = MediaEngine::synthetic(Arc::new(move |event| {
        let _ = tx.send(event);
    }));
    Side { engine, events }
}

fn prepare(call_id: &str, remote_offer: Option<String>) -> PrepareOptions {
    PrepareOptions {
        call_id: call_id.into(),
        ice_servers: vec![],
        video: false,
        muted: false,
        speaker: true,
        camera_lines: 2,
        remote_offer,
    }
}

/// The next event `pick` accepts, skipping the others; panics after 20 s.
async fn next<T>(side: &mut Side, mut pick: impl FnMut(MediaEvent) -> Option<T>) -> T {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
    loop {
        let event = tokio::time::timeout_at(deadline, side.events.recv())
            .await
            .expect("timed out waiting for an event")
            .expect("engine gone");
        if let MediaEvent::Failed { reason, .. } = &event {
            panic!("media failed: {reason}");
        }
        if let Some(found) = pick(event) {
            return found;
        }
    }
}

async fn local_description(side: &mut Side) -> String {
    next(side, |e| match e {
        MediaEvent::LocalDescription { sdp, .. } => Some(sdp),
        _ => None,
    })
    .await
}

async fn connected(side: &mut Side) {
    next(side, |e| {
        matches!(e, MediaEvent::Connected { .. }).then_some(())
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
async fn offer_and_answer_through_ngc_connect() {
    let mut caller = side();
    let mut callee = side();

    caller.engine.prepare(prepare("c", None));
    let offer = local_description(&mut caller).await;
    assert!(
        offer.contains("a=group:BUNDLE 0 1 2 3"),
        "audio, two camera lines, sharing:\n{offer}"
    );
    assert!(
        offer.contains("a=candidate:"),
        "candidates gathered before sending:\n{offer}"
    );
    assert!(
        offer.contains("a=recvonly"),
        "receive-only video lines stay so:\n{offer}"
    );

    // What the core sends, then what the other side's core hands its engine.
    let ngc_offer = sdp::to_ngc_in(&offer, Kind::Offer, false);
    assert!(ngc_offer.contains("RTP/SAVP"), "{ngc_offer}");
    let callee_offer = sdp::from_ngc(&ngc_offer, Kind::Offer, None);

    callee.engine.prepare(prepare("c", Some(callee_offer)));
    let answer = local_description(&mut callee).await;
    let ngc_answer = sdp::answer_to_ngc(&answer, &ngc_offer, false);
    caller.engine.set_remote_description(
        "c",
        &sdp::from_ngc(&ngc_answer, Kind::Answer, Some(&offer)),
        false,
    );

    connected(&mut caller).await;
    connected(&mut callee).await;

    // The callee's tone reaches the caller.
    next(&mut caller, |e| match e {
        MediaEvent::Diagnostics { diagnostics, .. } if diagnostics.audio_packets_received > 0 => {
            Some(())
        }
        _ => None,
    })
    .await;

    // An ICE restart renegotiates; the service rejecting it rolls back without ending the call.
    caller.engine.restart_ice("c");
    let restart = local_description(&mut caller).await;
    assert_ne!(restart, offer);
    caller.engine.offer_rejected("c", false);
    next(&mut caller, |e| {
        matches!(e, MediaEvent::Diagnostics { .. }).then_some(())
    })
    .await;

    caller.engine.close("c");
    callee.engine.close("c");
}

#[tokio::test(flavor = "multi_thread")]
async fn service_renegotiation_is_answered() {
    let mut caller = side();
    let mut callee = side();
    caller.engine.prepare(prepare("r", None));
    let offer = local_description(&mut caller).await;
    callee.engine.prepare(prepare("r", Some(offer)));
    let answer = local_description(&mut callee).await;
    caller.engine.set_remote_description("r", &answer, false);
    connected(&mut caller).await;
    connected(&mut callee).await;

    // The other side renegotiates (as the mixer does when a sharer joins): we answer, nothing breaks.
    callee.engine.restart_ice("r");
    let reoffer = local_description(&mut callee).await;
    caller.engine.answer_remote_offer("r", &reoffer);
    let reanswer = next(&mut caller, |e| match e {
        MediaEvent::RenegotiationAnswer { sdp, .. } => Some(sdp),
        _ => None,
    })
    .await;
    callee.engine.set_remote_description("r", &reanswer, false);

    // Still flowing afterwards: fresh statistics and no failure.
    let before = next(&mut caller, |e| match e {
        MediaEvent::Diagnostics { diagnostics, .. } => Some(diagnostics.audio_packets_received),
        _ => None,
    })
    .await;
    next(&mut caller, |e| match e {
        MediaEvent::Diagnostics { diagnostics, .. }
            if diagnostics.audio_packets_received > before =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    caller.engine.close("r");
    callee.engine.close("r");
}

/// Frames keep coming for seconds (a leaked buffer per delivery froze the macOS camera after one).
#[tokio::test(flavor = "multi_thread")]
async fn camera_keeps_flowing_to_the_other_side() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut caller = side();
    let mut callee = side();
    let mut options = prepare("cam", None);
    options.video = true;
    caller.engine.prepare(options);
    let offer = local_description(&mut caller).await;
    // Through the Teams conversion, as in a call: H264 only.
    let ngc_offer = sdp::to_ngc_in(&offer, Kind::Offer, false);
    callee.engine.prepare(prepare(
        "cam",
        Some(sdp::from_ngc(&ngc_offer, Kind::Offer, None)),
    ));
    // The remote line surfaces while the offer is applied, before our answer.
    let mut mid = None;
    let answer = next(&mut callee, |e| match e {
        MediaEvent::Video {
            slot: VideoSlot::Remote(m),
            present: true,
            ..
        } => {
            mid = Some(m);
            None
        }
        MediaEvent::LocalDescription { sdp, .. } => Some(sdp),
        _ => None,
    })
    .await;
    let mid = mid.expect("the caller's camera line surfaces");
    let ngc_answer = sdp::answer_to_ngc(&answer, &ngc_offer, false);
    caller.engine.set_remote_description(
        "cam",
        &sdp::from_ngc(&ngc_answer, Kind::Answer, Some(&offer)),
        false,
    );
    connected(&mut caller).await;
    let frames = Arc::new(AtomicUsize::new(0));
    let count = frames.clone();
    callee.engine.set_video_sink(
        VideoSlot::Remote(mid),
        Some(Arc::new(move |_: &VideoFrame<'_>| {
            count.fetch_add(1, Ordering::Relaxed);
        })),
    );
    tokio::time::sleep(Duration::from_secs(2)).await;
    let early = frames.load(Ordering::Relaxed);
    tokio::time::sleep(Duration::from_secs(4)).await;
    let late = frames.load(Ordering::Relaxed);
    assert!(early > 0, "no remote frame at all");
    assert!(
        late - early > 20,
        "remote video stalled: {early} then {late}"
    );
    caller.engine.close("cam");
    callee.engine.close("cam");
}
