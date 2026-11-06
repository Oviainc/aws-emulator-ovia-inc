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
//! 2. EXISTENCE. An admitted ARN that names no resource the region the request was
//!    processed in holds is refused with this surface's OWN not-found — HTTP 404
//!    `NotFoundException` stating one fixed, resource-agnostic sentence — and not as a
//!    validation failure, and not with the `ResourceNotFoundException` the
//!    resource-specific reads of the very same resources answer. A resource belongs to
//!    the region its ARN names, so each test
//!    addresses its fixture through the region the fixture was created in; the region
//!    scoping itself is asserted in `region_scoped_resources.rs`.
//! 3. ROUND TRIP. `TagResource` is additive — a key the resource carries takes the new
//!    value, every other key is kept — and what it writes is what `ListTagsForResource`
//!    and the resource's own `GetStage` read back. `UntagResource` removes exactly the
//!    named keys and is idempotent. Both answer the recorded 204 with no body.
//! 4. The modelled `Tags` `@length` maximum bounds the RESOURCE's tag set, so a call is
//!    refused when the set it would LEAVE is over the maximum, even though each request
//!    is within it — with this surface's own HTTP 400 `BadRequestException` and the one
//!    fixed sentence it states that bound with.
//! 5. RESERVED NAMESPACE. Every `Tags`/`TagKeyList` member of this model defers to the
//!    AWS-wide tagging restrictions ("Amazon IVS has no constraints on tags beyond what
//!    is documented there"), which reserve the `aws:` key namespace for AWS's own tags: a
//!    request that would create, change or remove a key claiming that namespace — in any
//!    casing, on any operation that carries tag keys — is refused with the modelled
//!    `ValidationException` (HTTP 400) and changes nothing, while a key that merely looks
//!    like one is an ordinary key.

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService;

/// A SigV4 `Authorization` header is how a request names the service and the region it
/// is addressed to, so every call carries one. The signature itself is never checked
/// here: request authentication is off unless a router turns it on.
///
/// The region every test that does not vary it signs for — and so the region those tests
/// create their fixtures in, since a resource is addressed through the region holding it.
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

/// The error code this surface answers for an ARN it holds no resource for, and the
/// fixed sentence it states — both distinct from the `ResourceNotFoundException` /
/// `Resource: <arn> not found` the resource-specific reads answer (see
/// `not_found_envelope`-style cover in `error_envelope.rs`).
const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";
const TAGGING_NOT_FOUND_MESSAGE: &str = "Resource was not found";

/// The error code this surface answers for a request it refuses as malformed.
const TAGGING_BAD_REQUEST_CODE: &str = "BadRequestException";

/// The sentence this surface states when the tag set a call would leave is over
/// `MAX_TAGS`. Written out independently of the service, with the bound interpolated
/// from the modelled maximum above.
fn tag_limit_refusal() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS} user tags.")
}

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

