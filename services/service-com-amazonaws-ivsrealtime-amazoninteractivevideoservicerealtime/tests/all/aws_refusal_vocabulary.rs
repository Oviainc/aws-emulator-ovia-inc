// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal this service answers, and in WHOSE words.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the wire error code and the JSON
//! body.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one request:
//!
//! 1. THE TAGGING SURFACE HAS ITS OWN VOCABULARY. The three operations bound to
//!    `/tags/{resourceArn}` answer an ARN that names no resource with HTTP 404
//!    `NotFoundException` and one fixed, resource-agnostic sentence, and a tag set past
//!    the modelled `Tags` maximum with HTTP 400 `BadRequestException` and one fixed
//!    sentence naming that maximum — whatever resource KIND the ARN names and however the
//!    resource came to be missing. Every resource-specific read of the very same ARN keeps
//!    the modelled `ResourceNotFoundException` and names the ARN, so a client can tell a
//!    tagging target from a resource.
//! 2. A DEFAULT THE SERVICE APPLIED IS NOT ECHOED. `CreateParticipantTokenRequest`'s
//!    `duration` and `capabilities` are optional members carrying a documented default, so
//!    the reported token carries each one exactly when the request did; what the default
//!    DID is visible in the token's `expirationTime` and in the capabilities the
//!    participant really holds.
//! 3. A DOCUMENTED-BUT-UNMODELLED RULE ABOUT ONE MEMBER IS REFUSED IN THE MEMBER
//!    -CONSTRAINT ENVELOPE: `Request member: <member> failed to satisfy the constraints:
//!    <requirement>`, naming the member whose own documentation carries the rule. The
//!    three such rules of this service are the 1 KB `attributes` budget — measured on the
//!    map's JSON encoding — the RTMP/insecure-ingest pairing, and the `video` resolution
//!    budget.
//! 4. A NOT-FOUND SAYS WHICH RESOURCE IS MISSING. An ARN names its own kind, so a refusal
//!    naming one needs nothing beside it; a bare identifier does not, so the refusal names
//!    the kind of resource and quotes the identifier.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::{DateTime, Format};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

use crate::attributes_measure::{
    json_punctuation, json_size, oversized_message, MAX_ATTRIBUTES_BYTES,
};

/// The region every request in this suite is signed for, and so the region every fixture
/// is created in: a resource is addressed through the region holding it.
const REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, and so the account every ARN
/// built by hand here carries too (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The code and the sentence the tagging surface answers for an ARN it holds no resource
/// for. Written out here so the expectation is stated independently of the service.
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The code the tagging surface answers for a request it refuses as malformed.
const TAGGING_BAD_REQUEST_CODE: &str = "BadRequestException";

/// The modelled error code every resource-specific read answers for a resource it does
/// not hold, and the modelled code for a refused request everywhere but the tagging
/// surface.
const RESOURCE_NOT_FOUND_CODE: &str = "ResourceNotFoundException";
const VALIDATION_CODE: &str = "ValidationException";

/// The modelled code for a request that contradicts the state the store already holds.
const CONFLICT_CODE: &str = "ConflictException";

/// The modelled `@length` maximum of `Tags` (`smithy_query(query="shape", shape="Tags")`):
/// the tags one resource carries.
const MAX_TAGS: usize = 50;

/// The documented default lifetime of a participant token, from
/// `smithy_query(query="doc", shape="CreateParticipantTokenRequest", member="duration")`:
/// "Default: 720 (12 hours)."
const DEFAULT_DURATION_MINUTES: i64 = 720;

/// The documented maximum total pixels of an encoder configuration's video, from
/// `smithy_query(query="doc", shape="Video", member="width")`: "the maximum total pixels
/// is 2073600 (1920x1080 or 1080x1920)".
const MAX_VIDEO_PIXELS: i64 = 2_073_600;

/// The capability that makes a participant a publisher, from
/// `smithy_query(query="shape", shape="ParticipantTokenCapability")`.
const PUBLISH_CAPABILITY: &str = "PUBLISH";
const SUBSCRIBE_CAPABILITY: &str = "SUBSCRIBE";

