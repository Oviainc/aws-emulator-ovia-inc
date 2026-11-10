// SPDX-License-Identifier: CC0-1.0

//! The tagging surface over a CODEGEN JOB: `ListTagsForResource`, `TagResource` and
//! `UntagResource` on the ARN of a job `StartCodegenJob` started.
//!
//! Every assertion goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes.
//!
//! The rule under test: EVERY resource kind of this service whose own record holds a tag
//! set is reached by the tagging operations through the documented ARN form
//! `arn:<partition>:amplifyuibuilder:<region>:<account>:app/<appId>/environment/<environmentName>/<collection>/<id>`,
//! whose `<collection>` is the element that kind's own REST URIs use. The codegen job is
//! one of those kinds — `smithy_query(query="shape", shape="CodegenJob")` targets `tags` at
//! `Tags` ("One or more key-value pairs to use when tagging the code generation job") and
//! `smithy_query(query="shape", shape="CodegenJobResource")` gives the ARN template
//! `app/{appId}/environment/{environmentName}/codegen-jobs/{id}` — so:
//!
//! 1. `ListTagsForResource` reports the tag set the start stored, and `GetCodegenJob`
//!    reports the same one: there is a single stored set, so the two readers of one job can
//!    never disagree about what it carries or about whether it exists.
//! 2. `TagResource` merges into that set and `UntagResource` removes exactly the named
//!    keys, and both readers read back what either wrote.
//! 3. Tagging a job is not an UPDATE of it: nothing else about the record moves.
//! 4. A `codegen-jobs` ARN that names no job this store holds is refused with HTTP 404 and
//!    this service's one not-found literal, and changes nothing.
//!
//! `resource_tagging.rs` owns the surface's rules over every kind at once; this suite is the
//! codegen job's own end-to-end coverage, including the members a tag write must leave
//! alone.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use serde_json::{json, Map, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service: it is what routes a request here.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment a case addresses unless it is about addressing another
/// one.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// The region and account elements the ARNs built here carry: the region the requests are
/// signed for, and the one account id a generated emulator presents
/// (`engine::emulator_account_id`).
const REGION: &str = "eu-west-1";
const ACCOUNT: &str = "000000000000";

/// The collection element of the codegen job's REST URIs and of its documented ARN form
/// (`/app/{appId}/environment/{environmentName}/codegen-jobs`, see this crate's
/// `OPERATIONS.md`).
const CODEGEN_JOBS: &str = "codegen-jobs";

/// The message of a not-found refusal — this service's one literal for an identifier that
/// names no resource.
const NOT_FOUND_MESSAGE: &str = "The item specified was not found";

/// The published bounds of a tag key and a tag value:
/// `smithy_query(query="shape", shape="Tags")` gives `TagKey` `@length(min: 1, max: 128)`
/// and `TagValue` `@length(min: 1, max: 256)`.
const TAG_KEY_MAX_LENGTH: usize = 128;
const TAG_VALUE_MAX_LENGTH: usize = 256;

/// A wait longer than the span the emulator takes to make a started job's terminal status
/// due, so a job read after it reports a status that then never moves again.
const PAST_THE_DUE_INSTANT: Duration = Duration::from_millis(800);

/// A one-entry tag set, for a case whose key is computed rather than written out.
fn one_entry(key: &str, value: &str) -> Value {
    let mut map = Map::new();
    map.insert(key.to_string(), json!(value));
    Value::Object(map)
}

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

impl Answer {
    /// The job id the answer carries, or the empty string. `StartCodegenJob` and
    /// `GetCodegenJob` bind the `CodegenJob` to `@httpPayload`, so the body IS the job.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The `message` the answer carries, in either spelling a JSON error body uses.
    fn message(&self) -> &str {
        ["message", "Message"]
            .iter()
            .find_map(|member| self.body[*member].as_str())
            .unwrap_or_default()
    }

    /// The tag set the answer reports: the `tags` member as the wire carries it.
    /// `Value::Null` here is a MISSING member, which is a disagreement with
    /// `ListTagsForResource` rather than the empty set.
    fn tags(&self) -> Value {
        self.body.get("tags").cloned().unwrap_or(Value::Null)
    }

