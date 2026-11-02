// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0

//! `aws-chunked` request framing.
//!
//! `aws-chunked` is a *transfer encoding*, not a payload: the body is a sequence
//! of length-prefixed chunks followed by a zero-length chunk and optional
//! trailers. It is what a modern AWS SDK emits whenever it streams a payload
//! with a trailing checksum — the default for S3 `PutObject`/`UploadPart`, where
//! botocore sends `content-encoding: aws-chunked` plus an
//! `x-amz-checksum-crc32` trailer:
//!
//! ```text
//! 17\r\nhello-world-exact-bytes\r\n0\r\nx-amz-checksum-crc32:5OQ7/Q==\r\n\r\n
//! ```
//!
//! A server that does not decode this stores the framing bytes as the object:
//! 23 bytes of payload become 65 bytes on disk, `ContentLength` and the ETag are
//! both wrong, and **nothing errors** — a silent data-integrity failure. Real S3
//! decodes the framing before the payload reaches the operation, and so must an
//! emulator that claims byte-exact round-trips.
//!
//! Decoding happens once, at dispatch, before any protocol reads the body:
//! transfer framing is a transport concern and is identical for every protocol.
//!
//! ## Scope
//!
//! * The trailers after the terminating chunk are **skipped, not verified**.
//!   They carry a checksum of the payload; validating it would make the emulator
//!   reject bodies real S3 accepts if either side's checksum implementation drifts,
//!   and the integrity property this module exists for (payload bytes in ==
//!   payload bytes out) does not depend on it.
//! * Chunk signatures (`;chunk-signature=…`, sent when the SDK signs each chunk)
//!   are skipped here: request authentication verifies them on the framed body
//!   before it is decoded.
//! * Malformed framing is an **error**, never a pass-through: a body that claims
//!   `aws-chunked` and is not decodable must not be silently stored, which is the
//!   exact failure mode being fixed.

use aws_smithy_runtime_api::http::Request;
use aws_smithy_types::body::SdkBody;
use std::fmt;

/// Why an `aws-chunked` body could not be decoded.
///
/// Every variant means the request is unusable: the declared framing does not
/// describe the bytes received, so the payload cannot be recovered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkedDecodeError {
    /// A chunk header (the hex size line) was not terminated by CRLF.
    UnterminatedChunkHeader,
    /// A chunk header's size field was not valid hex.
    InvalidChunkSize(String),
    /// A chunk declared more bytes than the body contains.
    TruncatedChunk { declared: usize, available: usize },
    /// A chunk's data was not followed by the mandatory CRLF.
    MissingChunkTerminator,
    /// The decoded length disagreed with `x-amz-decoded-content-length`.
    DecodedLengthMismatch { declared: usize, actual: usize },
}

impl fmt::Display for ChunkedDecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnterminatedChunkHeader => {
                write!(f, "aws-chunked body: chunk header is not CRLF-terminated")
            }
            Self::InvalidChunkSize(raw) => {
                write!(f, "aws-chunked body: invalid chunk size `{raw}`")
            }
            Self::TruncatedChunk {
                declared,
                available,
            } => write!(
                f,
                "aws-chunked body: chunk declares {declared} bytes but only {available} remain"
            ),
            Self::MissingChunkTerminator => {
                write!(f, "aws-chunked body: chunk data is not CRLF-terminated")
            }
            Self::DecodedLengthMismatch { declared, actual } => write!(
                f,
                "aws-chunked body: x-amz-decoded-content-length says {declared} bytes but the body decodes to {actual}"
            ),
        }
    }
}

impl std::error::Error for ChunkedDecodeError {}

/// Whether a `content-encoding` header value declares `aws-chunked`.
///
/// `content-encoding` is a comma-separated list, and `aws-chunked` may sit
/// alongside another token (`aws-chunked, gzip`). Token comparison is
/// case-insensitive per RFC 9110.
pub fn declares_aws_chunked(content_encoding: &str) -> bool {
    content_encoding
        .split(',')
        .any(|token| token.trim().eq_ignore_ascii_case("aws-chunked"))
}

/// Strip the `aws-chunked` token from a `content-encoding` value, returning the
/// remaining encodings (or `None` when nothing is left).
///
/// Once the framing is decoded the header must no longer advertise it, or a
/// downstream reader would try to decode the payload a second time.
pub fn content_encoding_without_aws_chunked(content_encoding: &str) -> Option<String> {
    let remaining: Vec<&str> = content_encoding
        .split(',')
        .map(str::trim)
        .filter(|token| !token.is_empty() && !token.eq_ignore_ascii_case("aws-chunked"))
        .collect();
    if remaining.is_empty() {
        None
    } else {
        Some(remaining.join(", "))
    }
}