/// One answer from the emulator.
struct Reply {
    status: u16,
    code: String,
    message: String,
    body: Value,
}

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// An operation bound to `POST /<Operation>`, which is every operation of this service
    /// except the three tagging ones.
    async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.send(
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in the
    /// URI the way a client does: percent-encoded into one path segment.
    async fn tagging(&self, method: &str, arn: &str, query: &str, body: Option<Value>) -> Reply {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(&self, method: &str, path: &str, body: Option<bytes::Bytes>) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{REGION}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{REGION}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            code: string_member(&parsed, "__type"),
            message: string_member(&parsed, "message"),
            body: parsed,
        }
    }

    /// A stage, created the way a client creates one, reported by its ARN.
    async fn stage(&self, case: &str, body: &Value) -> String {
        let reply = self.call("CreateStage", body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateStage {body} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        string_member(&reply.body["stage"], "arn")
    }

    /// An encoder configuration, reported by its ARN.
    async fn encoder_configuration(&self, case: &str, body: &Value) -> String {
        let reply = self.call("CreateEncoderConfiguration", body).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateEncoderConfiguration {body} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        string_member(&reply.body["encoderConfiguration"], "arn")
    }

    /// The tags the tagging surface answers for `arn`.
    async fn tags_of(&self, case: &str, arn: &str) -> HashMap<String, String> {
        let reply = self.tagging("GET", arn, "", None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListTagsForResource on {arn} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        serde_json::from_value(reply.body["tags"].clone()).unwrap_or_default()
    }

    /// How many encoder configurations this account view holds, so a refused create can be
    /// shown to have left nothing behind.
    async fn encoder_configuration_count(&self, case: &str) -> usize {
        let reply = self.call("ListEncoderConfigurations", &json!({})).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListEncoderConfigurations must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        reply.body["encoderConfigurations"]
            .as_array()
            .map_or(0, Vec::len)
    }
}

/// A string member, read as the empty string when absent — what a client sees for a member
/// the wire omits.
fn string_member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment.
fn encode_path_label(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// An IVS ARN of `resource_type` naming the resource `id`.
fn arn(resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{REGION}:{ACCOUNT}:{resource_type}/{id}")
}

/// The sentence every resource-specific read of this service states for an identifier it
/// holds no resource for.
fn resource_not_found(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// The member-constraint envelope this service words a rejection of one member's VALUE
/// with: the member, then the requirement the value failed.
fn member_constraint(member: &str, requirement: &str) -> String {
    format!("Request member: {member} failed to satisfy the constraints: {requirement}")
}

/// The three methods the tagging surface binds, with the query and body that make each a
/// well-formed call: `GET` reads, `POST` writes one tag, `DELETE` removes one key.
fn every_tagging_method() -> [(&'static str, &'static str, &'static str, Option<Value>); 3] {
    [
        ("ListTagsForResource", "GET", "", None),
        (
            "TagResource",
            "POST",
            "",
            Some(json!({ "tags": { "team": "video" } })),
        ),
        ("UntagResource", "DELETE", "?tagKeys=team", None),
    ]
}

/// A tag map of `count` distinct entries, each key carrying `prefix`.
fn tag_map(prefix: &str, count: usize) -> HashMap<String, String> {
    (0..count)
        .map(|index| (format!("{prefix}{index}"), format!("value{index}")))
        .collect()
}

/// One row of the tag-bound table: the case, the tags the resource is created with, the
/// tags the write carries, and whether the call is admitted.
type TagBoundRow = (
    &'static str,
    HashMap<String, String>,
    HashMap<String, String>,
    bool,
);

/// One row of the reported-presence table: the case, the `duration` the request carries
/// and the `capabilities` it carries.
type TokenMemberRow = (&'static str, Option<i64>, Option<Vec<&'static str>>);

/// RULE 1, on the missing side: whatever KIND of resource an admitted ARN names and
/// however it came to be missing, every tagging operation answers the surface's own
/// `NotFoundException` with its one fixed sentence, while the resource's own reader answers
/// the modelled `ResourceNotFoundException` naming the ARN.
///
/// The rows span the resource kind (a stage, an encoder configuration, a storage
/// configuration, a public key) and HOW it is missing (never created, and created then
/// deleted) — the two facts the rule generalises over — each paired with the
/// resource-specific read that addresses the same ARN.
#[tokio::test]
async fn the_tagging_surface_and_the_resource_reads_answer_different_not_founds() {
    let realtime = RealTime::new();

    let deleted_stage = realtime
        .stage("a stage to delete", &json!({ "name": "gone" }))
        .await;
    let removal = realtime
        .call("DeleteStage", &json!({ "arn": deleted_stage }))
        .await;
    assert_eq!(
        removal.status, 200,
        "the fixture stage must really be deleted, got {} {}",
        removal.status, removal.body
    );

    let deleted_encoder = realtime
        .encoder_configuration(
            "an encoder configuration to delete",
            &json!({ "name": "gone" }),
        )
        .await;
    let removal = realtime
        .call("DeleteEncoderConfiguration", &json!({ "arn": deleted_encoder }))
        .await;
    assert_eq!(
        removal.status, 200,
        "the fixture encoder configuration must really be deleted, got {} {}",
        removal.status, removal.body
    );

    // (case, the ARN, the operation that reads that one resource)
    let rows: Vec<(&str, String, &str)> = vec![
        (
            "a stage that was never created",
            arn("stage", "ffffffffffff"),
            "GetStage",
        ),
        ("a stage that was deleted", deleted_stage, "GetStage"),
        (
            "an encoder configuration that was never created",
            arn("encoder-configuration", "0123456789ab"),
            "GetEncoderConfiguration",
        ),
        (
            "an encoder configuration that was deleted",
            deleted_encoder,
            "GetEncoderConfiguration",
        ),
        (
            "a storage configuration that was never created",
            arn("storage-configuration", "0123456789ab"),
            "GetStorageConfiguration",
        ),
        (
            "a public key that was never imported",
            arn("public-key", "0123456789ab"),
            "GetPublicKey",
        ),
    ];

    for (case, missing, reader) in rows {
        for (operation, method, query, body) in every_tagging_method() {
            let reply = realtime.tagging(method, &missing, query, body).await;
            assert_eq!(
                (reply.status, reply.code.as_str(), reply.message.as_str()),
                (404, TAGGING_NOT_FOUND_CODE, TAGGING_NOT_FOUND_MESSAGE),
                "case {case:?} ({operation}): the tagging surface must answer its own \
                 not-found, got {} {} {}",
                reply.status,
                reply.code,
                reply.body
            );
        }

        let reply = realtime.call(reader, &json!({ "arn": missing })).await;
        assert_eq!(
            (reply.status, reply.code.as_str(), reply.message.as_str()),
            (
                404,
                RESOURCE_NOT_FOUND_CODE,
                resource_not_found(&missing).as_str()
            ),
            "case {case:?} ({reader}): the resource read must answer the modelled \
             not-found naming the ARN, got {} {} {}",
            reply.status,
            reply.code,
            reply.body
        );
    }
}

/// RULE 1, on the bound: the modelled `Tags` maximum bounds the tag set a call would
/// LEAVE on the resource, and a call past it is the tagging surface's own
/// `BadRequestException` stating that maximum — leaving every tag it already carried.
///
/// The rows differ along what the rule generalises over: how many tags the resource
/// already carries, how many the request adds, and whether the request's keys are new or
/// replace keys the resource already has (a replacement adds nothing, so it is admitted at
/// the bound). Each published side of the bound is exercised: exactly the maximum, and one
/// past it.
#[tokio::test]
async fn a_resultant_tag_set_past_the_modelled_maximum_is_a_bad_request() {
    let realtime = RealTime::new();
    let expected_message =
        format!("The resultant tag set must not have more than {MAX_TAGS} user tags.");

    // (case, tags the stage is created with, tags the write carries, admitted?)
    let rows: Vec<TagBoundRow> = vec![
        (
            "an untagged resource written to exactly the maximum",
            HashMap::new(),
            tag_map("k", MAX_TAGS),
            true,
        ),
        (
            "an untagged resource written one past the maximum",
            HashMap::new(),
            tag_map("k", MAX_TAGS + 1),
            false,
        ),
        (
            "a resource at the maximum, one new key",
            tag_map("k", MAX_TAGS),
            HashMap::from([("one-too-many".to_string(), "x".to_string())]),
            false,
        ),
        (
            "a resource at the maximum, every key replaced",
            tag_map("k", MAX_TAGS),
            tag_map("k", MAX_TAGS),
            true,
        ),
        (
            "half the maximum held, the other half added",
            tag_map("k", MAX_TAGS / 2),
            tag_map("n", MAX_TAGS / 2),
            true,
        ),
        (
            "half the maximum held, one more than the other half added",
            tag_map("k", MAX_TAGS / 2),
            tag_map("n", MAX_TAGS / 2 + 1),
            false,
        ),
        (
            "a batch of new and replacing keys landing on the maximum",
            tag_map("k", MAX_TAGS - 1),
            {
                let mut written = tag_map("k", 10);
                written.insert("fresh".to_string(), "x".to_string());
                written
            },
            true,
        ),
    ];

    for (case, held, written, admitted) in rows {
        let stage_arn = realtime
            .stage(case, &json!({ "name": "bounded", "tags": held }))
            .await;
        let reply = realtime
            .tagging("POST", &stage_arn, "", Some(json!({ "tags": written })))
            .await;

        if admitted {
            assert_eq!(
                reply.status, 204,
                "case {case:?}: a call leaving the resource within the maximum must be \
                 admitted, got {} {}",
                reply.status, reply.body
            );
            let mut expected = held.clone();
            expected.extend(written.clone());
            assert_eq!(
                realtime.tags_of(case, &stage_arn).await,
                expected,
                "case {case:?}: the admitted write must be what the surface reports"
            );
        } else {
            assert_eq!(
                (reply.status, reply.code.as_str(), reply.message.as_str()),
                (400, TAGGING_BAD_REQUEST_CODE, expected_message.as_str()),
                "case {case:?}: a call leaving the resource past the maximum must be the \
                 tagging surface's bad request, got {} {} {}",
                reply.status,
                reply.code,
                reply.body
            );
            assert_eq!(
                realtime.tags_of(case, &stage_arn).await,
                held,
                "case {case:?}: the refused write must leave the tag set exactly as it was"
            );
        }
    }
}

/// RULE 2: the reported token carries `duration` and `capabilities` exactly when the
/// request carried them, on BOTH operations that issue a token — and the documented
/// default is still what an omitted member is applied as, which `expirationTime` shows.
///
/// The rows differ along what the rule generalises over: each member present or absent
/// independently, both published bounds of `duration`, an explicitly empty capability list
/// and each non-empty subset.
#[tokio::test]
async fn a_token_reports_only_the_members_the_request_carried() {
    let realtime = RealTime::new();
    let stage_arn = realtime.stage("the token's stage", &json!({})).await;

    // (case, the duration the request carries, the capabilities it carries)
    let rows: Vec<TokenMemberRow> = vec![
        ("neither member", None, None),
        ("the modelled minimum duration alone", Some(1), None),
        ("the modelled maximum duration alone", Some(20160), None),
        ("a middling duration alone", Some(90), None),
        (
            "capabilities alone, one of them",
            None,
            Some(vec![PUBLISH_CAPABILITY]),
        ),
        (
            "capabilities alone, the other",
            None,
            Some(vec![SUBSCRIBE_CAPABILITY]),
        ),
        ("capabilities alone, explicitly empty", None, Some(vec![])),
        (
            "both members",
            Some(45),
            Some(vec![SUBSCRIBE_CAPABILITY, PUBLISH_CAPABILITY]),
        ),
    ];

    for (case, duration, capabilities) in rows {
        let mut request = json!({ "stageArn": stage_arn });
        if let Some(duration) = duration {
            request["duration"] = json!(duration);
        }
        if let Some(capabilities) = capabilities.as_ref() {
            request["capabilities"] = json!(capabilities);
        }

        let reply = realtime.call("CreateParticipantToken", &request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateParticipantToken {request} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        assert_reported_presence(
            case,
            "CreateParticipantToken",
            &reply.body["participantToken"],
            duration,
            capabilities.as_ref(),
        );

        // The same rule through `CreateStage`, which issues a token per configuration.
        let mut configuration = json!({ "userId": "alice" });
        if let Some(duration) = duration {
            configuration["duration"] = json!(duration);
        }
        if let Some(capabilities) = capabilities.as_ref() {
            configuration["capabilities"] = json!(capabilities);
        }
        let reply = realtime
            .call(
                "CreateStage",
                &json!({ "participantTokenConfigurations": [configuration] }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateStage must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        assert_reported_presence(
            case,
            "CreateStage",
            &reply.body["participantTokens"][0],
            duration,
            capabilities.as_ref(),
        );
    }
}

/// One reported token against rule 2: each member is there exactly when the request
/// carried it, and the token expires the EFFECTIVE duration out — the documented default
/// where the request named none.
fn assert_reported_presence(
    case: &str,
    operation: &str,
    token: &Value,
    duration: Option<i64>,
    capabilities: Option<&Vec<&str>>,
) {
    assert_eq!(
        token.get("duration").and_then(Value::as_i64),
        duration,
        "case {case:?} ({operation}): expected duration {duration:?}, got {token}"
    );
    // A capability list the caller asked for is reported as asked; an empty one holds no
    // entry, so the member is omitted the way an empty `attributes` map is.
    let expected: Option<Vec<&str>> = capabilities
        .filter(|capabilities| !capabilities.is_empty())
        .map(|capabilities| capabilities.to_vec());
    let reported: Option<Vec<&str>> = token.get("capabilities").map(|values| {
        values
            .as_array()
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default()
    });
    assert_eq!(
        reported, expected,
        "case {case:?} ({operation}): expected capabilities {expected:?}, got {reported:?}"
    );

    let expiry = string_member(token, "expirationTime");
    let effective = duration.unwrap_or(DEFAULT_DURATION_MINUTES);
    let seconds = seconds_until(&expiry);
    let expected_seconds = effective * 60;
    assert!(
        (seconds - expected_seconds).abs() <= 120,
        "case {case:?} ({operation}): expected a token expiring {effective} minutes out \
         ({expected_seconds}s), got {expiry:?} ({seconds}s away)"
    );
}

/// How far ahead of now an ISO-8601 instant the wire carried is, in seconds.
///
/// Only a STRING is accepted: the model gives `ParticipantTokenExpirationTime`
/// `@timestampFormat("date-time")`, so a bare epoch number fails the parse and the case.
fn seconds_until(rendered: &str) -> i64 {
    let instant = DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|error| panic!("{rendered:?} is not an ISO-8601 date-time: {error}"))
        .secs();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() as i64);
    instant - now
}

/// RULE 2, the other half: the default the service applied is REAL, not merely omitted —
/// a token issued with no `capabilities` publishes, which is what the documented
/// `PUBLISH, SUBSCRIBE` default grants, while one issued with `SUBSCRIBE` alone does not.
///
/// `StartParticipantReplication` is the reader: it replicates "a publishing participant",
/// so it admits the publisher and refuses the subscriber.
#[tokio::test]
async fn the_capability_default_is_applied_even_though_it_is_not_reported() {
    let realtime = RealTime::new();
    let source = realtime.stage("the source stage", &json!({})).await;
    let destination = realtime.stage("the destination stage", &json!({})).await;

    // (case, the capabilities the token is issued with, replicable?)
    let rows: Vec<(&str, Option<Vec<&str>>, bool)> = vec![
        ("the documented default, asked for by nobody", None, true),
        ("publish asked for explicitly", Some(vec![PUBLISH_CAPABILITY]), true),
        (
            "both asked for explicitly",
            Some(vec![PUBLISH_CAPABILITY, SUBSCRIBE_CAPABILITY]),
            true,
        ),
        (
            "subscribe alone",
            Some(vec![SUBSCRIBE_CAPABILITY]),
            false,
        ),
        ("an explicitly empty set", Some(vec![]), false),
    ];

    for (case, capabilities, replicable) in rows {
        let mut request = json!({ "stageArn": source });
        if let Some(capabilities) = capabilities.as_ref() {
            request["capabilities"] = json!(capabilities);
        }
        let reply = realtime.call("CreateParticipantToken", &request).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: CreateParticipantToken must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        let participant_id = string_member(&reply.body["participantToken"], "participantId");

        let reply = realtime
            .call(
                "StartParticipantReplication",
                &json!({
                    "sourceStageArn": source,
                    "destinationStageArn": destination,
                    "participantId": participant_id,
                }),
            )
            .await;
        if replicable {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a participant holding {PUBLISH_CAPABILITY} must be \
                 replicable, got {} {}",
                reply.status, reply.body
            );
            let stop = realtime
                .call(
                    "StopParticipantReplication",
                    &json!({
                        "sourceStageArn": source,
                        "destinationStageArn": destination,
                        "participantId": participant_id,
                    }),
                )
                .await;
            assert_eq!(
                stop.status, 200,
                "case {case:?}: the replication must be stoppable again, got {} {}",
                stop.status, stop.body
            );
        } else {
            // A participant that publishes nothing has nothing to replicate YET, which is
            // the modelled conflict about what the source stage is doing rather than a
            // malformed request.
            assert_eq!(
                (reply.status, reply.code.as_str()),
                (409, CONFLICT_CODE),
                "case {case:?}: a participant holding no {PUBLISH_CAPABILITY} must be \
                 refused, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.message,
                format!("{participant_id} is not publishing"),
                "case {case:?}: the refusal must state that the participant is not \
                 publishing, got {}",
                reply.body
            );
        }
    }
}

/// RULE 3 on the 1 KB `attributes` budget: the measure is the map's JSON encoding, and a
/// map past it is refused in the member-constraint envelope naming `attributes` and the
/// size that was measured.
///
/// The rows differ along what the rule generalises over: the number of entries (one, a
/// few, many), where the bytes sit (one long value against many short pairs), and where
/// the encoded size falls against the bound — including a map whose keys and values alone
/// are well inside the bound but whose punctuation carries it past, which is exactly what
/// the measure decides.
#[tokio::test]
async fn the_attributes_budget_is_measured_on_the_json_encoding() {
    let realtime = RealTime::new();
    let stage_arn = realtime.stage("the token's stage", &json!({})).await;

    // Rows built from the number of entries and the bytes the keys and values carry, so
    // each row's encoded size is a fact about the map rather than a number copied in.
    let rows: Vec<(&str, BTreeMap<String, String>)> = vec![
        (
            "one entry, far inside the bound",
            BTreeMap::from([("seat".to_string(), "12b".to_string())]),
        ),
        (
            "one entry whose value fills the bound exactly",
            BTreeMap::from([(
                "seat".to_string(),
                "v".repeat(MAX_ATTRIBUTES_BYTES - json_punctuation(1) - "seat".len()),
            )]),
        ),
        (
            "one entry one byte past the bound",
            BTreeMap::from([(
                "seat".to_string(),
                "v".repeat(MAX_ATTRIBUTES_BYTES - json_punctuation(1) - "seat".len() + 1),
            )]),
        ),
        (
            "one entry far past the bound",
            BTreeMap::from([("blob".to_string(), "x".repeat(2000))]),
        ),
        (
            "many short pairs inside the bound",
            (0..40)
                .map(|index| (format!("k{index:03}"), format!("v{index:03}")))
                .collect(),
        ),
        (
            "many short pairs whose keys and values fit but whose punctuation does not",
            (0..120)
                .map(|index| (format!("k{index:03}"), format!("v{index:03}")))
                .collect(),
        ),
    ];

    for (case, map) in rows {
        let size = json_size(&map);
        let request = json!({ "stageArn": stage_arn, "attributes": map });
        let reply = realtime.call("CreateParticipantToken", &request).await;

        if size <= MAX_ATTRIBUTES_BYTES {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: a map of {size} JSON bytes must be admitted, got {} {}",
                reply.status, reply.body
            );
            let reported: BTreeMap<String, String> =
                serde_json::from_value(reply.body["participantToken"]["attributes"].clone())
                    .unwrap_or_default();
            assert_eq!(
                reported, map,
                "case {case:?}: the admitted map must be reported unchanged, got {reported:?}"
            );
        } else {
            let expected = oversized_message("attributes", size);
            assert_eq!(
                (reply.status, reply.code.as_str(), reply.message.as_str()),
                (400, VALIDATION_CODE, expected.as_str()),
                "case {case:?}: a map of {size} JSON bytes must be refused naming that \
                 size, got {} {} {}",
                reply.status,
                reply.code,
                reply.body
            );
        }
    }

    // The keys-and-values sum is NOT the measure: a map whose entries total under the
    // bound is still refused when its encoding is over it, and the refusal names the
    // encoded size.
    let many: BTreeMap<String, String> = (0..120)
        .map(|index| (format!("k{index:03}"), format!("v{index:03}")))
        .collect();
    let raw: usize = many.iter().map(|(key, value)| key.len() + value.len()).sum();
    assert!(
        raw <= MAX_ATTRIBUTES_BYTES && json_size(&many) > MAX_ATTRIBUTES_BYTES,
        "the fixture must distinguish the two measures: {raw} raw against {} encoded",
        json_size(&many)
    );
}

/// RULE 3 on the RTMP pairing: cleartext ingest needs `insecureIngest`, and a request
/// without it is refused in the member-constraint envelope naming `ingestProtocol` — the
/// member whose own documentation carries the rule — while the same request carrying the
/// flag is created and read back.
///
/// The rows span both values of `IngestProtocol` against `insecureIngest` absent, false and
/// true, which is the whole input class the rule decides.
#[tokio::test]
async fn cleartext_ingest_is_refused_naming_the_documented_member() {
    let realtime = RealTime::new();
    let expected = member_constraint(
        "ingestProtocol",
        "insecure ingest is required for Ingest RTMP protocol",
    );

    // (case, the protocol, the flag the request carries, admitted?)
    let rows: Vec<(&str, &str, Option<bool>, bool)> = vec![
        ("cleartext with no flag", "RTMP", None, false),
        ("cleartext with the flag off", "RTMP", Some(false), false),
        ("cleartext with the flag on", "RTMP", Some(true), true),
        ("secure with no flag", "RTMPS", None, true),
        ("secure with the flag off", "RTMPS", Some(false), true),
    ];

    for (case, protocol, insecure, admitted) in rows {
        let held = realtime
            .call("ListIngestConfigurations", &json!({}))
            .await
            .body["ingestConfigurations"]
            .as_array()
            .map_or(0, Vec::len);
        let mut request = json!({ "name": "feed", "ingestProtocol": protocol });
        if let Some(insecure) = insecure {
            request["insecureIngest"] = json!(insecure);
        }
        let reply = realtime.call("CreateIngestConfiguration", &request).await;

        if admitted {
            assert_eq!(
                reply.status, 200,
                "case {case:?}: {request} must answer HTTP 200, got {} {}",
                reply.status, reply.body
            );
            let created = string_member(&reply.body["ingestConfiguration"], "arn");
            let read = realtime
                .call("GetIngestConfiguration", &json!({ "arn": created }))
                .await;
            assert_eq!(
                string_member(&read.body["ingestConfiguration"], "ingestProtocol"),
                protocol,
                "case {case:?}: the created configuration must read back with its \
                 protocol, got {}",
                read.body
            );
        } else {
            assert_eq!(
                (reply.status, reply.code.as_str(), reply.message.as_str()),
                (400, VALIDATION_CODE, expected.as_str()),
                "case {case:?}: expected the stated refusal, got {} {} {}",
                reply.status,
                reply.code,
                reply.body
            );
            assert_eq!(
                realtime.call("ListIngestConfigurations", &json!({})).await.body
                    ["ingestConfigurations"]
                    .as_array()
                    .map_or(0, Vec::len),
                held,
                "case {case:?}: a refused create must leave the store as it was"
            );
        }
    }
}

/// RULE 3 on the video resolution budget: a `video` whose members are each in range but
/// whose product is past the documented maximum is refused in the member-constraint
/// envelope naming `video`, carrying the named detail code AWS classifies this one limit
/// with — and nothing is created.
///
/// The rows span the resolution class: each published shape of the maximum (1920x1080 and
/// its transpose), one row either side of the product bound, and the odd-dimension rule of
/// the same member, whose refusal names the same member in the same envelope.
#[tokio::test]
async fn the_video_resolution_budget_carries_the_named_detail_code() {
    let realtime = RealTime::new();
    let over_budget = member_constraint(
        "video",
        "Code: VideoResolutionLimitError Message: video resolution exceeded max limit",
    );

    // (case, width, height, the refusal expected of it — `None` where it is admitted)
    let rows: Vec<(&str, i64, i64, Option<String>)> = vec![
        ("the modelled default resolution", 1280, 720, None),
        ("the published maximum", 1920, 1080, None),
        ("the published maximum, transposed", 1080, 1920, None),
        (
            "two pixels past the maximum",
            1920,
            1082,
            Some(over_budget.clone()),
        ),
        ("a square at the dimension maximum", 1920, 1920, Some(over_budget.clone())),
        (
            "an odd width, within the product bound",
            1919,
            1080,
            Some(member_constraint(
                "video",
                "width is required to be an even number, found 1919",
            )),
        ),
        (
            "an odd height, within the product bound",
            1280,
            719,
            Some(member_constraint(
                "video",
                "height is required to be an even number, found 719",
            )),
        ),
    ];

    for (case, width, height, refusal) in rows {
        let held = realtime.encoder_configuration_count(case).await;
        let request = json!({
            "name": "video",
            "video": { "width": width, "height": height },
        });
        let reply = realtime.call("CreateEncoderConfiguration", &request).await;

        match refusal {
            None => {
                assert!(
                    width * height <= MAX_VIDEO_PIXELS,
                    "case {case:?}: this row must be within the documented maximum"
                );
                assert_eq!(
                    reply.status, 200,
                    "case {case:?}: {request} must answer HTTP 200, got {} {}",
                    reply.status, reply.body
                );
                assert_eq!(
                    realtime.encoder_configuration_count(case).await,
                    held + 1,
                    "case {case:?}: the admitted create must leave one more configuration"
                );
            }
            Some(expected) => {
                assert_eq!(
                    (reply.status, reply.code.as_str(), reply.message.as_str()),
                    (400, VALIDATION_CODE, expected.as_str()),
                    "case {case:?}: expected the stated refusal, got {} {} {}",
                    reply.status,
                    reply.code,
                    reply.body
                );
                assert_eq!(
                    realtime.encoder_configuration_count(case).await,
                    held,
                    "case {case:?}: a refused create must leave no configuration behind"
                );
            }
        }
    }
}

/// RULE 4: a not-found identifies the missing resource the way the request addressed it —
/// an ARN alone, which names its own kind, or the kind of resource with the bare
/// identifier quoted.
///
/// The rows span the operations that address a resource by a bare identifier (a stage
/// session's `sessionId`, a participant's `participantId`) and, beside them, the same
/// operations refusing an ARN, so the two forms are pinned against each other rather than
/// one alone.
#[tokio::test]
async fn a_not_found_names_the_kind_of_a_bare_identifier() {
    let realtime = RealTime::new();
    let stage_arn = realtime.stage("the stage", &json!({ "name": "sessions" })).await;
    let absent_stage = arn("stage", "ffffffffffff");

    // A `sessionId` the model admits — `@length(16,16)` and `^st-[a-zA-Z0-9]+$` — that
    // this store holds no session for.
    let absent_session = "st-0000000000000";
    let absent_participant = "ghost-1";

    // (case, operation, request, the identifier the refusal must name)
    let rows: Vec<(&str, &str, Value, String)> = vec![
        (
            "a session of a stage that has none",
            "GetStageSession",
            json!({ "stageArn": stage_arn, "sessionId": absent_session }),
            format!("stage session \"{absent_session}\""),
        ),
        (
            "a session read through the participant reader",
            "GetParticipant",
            json!({
                "stageArn": stage_arn,
                "sessionId": absent_session,
                "participantId": absent_participant,
            }),
            format!("stage session \"{absent_session}\""),
        ),
        (
            // Neither `DisconnectParticipant` nor `StartParticipantReplication` is among
            // these rows: a participant id its stage holds nothing for names no connection
            // to end, so the disconnect is the idempotent empty success (see the
            // `disconnect_participant` suite), and the replication start answers the
            // modelled conflict about what the source stage is publishing (see the
            // `participant_replication` suite) rather than a not-found.
            "the stage of a session read, addressed by ARN",
            "GetStageSession",
            json!({ "stageArn": absent_stage, "sessionId": absent_session }),
            absent_stage.clone(),
        ),
        (
            "the stage of a disconnect, addressed by ARN",
            "DisconnectParticipant",
            json!({ "stageArn": absent_stage, "participantId": absent_participant }),
            absent_stage.clone(),
        ),
    ];

    for (case, operation, request, identifier) in rows {
        let reply = realtime.call(operation, &request).await;
        assert_eq!(
            (reply.status, reply.code.as_str(), reply.message.as_str()),
            (
                404,
                RESOURCE_NOT_FOUND_CODE,
                resource_not_found(&identifier).as_str()
            ),
            "case {case:?} ({operation}): the refusal must name the missing resource, \
             got {} {} {}",
            reply.status,
            reply.code,
            reply.body
        );
    }
}
