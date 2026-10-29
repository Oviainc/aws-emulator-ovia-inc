// SPDX-License-Identifier: CC0-1.0

//! The "last modified" instant a policy reports, across the writes that change
//! its tags.
//!
//! The rule under test: `LifecyclePolicy$DateModified` is "the local date and
//! time when the lifecycle policy was last modified", so EVERY accepted write of
//! a policy record stamps it with the instant of that write — whichever member
//! of the projection a read reports the write changed — while `DateCreated`
//! stays where the create put it, and a REFUSED request stamps nothing.
//!
//! The rows are the tag writes: `TagResource` adding, replacing and writing
//! several keys at once, and `UntagResource` removing one, several, and a key
//! the policy does not hold. The recorded case (one key added through
//! `TagResource`) is one row of it. `UpdateLifecyclePolicy` is the same rule on
//! another member and is pinned in `lifecycle_policy_crud.rs`.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_dlm_dlm_20180112::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An `ExecutionRoleArn` satisfying `^arn:aws(-[a-z]{1,3}){0,2}:iam::\d+:role/`.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AWSDataLifecycleManagerDefaultRole";

/// The region every call here is signed for.
const REGION: &str = "us-east-1";

/// The status each tag write answers on success, from the crate's
/// `OPERATIONS.md` entry for the two operations.
const TAG_RESOURCE_STATUS: u16 = 204;
const UNTAG_RESOURCE_STATUS: u16 = 200;

/// The modelled refusal for a request the service will not accept, and its
/// status (`InvalidRequestException (HTTP 400)`).
const INVALID_REQUEST_STATUS: u16 = 400;
/// `ResourceNotFoundException (HTTP 404)`, for an ARN naming no policy.
const RESOURCE_NOT_FOUND_STATUS: u16 = 404;

/// How long the test waits between the create and the write under test, so the
/// two instants are distinguishable at whatever precision the `date-time`
/// format renders. One wait covers every row: the rows are created before it
/// and written after it.
const DISTINGUISHABLE: Duration = Duration::from_millis(1_050);

/// One emulator: a router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

/// A SigV4 credential scope naming this service in `REGION` — what makes a call
/// a call to that region's endpoint.
fn signed_for(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{region}/dlm/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("dlm.{REGION}.amazonaws.com"))
        .header("authorization", signed_for(REGION));
    if !payload.is_empty() {
        builder = builder.header("content-type", "application/json");
    }
    let request = builder
        .body(Bytes::from(payload))
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            panic!(
                "{method} {uri} answered a body that is not JSON ({error}): {:?}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    Answer { status, body }
}

/// `/tags/{ResourceArn}` for `arn`. The label is a single path segment, so the
/// ARN's `/` is percent-encoded.
fn tags_uri(arn: &str) -> String {
    format!("/tags/{}", arn.replace('/', "%2F"))
}

/// A custom EBS snapshot policy's `PolicyDetails`, as a client sends it.
fn snapshot_details() -> Value {
    json!({
        "PolicyType": "EBS_SNAPSHOT_MANAGEMENT",
        "ResourceTypes": ["VOLUME"],
        "TargetTags": [{"Key": "backup", "Value": "daily"}],
        "Schedules": [{
            "Name": "daily",
            "CreateRule": {"Interval": 24, "IntervalUnit": "HOURS"},
            "RetainRule": {"Count": 7},
        }],
    })
}

/// A comparable key for a `DateCreated` / `DateModified` timestamp, whichever of
/// the two representations the codec renders it in (an epoch-seconds number or
/// an RFC-3339 string): a number is padded to a fixed width and a string's
/// fractional-second part to nine digits, after which either sorts
/// lexicographically in the order the instants fall.
fn timestamp_key(value: &Value) -> String {
    if let Some(seconds) = value.as_f64() {
        return format!("{seconds:030.9}");
    }
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("a timestamp must be a number or a string, got {value:?}"));
    let body = text.trim_end_matches('Z');
    let (whole, fraction) = body.split_once('.').unwrap_or((body, ""));
    format!("{whole}.{fraction:0<9}")
}

/// The policy `GetLifecyclePolicy` reports for `id`.
async fn policy(router: &MultiServiceRouter, id: &str) -> Value {
    let read = call(router, "GET", &format!("/policies/{id}"), None).await;
    assert_eq!(
        read.status, 200,
        "case {id:?}: expected HTTP 200 from GetLifecyclePolicy, got {} with {:?}",
        read.status, read.body
    );
    read.body["Policy"].clone()
}