/// Decode an `aws-chunked` body into its payload bytes.
///
/// `expected_decoded_len` is the value of `x-amz-decoded-content-length` when the
/// request carried one; the decoded payload is checked against it, turning a
/// framing mismatch into an error instead of a corrupted object.
pub fn decode(
    body: &[u8],
    expected_decoded_len: Option<usize>,
) -> Result<Vec<u8>, ChunkedDecodeError> {
    let mut out: Vec<u8> = Vec::with_capacity(body.len());
    let mut pos = 0usize;

    loop {
        let header_end = find_crlf(body, pos).ok_or(ChunkedDecodeError::UnterminatedChunkHeader)?;
        let header = &body[pos..header_end];

        // `<hex-size>` optionally followed by `;chunk-signature=…` (or any other
        // chunk extension), which carries no payload information.
        let size_field = header.split(|b| *b == b';').next().unwrap_or(header);
        let size_text = std::str::from_utf8(size_field)
            .map_err(|_| ChunkedDecodeError::InvalidChunkSize(lossy(size_field)))?
            .trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| ChunkedDecodeError::InvalidChunkSize(size_text.to_owned()))?;

        pos = header_end + CRLF.len();

        // The zero-length chunk terminates the stream; whatever follows is
        // trailers, which this decoder deliberately does not interpret.
        if size == 0 {
            break;
        }

        let data_end = pos
            .checked_add(size)
            .filter(|end| *end <= body.len())
            .ok_or(ChunkedDecodeError::TruncatedChunk {
                declared: size,
                available: body.len().saturating_sub(pos),
            })?;
        out.extend_from_slice(&body[pos..data_end]);
        pos = data_end;

        if body.get(pos..pos + CRLF.len()) != Some(CRLF) {
            return Err(ChunkedDecodeError::MissingChunkTerminator);
        }
        pos += CRLF.len();
    }

    if let Some(declared) = expected_decoded_len {
        if declared != out.len() {
            return Err(ChunkedDecodeError::DecodedLengthMismatch {
                declared,
                actual: out.len(),
            });
        }
    }

    Ok(out)
}

const CRLF: &[u8] = b"\r\n";

/// Decode a request's body in place when it declares `aws-chunked` framing.
///
/// Returns `Ok(false)` and leaves the request untouched when the request is not
/// `aws-chunked` — the overwhelmingly common case, and a pure header check.
///
/// On success the request is left exactly as if the client had sent the payload
/// unframed: the body holds the payload bytes, `content-length` reports the
/// payload length, `x-amz-decoded-content-length` is gone (it has been folded
/// into `content-length`), and `content-encoding` no longer advertises
/// `aws-chunked` so nothing tries to decode it twice.
///
/// `content-encoding` is the signal, because that is what AWS SDKs set to
/// announce the framing. `x-amz-decoded-content-length` is treated as a
/// *cross-check*, never as the trigger: a request carrying it without the
/// encoding is not framed, and decoding one would corrupt the payload.
pub fn decode_request(request: &mut Request) -> Result<bool, ChunkedDecodeError> {
    let Some(content_encoding) = request.headers().get("content-encoding") else {
        return Ok(false);
    };
    if !declares_aws_chunked(content_encoding) {
        return Ok(false);
    }
    let remaining_encoding = content_encoding_without_aws_chunked(content_encoding);

    let declared_len = request
        .headers()
        .get("x-amz-decoded-content-length")
        .and_then(|v| v.trim().parse::<usize>().ok());

    // A body the transport could not buffer cannot be reframed; leaving it as-is
    // would store framing bytes, so this is a hard failure rather than a
    // pass-through.
    let framed = request
        .body()
        .bytes()
        .ok_or(ChunkedDecodeError::UnterminatedChunkHeader)?;
    let payload = decode(framed, declared_len)?;

    let payload_len = payload.len();
    *request.body_mut() = SdkBody::from(payload);

    let headers = request.headers_mut();
    headers.insert("content-length", payload_len.to_string());
    headers.remove("x-amz-decoded-content-length");
    match remaining_encoding {
        Some(rest) => headers.insert("content-encoding", rest),
        None => headers.remove("content-encoding"),
    };

    Ok(true)
}