    /// The body with its `tags` member dropped: what a tag write must leave untouched.
    fn without_tags(&self) -> Value {
        let mut body = self.body.clone();
        if let Some(object) = body.as_object_mut() {
            object.remove("tags");
        }
        body
    }
}

async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "amplifyuibuilder.eu-west-1.amazonaws.com")
        .header("authorization", SIGNED_FOR_AMPLIFYUIBUILDER);
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

/// Percent-encode one value the way an SDK encodes a non-greedy `@httpLabel`, so a whole
/// ARN is ONE path segment.
fn encoded(value: &str) -> String {
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

/// The ARN of one resource of this service as the emulator's own account and region name it.
fn arn(collection: &str, app_id: &str, environment: &str, id: &str) -> String {
    format!(
        "arn:aws:amplifyuibuilder:{REGION}:{ACCOUNT}:app/{app_id}/environment/\
         {environment}/{collection}/{id}"
    )
}

async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", encoded(resource_arn)), None).await
}

async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &Value) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", encoded(resource_arn)),
        Some(json!({ "tags": tags })),
    )
    .await
}

async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", encoded(key)))
        .collect::<Vec<String>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{query}", encoded(resource_arn)),
        None,
    )
    .await
}

/// Start a codegen job in one app environment, with the tag set the case supplies.
/// `StartCodegenJobData` requires only the render config.
async fn start_job(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    tags: Option<&Value>,
) -> Answer {
    let mut data = json!({"renderConfig": {"react": {}}});
    if let Some(tags) = tags {
        data["tags"] = tags.clone();
    }
    call(
        router,
        "POST",
        &format!("/app/{app_id}/environment/{environment}/{CODEGEN_JOBS}"),
        Some(data),
    )
    .await
}

async fn get_job(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    id: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("/app/{app_id}/environment/{environment}/{CODEGEN_JOBS}/{id}"),
        None,
    )
    .await
}

/// Both readers of one job's tag set report `expected`: the tagging surface and
/// `GetCodegenJob`. One stored set, so the two can never disagree.
async fn assert_tags(case: &str, router: &MultiServiceRouter, id: &str, expected: &Value) {
    let listed = list_tags(router, &arn(CODEGEN_JOBS, APP_ID, ENVIRONMENT, id)).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource must answer HTTP 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        &listed.tags(),
        expected,
        "case {case:?}: ListTagsForResource must report {expected:?}, got {:?}",
        listed.tags()
    );

    let read = get_job(router, APP_ID, ENVIRONMENT, id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: the job must still read back, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        &read.tags(),
        expected,
        "case {case:?}: GetCodegenJob must report {expected:?}, got {:?}",
        read.tags()
    );
}

/// The tag sets a start supplies, differing along what the rule generalises over: how many
/// pairs, the form of a key and of a value, each published length bound, and whether the
/// start names `tags` at all.
fn tag_set_rows() -> Vec<(&'static str, Option<Value>)> {
    vec![
        ("no tags member at all", None),
        ("an empty tag set", Some(json!({}))),
        ("one pair", Some(json!({"team": "ui"}))),
        (
            "several pairs",
            Some(json!({"team": "ui", "owner": "studio", "tier": "free"})),
        ),
        (
            "a key and a value of one character, the shortest the model admits",
            Some(json!({"k": "v"})),
        ),
        (
            "a key and a value at the longest length the model admits",
            Some(one_entry(
                &"k".repeat(TAG_KEY_MAX_LENGTH),
                &"v".repeat(TAG_VALUE_MAX_LENGTH),
            )),
        ),
        (
            "keys carrying the punctuation the key pattern admits",
            Some(json!({
                "cost-center": "17",
                "path/to:thing": "yes",
                "team.name_v2": "studio",
            })),
        ),
        (
            "values carrying spaces, punctuation and non-ASCII text",
            Some(json!({"note": "Grüße, zum Test — 100%"})),
        ),
    ]
}

