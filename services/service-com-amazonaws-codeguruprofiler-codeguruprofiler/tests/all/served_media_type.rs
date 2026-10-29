// SPDX-License-Identifier: CC0-1.0

//! The media type `GetProfile` announces a served profile under, end to end over HTTP
//! through the router this service mounts in production.
//!
//! The rule every case here asserts: the response's `contentType` is one of the two media
//! types this service serves profiles under, never a media type carried over from an
//! upload. The model states the member's range itself — "The content type of the profile
//! in the payload. It is either application/json or the default application/x-amzn-ion"
//! (`smithy_query(query="doc",shape="GetProfileResponse",member="contentType")`) — so for
//! a stored profile whose bytes hold a document the member names the representation the
//! served bytes are in, and for bytes that are a document in NEITHER representation (there
//! is nothing in them to re-encode or to bound, so they are served exactly as stored) it
//! names the representation their stored media type names, or the default where it names
//! neither.
//!
//! Rows vary what the rule generalises over: the media type the stored profile was
//! announced under (each of the two the model names, one of them with a media-type
//! parameter, one of them in another letter case, three media types of other formats, a
//! near-miss subtype, a string that is no media type at all, and no media type at all)
//! crossed with the kind of payload that is no document in either representation (a
//! truncated text, every byte value, a document in an encoding this operation does not
//! read, no bytes at all, and a document nested past the depth it reads at) and with what
//! the request asks for (a representation, a depth bound, both, neither).

use crate::interval_validation::create_group;
use crate::profile_times::place_profile;
use crate::profiling_group_lifecycle::Harness;

/// The media types the model's `GetProfileResponse$contentType` doc names: "It is either
/// application/json or the default application/x-amzn-ion."
const JSON: &str = "application/json";
const ION: &str = "application/x-amzn-ion";

/// The media type the same doc calls "the default", which is what a served profile is
/// announced under when nothing about it names one of the two.
const DEFAULT: &str = ION;

/// The instant every profile below is placed at, in epoch seconds: long past, so the
/// aggregation it is owed has fallen due and the only thing deciding what is announced is
/// the profile and the request.
const RECEIPT_SECONDS: i64 = 1_600_000_000;

/// The media type a stored profile was announced under, and the media type a served
/// profile whose bytes are no document is then announced under.
///
/// SOURCE for the two media types a row can expect: the model's
/// `GetProfileResponse$contentType` doc (quoted above), which is also what
/// `GetProfile$accept` names as the formats this service serves.
const ANNOUNCED: &[(&str, &str, &str)] = &[
    ("JSON", JSON, JSON),
    ("Ion", ION, ION),
    ("JSON with a media-type parameter", "application/json; charset=utf-8", JSON),
    // A media type's type and subtype are case-insensitive, so this names Ion.
    ("Ion in upper case", "APPLICATION/X-AMZN-ION", ION),
    ("a text media type", "text/plain", DEFAULT),
    ("arbitrary bytes", "application/octet-stream", DEFAULT),
    ("another document format", "application/xml", DEFAULT),
    ("a near-miss subtype", "application/jsonl", DEFAULT),
    ("a string that is no media type", "profiling data", DEFAULT),
    ("no media type at all", "", DEFAULT),
];

/// Containers nested `levels` deep as a JSON text — deeper than the nesting this
/// operation reads a document at, so these bytes are no document it can see.
fn nested_containers(levels: usize) -> Vec<u8> {
    let mut bytes = vec![b'['; levels];
    bytes.extend(std::iter::repeat_n(b']', levels));
    bytes
}

/// The payloads that are a document in NEITHER representation this service reads, as (the
/// case, the bytes).
fn payloads() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("a truncated JSON text", b"{\"callgraph\":".to_vec()),
        ("every byte value", (0..=255u8).collect()),
        // Ion's TEXT encoding, which is not the binary encoding this operation reads a
        // document in, and is not JSON either (its field names are unquoted).
        (
            "a document in an encoding this operation does not read",
            br#"{ callgraph: { A: { } } }"#.to_vec(),
        ),
        ("no bytes at all", Vec::new()),
        ("a document nested past the read bound", nested_containers(200)),
    ]
}

