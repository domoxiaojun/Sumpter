//! Bounded buffering before committing a retry-enabled Responses stream to the client.

use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{StreamExt, stream};
use sumpter_core::stream_terminal::{
    ResponsesRetryProbe, SseDialect, SseTerminalTracker, StreamRetryDecision, StreamRetryError,
};

use super::admission::BufferBudget;
use crate::outbound::UpstreamResponse;
use std::sync::Arc;
use sumpter_core::request_policy::{RequestPolicy, StreamProtection};

/// Background Responses must not enter transparent replay.
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
    pub release_reason: &'static str,
    pub response: UpstreamResponse,
    pub retry_error: Option<StreamRetryError>,
    /// Transport ended before commit. Complete mode may also contain business
    /// frames; none were sent to the client, so replay does not duplicate output.
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
    policy: &RequestPolicy,
    budget: &Arc<BufferBudget>,
) -> ProbedResponse {
    let mut lease = budget.lease();
    let mut release_reason = "terminal";
    let started = Instant::now();
    let buffer_deadline = (policy.stream_buffer_seconds > 0)
        .then(|| started + Duration::from_secs(policy.stream_buffer_seconds as u64));
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
        let idle_deadline = idle_timeout.map(|idle| last_read_at + idle);
        let deadline = idle_deadline.into_iter().chain(buffer_deadline).min();
        let next = match deadline {
            Some(deadline) => {
                match tokio::time::timeout_at(deadline.into(), response.stream.next()).await {
                    Ok(item) => item,
                    Err(_) => {
                        if buffer_deadline.is_some_and(|limit| {
                            limit <= Instant::now()
                                && idle_deadline.is_none_or(|idle| limit <= idle)
                        }) {
                            release_reason = "time_budget";
                            buffered_until_terminal = false;
                            break;
                        }
                        prelude_error = Some(StreamRetryError {
                            code: "stream_idle_timeout_before_completion".into(),
                            retry_after: Vec::new(),
                        });
                        release_reason = "idle_timeout";
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
                let reserved = lease.reserve(
                    chunk.len().saturating_add(96),
                    policy.max_buffered_mib as usize * 1024 * 1024,
                );
                match retry_probe.push(&chunk) {
                    StreamRetryDecision::Retry(error) => {
                        release_reason = "retryable_terminal";
                        retry_error = Some(error);
                        break;
                    }
                    StreamRetryDecision::Forward
                        if policy.stream_protection == StreamProtection::Prelude =>
                    {
                        release_reason = "first_output";
                        buffered_until_terminal = false;
                        break;
                    }
                    _ => {}
                }
                if terminal_tracker.push(&chunk).is_some() {
                    break;
                }
                if !reserved
                    || bytes > policy.stream_buffer_mib as usize * 1024 * 1024
                    || buffered.len() >= policy.stream_buffer_chunks as usize
                {
                    release_reason = if !reserved {
                        "global_buffer_budget"
                    } else if buffered.len() >= policy.stream_buffer_chunks as usize {
                        "chunk_budget"
                    } else {
                        "byte_budget"
                    };
                    buffered_until_terminal = false;
                    break;
                }
            }
            Some(Err(error)) => {
                release_reason = "transport_error";
                buffered.push(Err(error));
                prelude_error = Some(StreamRetryError {
                    code: "stream_interrupted_before_completion".into(),
                    retry_after: Vec::new(),
                });
                break;
            }
            None => {
                release_reason = "missing_terminal";
                prelude_error = Some(StreamRetryError {
                    code: "stream_ended_before_completion".into(),
                    retry_after: Vec::new(),
                });
                break;
            }
        }
    }

    let upstream = stream::iter(buffered).chain(response.stream).boxed();
    response.stream = stream::unfold((upstream, lease), |(mut upstream, lease)| async move {
        upstream
            .next()
            .await
            .map(|chunk| (chunk, (upstream, lease)))
    })
    .boxed();
    ProbedResponse {
        release_reason,
        response,
        retry_error,
        prelude_error,
        prefix,
        last_read_at,
        buffered_until_terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outbound::TransportError;

    fn response(chunks: Vec<Bytes>) -> UpstreamResponse {
        UpstreamResponse {
            status: 200,
            headers: vec![],
            stream: stream::iter(chunks.into_iter().map(Ok)).boxed(),
        }
    }

    #[tokio::test]
    async fn resource_fallbacks_preserve_bytes_and_disable_replay() {
        for reason in ["global_buffer_budget", "chunk_budget", "byte_budget"] {
            let budget = Arc::new(BufferBudget::default());
            let mut occupied = budget.lease();
            let policy = RequestPolicy {
                max_buffered_mib: 4,
                stream_buffer_mib: 1,
                stream_buffer_chunks: 16,
                ..Default::default()
            };
            let chunks = match reason {
                "global_buffer_budget" => {
                    assert!(occupied.reserve(4 * 1024 * 1024, 4 * 1024 * 1024));
                    vec![Bytes::from_static(b": keepalive\n\n")]
                }
                "byte_budget" => vec![Bytes::from(vec![b' '; 1024 * 1024 + 1])],
                _ => vec![Bytes::from_static(b": keepalive\n\n"); 16],
            };
            let expected = chunks.concat();
            let probed =
                buffer_response_until_terminal(response(chunks), None, &policy, &budget).await;
            assert_eq!(probed.release_reason, reason);
            assert!(!probed.buffered_until_terminal);
            assert!(probed.retry_error.is_none() && probed.prelude_error.is_none());
            let output = probed
                .response
                .stream
                .collect::<Vec<_>>()
                .await
                .into_iter()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
                .concat();
            assert_eq!(output, expected);
            drop(occupied);
            assert!(budget.lease().reserve(4 * 1024 * 1024, 4 * 1024 * 1024));
        }
    }

    #[tokio::test]
    async fn time_fallback_releases_pending_stream_without_claiming_terminal() {
        let budget = Arc::new(BufferBudget::default());
        let policy = RequestPolicy {
            stream_buffer_seconds: 1,
            ..Default::default()
        };
        let pending = UpstreamResponse {
            status: 200,
            headers: vec![],
            stream: stream::pending().boxed(),
        };
        let probed = tokio::time::timeout(
            Duration::from_secs(2),
            buffer_response_until_terminal(pending, None, &policy, &budget),
        )
        .await
        .unwrap();
        assert_eq!(probed.release_reason, "time_budget");
        assert!(!probed.buffered_until_terminal);
        assert!(probed.prelude_error.is_none());
    }

    #[tokio::test]
    async fn cancelled_buffering_and_dropped_output_return_shared_memory() {
        let budget = Arc::new(BufferBudget::default());
        let (sender, receiver) =
            tokio::sync::mpsc::unbounded_channel::<Result<Bytes, TransportError>>();
        let upstream = UpstreamResponse {
            status: 200,
            headers: vec![],
            stream: stream::unfold(receiver, |mut receiver| async move {
                receiver.recv().await.map(|v| (v, receiver))
            })
            .boxed(),
        };
        sender
            .send(Ok(Bytes::from_static(b": waiting\n\n")))
            .unwrap();
        let task_budget = budget.clone();
        let task = tokio::spawn(async move {
            buffer_response_until_terminal(upstream, None, &RequestPolicy::default(), &task_budget)
                .await
        });
        tokio::task::yield_now().await;
        task.abort();
        let _ = task.await;
        assert!(budget.lease().reserve(1024, 1024));
        let completed = buffer_response_until_terminal(response(vec![Bytes::from_static(b"data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\"}}\n\n")]), None, &RequestPolicy::default(), &budget).await;
        assert!(!budget.lease().reserve(1024, 1024));
        drop(completed);
        assert!(budget.lease().reserve(1024, 1024));
    }
}
