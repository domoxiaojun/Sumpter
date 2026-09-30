//! Bounded look-ahead before committing a Responses stream to the client.

use std::time::{Duration, Instant};

use bytes::Bytes;
use futures_util::{StreamExt, stream};
use sumpter_core::stream_terminal::{ResponsesRetryProbe, StreamRetryDecision};

use crate::outbound::UpstreamResponse;

const MAX_PREFIX_BYTES: usize = 64 * 1024;
const MAX_PREFIX_WAIT: Duration = Duration::from_secs(5);

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
    pub retry_code: Option<String>,
    pub prefix: Vec<Bytes>,
    pub last_read_at: Instant,
}

pub(super) async fn probe_response(
    mut response: UpstreamResponse,
    idle_timeout: Option<Duration>,
) -> ProbedResponse {
    let mut probe = ResponsesRetryProbe::default();
    let mut buffered = Vec::new();
    let mut prefix = Vec::new();
    let mut bytes = 0;
    let mut retry_code = None;
    let started = Instant::now();
    let mut last_read_at = started;
    loop {
        let mut deadline = started + MAX_PREFIX_WAIT;
        if let Some(idle) = idle_timeout {
            deadline = deadline.min(last_read_at + idle);
        }
        let item = match tokio::time::timeout_at(deadline.into(), response.stream.next()).await {
            Ok(Some(item)) => item,
            Ok(None) => {
                response.stream = stream::empty().boxed();
                break;
            }
            Err(_) => break,
        };
        last_read_at = Instant::now();
        let decision = match &item {
            Ok(chunk) => {
                bytes += chunk.len();
                prefix.push(chunk.clone());
                if bytes > MAX_PREFIX_BYTES || prefix.len() >= 1024 {
                    StreamRetryDecision::Forward
                } else {
                    probe.push(chunk)
                }
            }
            Err(_) => StreamRetryDecision::Forward,
        };
        buffered.push(item);
        match decision {
            StreamRetryDecision::Pending => {}
            StreamRetryDecision::Forward => break,
            StreamRetryDecision::Retry(code) => {
                retry_code = Some(code);
                break;
            }
        }
    }
    // Restore every consumed byte (and any transport error) on non-retried
    // responses, including malformed, oversized and timed-out prefixes.
    response.stream = stream::iter(buffered).chain(response.stream).boxed();
    ProbedResponse {
        response,
        retry_code,
        prefix,
        last_read_at,
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
    async fn probe_limits_release_the_entire_original_body() {
        let error = Bytes::from_static(b"data: {\"type\":\"error\",\"code\":\"server_error\"}\n\n");
        for mut chunks in [
            vec![Bytes::from(vec![b':'; MAX_PREFIX_BYTES + 1])],
            vec![Bytes::from_static(b": heartbeat\n\n"); 1024],
        ] {
            chunks.push(error.clone());
            let original = chunks.concat();
            let probed = probe_response(response(chunks), None).await;
            assert_eq!(probed.retry_code, None);
            let restored = probed
                .response
                .stream
                .map(Result::unwrap)
                .collect::<Vec<_>>()
                .await
                .concat();
            assert_eq!(restored, original);
        }
    }

    #[tokio::test(start_paused = true)]
    async fn probe_wait_is_bounded_and_keeps_unread_tail() {
        let mut response = response(vec![]);
        response.stream = stream::once(async {
            tokio::time::sleep(Duration::from_secs(6)).await;
            Ok(Bytes::from_static(b"tail"))
        })
        .boxed();
        let started = tokio::time::Instant::now();
        let mut probed = probe_response(response, None).await;
        assert_eq!(probed.retry_code, None);
        assert!(started.elapsed() >= MAX_PREFIX_WAIT);
        assert!(started.elapsed() < MAX_PREFIX_WAIT + Duration::from_millis(100));
        assert_eq!(
            probed.response.stream.next().await.unwrap().unwrap(),
            "tail"
        );
    }

    #[tokio::test]
    async fn probe_preserves_transport_errors_and_does_not_retry_them() {
        let mut response = response(vec![]);
        response.stream =
            stream::iter([Err(TransportError::ConnectionFailed("reset".into()))]).boxed();
        let mut probed = probe_response(response, None).await;
        assert_eq!(probed.retry_code, None);
        assert!(matches!(
            probed.response.stream.next().await,
            Some(Err(TransportError::ConnectionFailed(_)))
        ));
    }
}
