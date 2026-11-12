//! Incremental observation copy. Wire bytes and headers are never rewritten here.

use std::collections::VecDeque;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use async_compression::tokio::bufread::{BrotliDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder};
use bytes::Bytes;
use tokio::io::{AsyncRead, BufReader, ReadBuf};

const BLOCK_BYTES: usize = 64 * 1024;
const MAX_DECODED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ObservationIssue {
    UnsupportedEncoding,
    InvalidCompressedData,
    TruncatedCompressedData,
    DecompressionLimitExceeded,
    ClientClosedBeforeValidation,
    BridgeOutputLimitExceeded,
}

impl ObservationIssue {
    pub(super) fn token(self) -> &'static str {
        match self {
            Self::UnsupportedEncoding => "unsupported_content_encoding",
            Self::InvalidCompressedData => "invalid_compressed_data",
            Self::TruncatedCompressedData => "truncated_compressed_data",
            Self::DecompressionLimitExceeded => "decoded_observation_limit_exceeded",
            Self::ClientClosedBeforeValidation => "compressed_tail_unverified_after_terminal",
            Self::BridgeOutputLimitExceeded => "bridge_terminal_buffer_limit_exceeded",
        }
    }
}

#[derive(Default)]
struct InputState {
    chunks: VecDeque<Bytes>,
    eof: bool,
}

struct Input(Arc<Mutex<InputState>>);

impl AsyncRead for Input {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut input = self.0.lock().unwrap();
        if let Some(chunk) = input.chunks.front_mut() {
            let take = chunk.len().min(buf.remaining());
            buf.put_slice(&chunk.split_to(take));
            if chunk.is_empty() {
                input.chunks.pop_front();
            }
            Poll::Ready(Ok(()))
        } else if input.eof || buf.remaining() == 0 {
            Poll::Ready(Ok(()))
        } else {
            // This source has no external IO: the caller pumps it again after
            // feeding the next wire chunk. No task or unbounded tee is spawned.
            Poll::Pending
        }
    }
}

trait CheckedRead: AsyncRead + Send + Unpin {
    fn buffered_input(&self) -> bool {
        false
    }
}

impl CheckedRead for Input {}

type Reader = Box<dyn CheckedRead>;

#[derive(Clone)]
struct SharedRead(Arc<Mutex<Reader>>);

impl SharedRead {
    fn new(reader: impl CheckedRead + 'static) -> Self {
        let reader: Reader = Box::new(reader);
        Self(Arc::new(Mutex::new(reader)))
    }

    fn buffered_input(&self) -> bool {
        self.0.lock().unwrap().buffered_input()
    }
}

impl AsyncRead for SharedRead {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut reader = self.0.lock().unwrap();
        Pin::new(&mut **reader).poll_read(cx, buf)
    }
}

macro_rules! checked_decoder {
    ($($decoder:ident),+ $(,)?) => {
        $(impl CheckedRead for $decoder<BufReader<SharedRead>> {
            fn buffered_input(&self) -> bool { !self.get_ref().buffer().is_empty() }
        })+
    };
}
checked_decoder!(BrotliDecoder, GzipDecoder, ZlibDecoder, ZstdDecoder);

pub(super) struct ResponseDecoder {
    input: Arc<Mutex<InputState>>,
    readers: Vec<SharedRead>,
    active_layer: usize,
    compressed: bool,
    decoded: usize,
    limit: usize,
    issue: Option<ObservationIssue>,
    ended: bool,
}

