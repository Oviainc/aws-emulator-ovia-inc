// SPDX-License-Identifier: CC0-1.0

//! The tagging surface over the wire: `TagResource`, `ListTagsForResource` and
//! `UntagResource`, all bound to `/tags/{resourceArn}`.
//!
//! Every test drives the service through the router's own `dispatch`, so each assertion
//! is about what an SDK client sees: the HTTP status, the error code and the JSON body.
//!
//! The rules under test, each exercised over a SET of inputs rather than on one ARN:
//!
//! 1. ADMISSION. A `resourceArn` label is admitted when it is an ARN of the form the
//!    model's own ARN shapes pin — `arn:aws:ivs:<region>:<account>:<type>/<id>`, whose
//!    `<type>` is one or more of the characters the modelled `ResourceArn` pattern names
//!    — and refused as a `ValidationException` (HTTP 400) naming the operation, the
//!    member and the violated constraint otherwise. The modelled pattern writes the
//!    resource-type element as `[a-z-]`, matching exactly ONE character, which no IVS
//!    resource type satisfies; so admission cannot turn on the type's LENGTH, and every
//!    resource type this model declares an ARN shape for is admitted alike.
//! 2. EXISTENCE. An admitted ARN that names no resource this store holds is the modelled
//!    `ResourceNotFoundException` (HTTP 404), not a validation failure.
//! 3. ROUND TRIP. `TagResource` is additive — a key the resource carries takes the new
//!    value, every other key is kept — and what it writes is what `ListTagsForResource`
//!    and the resource's own `GetStage` read back. `UntagResource` removes exactly the
//!    named keys and is idempotent. Both answer the recorded 204 with no body.
//! 4. The modelled `Tags` `@length` maximum bounds the RESOURCE's tag set, so a call is
//!    refused when the set it would LEAVE is over the maximum, even though each request
//!    is within it.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service and the region it
/// is addressed to, so every call carries one. The signature itself is never checked
/// here: request authentication is off unless a router turns it on.
const AUTHORIZATION_REGION: &str = "us-east-1";

/// The account id every ARN this emulator mints carries, and so the account every ARN
/// built by hand here carries too (`engine::mock_account_id`).
const ACCOUNT: &str = "000000000000";

/// The `@pattern` the tagging surface enforces: the modelled `ResourceArn` pattern with
/// its resource-type element read as one-or-more of its own character class. Quoted here
/// so the expected refusal text is written out independently of the service.
const RESOURCE_ARN_PATTERN: &str = "^arn:aws:ivs:[a-z0-9-]+:[0-9]+:[a-z-]+/[a-zA-Z0-9-]+$";

/// The modelled `@length` maximum of `ResourceArn`
/// (`smithy_query(query="shape", shape="ResourceArn")`).
const RESOURCE_ARN_MAX_LENGTH: usize = 128;

/// The modelled `@length` maximum of `Tags`
/// (`smithy_query(query="shape", shape="Tags")`): the tags one resource carries.
const MAX_TAGS: usize = 50;

/// What a tagging WRITE answers: `TagResource` and `UntagResource` leave their success
/// code defaulted in the model (`codeDefaulted: true`), and the recorded AWS responses
/// answer 204 with no body, stating its length.
const NO_CONTENT_STATUS: u16 = 204;
const NO_CONTENT_LENGTH: &str = "0";

/// Every resource type this model declares an ARN shape for — the resource types the
/// tagging surface can address. Source: `smithy_query(query="shape",
/// shape="CompositionArn,EncoderConfigurationArn,IngestConfigurationArn,PublicKeyArn,
/// StageArn,StorageConfigurationArn")`, whose patterns pin the resource segment.
/// `stage` is the one a `CreateStage` can put in this store today; each of the others is
/// the subject of its own create-operation gap.
const OWN_RESOURCE_TYPES: [&str; 6] = [
    "composition",
    "encoder-configuration",
    "ingest-configuration",
    "public-key",
    "stage",
    "storage-configuration",
];

/// One IVS RealTime emulator over one shared engine, reached the way the router reaches
/// it.
struct RealTime {
    router: MultiServiceRouter,
}