/// The tag set a start persisted is what the tagging surface reports — the empty map when
/// the start supplied none, since `tags` is `@required` on `ListTagsForResourceResponse`.
#[tokio::test]
async fn the_tagging_surface_reports_the_tag_set_a_codegen_job_start_persisted() {
    let router = emulator();

    for (case, tags) in tag_set_rows() {
        let started = start_job(&router, APP_ID, ENVIRONMENT, tags.as_ref()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: the start must succeed, got {} with {:?}",
            started.status, started.body
        );
        let expected = tags.unwrap_or_else(|| json!({}));
        assert_eq!(
            started.tags(),
            expected,
            "case {case:?}: the start must report {expected:?}, got {:?}",
            started.tags()
        );
        assert_tags(case, &router, &started.id(), &expected).await;
    }
}

/// `TagResource` merges into a job's stored tag set, `UntagResource` removes exactly the
/// named keys and is idempotent, and both readers of the job read back what either wrote.
#[tokio::test]
async fn a_tag_write_on_a_codegen_job_merges_and_a_removal_removes_the_named_keys() {
    let router = emulator();

    let started = start_job(
        &router,
        APP_ID,
        ENVIRONMENT,
        Some(&json!({"team": "ui", "owner": "studio"})),
    )
    .await;
    assert_eq!(
        started.status, 200,
        "the start must succeed, got {} with {:?}",
        started.status, started.body
    );
    let id = started.id();
    let resource_arn = arn(CODEGEN_JOBS, APP_ID, ENVIRONMENT, &id);
    let mut expected: Map<String, Value> = json!({"team": "ui", "owner": "studio"})
        .as_object()
        .cloned()
        .expect("the base tag set is an object");

    // The writes differ along what the rule generalises over: a key the job does not carry,
    // a key it does (whose value is replaced), several at once, and a pair it already
    // carries (which changes nothing).
    let writes: Vec<(&str, Value)> = vec![
        ("a key the job does not carry", json!({"env": "test"})),
        ("a key it carries takes the new value", json!({"env": "prod"})),
        (
            "several keys at once",
            json!({"cost-center": "17", "tier": "free"}),
        ),
        ("a pair it already carries", json!({"team": "ui"})),
    ];
    for (case, written) in writes {
        let reply = tag(&router, &resource_arn, &written).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: TagResource must answer HTTP 200, got {} with {:?}",
            reply.status, reply.body
        );
        for (key, value) in written.as_object().expect("the written tags are an object") {
            expected.insert(key.clone(), value.clone());
        }
        assert_tags(case, &router, &id, &Value::Object(expected.clone())).await;
    }

    // The removals differ the same way: one key, several, a key the job does not carry, and
    // every remaining key — each one run TWICE, since removing a key that is already gone
    // is nothing to remove.
    let removals: Vec<(&str, Vec<String>)> = vec![
        ("one key", vec!["owner".to_string()]),
        (
            "several keys",
            vec!["cost-center".to_string(), "tier".to_string()],
        ),
        (
            "a key the job does not carry",
            vec!["nothing-carries-this".to_string()],
        ),
        (
            "every remaining key",
            vec!["env".to_string(), "team".to_string()],
        ),
    ];
    for (case, keys) in removals {
        for key in &keys {
            expected.remove(key);
        }
        for round in 1..=2 {
            let reply = untag(&router, &resource_arn, &keys).await;
            assert_eq!(
                reply.status, 200,
                "case {case:?} (round {round}): UntagResource must answer HTTP 200, got {} with \
                 {:?}",
                reply.status, reply.body
            );
            assert_tags(
                &format!("{case} (round {round})"),
                &router,
                &id,
                &Value::Object(expected.clone()),
            )
            .await;
        }
    }

    assert!(
        expected.is_empty(),
        "the removals must leave no tag, got {expected:?}"
    );
}