/// The tag-key namespace AWS reserves for its own tags, written out here so the expected
/// refusal is stated independently of the service. Source: the "Tag naming limits and
/// requirements" restrictions every `Tags`/`TagKeyList` member of this model defers to
/// (`smithy_query(query="doc", shape="TagResourceRequest", member="tags")` and
/// `smithy_query(query="doc", shape="UntagResourceRequest", member="tagKeys")`).
const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

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
        self.call_in(AUTHORIZATION_REGION, operation, body).await
    }

    /// [`Self::call`] signed for `region`: the region a request is processed in is the
    /// account view it is answered from, so a resource is addressed through the region it
    /// was created in.
    async fn call_in(&self, region: &str, operation: &str, body: &Value) -> Reply {
        self.send(
            region,
            "POST",
            &format!("/{operation}"),
            Some(bytes::Bytes::from(body.to_string())),
        )
        .await
    }

    /// One of the three operations bound to `/tags/{resourceArn}`, carrying the ARN in
    /// the URI the way a client does: percent-encoded into one path segment.
    async fn tagging(&self, method: &str, arn: &str, query: &str, body: Option<Value>) -> Reply {
        self.tagging_in(AUTHORIZATION_REGION, method, arn, query, body)
            .await
    }

    /// [`Self::tagging`] signed for `region`.
    async fn tagging_in(
        &self,
        region: &str,
        method: &str,
        arn: &str,
        query: &str,
        body: Option<Value>,
    ) -> Reply {
        let path = format!("/tags/{}{query}", encode_path_label(arn));
        self.send(
            region,
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
        self.tags_of_in(case, AUTHORIZATION_REGION, arn).await
    }

    /// [`Self::tags_of`] asked through `region`'s endpoint.
    async fn tags_of_in(&self, case: &str, region: &str, arn: &str) -> HashMap<String, String> {
        let reply = self.tagging_in(region, "GET", arn, "", None).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: ListTagsForResource on {arn} in {region} must answer HTTP 200, \
             got {} {}",
            reply.status, reply.body
        );
        serde_json::from_value(reply.body["tags"].clone()).unwrap_or_default()
    }

    /// The tags `GetStage` reports for `arn` — the same state, read through the resource's
    /// own reader rather than the tagging surface.
    async fn tags_on_stage(&self, case: &str, arn: &str) -> HashMap<String, String> {
        self.tags_on_stage_in(case, AUTHORIZATION_REGION, arn).await
    }

    /// [`Self::tags_on_stage`] asked through `region`'s endpoint.
    async fn tags_on_stage_in(
        &self,
        case: &str,
        region: &str,
        arn: &str,
    ) -> HashMap<String, String> {
        let reply = self
            .call_in(region, "GetStage", &json!({ "arn": arn }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: GetStage on {arn} in {region} must answer HTTP 200, got {} {}",
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
/// maximum. Each row addresses its stage through the region the stage was created in,
/// which is the account view that holds it.
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
            realtime.tags_of_in(case, region, &stage_arn).await,
            at_creation,
            "case {case:?}: the surface must answer the tags the create persisted"
        );

        // The write: a key the stage already carries takes the new value, and every other
        // tag it carries is kept.
        let reply = realtime
            .tagging_in(region, "POST", &stage_arn, "", Some(json!({ "tags": written })))
            .await;
        assert_no_content(case, "TagResource", &reply);

        let mut expected = at_creation.clone();
        expected.extend(written.clone());
        assert_eq!(
            realtime.tags_of_in(case, region, &stage_arn).await,
            expected,
            "case {case:?}: tagging is additive and replaces only the keys it carries"
        );
        assert_eq!(
            realtime.tags_on_stage_in(case, region, &stage_arn).await,
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
                .tagging_in(region, "DELETE", &stage_arn, &format!("?{query}"), None)
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
                realtime.tags_of_in(case, region, &stage_arn).await,
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
                (
                    404,
                    TAGGING_NOT_FOUND_CODE,
                    TAGGING_NOT_FOUND_MESSAGE
                ),
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
            (404, TAGGING_NOT_FOUND_CODE),
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
    let expected = tag_limit_refusal();
    assert_eq!(
        (
            reply.status,
            reply.error_code.as_str(),
            reply.message.as_str()
        ),
        (400, TAGGING_BAD_REQUEST_CODE, expected.as_str()),
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

/// The refusal this service states for a request naming keys in the reserved namespace:
/// the construction every other refusal of this service uses — the operation, the member
/// the keys arrived in, the requirement, then the offending keys in sorted order.
fn reserved_refusal(operation: &str, member: &str, keys: &[&str]) -> String {
    format!(
        "{operation}.{member} is required to name no key in the reserved \
         {RESERVED_TAG_KEY_PREFIX} namespace, found {}",
        keys.join(", ")
    )
}

/// Assert that `reply` is the modelled `ValidationException` (HTTP 400) stating `expected`.
fn assert_validation_refusal(case: &str, operation: &str, reply: &Reply, expected: &str) {
    assert_eq!(
        (
            reply.status,
            reply.error_code.as_str(),
            reply.message.as_str()
        ),
        (400, "ValidationException", expected),
        "case {case:?} ({operation}): expected the stated refusal {expected:?}, got {} {} {}",
        reply.status,
        reply.error_code,
        reply.body
    );
}

/// Rule 5 on `TagResource`: a key claiming the reserved namespace is refused with the
/// modelled `ValidationException`, and the resource's tags are left exactly as they were.
///
/// The rows differ along what the rule generalises over: the CASING of the prefix, whether
/// the key carries a qualifier after it (the form AWS's own tags take) or nothing at all,
/// how many reserved keys one request carries, and whether ordinary keys travel beside
/// them — a refused request writes none of them either.
#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_by_tag_resource() {
    let realtime = RealTime::new();
    let stage_arn = realtime
        .create_stage_in("us-east-1", &json!({ "name": "reserved", "tags": { "env": "prod" } }))
        .await;
    let held: HashMap<String, String> =
        HashMap::from([("env".to_string(), "prod".to_string())]);

    // (case, the tags the request carries, the offending keys the refusal names)
    let rows: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "a reserved key",
            json!({ "aws:owner": "platform" }),
            vec!["aws:owner"],
        ),
        (
            "a qualified reserved key, the form AWS's own tags take",
            json!({ "aws:cloudformation:stack-name": "prod" }),
            vec!["aws:cloudformation:stack-name"],
        ),
        (
            "the reserved prefix and nothing after it",
            json!({ "aws:": "x" }),
            vec!["aws:"],
        ),
        (
            "the reserved prefix in upper case",
            json!({ "AWS:owner": "platform" }),
            vec!["AWS:owner"],
        ),
        (
            "the reserved prefix in mixed case",
            json!({ "Aws:owner": "platform" }),
            vec!["Aws:owner"],
        ),
        (
            "a reserved key beside an ordinary one",
            json!({ "aws:owner": "platform", "team": "video" }),
            vec!["aws:owner"],
        ),
        (
            "two reserved keys at once",
            json!({ "aws:a": "1", "aws:b": "2" }),
            vec!["aws:a", "aws:b"],
        ),
    ];

    for (case, tags, offending) in rows {
        let reply = realtime
            .tagging("POST", &stage_arn, "", Some(json!({ "tags": tags })))
            .await;
        assert_validation_refusal(
            case,
            "TagResource",
            &reply,
            &reserved_refusal("TagResource", "tags", &offending),
        );
        assert_eq!(
            realtime.tags_of(case, &stage_arn).await,
            held,
            "case {case:?}: a refused TagResource must write nothing, not even the keys \
             travelling beside the reserved one"
        );
        assert_eq!(
            realtime.tags_on_stage(case, &stage_arn).await,
            held,
            "case {case:?}: GetStage must report the untouched tag set too"
        );
    }
}

/// Rule 5 on `UntagResource`: the reserved namespace is AWS's own to write, so a caller
/// cannot DELETE a key claiming it either — the removal is refused and nothing is removed,
/// including the removable keys named beside it.
///
/// The rows differ along the same axes as the `TagResource` table, plus the POSITION of
/// the reserved key in the `tagKeys` list.
#[tokio::test]
async fn a_tag_key_in_the_reserved_namespace_is_refused_by_untag_resource() {
    let realtime = RealTime::new();
    // The fixture stage lives in one region and is addressed through that region, which
    // is the account view holding it.
    let region = "eu-west-1";
    let stage_arn = realtime
        .create_stage_in(
            region,
            &json!({ "name": "reserved", "tags": { "env": "prod", "team": "video" } }),
        )
        .await;
    let held: HashMap<String, String> = HashMap::from([
        ("env".to_string(), "prod".to_string()),
        ("team".to_string(), "video".to_string()),
    ]);

    // (case, the keys the request names, the offending keys the refusal names)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("a reserved key", vec!["aws:owner"], vec!["aws:owner"]),
        (
            "a qualified reserved key",
            vec!["aws:cloudformation:stack-name"],
            vec!["aws:cloudformation:stack-name"],
        ),
        (
            "the reserved prefix in upper case",
            vec!["AWS:owner"],
            vec!["AWS:owner"],
        ),
        (
            "a reserved key before a removable one",
            vec!["aws:owner", "team"],
            vec!["aws:owner"],
        ),
        (
            "a reserved key after a removable one",
            vec!["team", "aws:owner"],
            vec!["aws:owner"],
        ),
        (
            "two reserved keys at once",
            vec!["aws:b", "aws:a"],
            vec!["aws:a", "aws:b"],
        ),
    ];

    for (case, keys, offending) in rows {
        let query = keys
            .iter()
            .map(|key| format!("tagKeys={key}"))
            .collect::<Vec<String>>()
            .join("&");
        let reply = realtime
            .tagging_in(region, "DELETE", &stage_arn, &format!("?{query}"), None)
            .await;
        assert_validation_refusal(
            case,
            "UntagResource",
            &reply,
            &reserved_refusal("UntagResource", "tagKeys", &offending),
        );
        assert_eq!(
            realtime.tags_of_in(case, region, &stage_arn).await,
            held,
            "case {case:?}: a refused UntagResource must remove nothing, not even the \
             removable keys named beside the reserved one"
        );
    }
}