/// One answer from the emulator: the status, the wire error code (the restJson1 `__type`
/// discriminator), the body's `message` member, the stated `content-length`, whether the
/// body was empty, and the parsed body.
struct Reply {
    status: u16,
    error_code: String,
    message: String,
    content_length: Option<String>,
    body_empty: bool,
    body: Value,
}

impl RealTime {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    /// An operation bound to `POST /<Operation>`, which is every operation of this
    /// service except the three tagging ones.
    async fn call(&self, operation: &str, body: &Value) -> Reply {
        self.send(
            AUTHORIZATION_REGION,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in
    /// the URI the way a client does: percent-encoded into one path segment.
    async fn tagging(&self, method: &str, arn: &str, query: &str, body: Option<Value>) -> Reply {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            AUTHORIZATION_REGION,
            method,
            &path,
            body.map(|body| bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    async fn send(
        &self,
        region: &str,
        method: &str,
        path: &str,
        body: Option<bytes::Bytes>,
    ) -> Reply {
        let authorization = format!(
            "AWS4-HMAC-SHA256 Credential=AKIDTESTIVS/20260101/{region}/ivs/aws4_request, \
             SignedHeaders=host;x-amz-date, Signature=00"
        );
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://ivsrealtime.{region}.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header("authorization", authorization)
            .body(body.unwrap_or_default())
            .expect("the test request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let content_length = response
            .headers()
            .get("content-length")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let body_empty = response.body().is_empty();
        let parsed: Value = if body_empty {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code: member(&parsed, "__type"),
            message: member(&parsed, "message"),
            content_length,
            body_empty,
            body: parsed,
        }
    }

    /// Create a stage in `region` and return the ARN the service minted for it.
    async fn create_stage_in(&self, region: &str, body: &Value) -> String {
        let reply = self
            .send(
                region,
                "POST",
                "/CreateStage",
                Some(bytes::Bytes::from(body.to_string())),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "CreateStage {body} in {region}: expected HTTP 200, got {} {}",
            reply.status, reply.body
        );
        member(&reply.body["stage"], "arn")
    }

    /// The tags the tagging surface answers for `arn`, as a map.
    async fn tags_of(&self, case: &str, arn: &str) -> HashMap<String, String> {
        let reply = self.tagging("GET", arn, "", None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListTagsForResource on {arn} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        serde_json::from_value(reply.body["tags"].clone()).unwrap_or_default()
    }

    /// The tags `GetStage` reports for `arn` — the same state, read through the resource's
    /// own reader rather than the tagging surface.
    async fn tags_on_stage(&self, case: &str, arn: &str) -> HashMap<String, String> {
        let reply = self.call("GetStage", &json!({ "arn": arn })).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: GetStage on {arn} must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        serde_json::from_value(reply.body["stage"]["tags"].clone()).unwrap_or_default()
    }
}

/// Assert that `reply` is the empty-bodied success the recordings show for a tagging
/// write.
fn assert_no_content(case: &str, operation: &str, reply: &Reply) {
    assert_eq!(
        (
            reply.status,
            reply.body_empty,
            reply.content_length.as_deref()
        ),
        (NO_CONTENT_STATUS, true, Some(NO_CONTENT_LENGTH)),
        "case {case:?}: {operation} must answer {NO_CONTENT_STATUS} with no body, stating \
         content-length {NO_CONTENT_LENGTH}; got {} body {} content-length {:?}",
        reply.status,
        reply.body,
        reply.content_length
    );
}

/// A string member, read as the empty string when absent — which is what a client sees
/// for a member the wire omits.
fn member(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Percent-encode one `@httpLabel` value the way an SDK encodes a non-greedy label: an
/// ARN's `:` and `/` are escaped, so the whole ARN is ONE path segment. Every character
/// an IVS ARN can hold is ASCII, so one byte per character.
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

/// An IVS ARN of `resource_type` in `region`, naming the resource `id`.
fn arn(region: &str, resource_type: &str, id: &str) -> String {
    format!("arn:aws:ivs:{region}:{ACCOUNT}:{resource_type}/{id}")
}

/// The not-found text this service states for an ARN it holds no resource for.
fn not_found(identifier: &str) -> String {
    format!("Resource: {identifier} not found")
}

/// A tag map of `count` distinct entries.
fn tag_map(count: usize) -> HashMap<String, String> {
    (0..count)
        .map(|index| (format!("key{index}"), format!("value{index}")))
        .collect()
}

/// The three methods the tagging surface binds, with a body and query that make each a
/// well-formed call: `GET` reads, `POST` writes one tag, `DELETE` removes one key.
fn every_tagging_method() -> [(&'static str, &'static str, Option<Value>); 3] {
    [
        ("GET", "", None),
        ("POST", "", Some(json!({ "tags": { "team": "video" } }))),
        ("DELETE", "?tagKeys=team", None),
    ]
}

/// One row of the round-trip table: the case name, the region the stage is created in,
/// the tags the create persists, and the write under test.
type RoundTripRow = (
    &'static str,
    &'static str,
    HashMap<String, String>,
    HashMap<String, String>,
);

/// Rules 1, 2 and 3 together on the accepting side: a stage ARN is ADMITTED (never a
/// 400), its tags round-trip through the surface, and both writes answer the recorded 204
/// with no body.
///
/// The rows differ along what the rule generalises over: the region element of the ARN
/// (so the pattern's region class is exercised beyond one value), whether the stage was
/// created carrying tags, and the SIZE of the write — one entry, several, and the modelled
/// maximum.
#[tokio::test]
async fn tags_round_trip_through_the_tagging_surface_for_any_admitted_stage() {
    let realtime = RealTime::new();

    // (case, region, tags at creation, the write under test)
    let rows: Vec<RoundTripRow> = vec![
        (
            "untagged at creation, one tag written",
            "us-east-1",
            HashMap::new(),
            HashMap::from([("team".to_string(), "video".to_string())]),
        ),
        (
            "tagged at creation, several tags written",
            "eu-west-1",
            HashMap::from([("owner".to_string(), "behavioral".to_string())]),
            HashMap::from([
                ("env".to_string(), "sim2real".to_string()),
                ("owner".to_string(), "replaced".to_string()),
                ("cost-centre".to_string(), "42".to_string()),
            ]),
        ),
        (
            "tagged at creation, the modelled maximum written",
            "ap-southeast-2",
            HashMap::from([("owner".to_string(), "behavioral".to_string())]),
            tag_map(MAX_TAGS - 1),
        ),
    ];

    for (case, region, at_creation, written) in rows {
        let stage_arn = realtime
            .create_stage_in(region, &json!({ "name": "tagged", "tags": at_creation }))
            .await;
        assert!(
            stage_arn.contains(&format!(":{region}:")),
            "case {case:?}: the stage must be minted in {region}, got {stage_arn:?}"
        );
        assert_eq!(
            realtime.tags_of(case, &stage_arn).await,
            at_creation,
            "case {case:?}: the surface must answer the tags the create persisted"
        );

        // The write: a key the stage already carries takes the new value, and every other
        // tag it carries is kept.
        let reply = realtime
            .tagging("POST", &stage_arn, "", Some(json!({ "tags": written })))
            .await;
        assert_no_content(case, "TagResource", &reply);

        let mut expected = at_creation.clone();
        expected.extend(written.clone());
        assert_eq!(
            realtime.tags_of(case, &stage_arn).await,
            expected,
            "case {case:?}: tagging is additive and replaces only the keys it carries"
        );
        assert_eq!(
            realtime.tags_on_stage(case, &stage_arn).await,
            expected,
            "case {case:?}: GetStage must report the same tags the surface wrote"
        );

        // The removal, and the same removal again: `UntagResource` is modelled
        // `@idempotent`, so repeating it leaves the same tag set. The keys named are the
        // ones just written plus one the stage does not carry.
        let mut keys: Vec<String> = written.keys().cloned().collect();
        keys.sort();
        keys.push("a-key-nothing-carries".to_string());
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={key}"))
            .collect::<Vec<String>>()
            .join("&");
        for round in 0..2 {
            let reply = realtime
                .tagging("DELETE", &stage_arn, &format!("?{query}"), None)
                .await;
            assert_no_content(
                &format!("{case} round {round}"),
                "UntagResource",
                &reply,
            );
            let mut remaining = at_creation.clone();
            for key in written.keys() {
                remaining.remove(key);
            }
            assert_eq!(
                realtime.tags_of(case, &stage_arn).await,
                remaining,
                "case {case:?} round {round}: exactly the named keys are gone"
            );
        }
    }
}

/// Rule 1 and rule 2 on the ARNs that name nothing: every resource type this model
/// declares an ARN shape for is ADMITTED — a multi-character type is not a validation
/// failure — and an admitted ARN naming no stored resource is the modelled 404, on all
/// three operations.
///
/// The rows are every resource type in `OWN_RESOURCE_TYPES` plus a stage ARN this store
/// never minted and one whose stage was deleted, so the not-found covers a resource that
/// never existed and one that stopped existing.
#[tokio::test]
async fn an_admitted_arn_naming_no_resource_is_a_not_found_on_every_tagging_operation() {
    let realtime = RealTime::new();

    let deleted = realtime
        .create_stage_in("us-east-1", &json!({ "name": "gone" }))
        .await;
    let removal = realtime.call("DeleteStage", &json!({ "arn": deleted })).await;
    assert_eq!(
        removal.status, 200,
        "the fixture stage must really be deleted, got {} {}",
        removal.status, removal.body
    );

    let mut cases: Vec<(String, String)> = OWN_RESOURCE_TYPES
        .iter()
        .map(|resource_type| {
            (
                format!("{resource_type} that was never created"),
                arn("us-east-1", resource_type, "0123456789ab"),
            )
        })
        .collect();
    cases.push((
        "stage whose ARN this store never minted".to_string(),
        arn("eu-west-1", "stage", "ffffffffffff"),
    ));
    cases.push(("stage that was deleted".to_string(), deleted));

    for (case, missing) in cases {
        for (method, query, body) in every_tagging_method() {
            let reply = realtime.tagging(method, &missing, query, body).await;
            assert_eq!(
                (
                    reply.status,
                    reply.error_code.as_str(),
                    reply.message.as_str()
                ),
                (404, "ResourceNotFoundException", not_found(&missing).as_str()),
                "case {case:?} ({method}): an admitted ARN naming no resource must be a \
                 not-found, got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }
}

/// Rule 1 on the refusing side: a `resourceArn` outside the member's modelled
/// `@length(1..128)` or outside the corrected pattern is refused as a
/// `ValidationException` (HTTP 400) naming the operation, the member path and the
/// constraint it violated.
///
/// The rows differ in HOW the label fails: an uppercase region, an account that is not
/// digits, an id outside its character class, a missing resource id, and — on the length
/// bound — an ARN one character past the maximum, with the ARN exactly AT the maximum
/// admitted as the bound's other side. Every row is on the `stage` path space, because a
/// label naming no IVS resource type at all belongs to the sibling `ivs` endpoint (see
/// `signing_name_routing.rs`) and never reaches this service.
#[tokio::test]
async fn a_resource_arn_outside_its_constraints_is_refused_naming_the_constraint() {
    let realtime = RealTime::new();

    // The id that brings an `arn:aws:ivs:us-east-1:<account>:stage/<id>` to exactly the
    // modelled maximum length, and one character more.
    let prefix_len = arn("us-east-1", "stage", "").len();
    let at_maximum = arn(
        "us-east-1",
        "stage",
        &"a".repeat(RESOURCE_ARN_MAX_LENGTH - prefix_len),
    );
    let past_maximum = arn(
        "us-east-1",
        "stage",
        &"a".repeat(RESOURCE_ARN_MAX_LENGTH - prefix_len + 1),
    );
    assert_eq!(
        (at_maximum.chars().count(), past_maximum.chars().count()),
        (RESOURCE_ARN_MAX_LENGTH, RESOURCE_ARN_MAX_LENGTH + 1),
        "the length fixtures must sit on the bound and one past it"
    );

    // (case, the label, the clause the refusal states after `<Operation>.`)
    let pattern_clause =
        format!("resourceArn does not match the required pattern {RESOURCE_ARN_PATTERN}");
    let rows: Vec<(&str, String, String)> = vec![
        (
            "an uppercase region",
            arn("US-EAST-1", "stage", "0123456789ab"),
            pattern_clause.clone(),
        ),
        (
            "an account that is not digits",
            "arn:aws:ivs:us-east-1:not-an-account:stage/0123456789ab".to_string(),
            pattern_clause.clone(),
        ),
        (
            "an id outside its character class",
            arn("us-east-1", "stage", "has_underscore"),
            pattern_clause.clone(),
        ),
        (
            "no resource id at all",
            "arn:aws:ivs:us-east-1:000000000000:stage".to_string(),
            pattern_clause.clone(),
        ),
        (
            "one character past the length maximum",
            past_maximum,
            format!(
                "resourceArn is required to have a maximum length of \
                 {RESOURCE_ARN_MAX_LENGTH}, found {}",
                RESOURCE_ARN_MAX_LENGTH + 1
            ),
        ),
    ];

    for (case, label, clause) in rows {
        for (method, query, body) in every_tagging_method() {
            let operation = match method {
                "GET" => "ListTagsForResource",
                "POST" => "TagResource",
                _ => "UntagResource",
            };
            let reply = realtime.tagging(method, &label, query, body).await;
            let expected = format!("{operation}.{clause}");
            assert_eq!(
                (
                    reply.status,
                    reply.error_code.as_str(),
                    reply.message.as_str()
                ),
                (400, "ValidationException", expected.as_str()),
                "case {case:?} ({operation}): expected the stated refusal {expected:?}, \
                 got {} {} {}",
                reply.status,
                reply.error_code,
                reply.body
            );
        }
    }

    // The other side of the length bound: a label exactly at the maximum is admitted, so
    // it reaches the existence check rather than the refusal.
    for (method, query, body) in every_tagging_method() {
        let reply = realtime.tagging(method, &at_maximum, query, body).await;
        assert_eq!(
            (reply.status, reply.error_code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {method} at the length maximum: a label on the bound is admitted, got \
             {} {} {}",
            reply.status,
            reply.error_code,
            reply.body
        );
    }
}

/// Rule 4: the modelled `Tags` maximum bounds the RESOURCE's tag set, so a call whose own
/// entries are within the bound is still refused when the set it would leave is over it —
/// and the refused call writes nothing.
#[tokio::test]
async fn the_tag_maximum_bounds_the_resource_not_one_request() {
    let realtime = RealTime::new();
    let stage_arn = realtime
        .create_stage_in("us-east-1", &json!({ "name": "bounded" }))
        .await;

    // Fill the resource to exactly the maximum, in one call that is itself at the bound.
    let full = tag_map(MAX_TAGS);
    let reply = realtime
        .tagging("POST", &stage_arn, "", Some(json!({ "tags": full })))
        .await;
    assert_no_content(
        "a request at the modelled maximum",
        "TagResource",
        &reply,
    );
    assert_eq!(
        realtime.tags_of("filled to the maximum", &stage_arn).await,
        full,
        "the resource must carry exactly the maximum"
    );

    // One more KEY is over the resource's bound, even though the request carries one
    // entry.
    let reply = realtime
        .tagging(
            "POST",
            &stage_arn,
            "",
            Some(json!({ "tags": { "one-too-many": "x" } })),
        )
        .await;
    let expected = format!(
        "TagResource.tags is required to have a maximum length of {MAX_TAGS}, found {}",
        MAX_TAGS + 1
    );
    assert_eq!(
        (
            reply.status,
            reply.error_code.as_str(),
            reply.message.as_str()
        ),
        (400, "ValidationException", expected.as_str()),
        "a call that would leave the resource over the maximum must be refused, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
    assert_eq!(
        realtime.tags_of("refused write", &stage_arn).await,
        full,
        "the refused call must leave the tag set exactly as it was"
    );

    // A key the resource ALREADY carries replaces a value and adds nothing, so it is
    // accepted at the bound.
    let reply = realtime
        .tagging(
            "POST",
            &stage_arn,
            "",
            Some(json!({ "tags": { "key0": "replaced" } })),
        )
        .await;
    assert_no_content(
        "replacing a key the resource already carries adds nothing, so the call stays \
         within the maximum",
        "TagResource",
        &reply,
    );
    let mut expected = full.clone();
    expected.insert("key0".to_string(), "replaced".to_string());
    assert_eq!(
        realtime.tags_of("replacement at the bound", &stage_arn).await,
        expected,
        "the replacement must be the only change"
    );
}
