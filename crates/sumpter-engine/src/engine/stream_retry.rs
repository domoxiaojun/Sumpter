//! Bounded buffering before committing a retry-enabled Responses stream to the client.

use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{StreamExt, stream};
use sumpter_core::stream_terminal::{
    ResponsesRetryProbe, SseDialect, SseTerminalTracker, StreamRetryDecision, StreamRetryError,
};

use crate::outbound::UpstreamResponse;

/// Retried Responses streams are held only until a terminal event.  Beyond
/// this bound the proxy resumes normal streaming so a large response cannot
/// turn a retry policy into unbounded memory use.
const MAX_REPLAY_BUFFER_BYTES: usize = 16 * 1024 * 1024;

pub(super) fn request_allows_retry(body: &[u8]) -> bool {
    // Read this from the original body: the small routing projection omits
    // background. Ignore other fields without cloning the conversation.
    #[derive(serde::Deserialize)]
    struct Metadata {
        #[serde(default)]
        background: bool,
    }
    serde_json::from_slice::<Metadata>(body).is_ok_and(|metadata| !metadata.background)
}

pub(super) struct ProbedResponse {
    pub response: UpstreamResponse,
    pub retry_error: Option<StreamRetryError>,
    /// Transport ended while the probe still contained only lifecycle
    /// prelude frames.  No client-visible business output has been committed,
    /// so the request can be replayed safely under the stream retry budget.
    pub prelude_error: Option<StreamRetryError>,
    pub prefix: Vec<Bytes>,
    pub last_read_at: Instant,
    pub buffered_until_terminal: bool,
}

/// Buffer a Responses SSE attempt until a protocol terminal or transport
/// failure.  Nothing from this attempt reaches the client before completion,
/// so a mid-stream disconnect can be retried without duplicating text,
/// reasoning or tool calls.  A bounded buffer falls back to ordinary relay.
pub(super) async fn buffer_response_until_terminal(
    mut response: UpstreamResponse,
    idle_timeout: Option<Duration>,
) -> ProbedResponse {
    let mut retry_probe = ResponsesRetryProbe::default();
    let mut terminal_tracker = SseTerminalTracker::new(SseDialect::OpenAiResponses);
    let mut buffered = Vec::new();
    let mut prefix = Vec::new();
    let mut bytes = 0usize;
    let mut last_read_at = Instant::now();
    let mut retry_error = None;
    let mut prelude_error = None;
    let mut buffered_until_terminal = true;

    loop {
        let next = match idle_timeout {
            Some(idle) => {
                match tokio::time::timeout_at((last_read_at + idle).into(), response.stream.next())
                    .await
                {
                    Ok(item) => item,
                    Err(_) => {
                        prelude_error = Some(StreamRetryError {
                            code: "stream_idle_timeout_before_completion".into(),
                            retry_after: Vec::new(),
                        });
                        break;
                    }
                }
            }
            None => response.stream.next().await,
        };
        last_read_at = Instant::now();
        match next {
            Some(Ok(chunk)) => {
                bytes = bytes.saturating_add(chunk.len());
                prefix.push(chunk.clone());
                buffered.push(Ok(chunk.clone()));
                if let StreamRetryDecision::Retry(error) = retry_probe.push(&chunk) {
                    retry_error = Some(error);
                    break;
                }
                if terminal_tracker.push(&chunk).is_some() {
                    break;
                }
                if bytes > MAX_REPLAY_BUFFER_BYTES {
                    buffered_until_terminal = false;
                    break;
                }
            }
            Some(Err(error)) => {
                buffered.push(Err(error));
                prelude_error = Some(StreamRetryError {
                    code: "stream_interrupted_before_completion".into(),
                    retry_after: Vec::new(),
                });
                break;
            }
            None => {
                prelude_error = Some(StreamRetryError {
                    code: "stream_ended_before_completion".into(),
                    retry_after: Vec::new(),
                });
                break;
            }
        }
    }

    response.stream = stream::iter(buffered).chain(response.stream).boxed();
    ProbedResponse {
        response,
        retry_error,
        prelude_error,
        prefix,
        last_read_at,
        buffered_until_terminal,
    }
}