/// The other side of rule 5: it is the NAMESPACE that is reserved, so a key that merely
/// starts with the same letters, or carries the prefix anywhere but at the start, is an
/// ordinary key — written, read back through both readers, and removed again.
///
/// The rows differ along how close the key comes to the prefix without claiming it: the
/// letters followed by more letters, the letters alone, the prefix after another segment,
/// and the colon without the letters.
#[tokio::test]
async fn a_key_that_only_looks_reserved_is_an_ordinary_key() {
    let realtime = RealTime::new();
    // The fixture stage lives in one region and is addressed through that region, which
    // is the account view holding it.
    let region = "ap-southeast-2";
    let stage_arn = realtime
        .create_stage_in(region, &json!({ "name": "lookalike" }))
        .await;

    let rows: [(&str, &str); 4] = [
        ("the reserved letters followed by more", "awsome"),
        ("the reserved letters without the colon", "aws"),
        ("the prefix after another segment", "team-aws:owner"),
        ("a colon-led key that is not the namespace", "az:owner"),
    ];

    for (case, key) in rows {
        let reply = realtime
            .tagging_in(
                region,
                "POST",
                &stage_arn,
                "",
                Some(json!({ "tags": { key: "accepted" } })),
            )
            .await;
        assert_no_content(case, "TagResource", &reply);

        let expected: HashMap<String, String> =
            HashMap::from([(key.to_string(), "accepted".to_string())]);
        assert_eq!(
            realtime.tags_of_in(case, region, &stage_arn).await,
            expected,
            "case {case:?}: an ordinary key must round-trip through ListTagsForResource"
        );
        assert_eq!(
            realtime.tags_on_stage_in(case, region, &stage_arn).await,
            expected,
            "case {case:?}: GetStage must report it too"
        );

        let reply = realtime
            .tagging_in(region, "DELETE", &stage_arn, &format!("?tagKeys={key}"), None)
            .await;
        assert_no_content(case, "UntagResource", &reply);
        assert!(
            realtime.tags_of_in(case, region, &stage_arn).await.is_empty(),
            "case {case:?}: an ordinary key must be removable, got {:?}",
            realtime.tags_of_in(case, region, &stage_arn).await
        );
    }
}