/// Index of the next CRLF at or after `from`.
fn find_crlf(body: &[u8], from: usize) -> Option<usize> {
    if from >= body.len() {
        return None;
    }
    body[from..]
        .windows(CRLF.len())
        .position(|w| w == CRLF)
        .map(|i| from + i)
}

/// Render bytes for an error message without assuming UTF-8.
fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The exact framing botocore puts on the wire for a checksummed
    /// `PutObject`: one 0x17-byte chunk, terminator, CRC32 trailer.
    const BOTOCORE_PUT_OBJECT: &[u8] =
        b"17\r\nhello-world-exact-bytes\r\n0\r\nx-amz-checksum-crc32:5OQ7/Q==\r\n\r\n";
    const PAYLOAD: &[u8] = b"hello-world-exact-bytes";

    #[test]
    fn decodes_botocore_put_object_framing_to_exact_payload() {
        let decoded = decode(BOTOCORE_PUT_OBJECT, None).expect("valid framing");
        assert_eq!(decoded, PAYLOAD);
        assert_eq!(decoded.len(), 23, "0x17 == 23 payload bytes");
    }

    #[test]
    fn honours_declared_decoded_content_length() {
        assert_eq!(
            decode(BOTOCORE_PUT_OBJECT, Some(23)).expect("length agrees"),
            PAYLOAD
        );
    }

    /// Negative: a `x-amz-decoded-content-length` that disagrees with the framing
    /// is a corrupted request, not a silently-truncated object.
    #[test]
    fn rejects_decoded_content_length_mismatch() {
        assert_eq!(
            decode(BOTOCORE_PUT_OBJECT, Some(99)),
            Err(ChunkedDecodeError::DecodedLengthMismatch {
                declared: 99,
                actual: 23,
            })
        );
    }

    #[test]
    fn concatenates_multiple_chunks_in_order() {
        let body = b"4\r\nabcd\r\n3\r\nefg\r\n0\r\n\r\n";
        assert_eq!(decode(body, Some(7)).expect("valid"), b"abcdefg");
    }

    #[test]
    fn accepts_chunk_signature_extensions() {
        let body: &[u8] = b"5;chunk-signature=deadbeef\r\nhello\r\n\
                            0;chunk-signature=cafebabe\r\n\r\n";
        assert_eq!(decode(body, Some(5)).expect("valid"), b"hello");
    }

    #[test]
    fn accepts_uppercase_hex_sizes() {
        let body = b"1F\r\naaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n0\r\n\r\n";
        assert_eq!(decode(body, None).expect("valid").len(), 0x1F);
    }

    /// A body that is only the terminating chunk decodes to nothing — a legal
    /// zero-byte object, not an error.
    #[test]
    fn terminator_only_body_decodes_to_empty_payload() {
        assert_eq!(decode(b"0\r\n\r\n", Some(0)).expect("valid"), b"");
    }

    /// Negative: preserves payload bytes that themselves look like framing.
    #[test]
    fn payload_containing_crlf_and_digits_is_preserved_verbatim() {
        let payload = b"5\r\nnot-a-chunk\r\n";
        let mut body = format!("{:x}\r\n", payload.len()).into_bytes();
        body.extend_from_slice(payload);
        body.extend_from_slice(b"\r\n0\r\n\r\n");
        assert_eq!(
            decode(&body, Some(payload.len())).expect("valid"),
            payload,
            "chunk sizes are authoritative; payload is never re-scanned"
        );
    }

    // ---- negative cases: malformed framing must error, never pass through ----

    #[test]
    fn rejects_header_without_crlf() {
        assert_eq!(
            decode(b"17", None),
            Err(ChunkedDecodeError::UnterminatedChunkHeader)
        );
    }

    #[test]
    fn rejects_empty_body() {
        assert_eq!(
            decode(b"", None),
            Err(ChunkedDecodeError::UnterminatedChunkHeader)
        );
    }

    #[test]
    fn rejects_non_hex_chunk_size() {
        assert_eq!(
            decode(b"zz\r\nabc\r\n0\r\n\r\n", None),
            Err(ChunkedDecodeError::InvalidChunkSize("zz".to_owned()))
        );
    }

    #[test]
    fn rejects_empty_chunk_size() {
        assert_eq!(
            decode(b"\r\nabc\r\n", None),
            Err(ChunkedDecodeError::InvalidChunkSize(String::new()))
        );
    }

    #[test]
    fn rejects_non_utf8_chunk_size() {
        assert_eq!(
            decode(b"\xff\xfe\r\nabc\r\n", None),
            Err(ChunkedDecodeError::InvalidChunkSize(
                "\u{fffd}\u{fffd}".to_owned()
            ))
        );
    }

    /// A chunk longer than the remaining body: the classic truncated upload.
    #[test]
    fn rejects_chunk_longer_than_body() {
        assert_eq!(
            decode(b"20\r\nshort\r\n", None),
            Err(ChunkedDecodeError::TruncatedChunk {
                declared: 0x20,
                available: 7,
            })
        );
    }

    /// A chunk size so large it would overflow `usize` when added to the
    /// position must be rejected, not wrap around.
    #[test]
    fn rejects_chunk_size_that_would_overflow() {
        let body = format!("{:x}\r\nx\r\n", usize::MAX).into_bytes();
        assert!(matches!(
            decode(&body, None),
            Err(ChunkedDecodeError::TruncatedChunk { .. })
        ));
    }

    #[test]
    fn rejects_chunk_data_without_trailing_crlf() {
        assert_eq!(
            decode(b"3\r\nabcXX0\r\n\r\n", None),
            Err(ChunkedDecodeError::MissingChunkTerminator)
        );
    }

    /// Data that ends exactly at the end of the body has no room for its CRLF.
    #[test]
    fn rejects_chunk_data_that_ends_at_body_end() {
        assert_eq!(
            decode(b"3\r\nabc", None),
            Err(ChunkedDecodeError::MissingChunkTerminator)
        );
    }

    // ---- header helpers ----

    #[test]
    fn declares_aws_chunked_matches_tokens_case_insensitively() {
        assert!(declares_aws_chunked("aws-chunked"));
        assert!(declares_aws_chunked("AWS-Chunked"));
        assert!(declares_aws_chunked("aws-chunked, gzip"));
        assert!(declares_aws_chunked("gzip,aws-chunked"));
        assert!(!declares_aws_chunked("gzip"));
        assert!(!declares_aws_chunked(""));
        assert!(
            !declares_aws_chunked("aws-chunked-v2"),
            "token match, not substring match"
        );
    }

    #[test]
    fn content_encoding_without_aws_chunked_strips_only_that_token() {
        assert_eq!(content_encoding_without_aws_chunked("aws-chunked"), None);
        assert_eq!(
            content_encoding_without_aws_chunked("aws-chunked, gzip"),
            Some("gzip".to_owned())
        );
        assert_eq!(
            content_encoding_without_aws_chunked("gzip, aws-chunked, br"),
            Some("gzip, br".to_owned())
        );
        assert_eq!(content_encoding_without_aws_chunked(""), None);
    }

    #[test]
    fn find_crlf_locates_first_occurrence_from_offset() {
        assert_eq!(find_crlf(b"ab\r\ncd\r\n", 0), Some(2));
        assert_eq!(find_crlf(b"ab\r\ncd\r\n", 3), Some(6));
        assert_eq!(find_crlf(b"abcd", 0), None);
        assert_eq!(find_crlf(b"ab\r\n", 9), None, "offset past the end");
    }

    #[test]
    fn error_messages_name_the_failure() {
        assert!(ChunkedDecodeError::UnterminatedChunkHeader
            .to_string()
            .contains("CRLF-terminated"));
        assert!(ChunkedDecodeError::InvalidChunkSize("zz".into())
            .to_string()
            .contains("`zz`"));
        assert!(ChunkedDecodeError::TruncatedChunk {
            declared: 5,
            available: 2
        }
        .to_string()
        .contains("only 2 remain"));
        assert!(ChunkedDecodeError::MissingChunkTerminator
            .to_string()
            .contains("chunk data"));
        assert!(ChunkedDecodeError::DecodedLengthMismatch {
            declared: 9,
            actual: 3
        }
        .to_string()
        .contains("x-amz-decoded-content-length"));
    }

    // ---- request-level decoding ----

    fn framed_request(headers: &[(&str, &str)], body: &[u8]) -> Request {
        let mut request = Request::new(SdkBody::from(body.to_vec()));
        request.set_uri("/bucket/key").expect("valid uri");
        request.set_method("PUT").expect("valid method");
        for (name, value) in headers {
            request
                .headers_mut()
                .insert(name.to_string(), value.to_string());
        }
        request
    }

    fn body_of(request: &Request) -> Vec<u8> {
        request.body().bytes().expect("in-memory body").to_vec()
    }

    /// The whole point: what botocore sends must reach the handler as the exact
    /// payload, with `content-length` telling the truth about it.
    #[test]
    fn decode_request_replaces_body_and_corrects_content_length() {
        let mut request = framed_request(
            &[
                ("content-encoding", "aws-chunked"),
                ("x-amz-decoded-content-length", "23"),
                ("content-length", "65"),
            ],
            BOTOCORE_PUT_OBJECT,
        );

        assert!(decode_request(&mut request).expect("valid framing"));

        assert_eq!(body_of(&request), PAYLOAD);
        assert_eq!(request.headers().get("content-length"), Some("23"));
        assert_eq!(
            request.headers().get("x-amz-decoded-content-length"),
            None,
            "folded into content-length"
        );
        assert_eq!(
            request.headers().get("content-encoding"),
            None,
            "framing must not be advertised after decoding"
        );
    }

    /// A request with no `content-encoding` is untouched — no header probing
    /// beyond the one lookup, and the body is left byte-identical.
    #[test]
    fn decode_request_is_a_no_op_without_content_encoding() {
        let mut request = framed_request(&[("content-length", "5")], b"plain");
        assert!(!decode_request(&mut request).expect("no framing"));
        assert_eq!(body_of(&request), b"plain");
        assert_eq!(request.headers().get("content-length"), Some("5"));
    }

    /// `x-amz-decoded-content-length` alone must NOT trigger decoding: treating
    /// it as the signal would shred an unframed payload.
    #[test]
    fn decode_request_ignores_decoded_length_header_without_the_encoding() {
        let mut request = framed_request(
            &[("x-amz-decoded-content-length", "23")],
            BOTOCORE_PUT_OBJECT,
        );
        assert!(!decode_request(&mut request).expect("no framing"));
        assert_eq!(body_of(&request), BOTOCORE_PUT_OBJECT);
    }

    #[test]
    fn decode_request_is_a_no_op_for_other_encodings() {
        let mut request = framed_request(&[("content-encoding", "gzip")], b"\x1f\x8bnot-chunked");
        assert!(!decode_request(&mut request).expect("no aws-chunked"));
        assert_eq!(body_of(&request), b"\x1f\x8bnot-chunked");
        assert_eq!(request.headers().get("content-encoding"), Some("gzip"));
    }

    /// A second encoding alongside `aws-chunked` survives; only the framing
    /// token is consumed.
    #[test]
    fn decode_request_keeps_other_encodings() {
        let mut request = framed_request(
            &[("content-encoding", "aws-chunked, gzip")],
            BOTOCORE_PUT_OBJECT,
        );
        assert!(decode_request(&mut request).expect("valid framing"));
        assert_eq!(request.headers().get("content-encoding"), Some("gzip"));
    }

    /// Negative: malformed framing surfaces as an error so the caller can reject
    /// the request, instead of storing framing bytes as the object.
    #[test]
    fn decode_request_errors_on_malformed_framing() {
        let mut request =
            framed_request(&[("content-encoding", "aws-chunked")], b"20\r\nshort\r\n");
        assert_eq!(
            decode_request(&mut request),
            Err(ChunkedDecodeError::TruncatedChunk {
                declared: 0x20,
                available: 7,
            })
        );
    }

    /// Negative: a declared length that disagrees with the framing is rejected
    /// at the request level too.
    #[test]
    fn decode_request_errors_on_declared_length_mismatch() {
        let mut request = framed_request(
            &[
                ("content-encoding", "aws-chunked"),
                ("x-amz-decoded-content-length", "1"),
            ],
            BOTOCORE_PUT_OBJECT,
        );
        assert_eq!(
            decode_request(&mut request),
            Err(ChunkedDecodeError::DecodedLengthMismatch {
                declared: 1,
                actual: 23,
            })
        );
    }

    /// A junk `x-amz-decoded-content-length` is ignored rather than fatal: the
    /// framing itself is authoritative.
    #[test]
    fn decode_request_ignores_unparseable_declared_length() {
        let mut request = framed_request(
            &[
                ("content-encoding", "aws-chunked"),
                ("x-amz-decoded-content-length", "not-a-number"),
            ],
            BOTOCORE_PUT_OBJECT,
        );
        assert!(decode_request(&mut request).expect("framing is authoritative"));
        assert_eq!(body_of(&request), PAYLOAD);
    }
}