fn at(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// Create a policy carrying `tags`, answering its id and its stored `PolicyArn`.
async fn create(router: &MultiServiceRouter, case: &str, tags: Value) -> (String, String) {
    let created = call(
        router,
        "POST",
        "/policies",
        Some(json!({
            "ExecutionRoleArn": ROLE_ARN,
            "Description": "tag timestamp subject",
            "State": "ENABLED",
            "PolicyDetails": snapshot_details(),
            "Tags": tags,
        })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: expected HTTP 200 from CreateLifecyclePolicy, got {} with {:?}",
        created.status, created.body
    );
    let id = created.body["PolicyId"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: a create must report a PolicyId, got {:?}",
                created.body
            )
        })
        .to_string();
    let arn = at(&policy(router, &id).await, "/PolicyArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: a policy must report its ARN"))
        .to_string();
    (id, arn)
}

/// One accepted tag write: what the policy is created holding, the call that
/// changes its tags, and the tag set a read must report afterwards.
struct Row {
    case: &'static str,
    created_tags: Value,
    method: &'static str,
    /// Appended to `/tags/{ResourceArn}` (`UntagResource` names its keys there).
    query: &'static str,
    body: Option<Value>,
    expected_tags: Value,
    expected_status: u16,
}

/// The accepted tag writes, differing along which operation writes, how many
/// keys it names, and whether those keys were already held.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "TagResource adds a key the policy does not hold",
            created_tags: json!({"team": "backups"}),
            method: "POST",
            query: "",
            body: Some(json!({"Tags": {"env": "prod"}})),
            expected_tags: json!({"team": "backups", "env": "prod"}),
            expected_status: TAG_RESOURCE_STATUS,
        },
        Row {
            case: "TagResource replaces the value of a key the policy holds",
            created_tags: json!({"env": "dev"}),
            method: "POST",
            query: "",
            body: Some(json!({"Tags": {"env": "prod"}})),
            expected_tags: json!({"env": "prod"}),
            expected_status: TAG_RESOURCE_STATUS,
        },
        Row {
            case: "TagResource writes several keys at once",
            created_tags: json!({"team": "backups"}),
            method: "POST",
            query: "",
            body: Some(json!({"Tags": {"env": "prod", "owner": "platform", "tier": "gold"}})),
            expected_tags: json!({
                "team": "backups",
                "env": "prod",
                "owner": "platform",
                "tier": "gold",
            }),
            expected_status: TAG_RESOURCE_STATUS,
        },
        Row {
            // Accepted, and a write of the pair already held: the instant moves
            // because the service accepted a write of the record, the same way
            // an update restating a value the policy already carries moves it.
            case: "TagResource writes the tag the policy already holds",
            created_tags: json!({"env": "prod"}),
            method: "POST",
            query: "",
            body: Some(json!({"Tags": {"env": "prod"}})),
            expected_tags: json!({"env": "prod"}),
            expected_status: TAG_RESOURCE_STATUS,
        },
        Row {
            case: "UntagResource removes a key the policy holds",
            created_tags: json!({"env": "prod", "team": "backups"}),
            method: "DELETE",
            query: "?tagKeys=env",
            body: None,
            expected_tags: json!({"team": "backups"}),
            expected_status: UNTAG_RESOURCE_STATUS,
        },
        Row {
            case: "UntagResource removes several keys at once",
            created_tags: json!({"env": "prod", "team": "backups", "tier": "gold"}),
            method: "DELETE",
            query: "?tagKeys=env&tagKeys=team",
            body: None,
            expected_tags: json!({"tier": "gold"}),
            expected_status: UNTAG_RESOURCE_STATUS,
        },
        Row {
            // Also accepted: the request asks for that key to be absent, and
            // afterwards it is.
            case: "UntagResource removes a key the policy does not hold",
            created_tags: json!({"team": "backups"}),
            method: "DELETE",
            query: "?tagKeys=absent",
            body: None,
            expected_tags: json!({"team": "backups"}),
            expected_status: UNTAG_RESOURCE_STATUS,
        },
    ]
}