impl ResponseDecoder {
    pub(super) fn new(headers: &[(String, String)]) -> Self {
        let encodings: Vec<String> = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-encoding"))
            .flat_map(|(_, value)| value.split(','))
            .map(|value| value.trim().to_ascii_lowercase())
            .collect();
        let compressed = encodings.iter().any(|value| value != "identity");
        let input = Arc::new(Mutex::new(InputState::default()));
        let mut reader = SharedRead::new(Input(input.clone()));
        let mut readers = vec![reader.clone()];
        let mut issue = (encodings.len() > 4).then_some(ObservationIssue::UnsupportedEncoding);
        if issue.is_none() {
            for encoding in encodings.iter().rev() {
                reader = match encoding.as_str() {
                    "identity" => continue,
                    "gzip" => {
                        let mut decoder = GzipDecoder::new(BufReader::new(reader));
                        decoder.multiple_members(true);
                        SharedRead::new(decoder)
                    }
                    "br" => SharedRead::new(BrotliDecoder::new(BufReader::new(reader))),
                    "deflate" => SharedRead::new(ZlibDecoder::new(BufReader::new(reader))),
                    "zstd" => {
                        let mut decoder = ZstdDecoder::new(BufReader::new(reader));
                        decoder.multiple_members(true);
                        SharedRead::new(decoder)
                    }
                    _ => {
                        issue = Some(ObservationIssue::UnsupportedEncoding);
                        break;
                    }
                };
                readers.push(reader.clone());
            }
        }
        if issue.is_some() {
            readers.clear();
        }
        let active_layer = readers.len().saturating_sub(1);
        Self {
            input,
            readers,
            active_layer,
            compressed,
            decoded: 0,
            limit: MAX_DECODED_BYTES,
            issue,
            ended: false,
        }
    }

    pub(super) fn is_compressed(&self) -> bool {
        self.compressed
    }

    pub(super) fn issue(&self) -> Option<ObservationIssue> {
        self.issue
    }

    pub(super) fn feed(&mut self, bytes: &Bytes, observe: impl FnMut(&[u8])) {
        if self.issue.is_some() || bytes.is_empty() {
            return;
        }
        if self.ended {
            self.fail(ObservationIssue::InvalidCompressedData);
            return;
        }
        self.input.lock().unwrap().chunks.push_back(bytes.clone());
        self.drain(false, observe);
    }

    pub(super) fn finish(&mut self, observe: impl FnMut(&[u8])) {
        self.input.lock().unwrap().eof = true;
        self.drain(true, observe);
    }

    fn fail(&mut self, issue: ObservationIssue) {
        self.issue.get_or_insert(issue);
        self.readers.clear();
        self.input.lock().unwrap().chunks.clear();
    }

