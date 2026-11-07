// SPDX-License-Identifier: CC0-1.0

//! Which input source an asynchronous submission carries, and WHEN that is decided.
//!
//! `InvokeEndpointAsync` takes its input either inline (`Body`) or by reference
//! (`InputLocation`), and the model documents them as mutually exclusive: "Body and
//! InputLocation are mutually exclusive. Provide exactly one of them."
//! (`smithy_query(query="doc", shape="InvokeEndpointAsyncInput", member="Body")`).
//!
//! The rule asserted here is about the REQUEST, not about one recorded call: how many
//! input sources a submission carries is a property of the request alone, so it is decided
//! before the endpoint the submission addresses is resolved — a submission carrying none,
//! or two, is refused for that whether or not the account hosts the endpoint, and the
//! refusal states the rule and never the endpoint. A submission carrying exactly one
//! source is well formed, and only then does the endpoint lookup answer.
//!
//! The rows span what the rule generalises over: how many sources the request carries,
//! the form and size of an inline body (an empty payload is indistinguishable on the wire
//! from an absent one, so it is no source at all, and one past the modelled ceiling is no
//! usable source either), the scheme of a referenced location — both `InputLocation`'s
//! modelled pattern `^(https|s3)://([^/]+)/?(.*)$` admits — and whether the endpoint
//! exists at all.

use service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::state::InvocationOutcome;

use crate::harness::{
    async_inference_config, async_invocations, async_queue, post, Emulator, ACCOUNT,
};

/// Where the seeded deployment writes its asynchronous results.
const OUTPUT_PATH: &str = "s3://aws-emulator-sagemaker/async/output";

/// The header `InputLocation` binds (`@httpHeader X-Amzn-SageMaker-InputLocation`).
const INPUT_LOCATION: &str = "x-amzn-sagemaker-inputlocation";

/// The refusal of a submission that carries no input source at all.
const NO_SOURCE: &str = "One of Body or InputLocation is required";

/// The refusal of a submission that carries both.
const TWO_SOURCES: &str = "Both Body and InputLocation were provided. Provide exactly one.";

/// `AsyncBodyBlob`'s modelled maximum — "The inline payload can be up to 128,000 bytes"
/// (`smithy_query(query="shape", shape="AsyncBodyBlob")`) — and the refusal of an inline
/// source past it, in the framework's own clause for a `@length` maximum. No value is
/// quoted: the member is `@sensitive`.
const CEILING: usize = 128_000;
const OVER_CEILING: &str = "1 validation error detected: Value at 'body' failed to satisfy \
                            constraint: Member must have length less than or equal to 128000";

/// What a row expects of the answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Expected {
    /// Refused for the input-source rule, with this `Message`, before the endpoint resolves.
    Refused(&'static str, InvocationOutcome),
    /// Well formed: exactly one source, so the endpoint lookup answers.
    Addressed,
}

/// One row: the case, the `InputLocation` header it sends (if any), the inline body it
/// sends, the other request members it carries, and what the answer must be.
type Row = (
    &'static str,
    Option<&'static str>,
    Vec<u8>,
    &'static [(&'static str, &'static str)],
    Expected,
);

/// Members of the request that name no input source at all: whatever a submission carries
/// beside its source, the rule is decided on the source alone.
const OTHER_MEMBERS: &[(&str, &str)] = &[
    ("x-amzn-sagemaker-inferenceid", "caller-chosen-id"),
    ("x-amzn-sagemaker-custom-attributes", "tenant=blue"),
    ("accept", "application/json"),
];

/// A body at `InputLocation`-free sizes the member admits (`AsyncBodyBlob` is
/// `@length 0..128000`, `smithy_query(query="shape", shape="AsyncBodyBlob")`).
const JSON_BODY: &[u8] = b"{\"instances\":[[1.0,2.0]]}";
const BINARY_BODY: &[u8] = &[0x00, 0x01, 0x02, 0xff];
const ONE_BYTE_BODY: &[u8] = b"x";

