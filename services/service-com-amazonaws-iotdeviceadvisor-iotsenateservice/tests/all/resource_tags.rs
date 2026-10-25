// SPDX-License-Identifier: CC0-1.0

//! The tagging surface over the wire: `ListTagsForResource` and `TagResource`.
//!
//! Every case drives real HTTP requests through the same `MultiServiceRouter` the emulator
//! serves on a socket, so what is asserted is the service's wire behaviour.
//!
//! The rules under test, each asserted over a TABLE rather than one recorded request:
//!
//! * a tag read answers exactly the tags the store holds for the resource its
//!   `resourceArn` names — for BOTH taggable resource kinds (suite definition, suite run)
//!   and over tag maps differing in size, in key/value length and in the resource they
//!   were written by (a create, a run start, a later `TagResource`);
//! * `TagResource` MERGES: it overwrites the value of a key already there, adds the keys
//!   that are new, and leaves every other key in place;
//! * a `resourceArn` the store holds no resource for is a 404, and one that is not an ARN
//!   of this service is a 400 — for `ListTagsForResource`, `TagResource` and
//!   `UntagResource` alike, never a 501 and never a success;
//! * that 404 carries the service's SINGLE not-found template, built from the input member
//!   that named the missing resource — the same wording the id-keyed lookups answer with;
//! * a read changes nothing a later caller sees.
//!
//! What `UntagResource` REMOVES, and that removing is idempotent, is asserted in
//! `tests/all/resource_tagging.rs`.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iotdeviceadvisor_iotsenateservice as iotdeviceadvisor;
use smithy_server_runtime::MultiServiceRouter;

/// `SuiteDefinitionConfiguration$devicePermissionRoleArn` is `len 20..2048`; this is the
/// role every row below points at.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/DeviceAdvisorRole";

/// The thing ARN of the device a suite run names as its primary device.
const THING_ARN: &str = "arn:aws:iot:us-east-1:000000000000:thing/device-under-test";

/// The region every case signs for, so a minted ARN is predictable.
const REGION: &str = "us-east-1";

/// `TagMap` is `len 0..50` (source: `smithy_query(query="shape",shape="TagMap")`), so this
/// is how many tags one resource can hold at once.
const MAX_TAGS: usize = 50;

/// `TagMap`'s key target is `String128` (`len 1..128`) and its value target `String256`
/// (`len 1..256`) — the bounds the long-key/long-value row below sits on.
const MAX_TAG_KEY_LEN: usize = 128;
const MAX_TAG_VALUE_LEN: usize = 256;

/// Percent-encode one value for a single URI path segment: `/` and `:` would otherwise
/// split the segment or the authority, exactly as an SDK encodes an `@httpLabel`.
fn percent_encode(value: &str) -> String {
    value
        .replace('%', "%25")
        .replace(':', "%3A")
        .replace('/', "%2F")
}

/// The `/tags/{resourceArn}` URI for `arn`.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", percent_encode(arn))
}

/// Percent-encode one QUERY value, the way an SDK encodes an `@httpQuery` entry:
/// everything but the unreserved characters, byte by byte. Stricter than
/// [`percent_encode`] above because a tag key is free text — `String128` carries no
/// pattern — so it can hold `&`, `=` or a non-ASCII character, none of which may reach
/// the URI raw.
fn query_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(char::from(byte));
            }
            other => {
                use std::fmt::Write as _;
                let _ = write!(encoded, "%{other:02X}");
            }
        }
    }
    encoded
}

/// The emulator, assembled in-process with authentication off: this suite asserts
/// behaviour, and signature verification is the framework's own concern.
struct Emulator {
    router: MultiServiceRouter,
    /// The one shared engine the handlers write through, kept so a case can assert an
    /// invariant of the STORE itself and not only of the responses.
    engine: Arc<engine::InMemoryEngine>,
}

/// One response, reduced to what a client branches on.
struct Answer {
    status: u16,
    /// The parsed JSON body, or `Value::Null` for an empty one.
    body: Value,
    /// The restJson1 error code (`x-amzn-errortype`), when the answer carries one.
    error_code: Option<String>,
}