    fn drain(&mut self, eof: bool, mut observe: impl FnMut(&[u8])) {
        if self.issue.is_some() || self.ended {
            return;
        }
        let mut output = [0_u8; BLOCK_BYTES];
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            let mut buf = ReadBuf::new(&mut output);
            let result =
                Pin::new(&mut self.readers[self.active_layer]).poll_read(&mut cx, &mut buf);
            match result {
                Poll::Ready(Ok(())) if buf.filled().is_empty() => {
                    if self.readers[self.active_layer].buffered_input() {
                        self.fail(ObservationIssue::InvalidCompressedData);
                        return;
                    }
                    if self.active_layer == 0 {
                        self.ended = true;
                        return;
                    }
                    // An outer codec can end before an inner gzip/zstd trailer.
                    // Drain and validate each inner layer, down to physical EOF.
                    self.active_layer -= 1;
                }
                Poll::Ready(Ok(())) => {
                    let bytes = buf.filled();
                    if self.active_layer + 1 != self.readers.len() {
                        self.fail(ObservationIssue::InvalidCompressedData);
                        return;
                    }
                    if bytes.len() > self.limit.saturating_sub(self.decoded) {
                        self.fail(ObservationIssue::DecompressionLimitExceeded);
                        return;
                    }
                    self.decoded += bytes.len();
                    observe(bytes);
                }
                Poll::Ready(Err(error)) => {
                    self.fail(if eof && error.kind() == io::ErrorKind::UnexpectedEof {
                        ObservationIssue::TruncatedCompressedData
                    } else {
                        ObservationIssue::InvalidCompressedData
                    });
                    return;
                }
                Poll::Pending => {
                    if eof {
                        self.fail(ObservationIssue::TruncatedCompressedData);
                    }
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_compression::tokio::write::{BrotliEncoder, GzipEncoder, ZlibEncoder, ZstdEncoder};
    use tokio::io::AsyncWriteExt;

    async fn encode(kind: &str, data: &[u8]) -> Vec<u8> {
        macro_rules! encoded {
            ($encoder:ident) => {{
                let mut encoder = $encoder::new(Vec::new());
                encoder.write_all(data).await.unwrap();
                encoder.shutdown().await.unwrap();
                encoder.into_inner()
            }};
        }
        match kind {
            "gzip" => encoded!(GzipEncoder),
            "br" => encoded!(BrotliEncoder),
            "deflate" => encoded!(ZlibEncoder),
            "zstd" => encoded!(ZstdEncoder),
            _ => data.to_vec(),
        }
    }

    fn restore(coding: &str, wire: &[u8], split: usize) -> (Vec<u8>, Option<ObservationIssue>) {
        let mut decoder = ResponseDecoder::new(&[("Content-Encoding".into(), coding.into())]);
        let mut out = Vec::new();
        for bytes in wire.chunks(split) {
            decoder.feed(&Bytes::copy_from_slice(bytes), |data| {
                assert!(data.len() <= BLOCK_BYTES);
                out.extend_from_slice(data);
            });
        }
        decoder.finish(|data| out.extend_from_slice(data));
        (out, decoder.issue())
    }

    #[tokio::test]
    async fn codecs_and_nested_codings_preserve_arbitrary_split_bytes() {
        let data = "data: {\"text\":\"分块中文\"}\n\n".repeat(128).into_bytes();
        for coding in ["identity", "gzip", "br", "deflate", "zstd"] {
            let wire = encode(coding, &data).await;
            for split in [1, 7, 65536] {
                let (out, issue) = restore(coding, &wire, split);
                assert_eq!(issue, None, "{coding}/{split}");
                assert_eq!(out, data, "{coding}/{split}");
            }
        }
        let large = vec![b'x'; BLOCK_BYTES + 17];
        assert_eq!(restore("identity", &large, large.len()), (large, None));
        let wire = encode("br", &encode("gzip", &data).await).await;
        assert_eq!(restore("GZIP, br", &wire, 3), (data.clone(), None));
        for codings in [
            vec!["br", "gzip"],
            vec!["deflate", "gzip"],
            vec!["br", "deflate", "zstd", "gzip"],
        ] {
            let mut wire = data.clone();
            for coding in &codings {
                wire = encode(coding, &wire).await;
            }
            assert_eq!(restore(&codings.join(", "), &wire, 1), (data.clone(), None));
        }
        let mut wire = encode("gzip", &encode("br", &data).await).await;
        let crc = wire.len() - 8;
        wire[crc] ^= 1;
        assert_eq!(
            restore("br, gzip", &wire, 1).1,
            Some(ObservationIssue::InvalidCompressedData)
        );
    }

    #[tokio::test]
    async fn gzip_members_limits_and_invalid_streams_are_distinct() {
        let first = encode("gzip", b"one").await;
        let second = encode("gzip", b"two").await;
        assert_eq!(
            restore("gzip", &[first, second].concat(), 1),
            (b"onetwo".to_vec(), None)
        );
        let mut wire = encode("gzip", b"hello").await;
        assert_eq!(
            restore("gzip", &wire[..wire.len() - 4], 1).1,
            Some(ObservationIssue::TruncatedCompressedData)
        );
        let crc = wire.len() - 8;
        wire[crc] ^= 1;
        assert_eq!(
            restore("gzip", &wire, 1).1,
            Some(ObservationIssue::InvalidCompressedData)
        );
        assert_eq!(
            restore("future", &wire, 1).1,
            Some(ObservationIssue::UnsupportedEncoding)
        );
        assert_eq!(
            restore("identity,identity,identity,identity,identity", &wire, 1).1,
            Some(ObservationIssue::UnsupportedEncoding)
        );
        let wire = encode("gzip", b"too much").await;
        let mut compressed = ResponseDecoder::new(&[("content-encoding".into(), "gzip".into())]);
        compressed.limit = 4;
        compressed.feed(&Bytes::from(wire), |_| {
            panic!("over-budget compressed output")
        });
        compressed.finish(|_| panic!("disabled observer"));
        assert_eq!(
            compressed.issue(),
            Some(ObservationIssue::DecompressionLimitExceeded)
        );
        let mut decoder = ResponseDecoder::new(&[]);
        decoder.limit = 4;
        decoder.feed(&Bytes::from_static(b"too much"), |_| {
            panic!("over-budget block must not reach observers")
        });
        assert_eq!(
            decoder.issue(),
            Some(ObservationIssue::DecompressionLimitExceeded)
        );
    }
}