/// RULE — an accepted tag write stamps `DateModified` with the instant of that
/// write, because the tags are part of the policy a read reports; `DateCreated`
/// does not move, and no policy the call did not name is stamped.
#[tokio::test]
async fn every_accepted_tag_write_advances_date_modified() {
    let router = emulator();
    let rows = rows();

    // Every subject is created BEFORE the single wait, so one wait makes each
    // row's create and its write fall at distinguishable instants.
    let mut subjects = Vec::new();
    for row in &rows {
        let (id, arn) = create(&router, row.case, row.created_tags.clone()).await;
        let before = policy(&router, &id).await;
        subjects.push((id, arn, before));
    }
    // A control policy no tag call names, to show the stamp reaches one record.
    let (control_id, _) = create(&router, "the control policy", json!({"team": "backups"})).await;
    let control_before = policy(&router, &control_id).await;

    tokio::time::sleep(DISTINGUISHABLE).await;

    for (row, (id, arn, before)) in rows.iter().zip(&subjects) {
        let case = row.case;
        let written = call(
            &router,
            row.method,
            &format!("{}{}", tags_uri(arn), row.query),
            row.body.clone(),
        )
        .await;
        assert_eq!(
            written.status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {} with {:?}",
            row.expected_status, written.status, written.body
        );

        let after = policy(&router, id).await;
        assert_eq!(
            at(&after, "/Tags"),
            row.expected_tags,
            "case {case:?}: expected the tags {:?}, got {:?}",
            row.expected_tags,
            at(&after, "/Tags")
        );
        let modified_before = timestamp_key(&at(before, "/DateModified"));
        let modified_after = timestamp_key(&at(&after, "/DateModified"));
        assert!(
            modified_after > modified_before,
            "case {case:?}: DateModified must advance past {modified_before:?}, got \
             {modified_after:?}"
        );
        assert_eq!(
            at(&after, "/DateCreated"),
            at(before, "/DateCreated"),
            "case {case:?}: DateCreated must not move, got {:?}",
            at(&after, "/DateCreated")
        );
        assert!(
            timestamp_key(&at(&after, "/DateModified"))
                >= timestamp_key(&at(&after, "/DateCreated")),
            "case {case:?}: DateModified {:?} must not precede DateCreated {:?}",
            at(&after, "/DateModified"),
            at(&after, "/DateCreated")
        );
    }

    let control_after = policy(&router, &control_id).await;
    assert_eq!(
        at(&control_after, "/DateModified"),
        at(&control_before, "/DateModified"),
        "case {:?}: a policy no tag call named keeps the instant it had, got {:?}",
        "the control policy",
        at(&control_after, "/DateModified")
    );
}

/// RULE — the other half of it: a REFUSED request changes nothing, so the
/// instant a read reports as last modified is the one the policy already had.
///
/// The rows differ along WHICH refusal: a tag key under the reserved prefix a
/// caller may not set, and an ARN that names no policy in the calling region.
#[tokio::test]
async fn a_refused_tag_write_leaves_date_modified_alone() {
    let router = emulator();
    let (id, arn) = create(&router, "the refusal subject", json!({"team": "backups"})).await;
    let before = policy(&router, &id).await;
    let unknown_arn = format!(
        "arn:aws:dlm:{REGION}:{}:policy/policy-ffffffffffffffff",
        engine::emulator_account_id()
    );

    tokio::time::sleep(DISTINGUISHABLE).await;

    let refusals: Vec<(&str, &str, String, Option<Value>, u16)> = vec![
        (
            "TagResource with a key under the reserved prefix",
            "POST",
            tags_uri(&arn),
            Some(json!({"Tags": {"aws:reserved": "no"}})),
            INVALID_REQUEST_STATUS,
        ),
        (
            "TagResource on an ARN naming no policy",
            "POST",
            tags_uri(&unknown_arn),
            Some(json!({"Tags": {"env": "prod"}})),
            RESOURCE_NOT_FOUND_STATUS,
        ),
        (
            "UntagResource on an ARN naming no policy",
            "DELETE",
            format!("{}?tagKeys=team", tags_uri(&unknown_arn)),
            None,
            RESOURCE_NOT_FOUND_STATUS,
        ),
    ];

    for (case, method, uri, body, expected_status) in refusals {
        let answer = call(&router, method, &uri, body).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with {:?}",
            answer.status, answer.body
        );
        let after = policy(&router, &id).await;
        assert_eq!(
            at(&after, "/DateModified"),
            at(&before, "/DateModified"),
            "case {case:?}: a refused request modifies nothing, so DateModified must still be \
             {:?}, got {:?}",
            at(&before, "/DateModified"),
            at(&after, "/DateModified")
        );
        assert_eq!(
            at(&after, "/Tags"),
            at(&before, "/Tags"),
            "case {case:?}: a refused request must leave the tags alone, got {:?}",
            at(&after, "/Tags")
        );
    }
}