/// Tagging a job is not an UPDATE of it: only the tag set moves. Asserted on a job the
/// clock has already carried to the status it never leaves, so every other member is
/// frozen and a difference can only come from the tag write.
#[tokio::test]
async fn tagging_a_codegen_job_changes_nothing_but_its_tag_set() {
    let router = emulator();

    let started = start_job(&router, APP_ID, ENVIRONMENT, Some(&json!({"team": "ui"}))).await;
    assert_eq!(
        started.status, 200,
        "the start must succeed, got {} with {:?}",
        started.status, started.body
    );
    let id = started.id();
    let resource_arn = arn(CODEGEN_JOBS, APP_ID, ENVIRONMENT, &id);

    tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
    let settled = get_job(&router, APP_ID, ENVIRONMENT, &id).await;
    assert_eq!(
        settled.status, 200,
        "the settled job must read back, got {} with {:?}",
        settled.status, settled.body
    );

    // Each row writes the tag set and then checks the rest of the record against the
    // reading taken before any of them ran.
    let changes: Vec<(&str, Value)> = vec![
        ("a tag write", json!({"env": "test"})),
        ("a second tag write", json!({"env": "prod", "tier": "free"})),
    ];
    for (case, written) in changes {
        let reply = tag(&router, &resource_arn, &written).await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: TagResource must answer HTTP 200, got {} with {:?}",
            reply.status, reply.body
        );
        let read = get_job(&router, APP_ID, ENVIRONMENT, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the job must read back, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.without_tags(),
            settled.without_tags(),
            "case {case:?}: every member but tags must be unchanged, got {:?}",
            read.without_tags()
        );
    }

    let removed = untag(&router, &resource_arn, &["env".to_string()]).await;
    assert_eq!(
        removed.status, 200,
        "UntagResource must answer HTTP 200, got {} with {:?}",
        removed.status, removed.body
    );
    let read = get_job(&router, APP_ID, ENVIRONMENT, &id).await;
    assert_eq!(
        read.without_tags(),
        settled.without_tags(),
        "a removal must leave every member but tags unchanged, got {:?}",
        read.without_tags()
    );
    assert_tags(
        "after a removal",
        &router,
        &id,
        &json!({"team": "ui", "tier": "free"}),
    )
    .await;
}

/// A `codegen-jobs` ARN that names no job this store holds is absent on every tagging
/// operation: HTTP 404 with this service's one not-found literal, and nothing written.
#[tokio::test]
async fn a_codegen_job_arn_naming_no_job_is_absent_on_every_tagging_operation() {
    let router = emulator();

    let live = start_job(&router, APP_ID, ENVIRONMENT, Some(&json!({"team": "ui"}))).await;
    assert_eq!(
        live.status, 200,
        "the start must succeed, got {} with {:?}",
        live.status, live.body
    );
    let live_id = live.id();

    // The rows differ along WHY the ARN names nothing: the id, the app, the environment,
    // and the collection the live id is addressed through.
    let absent: Vec<(&str, String)> = vec![
        (
            "an id no job of this app environment holds",
            arn(
                CODEGEN_JOBS,
                APP_ID,
                ENVIRONMENT,
                "11111111-1111-4111-8111-111111111111",
            ),
        ),
        (
            "a live job id under another app",
            arn(CODEGEN_JOBS, "d9999zzzz", ENVIRONMENT, &live_id),
        ),
        (
            "a live job id under another environment",
            arn(CODEGEN_JOBS, APP_ID, "production", &live_id),
        ),
        (
            "a live job id under another collection of this service",
            arn("components", APP_ID, ENVIRONMENT, &live_id),
        ),
        (
            "an ARN with an empty job id",
            arn(CODEGEN_JOBS, APP_ID, ENVIRONMENT, ""),
        ),
    ];

    for (case, resource_arn) in &absent {
        for (operation, answer) in [
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
            (
                "TagResource",
                tag(&router, resource_arn, &json!({"env": "prod"})).await,
            ),
            (
                "UntagResource",
                untag(&router, resource_arn, &["team".to_string()]).await,
            ),
        ] {
            let case = format!("{case} on {operation}");
            assert_eq!(
                answer.status, 404,
                "case {case:?}: an ARN naming no job must answer HTTP 404, got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.message(),
                NOT_FOUND_MESSAGE,
                "case {case:?}: expected the message {NOT_FOUND_MESSAGE:?}, got {:?}",
                answer.message()
            );
        }
    }

    // None of those refusals touched the live job's stored tag set.
    assert_tags(
        "after the refusals, the live job",
        &router,
        &live_id,
        &json!({"team": "ui"}),
    )
    .await;
}
