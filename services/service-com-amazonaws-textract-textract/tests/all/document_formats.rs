// SPDX-License-Identifier: CC0-1.0

//! WHICH DOCUMENTS Amazon Textract reads, end to end over the wire (`awsJson1_1`,
//! `POST /` with `X-Amz-Target: Textract.<Operation>`).
//!
//! THE RULE UNDER TEST: a document is read only when its bytes are in one of the formats
//! the service processes on the channel they arrived on, and that format is decided on
//! the bytes themselves rather than on anything the request declared about them. AWS
//! states the formats per channel: `Document$Bytes` is "a blob of base64-encoded document
//! bytes … The document bytes must be in PNG or JPEG format", while
//! `UnsupportedDocumentException` documents the formats of a document at large —
//! "Documents for operations can be in PNG, JPEG, PDF, or TIFF format" — which is what an
//! Amazon S3 object may hold. Every other byte string is a document no page can be read
//! out of, and is refused with `UnsupportedDocumentException` (HTTP 400) and the one
//! sentence the service states for that class, by every path that reads a document.
//!
//! The table spans what the rule generalises over: the FORMAT the bytes declare (the two
//! accepted inline, the one accepted only from an object, two the service names but this
//! emulator reads nothing out of, two formats it does not process at all, characters, and
//! bytes that declare no format), the CHANNEL the document arrived on, and the operation
//! that read it. A PDF is among the refused rows deliberately: Textract accepts the
//! format, and this emulator renders no PDF page, so it reads nothing out of one.

use image::ImageFormat;
use serde_json::{json, Value};

use crate::document_extraction::{error_code, error_message};
use crate::raster_documents::{encode, render, Paper};
use crate::s3_sourced_documents::{
    call, emulator, given_bucket, given_object, s3_document, BUCKET,
};

/// The sentence Amazon Textract states `UnsupportedDocumentException` with: fixed text,
/// with no part of it taken from the request.
const UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE: &str = "Request has unsupported document format";

/// Comfortably past `state::JOB_COMPLETION_DELAY`, so a job's terminal transition is due.
const PAST_JOB_COMPLETION: std::time::Duration = std::time::Duration::from_millis(900);

/// One row of the format table: the bytes, and which channel carries them.
struct FormatCase {
    name: &'static str,
    bytes: Vec<u8>,
    /// Whether inline `Document$Bytes` carry a document in this format.
    inline_read: bool,
    /// Whether an Amazon S3 object may hold a document in this format.
    object_read: bool,
}

/// A page of real pixels, encoded in `format`.
fn page(format: ImageFormat) -> Vec<u8> {
    encode(
        &render(
            &["SIMPARITY LEDGER", "Account Number: 48213"],
            Paper {
                scale: 4,
                ..Paper::plain()
            },
        ),
        format,
    )
}

fn format_cases() -> Vec<FormatCase> {
    let png = page(ImageFormat::Png);
    vec![
        FormatCase {
            name: "a PNG",
            bytes: png.clone(),
            inline_read: true,
            object_read: true,
        },
        FormatCase {
            name: "a JPEG",
            bytes: page(ImageFormat::Jpeg),
            inline_read: true,
            object_read: true,
        },
        FormatCase {
            // A TIFF is a document format the service names, and not one inline bytes
            // carry.
            name: "a TIFF",
            bytes: page(ImageFormat::Tiff),
            inline_read: false,
            object_read: true,
        },
        FormatCase {
            // Textract accepts PDF; this emulator renders no PDF page and parses no PDF
            // content stream, so it reads nothing out of one on either channel.
            name: "a PDF",
            bytes: b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n%%EOF\n".to_vec(),
            inline_read: false,
            object_read: false,
        },
        FormatCase {
            // A real raster format, and not one the service names.
            name: "a GIF",
            bytes: b"GIF89a\x01\x00\x01\x00\x80\x00\x00\x00\x00\x00\xff\xff\xff\
                     !\xf9\x04\x01\x00\x00\x00\x00,\x00\x00\x00\x00\x01\x00\x01\x00\x00\
                     \x02\x02D\x01\x00;"
                .to_vec(),
            inline_read: false,
            object_read: false,
        },
        FormatCase {
            // Netpbm's P1 is a raster whose container is printable text, so it is a
            // format declared by its own magic number rather than by what it holds.
            name: "a Netpbm bitmap",
            bytes: b"P1\n2 2\n0 1\n1 0\n".to_vec(),
            inline_read: false,
            object_read: false,
        },
        FormatCase {
            // Characters are not one of the document formats the service processes.
            name: "a text document",
            bytes: b"Invoice Number: INV-2048\nTotal Due: 17.25\n".to_vec(),
            inline_read: false,
            object_read: false,
        },
        FormatCase {
            name: "bytes that declare no format at all",
            bytes: vec![0x00, 0x01, 0x02, 0xFF, 0xFE, 0x7F],
            inline_read: false,
            object_read: false,
        },
        FormatCase {
            // An accepted format whose stream is cut: a document no page can be read out
            // of, which gets the same answer as a format the service does not process.
            name: "a PNG truncated mid-stream",
            bytes: png[..64].to_vec(),
            inline_read: false,
            object_read: false,
        },
    ]
}