impl Answer {
    fn member(&self, name: &str) -> &Value {
        &self.body[name]
    }

    fn string(&self, name: &str) -> String {
        self.member(name)
            .as_str()
            .unwrap_or_else(|| panic!("member {name} is not a string in {}", self.body))
            .to_string()
    }
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(iotdeviceadvisor::EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// How many tag rows the shared store holds.
    fn stored_tag_rows(&self) -> usize {
        let state = iotdeviceadvisor::state::service_state(&*self.engine)
            .expect("the service's store is reachable from the shared engine");
        let store = state.lock();
        store.tags.len()
    }

    /// Send one request, signed (in scope only — authentication is off) for `REGION`, so
    /// the handler resolves the same region a real SDK client would.
    async fn call(&self, method: &str, path_and_query: &str, body: Option<Value>) -> Answer {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("the request body serializes"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(path_and_query)
            .header("host", format!("api.iotdeviceadvisor.{REGION}.amazonaws.com"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iotdeviceadvisor/\
                     aws4_request, SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the request builds");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Answer {
            status,
            body,
            error_code,
        }
    }

    /// Create a suite definition named `name`, with `tags` when the row carries any, and
    /// answer `(suiteDefinitionId, suiteDefinitionArn)`.
    async fn create_definition(&self, name: &str, tags: &Value) -> (String, String) {
        let mut body = json!({
            "suiteDefinitionConfiguration": {
                "suiteDefinitionName": name,
                "rootGroup": "group",
                "devicePermissionRoleArn": ROLE_ARN,
            }
        });
        if !tags.as_object().is_some_and(serde_json::Map::is_empty) {
            body["tags"] = tags.clone();
        }
        let created = self.call("POST", "/suiteDefinitions", Some(body)).await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: expected HTTP 200 from CreateSuiteDefinition, got {} {}",
            created.status, created.body
        );
        (
            created.string("suiteDefinitionId"),
            created.string("suiteDefinitionArn"),
        )
    }

    /// Start a suite run of `definition_id`, with `tags` when the row carries any, and
    /// answer its `suiteRunArn`.
    async fn start_run(&self, case: &str, definition_id: &str, tags: &Value) -> String {
        let mut body = json!({
            "suiteRunConfiguration": { "primaryDevice": { "thingArn": THING_ARN } }
        });
        if !tags.as_object().is_some_and(serde_json::Map::is_empty) {
            body["tags"] = tags.clone();
        }
        let started = self
            .call(
                "POST",
                &format!("/suiteDefinitions/{definition_id}/suiteRuns"),
                Some(body),
            )
            .await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: expected HTTP 200 from StartSuiteRun, got {} {}",
            started.status, started.body
        );
        started.string("suiteRunArn")
    }

    /// `ListTagsForResource` for `arn`.
    async fn list_tags(&self, arn: &str) -> Answer {
        self.call("GET", &tags_uri(arn), None).await
    }

    /// `TagResource` for `arn`.
    async fn tag_resource(&self, arn: &str, tags: &Value) -> Answer {
        self.call("POST", &tags_uri(arn), Some(json!({ "tags": tags })))
            .await
    }

    /// `UntagResource` for `arn`: `DELETE /tags/{resourceArn}` with one `tagKeys` query
    /// entry per key, as an SDK serializes the modelled `@httpQuery` list.
    async fn untag_resource(&self, arn: &str, tag_keys: &[&str]) -> Answer {
        let query = tag_keys
            .iter()
            .map(|key| format!("tagKeys={}", query_encode(key)))
            .collect::<Vec<_>>()
            .join("&");
        self.call("DELETE", &format!("{}?{query}", tags_uri(arn)), None)
            .await
    }
}

/// One tag whose key and value sit on the modelled maximum lengths.
fn longest_tag() -> Value {
    let mut map = serde_json::Map::new();
    map.insert(
        "k".repeat(MAX_TAG_KEY_LEN),
        json!("v".repeat(MAX_TAG_VALUE_LEN)),
    );
    Value::Object(map)
}

/// `left` with every entry of `right` merged over it — what a merge must leave behind.
fn merged(left: &Value, right: &Value) -> Value {
    let mut map = left.as_object().expect("a tag map is an object").clone();
    for (key, value) in right.as_object().expect("a tag map is an object") {
        map.insert(key.clone(), value.clone());
    }
    Value::Object(map)
}

/// A tag map of `count` distinct keys.
fn tag_map(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("key-{index}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

/// The tag maps the read is asserted over: rows differing in SIZE (none, one, a few, and
/// the modelled maximum of 50) and in key/value LENGTH (both modelled bounds).
fn tag_rows() -> Vec<(String, Value)> {
    vec![
        ("no tags".to_string(), json!({})),
        ("one tag".to_string(), json!({ "team": "iot" })),
        (
            "three tags".to_string(),
            json!({ "team": "iot", "env": "test", "owner": "device-advisor" }),
        ),
        (
            "key and value at the modelled minimum length".to_string(),
            json!({ "k": "v" }),
        ),
        (
            "key and value at the modelled maximum length".to_string(),
            longest_tag(),
        ),
        (format!("{MAX_TAGS} tags"), tag_map(MAX_TAGS)),
    ]
}

/// A tag read answers exactly the tags the store holds for the resource the ARN names —
/// for a suite definition and for a suite run, over every row of the table.
#[tokio::test]
async fn listing_answers_the_tags_the_store_holds_for_either_resource_kind() {
    let emulator = Emulator::new();

    for (index, (case, tags)) in tag_rows().into_iter().enumerate() {
        let (definition_id, definition_arn) = emulator
            .create_definition(&format!("tag-suite-{index}"), &tags)
            .await;

        let listed = emulator.list_tags(&definition_arn).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected HTTP 200 from ListTagsForResource, got {} {}",
            listed.status, listed.body
        );
        assert_eq!(
            listed.member("tags"),
            &tags,
            "case {case:?}: the definition's listed tags must be the stored ones, got {}",
            listed.body
        );

        // The same rule for the other taggable resource kind, whose tags a run start
        // wrote.
        let run_arn = emulator.start_run(&case, &definition_id, &tags).await;
        let listed_run = emulator.list_tags(&run_arn).await;
        assert_eq!(
            listed_run.status, 200,
            "case {case:?}: expected HTTP 200 listing a suite run's tags, got {} {}",
            listed_run.status, listed_run.body
        );
        assert_eq!(
            listed_run.member("tags"),
            &tags,
            "case {case:?}: the run's listed tags must be the stored ones, got {}",
            listed_run.body
        );

        // The resource reads and the tag read project ONE tag row, so they cannot
        // disagree: `GetSuiteDefinition` omits `tags` only where there are none.
        let read = emulator
            .call("GET", &format!("/suiteDefinitions/{definition_id}"), None)
            .await;
        let expected_on_read = if tags.as_object().is_some_and(serde_json::Map::is_empty) {
            Value::Null
        } else {
            tags.clone()
        };
        assert_eq!(
            read.member("tags"),
            &expected_on_read,
            "case {case:?}: GetSuiteDefinition and ListTagsForResource must agree, got {}",
            read.body
        );
    }
}

/// `TagResource` merges: it overwrites the keys it names, adds the ones that are new, and
/// leaves the rest alone — and what it wrote is what the read answers.
#[tokio::test]
async fn tagging_merges_into_the_stored_tags_and_round_trips_through_the_read() {
    let emulator = Emulator::new();

    // (case, tags at creation, tags the call adds, the tags the resource must then hold)
    let rows: Vec<(&str, Value, Value, Value)> = vec![
        (
            "tag an untagged resource",
            json!({}),
            json!({ "team": "iot" }),
            json!({ "team": "iot" }),
        ),
        (
            "add a new key",
            json!({ "team": "iot" }),
            json!({ "env": "test" }),
            json!({ "team": "iot", "env": "test" }),
        ),
        (
            "overwrite an existing key",
            json!({ "team": "iot", "env": "test" }),
            json!({ "env": "prod" }),
            json!({ "team": "iot", "env": "prod" }),
        ),
        (
            "overwrite and add at once",
            json!({ "team": "iot", "env": "test" }),
            json!({ "env": "prod", "owner": "device-advisor" }),
            json!({ "team": "iot", "env": "prod", "owner": "device-advisor" }),
        ),
        (
            "an empty tag map changes nothing",
            json!({ "team": "iot" }),
            json!({}),
            json!({ "team": "iot" }),
        ),
        (
            "keys at the modelled length bounds",
            json!({ "k": "v" }),
            longest_tag(),
            merged(&json!({ "k": "v" }), &longest_tag()),
        ),
        (
            "fill a resource up to the tag limit",
            tag_map(MAX_TAGS - 1),
            json!({ "last": "one" }),
            merged(&tag_map(MAX_TAGS - 1), &json!({ "last": "one" })),
        ),
    ];

    for (index, (case, initial, added, expected)) in rows.into_iter().enumerate() {
        let (definition_id, definition_arn) = emulator
            .create_definition(&format!("merge-suite-{index}"), &initial)
            .await;

        let tagged = emulator.tag_resource(&definition_arn, &added).await;
        assert_eq!(
            tagged.status, 200,
            "case {case:?}: expected HTTP 200 from TagResource, got {} {}",
            tagged.status, tagged.body
        );

        let listed = emulator.list_tags(&definition_arn).await;
        assert_eq!(
            listed.member("tags"),
            &expected,
            "case {case:?}: the merged tags must be what the read answers, got {}",
            listed.body
        );

        // The same merge rule on the other resource kind, and on a resource whose tags a
        // run start wrote.
        let run_arn = emulator.start_run(case, &definition_id, &initial).await;
        let tagged_run = emulator.tag_resource(&run_arn, &added).await;
        assert_eq!(
            tagged_run.status, 200,
            "case {case:?}: expected HTTP 200 tagging a suite run, got {} {}",
            tagged_run.status, tagged_run.body
        );
        let listed_run = emulator.list_tags(&run_arn).await;
        assert_eq!(
            listed_run.member("tags"),
            &expected,
            "case {case:?}: a run's merged tags must be what the read answers, got {}",
            listed_run.body
        );
    }
}

/// Tagging past the per-resource limit is refused whole, and the resource keeps the tags
/// it had.
#[tokio::test]
async fn tagging_past_the_tag_limit_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();
    let full = tag_map(MAX_TAGS);
    let (_, arn) = emulator.create_definition("full-suite", &full).await;

    // (case, the tags the call adds, expected status)
    let rows: Vec<(&str, Value, u16)> = vec![
        ("one more key past the limit", json!({ "extra": "x" }), 400),
        (
            "several more keys past the limit",
            json!({ "extra-1": "x", "extra-2": "y" }),
            400,
        ),
        (
            "overwriting a key at the limit does not grow the resource",
            json!({ "key-0": "overwritten" }),
            200,
        ),
    ];

    for (case, added, expected_status) in rows {
        let tagged = emulator.tag_resource(&arn, &added).await;
        assert_eq!(
            tagged.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from TagResource, got {} {}",
            tagged.status, tagged.body
        );
        if expected_status == 400 {
            assert_eq!(
                tagged.error_code.as_deref(),
                Some("ValidationException"),
                "case {case:?}: expected a ValidationException, got {:?} {}",
                tagged.error_code,
                tagged.body
            );
        }

        let listed = emulator.list_tags(&arn).await;
        let tags = listed
            .member("tags")
            .as_object()
            .unwrap_or_else(|| panic!("case {case:?}: tags is not a map in {}", listed.body));
        assert_eq!(
            tags.len(),
            MAX_TAGS,
            "case {case:?}: the resource must still hold exactly {MAX_TAGS} tags, got {}",
            tags.len()
        );
        if expected_status == 400 {
            assert_eq!(
                listed.member("tags"),
                &full,
                "case {case:?}: a refused tagging call must change nothing, got {}",
                listed.body
            );
        } else {
            assert_eq!(
                tags.get("key-0").and_then(Value::as_str),
                Some("overwritten"),
                "case {case:?}: the overwritten value must be observable, got {}",
                listed.body
            );
        }
    }
}

/// A `resourceArn` the store holds no resource for is a 404, and one that is not an ARN of
/// this service is a 400 — identically for the read, for the merge and for the removal.
#[tokio::test]
async fn an_unknown_or_foreign_resource_arn_is_refused_by_every_tagging_operation() {
    let emulator = Emulator::new();
    let (definition_id, definition_arn) = emulator
        .create_definition("refusal-suite", &json!({ "team": "iot" }))
        .await;
    let run_arn = emulator
        .start_run("refusal", &definition_id, &json!({ "team": "iot" }))
        .await;
    let known_run_id = run_arn
        .rsplit('/')
        .next()
        .expect("the run ARN ends in the run id")
        .to_string();

    // (case, resourceArn, expected status, expected error code)
    let rows: Vec<(String, String, u16, &str)> = vec![
        (
            "a suite definition the store does not hold".to_string(),
            format!(
                "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suitedefinition/\
                 11111111-2222-3333-4444-555555555555"
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a suite run the store does not hold".to_string(),
            format!(
                "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/{definition_id}/\
                 11111111-2222-3333-4444-555555555555"
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a known run id under another definition".to_string(),
            format!(
                "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/\
                 11111111-2222-3333-4444-555555555555/{known_run_id}"
            ),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the right id in another region".to_string(),
            definition_arn.replace(REGION, "eu-central-1"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "the right id in another account".to_string(),
            definition_arn.replace("000000000000", "111111111111"),
            404,
            "ResourceNotFoundException",
        ),
        (
            "not an ARN at all".to_string(),
            "this-value-is-not-an-arn-at-all".to_string(),
            400,
            "ValidationException",
        ),
        (
            "another service's ARN".to_string(),
            format!("arn:aws:iot:{REGION}:000000000000:thing/device-under-test"),
            400,
            "ValidationException",
        ),
        (
            "a resource type this service does not mint".to_string(),
            format!("arn:aws:iotdeviceadvisor:{REGION}:000000000000:suite/{definition_id}"),
            400,
            "ValidationException",
        ),
        (
            "a suite run ARN missing its run id".to_string(),
            format!("arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/{definition_id}"),
            400,
            "ValidationException",
        ),
        (
            "an ARN with an empty account".to_string(),
            format!("arn:aws:iotdeviceadvisor:{REGION}::suitedefinition/{definition_id}"),
            400,
            "ValidationException",
        ),
    ];

    for (case, arn, expected_status, expected_code) in rows {
        for (operation, answer) in [
            ("ListTagsForResource", emulator.list_tags(&arn).await),
            (
                "TagResource",
                emulator.tag_resource(&arn, &json!({ "env": "test" })).await,
            ),
            (
                "UntagResource",
                emulator.untag_resource(&arn, &["env"]).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (expected_status, Some(expected_code)),
                "case {case:?} on {operation}: expected HTTP {expected_status} \
                 {expected_code}, got {} {:?} {}",
                answer.status,
                answer.error_code,
                answer.body
            );
        }
    }

    // A refused call wrote nothing: only the two real resources carry a tag row.
    assert_eq!(
        emulator.stored_tag_rows(),
        2,
        "case \"refused calls\": a refused tagging call must not create a tag row, got {}",
        emulator.stored_tag_rows()
    );
}

/// The one message a refusal carries when the resource the request NAMED does not exist:
/// `Unable to find resource with <member>: <value>`, with `<member>` the input member that
/// identified the missing resource and `<value>` the value the caller sent.
fn not_found_message(member: &str, value: &str) -> String {
    format!("Unable to find resource with {member}: {value}")
}

/// Every tagging refusal of a resource the store does not hold words it with that single
/// template, keyed on the member the request used (`resourceArn`) — over rows differing in
/// resource KIND (suite definition, suite run), in WHY the resource is missing (never
/// created, the right id under another definition, another region, another account) and in
/// the LENGTH of the id the ARN carries. The last case shows the template is the service's
/// and not the tagging surface's: an id-keyed read of a missing resource words it the same
/// way, with the member THAT request named.
#[tokio::test]
async fn every_tagging_refusal_carries_the_services_single_not_found_template() {
    let emulator = Emulator::new();
    let (definition_id, definition_arn) = emulator
        .create_definition("template-suite", &json!({ "team": "iot" }))
        .await;
    let run_arn = emulator
        .start_run("template", &definition_id, &json!({ "team": "iot" }))
        .await;
    let known_run_id = run_arn
        .rsplit('/')
        .next()
        .expect("the run ARN ends in the run id")
        .to_string();

    // `UUID` is `len 12..36` (source: `smithy_query(query="shape",shape="UUID")`), so a
    // 36-character id and one at the shortest legal length are both rows below.
    let never_created = "11111111-2222-3333-4444-555555555555";
    let shortest = "abcdefghijkl";

    // (case, the resourceArn the call names)
    let rows: Vec<(String, String)> = vec![
        (
            "a suite definition the store does not hold".to_string(),
            format!(
                "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suitedefinition/{never_created}"
            ),
        ),
        (
            "a definition id at the shortest legal length".to_string(),
            format!("arn:aws:iotdeviceadvisor:{REGION}:000000000000:suitedefinition/{shortest}"),
        ),
        (
            "a suite run the store does not hold".to_string(),
            format!(
                "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/{definition_id}/\
                 {never_created}"
            ),
        ),
        (
            "a known run id under another definition".to_string(),
            format!(
                "arn:aws:iotdeviceadvisor:{REGION}:000000000000:suiterun/{never_created}/\
                 {known_run_id}"
            ),
        ),
        (
            "the right definition in another region".to_string(),
            definition_arn.replace(REGION, "eu-central-1"),
        ),
        (
            "the right definition in another account".to_string(),
            definition_arn.replace("000000000000", "111111111111"),
        ),
        (
            "the right run in another region".to_string(),
            run_arn.replace(REGION, "ap-southeast-2"),
        ),
    ];

    for (case, arn) in rows {
        let expected = not_found_message("resourceArn", &arn);
        for (operation, answer) in [
            ("ListTagsForResource", emulator.list_tags(&arn).await),
            (
                "TagResource",
                emulator.tag_resource(&arn, &json!({ "env": "test" })).await,
            ),
            (
                "UntagResource",
                emulator.untag_resource(&arn, &["env"]).await,
            ),
        ] {
            assert_eq!(
                (answer.status, answer.error_code.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {case:?} on {operation}: expected HTTP 404 ResourceNotFoundException, \
                 got {} {:?} {}",
                answer.status,
                answer.error_code,
                answer.body
            );
            assert_eq!(
                answer.member("message").as_str(),
                Some(expected.as_str()),
                "case {case:?} on {operation}: expected message {expected:?}, got {}",
                answer.body
            );
        }
    }

    // Same template, different member: the rule belongs to the service, not to one surface.
    let read = emulator
        .call("GET", &format!("/suiteDefinitions/{never_created}"), None)
        .await;
    let expected = not_found_message("suiteDefinitionId", never_created);
    assert_eq!(
        (read.status, read.error_code.as_deref()),
        (404, Some("ResourceNotFoundException")),
        "case \"an id-keyed read of a missing definition\": expected HTTP 404 \
         ResourceNotFoundException, got {} {:?} {}",
        read.status,
        read.error_code,
        read.body
    );
    assert_eq!(
        read.member("message").as_str(),
        Some(expected.as_str()),
        "case \"an id-keyed read of a missing definition\": expected message {expected:?}, \
         got {}",
        read.body
    );
}

/// Reading tags changes nothing a later caller sees: the answer repeats, and listing an
/// untagged resource does not give it a tag row.
#[tokio::test]
async fn reading_tags_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    for (index, (case, tags)) in tag_rows().into_iter().enumerate() {
        let (_, arn) = emulator
            .create_definition(&format!("read-suite-{index}"), &tags)
            .await;
        let rows_before = emulator.stored_tag_rows();

        let first = emulator.list_tags(&arn).await;
        let second = emulator.list_tags(&arn).await;
        assert_eq!(
            first.member("tags"),
            second.member("tags"),
            "case {case:?}: two reads must answer the same tags, got {} then {}",
            first.body,
            second.body
        );
        assert_eq!(
            emulator.stored_tag_rows(),
            rows_before,
            "case {case:?}: a read must not add or drop a tag row, got {} (was {rows_before})",
            emulator.stored_tag_rows()
        );
    }
}