/// The rows, spanning the source combinations and the forms each source can take.
fn rows() -> Vec<Row> {
    vec![
        (
            // An empty `@httpPayload` blob is not distinguishable on the wire from an
            // absent one, so an empty inline body is no source.
            "neither source",
            None,
            Vec::new(),
            &[],
            Expected::Refused(NO_SOURCE, InvocationOutcome::NoInput),
        ),
        (
            "neither source, every other member set",
            None,
            Vec::new(),
            OTHER_MEMBERS,
            Expected::Refused(NO_SOURCE, InvocationOutcome::NoInput),
        ),
        (
            "both: s3 location and a json body",
            Some("s3://bucket/in/req.json"),
            JSON_BODY.to_vec(),
            &[],
            Expected::Refused(TWO_SOURCES, InvocationOutcome::InputAmbiguous),
        ),
        (
            "both: https location and a one-byte body",
            Some("https://bucket.example/in/req.bin"),
            ONE_BYTE_BODY.to_vec(),
            &[],
            Expected::Refused(TWO_SOURCES, InvocationOutcome::InputAmbiguous),
        ),
        (
            "both: s3 location and a binary body, every other member set",
            Some("s3://bucket/in/req.bin"),
            BINARY_BODY.to_vec(),
            OTHER_MEMBERS,
            Expected::Refused(TWO_SOURCES, InvocationOutcome::InputAmbiguous),
        ),
        (
            "an inline source one byte past the modelled ceiling",
            None,
            vec![b'x'; CEILING + 1],
            &[],
            Expected::Refused(OVER_CEILING, InvocationOutcome::PayloadOverMaximum),
        ),
        (
            "only an s3 location",
            Some("s3://bucket/in/req.json"),
            Vec::new(),
            &[],
            Expected::Addressed,
        ),
        (
            "only an https location",
            Some("https://bucket.example/in/req.json"),
            Vec::new(),
            &[],
            Expected::Addressed,
        ),
        (
            "only a json body",
            None,
            JSON_BODY.to_vec(),
            &[],
            Expected::Addressed,
        ),
        (
            "only a binary body",
            None,
            BINARY_BODY.to_vec(),
            &[],
            Expected::Addressed,
        ),
        (
            "only a one-byte body",
            None,
            ONE_BYTE_BODY.to_vec(),
            &[],
            Expected::Addressed,
        ),
        (
            "an inline source at the modelled ceiling",
            None,
            vec![b'x'; CEILING],
            &[],
            Expected::Addressed,
        ),
    ]
}

/// The headers one row sends: its content type, its `InputLocation` when it names one, and
/// whatever other members the row carries.
fn headers<'h>(location: Option<&'h str>, extra: &[(&'h str, &'h str)]) -> Vec<(&'h str, &'h str)> {
    let mut headers = vec![("content-type", "application/json")];
    if let Some(location) = location {
        headers.push((INPUT_LOCATION, location));
    }
    headers.extend_from_slice(extra);
    headers
}

/// RULE: a submission the request layer cannot take an input source from — one carrying
/// neither `Body` nor `InputLocation`, one carrying both, or one whose inline source is
/// past the ceiling the model declares for it — is refused with the modelled
/// `ValidationError` (HTTP 400) naming that rule, decided on the request before the
/// endpoint is resolved, so the refusal is the same for an endpoint the account does not
/// host, names neither the endpoint nor the account, and queues nothing.
#[tokio::test]
async fn the_input_source_rule_is_decided_before_the_endpoint_is_resolved() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "hosted",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    // The endpoint half of each row: one the account hosts and one it does not. The
    // input-source rule is decided before either is resolved, so it must answer the same.
    let endpoints = ["hosted", "absent-endpoint"];

    for (case, location, body, extra, expected) in rows() {
        let Expected::Refused(message, outcome) = expected else {
            continue;
        };
        for endpoint in endpoints {
            let case = format!("{case} / {endpoint}");
            let before = async_queue(&emulator).len();
            let call = post(
                &emulator,
                &async_invocations(endpoint),
                &headers(location, extra),
                &body,
            )
            .await;
            assert_eq!(
                call.status, 400,
                "case {case}: expected HTTP 400, got {} / {}",
                call.status,
                String::from_utf8_lossy(&call.body)
            );
            assert_eq!(
                call.error_code(),
                "ValidationError",
                "case {case}: expected the modelled ValidationError, got {:?}",
                call.error_code()
            );
            assert_eq!(
                call.message(),
                message,
                "case {case}: expected the input-source rule, got {:?}",
                call.message()
            );
            assert!(
                !call.message().contains(endpoint) && !call.message().contains(ACCOUNT),
                "case {case}: the rule is decided before the endpoint resolves, so the refusal \
                 names neither it nor the account, got {:?}",
                call.message()
            );
            assert_eq!(
                async_queue(&emulator).len(),
                before,
                "case {case}: a refused submission must queue nothing"
            );
            let logged = emulator
                .last_logged()
                .unwrap_or_else(|| panic!("case {case}: the refusal must be logged"));
            assert_eq!(
                logged.outcome,
                outcome,
                "case {case}: expected the {} outcome to be logged, got {}",
                outcome.label(),
                logged.outcome.label()
            );
        }
    }
}

