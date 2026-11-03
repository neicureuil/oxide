//! Runs a [`CallSession`] on tokio: executes its HTTP commands and timers and feeds the results back.
use std::collections::BTreeMap;

use teams_auth::Authenticator;
use tokio::sync::mpsc;

use crate::callbacks::{CallbackPath, decode_body};
use crate::client::ConversationClient;
use crate::session::{CallSession, Command, Input};

/// Turns a request pushed to `path` into an input for call `call_id`; `None` if not ours.
pub fn route(
    call_id: &str,
    path: &str,
    headers: &BTreeMap<String, String>,
    body: &str,
) -> Option<Input> {
    let path = CallbackPath::parse(path)?;
    if path.call != call_id {
        return None;
    }
    Some(Input::Callback {
        scope: path.scope,
        name: path.name,
        body: decode_body(body, headers)?,
    })
}

/// What the driver reports after each step.
pub enum Event<'a> {
    Changed(&'a CallSession),
    /// NGC SDP from the other side, for the media engine.
    RemoteMedia(String),
    RemoteProvisionalMedia(String),
    /// The service renegotiates: answer with `Input::RenegotiationAnswer`.
    RemoteOffer(String),
}

/// `feedback` must send into the `inputs` channel: HTTP results and timers come back that way.
pub async fn drive<A: Authenticator + 'static>(
    client: ConversationClient<A>,
    mut session: CallSession,
    mut inputs: mpsc::UnboundedReceiver<Input>,
    feedback: mpsc::UnboundedSender<Input>,
    mut report: impl FnMut(Event<'_>) + Send,
) -> CallSession {
    let _ = feedback.send(Input::Begin);
    while let Some(input) = inputs.recv().await {
        for command in session.handle(input) {
            match command {
                Command::Http {
                    op,
                    method,
                    url,
                    body,
                } => {
                    let client = client.clone();
                    let feedback = feedback.clone();
                    let call = session.id.clone();
                    tokio::spawn(async move {
                        let result = client.send_op(&call, op, method, &url, &body).await;
                        let _ = feedback.send(Input::Response(op, result));
                    });
                }
                Command::Timer(timer, delay) => {
                    let feedback = feedback.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(delay).await;
                        let _ = feedback.send(Input::Timeout(timer));
                    });
                }
                Command::RemoteMedia(sdp) => report(Event::RemoteMedia(sdp)),
                Command::RemoteProvisionalMedia(sdp) => report(Event::RemoteProvisionalMedia(sdp)),
                Command::RemoteOffer(sdp) => report(Event::RemoteOffer(sdp)),
                // This driver sends no media (probes, roster-only joins).
                Command::SendLimits(_) | Command::LocalOfferRejected { .. } => {}
            }
        }
        report(Event::Changed(&session));
        if session.ended() {
            break;
        }
    }
    session
}