/// What a request asks the operation to do with the profile, as (the case, the query
/// string, the `Accept` header).
const ASKS: &[(&str, &str, Option<&str>)] = &[
    ("nothing", "", None),
    ("a depth bound", "?maxDepth=2", None),
    ("JSON", "", Some(JSON)),
    ("Ion", "", Some(ION)),
    ("a representation this service does not serve", "", Some("text/csv")),
    ("a representation and a depth bound", "?maxDepth=3", Some(ION)),
];

/// A stored profile whose bytes are no document is served exactly as stored, and announced
/// under the media type its announcement names out of the two this service serves — the
/// default where it names neither — whatever the request asks for.
///
/// Such bytes are placed in the store rather than submitted: `PostAgentProfile` parses the
/// data it is handed and refuses data that is not an agent profile, so the cases about what
/// a READER announces for stored bytes it cannot read place them through the one shared
/// writer.
#[tokio::test]
async fn a_served_profile_is_announced_under_a_media_type_this_service_serves() {
    let harness = Harness::new();
    let payloads = payloads();

    for (media_index, (announced_case, announced, expected)) in ANNOUNCED.iter().enumerate() {
        for (payload_index, (payload_case, bytes)) in payloads.iter().enumerate() {
            let case = format!("{announced_case} holding {payload_case}");
            let name = format!("announced-{media_index}-{payload_index}");
            create_group(&harness, &name).await;
            place_profile(&harness, &name, announced, bytes.clone(), RECEIPT_SECONDS);

            for (ask_case, query, accept) in ASKS {
                let answer = harness.get_profile_accepting(&name, query, *accept).await;
                assert_eq!(
                    answer.status,
                    200,
                    "case {case:?} asked for {ask_case}: expected HTTP 200, got {} {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                let served = answer.content_type.as_deref();
                assert!(
                    served == Some(JSON) || served == Some(ION),
                    "case {case:?} asked for {ask_case}: expected one of the two media types \
                     this service serves, got {served:?}"
                );
                assert_eq!(
                    served,
                    Some(*expected),
                    "case {case:?} asked for {ask_case}: expected {expected}, got {served:?}"
                );
                assert_eq!(
                    &answer.body,
                    bytes,
                    "case {case:?} asked for {ask_case}: expected the stored bytes, got {:?}",
                    String::from_utf8_lossy(&answer.body)
                );
            }
        }
    }
}

/// The announcement a profile was uploaded under does not reach the response even when the
/// upload went through `PostAgentProfile`: a profile submitted under a media type this
/// service does not serve is still announced under the representation its bytes are a
/// document in.
#[tokio::test]
async fn an_uploaded_media_type_is_not_what_a_served_profile_is_announced_under() {
    let harness = Harness::new();
    // A profile `PostAgentProfile` accepts: a document whose root is a struct carrying the
    // `start` of the period it covers.
    let body = br#"{"start":1600000000000,"callgraph":{"A":{}}}"#.to_vec();

    for (index, (case, announced)) in [
        ("arbitrary bytes", "application/octet-stream"),
        ("a text media type", "text/plain"),
        ("JSON in upper case", "APPLICATION/JSON"),
        ("JSON with a media-type parameter", "application/json; charset=utf-8"),
    ]
    .into_iter()
    .enumerate()
    {
        let name = format!("uploaded-{index}");
        create_group(&harness, &name).await;
        let posted = harness
            .post_agent_profile(&name, announced, None, body.clone())
            .await;
        assert_eq!(
            posted.status,
            204,
            "case {case:?}: posting the profile must succeed, got {} {}",
            posted.status,
            String::from_utf8_lossy(&posted.body)
        );
        crate::profile_times::place_receipts(&harness, &name, &[RECEIPT_SECONDS]);

        let answer = harness.get_profile(&name, "").await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: expected HTTP 200, got {} {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.content_type.as_deref(),
            Some(JSON),
            "case {case:?}: expected the representation the bytes are a document in, got {:?}",
            answer.content_type
        );
        assert_eq!(
            answer.body,
            body,
            "case {case:?}: expected the stored bytes, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
    }
}