/// Rule 5 at CREATION time: every operation of this service whose request carries a `tags`
/// map defers to the same documented restrictions, so none of them may persist a key a
/// `TagResource` would refuse — and a refused create leaves no resource behind.
///
/// The rows are every implemented create that accepts tags, each with the rest of its
/// request filled in minimally; the accepting half of each row proves the create still
/// stores an ordinary key, read back off the resource the create reports.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_by_every_create_that_accepts_tags() {
    let realtime = RealTime::new();

    // (operation, the member reporting the created resource, the rest of the request)
    let creates: [(&str, &str, Value); 4] = [
        ("CreateStage", "stage", json!({ "name": "created" })),
        (
            "CreateIngestConfiguration",
            "ingestConfiguration",
            json!({ "name": "created", "ingestProtocol": "RTMPS" }),
        ),
        (
            "CreateEncoderConfiguration",
            "encoderConfiguration",
            json!({ "name": "created" }),
        ),
        (
            "CreateStorageConfiguration",
            "storageConfiguration",
            json!({ "name": "created", "s3": { "bucketName": "recordings" } }),
        ),
    ];

    for (operation, member_name, base) in creates {
        for (case, tags, offending) in [
            (
                "a reserved key",
                json!({ "aws:owner": "platform" }),
                vec!["aws:owner"],
            ),
            (
                "the reserved prefix in upper case, beside an ordinary key",
                json!({ "AWS:cloudformation:stack-name": "prod", "team": "video" }),
                vec!["AWS:cloudformation:stack-name"],
            ),
        ] {
            let mut request = base.clone();
            request["tags"] = tags;
            let reply = realtime.call(operation, &request).await;
            assert_validation_refusal(
                case,
                operation,
                &reply,
                &reserved_refusal(operation, "tags", &offending),
            );
            assert!(
                reply.body.get(member_name).is_none(),
                "case {case:?} ({operation}): a refused create must report no resource, got {}",
                reply.body
            );
        }

        // The accepting half: an ordinary key is still persisted, and the create reports
        // it on the resource it created.
        let mut request = base.clone();
        request["tags"] = json!({ "awsome": "yes" });
        let reply = realtime.call(operation, &request).await;
        assert_eq!(
            reply.status, 200,
            "{operation} with an ordinary key must answer HTTP 200, got {} {}",
            reply.status, reply.body
        );
        let stored: HashMap<String, String> =
            serde_json::from_value(reply.body[member_name]["tags"].clone()).unwrap_or_default();
        assert_eq!(
            stored,
            HashMap::from([("awsome".to_string(), "yes".to_string())]),
            "{operation} must persist an ordinary key, got {}",
            reply.body
        );

        // Read back through the tagging surface, which reads the stored record rather than
        // the create's own answer.
        let created_arn = member(&reply.body[member_name], "arn");
        assert_eq!(
            realtime.tags_of(operation, &created_arn).await,
            stored,
            "{operation}: the tagging surface must report the tags the create persisted"
        );
    }

    // Nothing the refused creates named is in the store: each resource type's listing
    // reports only the resource their accepted create made.
    for (operation, member_name) in [
        ("ListStages", "stages"),
        ("ListIngestConfigurations", "ingestConfigurations"),
        ("ListEncoderConfigurations", "encoderConfigurations"),
        ("ListStorageConfigurations", "storageConfigurations"),
    ] {
        let reply = realtime.call(operation, &json!({})).await;
        let listed = reply.body[member_name].as_array().cloned().unwrap_or_default();
        assert_eq!(
            listed.len(),
            1,
            "{operation} must list only the resource the accepted create made, got {}",
            reply.body
        );
    }
}