/// RULE: a submission carrying EXACTLY ONE input source is well formed, so it gets as far
/// as the endpoint lookup: an endpoint the account does not host is refused for that, and
/// a hosted one queues the submission and reports where its result will be.
#[tokio::test]
async fn exactly_one_input_source_reaches_the_endpoint_lookup() {
    let emulator = Emulator::new();
    emulator
        .deploy(
            "hosted",
            &["AllTraffic"],
            async_inference_config(OUTPUT_PATH, None),
        )
        .await;

    for (case, location, body, extra, expected) in rows() {
        if expected != Expected::Addressed {
            continue;
        }

        // 1. An endpoint the account does not host: the lookup is what answers, naming the
        //    endpoint and the account.
        let absent = post(
            &emulator,
            &async_invocations("absent-endpoint"),
            &headers(location, extra),
            &body,
        )
        .await;
        assert_eq!(
            absent.status, 400,
            "case {case} / absent: expected HTTP 400, got {} / {}",
            absent.status,
            String::from_utf8_lossy(&absent.body)
        );
        assert_eq!(
            absent.message(),
            format!("Endpoint absent-endpoint of account {ACCOUNT} not found."),
            "case {case} / absent: the endpoint lookup must answer, got {:?}",
            absent.message()
        );

        // 2. The hosted endpoint: the submission is queued and read back under the id the
        //    answer reported.
        let queued_before = async_queue(&emulator).len();
        let accepted = post(
            &emulator,
            &async_invocations("hosted"),
            &headers(location, extra),
            &body,
        )
        .await;
        assert_eq!(
            accepted.status, 202,
            "case {case} / hosted: expected HTTP 202, got {} / {}",
            accepted.status,
            String::from_utf8_lossy(&accepted.body)
        );
        let reported = accepted.json()["InferenceId"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !reported.is_empty(),
            "case {case} / hosted: an accepted submission reports its inference id, got {}",
            accepted.json()
        );
        assert_eq!(
            async_queue(&emulator).len(),
            queued_before + 1,
            "case {case} / hosted: an accepted submission must be queued"
        );
        let record = async_queue(&emulator)
            .into_iter()
            .find(|invocation| invocation.inference_id == reported)
            .unwrap_or_else(|| {
                panic!("case {case} / hosted: the queued record must be keyed by {reported:?}")
            });
        // The record keeps the source the submission really carried: the referenced
        // location, or the size of the inline payload — never both.
        match location {
            Some(location) => assert_eq!(
                (record.input_location.as_deref(), record.inline_input_bytes),
                (Some(location), None),
                "case {case} / hosted: the referenced location is the queued source, got {:?} / \
                 {:?}",
                record.input_location,
                record.inline_input_bytes
            ),
            None => assert_eq!(
                (record.input_location.as_deref(), record.inline_input_bytes),
                (None, Some(body.len())),
                "case {case} / hosted: the inline payload is the queued source, got {:?} / {:?}",
                record.input_location,
                record.inline_input_bytes
            ),
        }
    }
}