/// The operation rows every document is submitted through: every synchronous path that
/// reads one, with the request each needs.
fn synchronous_requests(document: &Value) -> Vec<(&'static str, Value)> {
    vec![
        (
            "DetectDocumentText",
            json!({ "Document": document.clone() }),
        ),
        (
            "AnalyzeDocument",
            json!({ "Document": document.clone(), "FeatureTypes": ["FORMS"] }),
        ),
        ("AnalyzeExpense", json!({ "Document": document.clone() })),
    ]
}

#[tokio::test]
async fn a_document_is_read_only_in_a_format_the_channel_it_arrived_on_carries() {
    let router = emulator();
    given_bucket(&router).await;

    for (index, case) in format_cases().iter().enumerate() {
        // The same bytes on both channels: as the request's own `Document$Bytes`, and as
        // the body of an object staged with `PutObject`.
        let key = format!("document-{index}");
        given_object(&router, &key, &case.bytes).await;
        let channels: Vec<(&str, Value, bool)> = vec![
            (
                "inline Bytes",
                json!({ "Bytes": aws_smithy_types::base64::encode(&case.bytes) }),
                case.inline_read,
            ),
            ("an S3 object", s3_document(&key), case.object_read),
        ];

        for (channel, document, must_be_read) in channels {
            for (operation, request) in synchronous_requests(&document) {
                let name = format!("{} on {channel} / {operation}", case.name);
                let (status, body) = call(&router, operation, &request).await;
                if must_be_read {
                    assert_eq!(
                        status, 200,
                        "case {name:?}: a document in a format this channel carries must be \
                         read, got {status} with {body}"
                    );
                    assert_eq!(
                        body["DocumentMetadata"]["Pages"].as_u64(),
                        Some(1),
                        "case {name:?}: one raster is one page, got {body}"
                    );
                    continue;
                }
                assert_eq!(
                    status, 400,
                    "case {name:?}: a document in a format this channel does not carry must \
                     be refused, got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "UnsupportedDocumentException",
                    "case {name:?}: expected UnsupportedDocumentException, got {body}"
                );
                assert_eq!(
                    error_message(&body),
                    UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE,
                    "case {name:?}: expected the message {UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE:?}, \
                     got {:?}",
                    error_message(&body)
                );
                assert!(
                    body.get("Blocks").is_none() && body.get("ExpenseDocuments").is_none(),
                    "case {name:?}: a refusal must carry no result, got {body}"
                );
            }
        }
    }
}

/// RULE, on the asynchronous side: the same classification decides what a JOB reports. An
/// object in a format the service reads settles `SUCCEEDED`; one in a format it does not
/// read is taken on and ends in the modelled `FAILED` state carrying the reason, rather
/// than reporting blocks for a document nothing read.
#[tokio::test]
async fn a_job_over_an_object_the_service_cannot_read_ends_failed() {
    let router = emulator();
    given_bucket(&router).await;

    // (case, the object's bytes, the terminal status it settles in)
    let objects: Vec<(&str, Vec<u8>, &str)> = vec![
        ("a PNG", page(ImageFormat::Png), "SUCCEEDED"),
        ("a TIFF", page(ImageFormat::Tiff), "SUCCEEDED"),
        (
            "a text document",
            b"Invoice Number: INV-2048\n".to_vec(),
            "FAILED",
        ),
        (
            "a PDF this emulator reads no page out of",
            b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\ntrailer\n%%EOF\n".to_vec(),
            "FAILED",
        ),
    ];

    for (index, (name, bytes, expected)) in objects.iter().enumerate() {
        let key = format!("async-document-{index}");
        given_object(&router, &key, bytes).await;
        let (status, started) = call(
            &router,
            "StartDocumentTextDetection",
            &json!({ "DocumentLocation": { "S3Object": { "Bucket": BUCKET, "Name": key } } }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a start over an object the store holds is accepted, got {status} \
             with {started}"
        );
        let job_id = started["JobId"].as_str().unwrap_or_default().to_string();
        assert!(
            !job_id.is_empty(),
            "case {name:?}: an accepted start must answer with a JobId, got {started}"
        );

        std::thread::sleep(PAST_JOB_COMPLETION);
        let (status, settled) = call(
            &router,
            "GetDocumentTextDetection",
            &json!({ "JobId": job_id }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the settled read must succeed, got {status} with {settled}"
        );
        assert_eq!(
            settled["JobStatus"].as_str(),
            Some(*expected),
            "case {name:?}: expected the job to settle {expected}, got {settled}"
        );
        if *expected == "FAILED" {
            assert_eq!(
                settled["StatusMessage"].as_str(),
                Some(UNSUPPORTED_DOCUMENT_FORMAT_MESSAGE),
                "case {name:?}: a failed job must say why, got {settled}"
            );
            assert!(
                settled["Blocks"]
                    .as_array()
                    .is_none_or(|blocks| blocks.is_empty()),
                "case {name:?}: a failed job reports no blocks, got {settled}"
            );
        }
    }
}
