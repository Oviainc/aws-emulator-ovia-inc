// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the Q App lifecycle: `CreateQApp`, `GetQApp`,
//! `UpdateQApp`, `ListQApps`, `StartQAppSession`, `GetQAppSession` and
//! `DeleteQApp`.
//!
//! Every test drives the service the way a client does — through the router,
//! over restJson1, with the required `instance-id` header — so what it asserts
//! is the wire behaviour and not a handler's internals.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_qapps_qappsservice::ops::types::{
    APP_REQUIRED_CAPABILITY_VALUES, APP_STATUS_VALUES, CARD_TYPE_VALUES, EXECUTION_STATUS_VALUES,
};
use service_com_amazonaws_qapps_qappsservice::state::{
    is_uuid_shaped as crate_is_uuid_shaped, AppStatus, ExecutionStatus, DEFAULT_PAGE_SIZE,
    MAX_APPS_PER_INSTANCE, MAX_APPS_TOTAL, MAX_RETAINED_VERSIONS, MAX_SESSIONS_PER_APP,
    MAX_SESSIONS_TOTAL,
};
use service_com_amazonaws_qapps_qappsservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// An instance of the Amazon Q Business application environment. Every
/// operation takes it as the required `instance-id` header, and it scopes all
/// state.
const INSTANCE: &str = "11111111-2222-4333-8444-555555555555";
/// A second instance, to show the first one's apps are not visible from it.
const OTHER_INSTANCE: &str = "99999999-8888-4777-8666-555555555555";

/// Card ids. Shaped as the model's `UUID`:
/// `^[\da-f]{8}-[\da-f]{4}-[45][\da-f]{3}-[89ABab][\da-f]{3}-[\da-f]{12}$`.
const CARD_A: &str = "aaaaaaaa-1111-4111-8111-111111111111";
const CARD_B: &str = "bbbbbbbb-2222-4222-9222-222222222222";
const CARD_C: &str = "cccccccc-3333-4333-a333-333333333333";
/// `QPluginCardInput$pluginId` is `length(min: 36, max: 36)` in the model.
const PLUGIN_ID: &str = "dddddddd-4444-4444-b444-444444444444";

/// The access key id these tests sign as: a credential the IAM Identity Center
/// portal issued to a signed-in user, which is what Q Apps serves (see
/// [`crate::identity_center`]).
const CALLER: &str = "AKIDLIFECYCLE0001";

/// The emulator under test: the production router with this one service mounted
/// on a shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // Signatures are not the subject of these tests; routing still keys on
        // the credential scope below.
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        // The lifecycle is what this suite asserts, so its caller is signed in
        // through Identity Center first; the front door that requires it is pinned
        // in `tests/all/identity_center_front_door.rs`.
        crate::identity_center::federate(&engine, CALLER);
        Self { router }
    }

    /// Send one request and return its status and JSON body (`Value::Null` for
    /// an empty body).
    async fn call(
        &self,
        method: &str,
        uri: &str,
        instance: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize the request"))
        });
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("instance-id", instance)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 \
                     Credential={CALLER}/20260101/us-east-1/qapps/aws4_request, \
                     SignedHeaders=host, Signature=deadbeef"
                ),
            )
            .body(payload)
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    async fn create(&self, instance: &str, body: Value) -> (u16, Value) {
        self.call("POST", "/apps.create", instance, Some(body)).await
    }

    async fn get(&self, instance: &str, app_id: &str) -> (u16, Value) {
        self.call("GET", &format!("/apps.get?appId={app_id}"), instance, None)
            .await
    }

    async fn get_version(&self, instance: &str, app_id: &str, version: i64) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/apps.get?appId={app_id}&appVersion={version}"),
            instance,
            None,
        )
        .await
    }

    async fn update(&self, instance: &str, body: Value) -> (u16, Value) {
        self.call("POST", "/apps.update", instance, Some(body)).await
    }

    async fn list(&self, instance: &str, query: &str) -> (u16, Value) {
        self.call("GET", &format!("/apps.list{query}"), instance, None)
            .await
    }

    async fn start_session(&self, instance: &str, body: Value) -> (u16, Value) {
        self.call("POST", "/runtime.startQAppSession", instance, Some(body))
            .await
    }

    async fn get_session(&self, instance: &str, session_id: &str) -> (u16, Value) {
        self.call(
            "GET",
            &format!("/runtime.getQAppSession?sessionId={session_id}"),
            instance,
            None,
        )
        .await
    }

    async fn delete(&self, instance: &str, app_id: &str) -> (u16, Value) {
        self.call(
            "POST",
            "/apps.delete",
            instance,
            Some(json!({ "appId": app_id })),
        )
        .await
    }
}

/// A text-input card. `type` is the model's `CardType` value for the variant.
fn text_card(id: &str, title: &str) -> Value {
    json!({ "textInput": { "id": id, "title": title, "type": "text-input" } })
}

/// A query card whose prompt is free text — it references another card when
/// that card's id appears in it.
fn query_card(id: &str, title: &str, prompt: &str, output_source: &str) -> Value {
    json!({ "qQuery": {
        "id": id,
        "title": title,
        "type": "q-query",
        "prompt": prompt,
        "outputSource": output_source,
    }})
}

fn plugin_card(id: &str, title: &str, prompt: &str) -> Value {
    json!({ "qPlugin": {
        "id": id,
        "title": title,
        "type": "q-plugin",
        "prompt": prompt,
        "pluginId": PLUGIN_ID,
    }})
}

fn file_upload_card(id: &str, title: &str) -> Value {
    json!({ "fileUpload": { "id": id, "title": title, "type": "file-upload" } })
}

fn form_input_card(id: &str, title: &str) -> Value {
    json!({ "formInput": {
        "id": id,
        "title": title,
        "type": "form-input",
        "metadata": { "schema": { "type": "object" } },
    }})
}

/// The id of a card in a returned `appDefinition`, whichever variant it is.
fn returned_card_id(card: &Value) -> &str {
    card.as_object()
        .and_then(|variant| variant.values().next())
        .and_then(|body| body["id"].as_str())
        .unwrap_or_default()
}

fn returned_card_dependencies(card: &Value) -> Vec<String> {
    card.as_object()
        .and_then(|variant| variant.values().next())
        .and_then(|body| body["dependencies"].as_array())
        .map(|deps| {
            deps.iter()
                .map(|dep| dep.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `value` matches the model's `UUID` pattern. Written out here so the
/// test checks the minted shape itself rather than trusting the handler's own
/// helper.
fn is_uuid_shaped(value: &str) -> bool {
    let parts: Vec<&str> = value.split('-').collect();
    let [a, b, c, d, e] = parts[..] else {
        return false;
    };
    let hex = |s: &str, len: usize| {
        s.len() == len && s.chars().all(|ch| ch.is_ascii_digit() || ('a'..='f').contains(&ch))
    };
    hex(a, 8)
        && hex(b, 4)
        && c.len() == 4
        && matches!(&c[0..1], "4" | "5")
        && hex(&c[1..], 3)
        && d.len() == 4
        && matches!(&d[0..1], "8" | "9" | "a" | "b" | "A" | "B")
        && hex(&d[1..], 3)
        && hex(e, 12)
}

fn error_code(body: &Value) -> &str {
    body["__type"].as_str().unwrap_or_default()
}

/// Create one app and return its id, failing the test with the body on any
/// non-200.
async fn created_app_id(emulator: &Emulator, title: &str, cards: Vec<Value>) -> String {
    let (status, body) = emulator
        .create(
            INSTANCE,
            json!({
                "title": title,
                "description": "created by the lifecycle test",
                "appDefinition": { "cards": cards },
            }),
        )
        .await;
    assert_eq!(status, 200, "create {title}: body {body}");
    body["appId"]
        .as_str()
        .unwrap_or_else(|| panic!("create {title} returned no appId: {body}"))
        .to_string()
}

/// Each row carries what the model's `UUID` pattern
/// (`^[\da-f]{8}-[\da-f]{4}-[45][\da-f]{3}-[89ABab][\da-f]{3}-[\da-f]{12}$`)
/// says about the value, and BOTH predicates — the crate's and this file's —
/// must answer it: the expectation is the pattern, not the other
/// implementation.
#[test]
fn the_uuid_predicates_answer_what_the_models_pattern_says() {
    let cases: &[(&str, &str, bool)] = &[
        ("a v4 id", "aaaaaaaa-1111-4111-8111-111111111111", true),
        (
            "a v5 id (the pattern's version nibble is [45])",
            "aaaaaaaa-1111-5111-9111-111111111111",
            true,
        ),
        ("variant nibble a", "aaaaaaaa-1111-4111-a111-111111111111", true),
        ("variant nibble b", "aaaaaaaa-1111-4111-b111-111111111111", true),
        ("digits only", "12345678-1234-4123-8123-123456789012", true),
        (
            "version nibble 3 is outside [45]",
            "aaaaaaaa-1111-3111-8111-111111111111",
            false,
        ),
        (
            "variant nibble c is outside [89ABab]",
            "aaaaaaaa-1111-4111-c111-111111111111",
            false,
        ),
        (
            "uppercase hex is outside [\\da-f]",
            "AAAAAAAA-1111-4111-8111-111111111111",
            false,
        ),
        ("a short first group", "aaaaaaa-1111-4111-8111-111111111111", false),
        ("a long last group", "aaaaaaaa-1111-4111-8111-1111111111111", false),
        ("too few groups", "aaaaaaaa-1111-4111-8111", false),
        ("not an id at all", "not-a-uuid", false),
        ("empty", "", false),
    ];
    for (case, value, expected) in cases {
        assert_eq!(
            crate_is_uuid_shaped(value),
            *expected,
            "case {case:?}: the crate's predicate on {value:?} must be {expected}, got {}",
            crate_is_uuid_shaped(value)
        );
        assert_eq!(
            is_uuid_shaped(value),
            *expected,
            "case {case:?}: this file's predicate on {value:?} must be {expected}, got {}",
            is_uuid_shaped(value)
        );
    }
}

/// Every status this crate reports is one of the model's own enum values.
#[test]
fn reported_statuses_are_values_the_model_declares() {
    // Driven out of the enums the handlers report, so a wire spelling that
    // drifts from the model fails here.
    for status in [AppStatus::Draft, AppStatus::Published, AppStatus::Deleted] {
        let wire = status.as_wire();
        assert!(
            APP_STATUS_VALUES.contains(&wire),
            "case {status:?}: expected an AppStatus value of {APP_STATUS_VALUES:?}, got {wire:?}"
        );
    }
    for status in [
        ExecutionStatus::Completed,
        ExecutionStatus::Error,
        ExecutionStatus::InProgress,
        ExecutionStatus::Waiting,
    ] {
        let wire = status.as_wire();
        assert!(
            EXECUTION_STATUS_VALUES.contains(&wire),
            "case {status:?}: expected an ExecutionStatus value of \
             {EXECUTION_STATUS_VALUES:?}, got {wire:?}"
        );
    }
}

/// Updating an app mints a version on every call, so the retained window of
/// definitions is bounded: past it the oldest version is dropped and reads of
/// it answer 404, while the newest ones stay readable.
#[tokio::test]
async fn only_a_bounded_window_of_app_versions_is_retained() {
    let emulator = Emulator::new();
    let app_id = created_app_id(&emulator, "Often revised", vec![text_card(CARD_A, "Topic")]).await;

    // One more update than the window holds, so the first version falls out.
    let updates = MAX_RETAINED_VERSIONS + 1;
    for round in 0..updates {
        let (status, body) = emulator
            .update(
                INSTANCE,
                json!({ "appId": app_id, "title": format!("Revision {round}") }),
            )
            .await;
        assert_eq!(status, 200, "update {round}: {body}");
    }

    let latest = i64::try_from(updates).expect("the update count fits") + 1;
    let oldest_retained = latest - i64::try_from(MAX_RETAINED_VERSIONS).expect("the window fits") + 1;
    let cases: Vec<(&str, i64, u16)> = vec![
        ("the version that fell out of the window", 1, 404),
        ("the oldest version still retained", oldest_retained, 200),
        ("the current version", latest, 200),
    ];
    for (name, version, expect_status) in cases {
        let (status, body) = emulator.get_version(INSTANCE, &app_id, version).await;
        assert_eq!(
            status, expect_status,
            "case {name:?} (version {version}): expected {expect_status}, got {status} {body}"
        );
    }
}

/// The store does not grow without limit: one instance holds a bounded number
/// of apps, and one app a bounded number of sessions, each refused past the
/// bound with the quota error the operation models.
#[tokio::test]
async fn growth_past_a_quota_is_refused_with_the_modelled_quota_error() {
    let emulator = Emulator::new();

    let mut last_app = String::new();
    for index in 0..MAX_APPS_PER_INSTANCE {
        let (status, body) = emulator
            .create(
                INSTANCE,
                json!({
                    "title": format!("App {index}"),
                    "appDefinition": { "cards": [text_card(CARD_A, "Topic")] },
                }),
            )
            .await;
        assert_eq!(status, 200, "app {index}: {body}");
        last_app = body["appId"].as_str().unwrap_or_default().to_string();
    }

    let (status, body) = emulator
        .create(
            INSTANCE,
            json!({
                "title": "One too many",
                "appDefinition": { "cards": [text_card(CARD_A, "Topic")] },
            }),
        )
        .await;
    assert_eq!(
        status, 402,
        "case {MAX_APPS_PER_INSTANCE} apps already held: expected 402, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {body}"
    );

    // Another instance is unaffected: the quota is per instance, like the state.
    let (status, body) = emulator
        .create(
            OTHER_INSTANCE,
            json!({
                "title": "Another tenant",
                "appDefinition": { "cards": [text_card(CARD_A, "Topic")] },
            }),
        )
        .await;
    assert_eq!(status, 200, "another instance is not at its quota: {body}");

    for index in 0..MAX_SESSIONS_PER_APP {
        let (status, body) = emulator
            .start_session(INSTANCE, json!({ "appId": last_app, "appVersion": 1 }))
            .await;
        assert_eq!(status, 200, "session {index}: {body}");
    }
    let (status, body) = emulator
        .start_session(INSTANCE, json!({ "appId": last_app, "appVersion": 1 }))
        .await;
    assert_eq!(
        status, 402,
        "case {MAX_SESSIONS_PER_APP} sessions already held: expected 402, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "expected ServiceQuotaExceededException, got {body}"
    );
}

/// The store does not grow without limit ACROSS instances either. The model's
/// `InstanceId` is an unconstrained string, so the per-instance quota alone
/// would bound nothing: what this endpoint holds in total is bounded as well,
/// and growth past that total is the same modelled quota refusal — for apps and
/// for sessions alike.
#[tokio::test]
async fn growth_across_instances_is_bounded_by_an_endpoint_total() {
    let emulator = Emulator::new();

    // Several distinct instances, each filled to its own per-instance quota,
    // until the endpoint total is reached.
    let instances = MAX_APPS_TOTAL / MAX_APPS_PER_INSTANCE;
    assert!(
        instances >= 2,
        "case apps: the endpoint total must span several instances, \
         got {MAX_APPS_TOTAL} total / {MAX_APPS_PER_INSTANCE} per instance"
    );
    let mut first_app_of_first_instance = String::new();
    for instance in 0..instances {
        let instance_id = format!("{instance:08x}-2222-4333-8444-555555555555");
        for index in 0..MAX_APPS_PER_INSTANCE {
            let (status, body) = emulator
                .create(
                    &instance_id,
                    json!({
                        "title": format!("App {index}"),
                        "appDefinition": { "cards": [text_card(CARD_A, "Topic")] },
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case instance {instance} app {index}: expected 200, got {status} {body}"
            );
            if instance == 0 && index == 0 {
                first_app_of_first_instance =
                    body["appId"].as_str().unwrap_or_default().to_string();
            }
        }
    }

    // A fresh instance, well under its OWN quota, is refused because the
    // endpoint total is what is full.
    let fresh_instance = "ffffffff-2222-4333-8444-555555555555";
    let one_too_many = json!({
        "title": "One too many",
        "appDefinition": { "cards": [text_card(CARD_A, "Topic")] },
    });
    let (status, body) = emulator.create(fresh_instance, one_too_many.clone()).await;
    assert_eq!(
        status, 402,
        "case {MAX_APPS_TOTAL} apps held across instances: expected 402, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case endpoint app total: expected ServiceQuotaExceededException, got {body}"
    );

    // The bound counts what is HELD, so deleting an app frees room again.
    let (status, body) = emulator
        .delete(
            "00000000-2222-4333-8444-555555555555",
            &first_app_of_first_instance,
        )
        .await;
    assert_eq!(status, 200, "case delete to free room: {body}");
    let (status, body) = emulator.create(fresh_instance, one_too_many).await;
    assert_eq!(
        status, 200,
        "case one app deleted: expected 200, got {status} {body}"
    );
}

/// The same for sessions: one app's sessions are bounded, and so is the number
/// of sessions this endpoint holds across every app and instance.
#[tokio::test]
async fn session_growth_across_apps_is_bounded_by_an_endpoint_total() {
    let emulator = Emulator::new();
    let apps = MAX_SESSIONS_TOTAL / MAX_SESSIONS_PER_APP;
    assert!(
        apps >= 2,
        "case sessions: the endpoint total must span several apps, got \
         {MAX_SESSIONS_TOTAL} total / {MAX_SESSIONS_PER_APP} per app"
    );

    for app in 0..apps {
        let app_id =
            created_app_id(&emulator, &format!("App {app}"), vec![text_card(CARD_A, "Topic")]).await;
        for index in 0..MAX_SESSIONS_PER_APP {
            let (status, body) = emulator
                .start_session(INSTANCE, json!({ "appId": app_id, "appVersion": 1 }))
                .await;
            assert_eq!(
                status, 200,
                "case app {app} session {index}: expected 200, got {status} {body}"
            );
        }
    }

    // A brand-new app holds no sessions of its own, and is still refused.
    let spare = created_app_id(&emulator, "Spare", vec![text_card(CARD_A, "Topic")]).await;
    let (status, body) = emulator
        .start_session(INSTANCE, json!({ "appId": spare, "appVersion": 1 }))
        .await;
    assert_eq!(
        status, 402,
        "case {MAX_SESSIONS_TOTAL} sessions held across apps: expected 402, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ServiceQuotaExceededException",
        "case endpoint session total: expected ServiceQuotaExceededException, got {body}"
    );
}

/// A delete reclaims what the app held: creating and deleting repeatedly never
/// fills the store, leaves nothing listed, and leaves no record behind for a
/// later read.
#[tokio::test]
async fn deleting_an_app_reclaims_the_room_it_held() {
    let emulator = Emulator::new();
    // More cycles than either quota lets the store HOLD at once, so a store
    // that kept a record per delete would refuse one of these creates.
    let cycles = MAX_APPS_TOTAL + 5;
    for cycle in 0..cycles {
        let app_id = created_app_id(
            &emulator,
            &format!("Cycle {cycle}"),
            vec![text_card(CARD_A, "Topic")],
        )
        .await;
        let (status, body) = emulator.delete(INSTANCE, &app_id).await;
        assert_eq!(
            status, 200,
            "case cycle {cycle}: expected the delete to succeed, got {status} {body}"
        );
        let (status, body) = emulator.get(INSTANCE, &app_id).await;
        assert_eq!(
            status, 404,
            "case cycle {cycle}: expected 404 after the delete, got {status} {body}"
        );
    }

    let (status, body) = emulator.list(INSTANCE, "").await;
    assert_eq!(status, 200, "case after the cycles: list {body}");
    let listed = body["apps"].as_array().cloned().unwrap_or_default();
    assert!(
        listed.is_empty(),
        "case after {cycles} create/delete cycles: expected nothing listed, got {}",
        listed.len()
    );
}

/// A list request that names no `limit` still answers a BOUNDED page — the
/// largest page the model's `limit` (range 1..100) lets a caller ask for — and
/// a token for the apps that remain.
#[tokio::test]
async fn a_list_without_a_limit_answers_a_bounded_page() {
    let emulator = Emulator::new();
    // One page's worth plus a remainder, so both the cap and the token show.
    let remainder = 3;
    for index in 0..(DEFAULT_PAGE_SIZE + remainder) {
        let (status, body) = emulator
            .create(
                INSTANCE,
                json!({
                    "title": format!("App {index}"),
                    "appDefinition": { "cards": [text_card(CARD_A, "Topic")] },
                }),
            )
            .await;
        assert_eq!(status, 200, "case app {index}: {body}");
    }

    let (status, body) = emulator.list(INSTANCE, "").await;
    assert_eq!(status, 200, "case no limit: {body}");
    let page = body["apps"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        page.len(),
        DEFAULT_PAGE_SIZE,
        "case no limit: expected a page of {DEFAULT_PAGE_SIZE}, got {}",
        page.len()
    );
    let token = body["nextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("case no limit: expected a nextToken, got {body}"))
        .to_string();

    let (status, body) = emulator
        .list(INSTANCE, &format!("?nextToken={token}"))
        .await;
    assert_eq!(status, 200, "case second page: {body}");
    let rest = body["apps"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        rest.len(),
        remainder,
        "case second page: expected the remaining {remainder} apps, got {}",
        rest.len()
    );
    assert!(
        body["nextToken"].is_null(),
        "case second page: expected no token on the last page, got {body}"
    );
}

/// CreateQApp persists the whole record and GetQApp reads it back: the table
/// spans every card kind the model's `CardType` declares, alone and combined,
/// with the dependencies each definition implies and the capabilities it needs.
#[tokio::test]
async fn create_then_get_returns_the_definition_that_was_submitted() {
    struct Case {
        name: &'static str,
        cards: Vec<Value>,
        card_ids: Vec<&'static str>,
        /// (card id, its expected `dependencies`)
        dependencies: Vec<(&'static str, Vec<&'static str>)>,
        capabilities: Vec<&'static str>,
    }

    let emulator = Emulator::new();
    let cases = vec![
        Case {
            name: "one text-input card",
            cards: vec![text_card(CARD_A, "Topic")],
            card_ids: vec![CARD_A],
            dependencies: vec![(CARD_A, vec![])],
            capabilities: vec![],
        },
        Case {
            name: "a query card whose prompt references the input card",
            cards: vec![
                text_card(CARD_A, "Topic"),
                query_card(CARD_B, "Summary", &format!("Summarise @{CARD_A}"), "llm"),
            ],
            card_ids: vec![CARD_A, CARD_B],
            dependencies: vec![(CARD_A, vec![]), (CARD_B, vec![CARD_A])],
            capabilities: vec!["CreatorMode"],
        },
        Case {
            name: "a query card over approved sources",
            cards: vec![query_card(
                CARD_A,
                "Research",
                "What do our documents say?",
                "approved-sources",
            )],
            card_ids: vec![CARD_A],
            dependencies: vec![(CARD_A, vec![])],
            capabilities: vec!["RetrievalMode"],
        },
        Case {
            name: "a file upload card feeding a plugin card",
            cards: vec![
                file_upload_card(CARD_A, "Receipts"),
                plugin_card(CARD_B, "File it", &format!("File @{CARD_A} in the tracker")),
            ],
            card_ids: vec![CARD_A, CARD_B],
            dependencies: vec![(CARD_A, vec![]), (CARD_B, vec![CARD_A])],
            capabilities: vec!["FileUpload", "PluginMode"],
        },
        Case {
            name: "a form input card and a query card over two inputs",
            cards: vec![
                form_input_card(CARD_A, "Details"),
                text_card(CARD_B, "Extra"),
                query_card(
                    CARD_C,
                    "Report",
                    &format!("Combine @{CARD_A} with @{CARD_B}"),
                    "approved-sources",
                ),
            ],
            card_ids: vec![CARD_A, CARD_B, CARD_C],
            dependencies: vec![
                (CARD_A, vec![]),
                (CARD_B, vec![]),
                (CARD_C, vec![CARD_A, CARD_B]),
            ],
            capabilities: vec!["RetrievalMode"],
        },
    ];

    for case in cases {
        let (status, created) = emulator
            .create(
                INSTANCE,
                json!({
                    "title": case.name,
                    "description": "a description that must survive the round trip",
                    "appDefinition": { "cards": case.cards.clone(), "initialPrompt": "Start here" },
                }),
            )
            .await;
        assert_eq!(status, 200, "case {:?}: create body {created}", case.name);

        let app_id = created["appId"].as_str().unwrap_or_default();
        assert!(
            is_uuid_shaped(app_id),
            "case {:?}: expected a UUID-shaped appId, got {app_id:?}",
            case.name
        );
        assert_eq!(
            created["appVersion"], 1,
            "case {:?}: expected appVersion 1, got {}",
            case.name, created["appVersion"]
        );
        assert_eq!(
            created["status"], "DRAFT",
            "case {:?}: expected status DRAFT, got {}",
            case.name, created["status"]
        );
        assert!(
            created["appArn"]
                .as_str()
                .is_some_and(|arn| arn.contains(":qapps:") && arn.ends_with(app_id)),
            "case {:?}: expected an appArn naming the app, got {}",
            case.name,
            created["appArn"]
        );
        for member in ["createdAt", "createdBy", "updatedAt", "updatedBy", "title"] {
            assert!(
                !created[member].is_null(),
                "case {:?}: expected {member} to be populated, got null",
                case.name
            );
        }
        assert_eq!(
            created["initialPrompt"], "Start here",
            "case {:?}: expected the submitted initialPrompt, got {}",
            case.name, created["initialPrompt"]
        );
        let reported: Vec<&str> = created["requiredCapabilities"]
            .as_array()
            .map(|values| values.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        assert_eq!(
            reported, case.capabilities,
            "case {:?}: expected requiredCapabilities {:?}, got {reported:?}",
            case.name, case.capabilities
        );
        for capability in &reported {
            assert!(
                APP_REQUIRED_CAPABILITY_VALUES.contains(capability),
                "case {:?}: {capability:?} is not an AppRequiredCapability value of \
                 {APP_REQUIRED_CAPABILITY_VALUES:?}",
                case.name
            );
        }

        // The round trip: the stored record, read back through GetQApp.
        let (status, fetched) = emulator.get(INSTANCE, app_id).await;
        assert_eq!(status, 200, "case {:?}: get body {fetched}", case.name);
        assert_eq!(
            fetched["title"], created["title"],
            "case {:?}: expected the stored title {}, got {}",
            case.name, created["title"], fetched["title"]
        );
        assert_eq!(
            fetched["description"], "a description that must survive the round trip",
            "case {:?}: expected the stored description, got {}",
            case.name, fetched["description"]
        );
        assert_eq!(
            fetched["appVersion"], 1,
            "case {:?}: expected appVersion 1, got {}",
            case.name, fetched["appVersion"]
        );
        assert_eq!(
            fetched["createdAt"], created["createdAt"],
            "case {:?}: expected the stored createdAt {}, got {}",
            case.name, created["createdAt"], fetched["createdAt"]
        );

        let definition = &fetched["appDefinition"];
        assert!(
            !definition["appDefinitionVersion"].is_null(),
            "case {:?}: expected appDefinition.appDefinitionVersion, got null",
            case.name
        );
        let cards = definition["cards"]
            .as_array()
            .unwrap_or_else(|| panic!("case {:?}: no cards in {definition}", case.name));
        let ids: Vec<&str> = cards.iter().map(returned_card_id).collect();
        assert_eq!(
            ids, case.card_ids,
            "case {:?}: expected the submitted card ids {:?} in order, got {ids:?}",
            case.name, case.card_ids
        );
        for (card_id, expected) in &case.dependencies {
            let card = cards
                .iter()
                .find(|card| returned_card_id(card) == *card_id)
                .unwrap_or_else(|| panic!("case {:?}: card {card_id} is missing", case.name));
            let actual = returned_card_dependencies(card);
            assert_eq!(
                actual, *expected,
                "case {:?}: expected card {card_id} dependencies {expected:?}, got {actual:?}",
                case.name
            );
            let kind = card
                .as_object()
                .and_then(|variant| variant.values().next())
                .map(|body| body["type"].as_str().unwrap_or_default())
                .unwrap_or_default();
            assert!(
                CARD_TYPE_VALUES.contains(&kind),
                "case {:?}: card {card_id} reports type {kind:?}, not a CardType value of \
                 {CARD_TYPE_VALUES:?}",
                case.name
            );
        }
    }
}

/// An unknown app, and an app version that never existed, are both 404
/// `ResourceNotFoundException`.
#[tokio::test]
async fn get_refuses_an_app_or_version_that_does_not_exist() {
    let emulator = Emulator::new();
    let app_id = created_app_id(&emulator, "Only version one", vec![text_card(CARD_A, "Topic")]).await;

    let cases: Vec<(&str, String, Option<i64>)> = vec![
        (
            "an app id nothing created",
            "00000000-0000-4000-8000-000000000000".to_string(),
            None,
        ),
        ("version zero", app_id.clone(), Some(0)),
        ("a version past the latest", app_id.clone(), Some(2)),
        ("a far future version", app_id.clone(), Some(4096)),
    ];

    for (name, id, version) in cases {
        let (status, body) = match version {
            None => emulator.get(INSTANCE, &id).await,
            Some(version) => emulator.get_version(INSTANCE, &id, version).await,
        };
        assert_eq!(status, 404, "case {name:?}: expected 404, got {status} {body}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {name:?}: expected ResourceNotFoundException, got {body}"
        );
    }
}

/// UpdateQApp applies only the members it was given, bumps `appVersion` by one
/// and leaves every other member alone — whichever subset of members is sent.
#[tokio::test]
async fn update_applies_only_the_supplied_members_and_bumps_the_version() {
    struct Case {
        name: &'static str,
        patch: Value,
        expect_title: &'static str,
        expect_description: &'static str,
        expect_card_ids: Vec<&'static str>,
    }

    let emulator = Emulator::new();
    let cases = vec![
        Case {
            name: "title only",
            patch: json!({ "title": "Renamed" }),
            expect_title: "Renamed",
            expect_description: "the original description",
            expect_card_ids: vec![CARD_A],
        },
        Case {
            name: "description only",
            patch: json!({ "description": "a new description" }),
            expect_title: "The original title",
            expect_description: "a new description",
            expect_card_ids: vec![CARD_A],
        },
        Case {
            name: "definition only",
            patch: json!({ "appDefinition": { "cards": [text_card(CARD_B, "Second")] } }),
            expect_title: "The original title",
            expect_description: "the original description",
            expect_card_ids: vec![CARD_B],
        },
        Case {
            name: "title and definition together",
            patch: json!({
                "title": "Both",
                "appDefinition": { "cards": [
                    text_card(CARD_B, "Second"),
                    query_card(CARD_C, "Ask", &format!("About @{CARD_B}"), "llm"),
                ]},
            }),
            expect_title: "Both",
            expect_description: "the original description",
            expect_card_ids: vec![CARD_B, CARD_C],
        },
        Case {
            name: "nothing but the app id",
            patch: json!({}),
            expect_title: "The original title",
            expect_description: "the original description",
            expect_card_ids: vec![CARD_A],
        },
    ];

    for case in cases {
        let (status, created) = emulator
            .create(
                INSTANCE,
                json!({
                    "title": "The original title",
                    "description": "the original description",
                    "appDefinition": { "cards": [text_card(CARD_A, "First")] },
                }),
            )
            .await;
        assert_eq!(status, 200, "case {:?}: create body {created}", case.name);
        let app_id = created["appId"].as_str().unwrap_or_default().to_string();

        let mut patch = case.patch.clone();
        patch["appId"] = json!(app_id);
        let (status, updated) = emulator.update(INSTANCE, patch).await;
        assert_eq!(status, 200, "case {:?}: update body {updated}", case.name);
        assert_eq!(
            updated["appVersion"], 2,
            "case {:?}: expected appVersion 2, got {}",
            case.name, updated["appVersion"]
        );
        assert_eq!(
            updated["title"], case.expect_title,
            "case {:?}: expected title {:?}, got {}",
            case.name, case.expect_title, updated["title"]
        );
        assert_eq!(
            updated["createdAt"], created["createdAt"],
            "case {:?}: expected createdAt to stay {}, got {}",
            case.name, created["createdAt"], updated["createdAt"]
        );

        let (status, fetched) = emulator.get(INSTANCE, &app_id).await;
        assert_eq!(status, 200, "case {:?}: get body {fetched}", case.name);
        assert_eq!(
            fetched["appVersion"], 2,
            "case {:?}: expected GetQApp to report appVersion 2, got {}",
            case.name, fetched["appVersion"]
        );
        assert_eq!(
            fetched["title"], case.expect_title,
            "case {:?}: expected title {:?}, got {}",
            case.name, case.expect_title, fetched["title"]
        );
        assert_eq!(
            fetched["description"], case.expect_description,
            "case {:?}: expected description {:?}, got {}",
            case.name, case.expect_description, fetched["description"]
        );
        let ids: Vec<&str> = fetched["appDefinition"]["cards"]
            .as_array()
            .map(|cards| cards.iter().map(returned_card_id).collect())
            .unwrap_or_default();
        assert_eq!(
            ids, case.expect_card_ids,
            "case {:?}: expected cards {:?}, got {ids:?}",
            case.name, case.expect_card_ids
        );

        // The version that was in force before the update is still readable,
        // and still carries the definition it was created with.
        let (status, version_one) = emulator.get_version(INSTANCE, &app_id, 1).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected version 1 to remain readable, got {status} {version_one}",
            case.name
        );
        let original_ids: Vec<&str> = version_one["appDefinition"]["cards"]
            .as_array()
            .map(|cards| cards.iter().map(returned_card_id).collect())
            .unwrap_or_default();
        assert_eq!(
            original_ids,
            vec![CARD_A],
            "case {:?}: expected version 1 to keep its own cards, got {original_ids:?}",
            case.name
        );
    }
}

/// Updating an app that does not exist is 404, not a silent create.
#[tokio::test]
async fn update_refuses_an_app_that_does_not_exist() {
    let emulator = Emulator::new();
    let (status, body) = emulator
        .update(
            INSTANCE,
            json!({
                "appId": "00000000-0000-4000-8000-000000000000",
                "title": "Nothing to rename",
            }),
        )
        .await;
    assert_eq!(status, 404, "expected 404, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );
}

/// ListQApps pages through every persisted app exactly once, for any number of
/// apps and any page size, and omits `nextToken` on the last page.
#[tokio::test]
async fn list_pages_through_every_app_exactly_once() {
    // (app count, limit query) — the rule generalises over both.
    let cases: Vec<(usize, Option<i64>)> = vec![
        (1, Some(1)),
        (2, Some(1)),
        (3, Some(2)),
        (5, Some(100)),
        (7, Some(3)),
        (4, None),
    ];

    for (count, limit) in cases {
        let case = format!("{count} apps, limit {limit:?}");
        let emulator = Emulator::new();
        let mut created: Vec<String> = Vec::new();
        for index in 0..count {
            created.push(
                created_app_id(
                    &emulator,
                    &format!("App {index}"),
                    vec![text_card(CARD_A, "Topic")],
                )
                .await,
            );
        }

        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            assert!(
                pages <= count + 1,
                "case {case:?}: pagination did not terminate after {pages} pages"
            );
            let mut query = String::new();
            if let Some(limit) = limit {
                query.push_str(&format!("?limit={limit}"));
            }
            if let Some(token) = &token {
                query.push_str(if query.is_empty() { "?" } else { "&" });
                query.push_str(&format!("nextToken={token}"));
            }
            let (status, body) = emulator.list(INSTANCE, &query).await;
            assert_eq!(status, 200, "case {case:?}: list body {body}");
            let page: Vec<String> = body["apps"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case:?}: no apps array in {body}"))
                .iter()
                .map(|app| app["appId"].as_str().unwrap_or_default().to_string())
                .collect();
            if let Some(limit) = limit {
                assert!(
                    i64::try_from(page.len()).unwrap_or(i64::MAX) <= limit,
                    "case {case:?}: expected at most {limit} apps in a page, got {}",
                    page.len()
                );
            }
            for app in &page {
                assert!(
                    !seen.contains(app),
                    "case {case:?}: app {app} was returned on two pages"
                );
            }
            seen.extend(page);
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }

        assert_eq!(
            seen, created,
            "case {case:?}: expected every created app once, in creation order"
        );
    }
}

/// A `nextToken` that this service did not issue for this instance is refused
/// as invalid input, not served as a position.
#[tokio::test]
async fn list_refuses_a_token_it_did_not_issue() {
    let emulator = Emulator::new();
    let app_id = created_app_id(&emulator, "Listed", vec![text_card(CARD_A, "Topic")]).await;

    // One real token, to show the same walk accepts what the service issued.
    let (status, first) = emulator.list(INSTANCE, "?limit=1").await;
    assert_eq!(status, 200, "first page: {first}");
    let issued = first["nextToken"].as_str();
    assert!(
        issued.is_none(),
        "a single app is one page, so no token is issued: {first}"
    );
    assert_eq!(
        first["apps"][0]["appId"].as_str(),
        Some(app_id.as_str()),
        "the one page lists the one app: {first}"
    );

    for token in [
        "not-hex-at-all",
        "6e6f742d612d746f6b656e",                                     // hex of "not-a-token"
        "76313a6f74686572696e7374616e63653a6162",                     // hex of "v1:otherinstance:ab"
        "76313a31313131313131312d323232322d343333332d383434342d3535353535353535353535353a30303030303030302d303030302d343030302d383030302d303030303030303030303030", // a token for an app that does not exist
    ] {
        let (status, body) = emulator.list(INSTANCE, &format!("?nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected 400 for a token this service did not issue, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {token:?}: expected ValidationException, got {body}"
        );
    }
}

/// State is scoped by `instance-id`: another instance neither lists nor reads
/// an app created under this one.
#[tokio::test]
async fn apps_are_scoped_to_the_instance_that_created_them() {
    let emulator = Emulator::new();
    let app_id = created_app_id(&emulator, "Tenant one", vec![text_card(CARD_A, "Topic")]).await;

    let (status, body) = emulator.get(OTHER_INSTANCE, &app_id).await;
    assert_eq!(status, 404, "expected 404 from another instance, got {status} {body}");

    let (status, body) = emulator.list(OTHER_INSTANCE, "").await;
    assert_eq!(status, 200, "list from another instance: {body}");
    assert_eq!(
        body["apps"].as_array().map(Vec::len),
        Some(0),
        "expected no apps for another instance, got {body}"
    );

    let (status, body) = emulator.list(INSTANCE, "").await;
    assert_eq!(status, 200, "list from the owning instance: {body}");
    assert_eq!(
        body["apps"][0]["appId"].as_str(),
        Some(app_id.as_str()),
        "expected the owning instance to list its app, got {body}"
    );
}

/// StartQAppSession seeds `cardStatus` from the app's definition and the values
/// it was given, and GetQAppSession reports exactly that.
#[tokio::test]
async fn a_session_reports_the_card_values_it_was_started_with() {
    // Which cards get an initial value: the rule holds for none, some and all.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("no initial values", vec![]),
        ("one of three cards", vec![(CARD_A, "Paris")]),
        (
            "two of three cards",
            vec![(CARD_A, "Paris"), (CARD_B, "three days")],
        ),
        (
            "every card",
            vec![
                (CARD_A, "Paris"),
                (CARD_B, "three days"),
                (CARD_C, "a summary"),
            ],
        ),
    ];

    for (name, values) in cases {
        let emulator = Emulator::new();
        let app_id = created_app_id(
            &emulator,
            "Trip Planner",
            vec![
                text_card(CARD_A, "Destination"),
                text_card(CARD_B, "Duration"),
                query_card(CARD_C, "Itinerary", &format!("Plan for @{CARD_A}"), "llm"),
            ],
        )
        .await;

        let initial_values: Vec<Value> = values
            .iter()
            .map(|(card_id, value)| json!({ "cardId": card_id, "value": value }))
            .collect();
        let (status, started) = emulator
            .start_session(
                INSTANCE,
                json!({
                    "appId": app_id,
                    "appVersion": 1,
                    "initialValues": initial_values,
                }),
            )
            .await;
        assert_eq!(status, 200, "case {name:?}: start body {started}");
        let session_id = started["sessionId"].as_str().unwrap_or_default().to_string();
        assert!(
            is_uuid_shaped(&session_id),
            "case {name:?}: expected a UUID-shaped sessionId, got {session_id:?}"
        );
        assert!(
            started["sessionArn"]
                .as_str()
                .is_some_and(|arn| arn.contains(":qapps:") && arn.ends_with(&session_id)),
            "case {name:?}: expected a sessionArn naming the session, got {}",
            started["sessionArn"]
        );

        let (status, session) = emulator.get_session(INSTANCE, &session_id).await;
        assert_eq!(status, 200, "case {name:?}: get session body {session}");
        assert_eq!(
            session["sessionId"].as_str(),
            Some(session_id.as_str()),
            "case {name:?}: expected the same sessionId, got {}",
            session["sessionId"]
        );
        assert_eq!(
            session["sessionArn"], started["sessionArn"],
            "case {name:?}: expected the sessionArn StartQAppSession returned, got {}",
            session["sessionArn"]
        );
        assert_eq!(
            session["appVersion"], 1,
            "case {name:?}: expected appVersion 1, got {}",
            session["appVersion"]
        );
        let status_value = session["status"].as_str().unwrap_or_default();
        assert!(
            EXECUTION_STATUS_VALUES.contains(&status_value),
            "case {name:?}: expected an ExecutionStatus value of {EXECUTION_STATUS_VALUES:?}, \
             got {status_value:?}"
        );
        let expect_session_status = if values.len() == 3 { "COMPLETED" } else { "WAITING" };
        assert_eq!(
            status_value, expect_session_status,
            "case {name:?}: expected session status {expect_session_status}, got {status_value}"
        );

        let card_status = session["cardStatus"]
            .as_object()
            .unwrap_or_else(|| panic!("case {name:?}: no cardStatus map in {session}"));
        let mut keys: Vec<&str> = card_status.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected_keys = vec![CARD_A, CARD_B, CARD_C];
        expected_keys.sort_unstable();
        assert_eq!(
            keys, expected_keys,
            "case {name:?}: expected cardStatus keyed by the definition's card ids"
        );
        for card_id in [CARD_A, CARD_B, CARD_C] {
            let entry = &card_status[card_id];
            let supplied = values
                .iter()
                .find(|(id, _)| *id == card_id)
                .map(|(_, value)| *value);
            match supplied {
                Some(value) => {
                    assert_eq!(
                        entry["currentValue"], value,
                        "case {name:?}: expected card {card_id} to hold {value:?}, got {}",
                        entry["currentValue"]
                    );
                    assert_eq!(
                        entry["currentState"], "COMPLETED",
                        "case {name:?}: expected card {card_id} COMPLETED, got {}",
                        entry["currentState"]
                    );
                }
                None => {
                    assert_eq!(
                        entry["currentState"], "WAITING",
                        "case {name:?}: expected card {card_id} WAITING, got {}",
                        entry["currentState"]
                    );
                    assert_eq!(
                        entry["currentValue"], "",
                        "case {name:?}: expected card {card_id} to hold no value, got {}",
                        entry["currentValue"]
                    );
                }
            }
        }
    }
}

/// A session over an app or version that does not exist is 404; one seeded with
/// a card the definition does not have, or an unusable session id, is 400. An
/// unknown session id is 404.
#[tokio::test]
async fn session_requests_that_name_nothing_in_state_are_refused() {
    let emulator = Emulator::new();
    let app_id = created_app_id(
        &emulator,
        "Trip Planner",
        vec![text_card(CARD_A, "Destination")],
    )
    .await;

    let cases: Vec<(&str, Value, u16, &str)> = vec![
        (
            "an app id nothing created",
            json!({ "appId": "00000000-0000-4000-8000-000000000000", "appVersion": 1 }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a version the app never had",
            json!({ "appId": app_id, "appVersion": 9 }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an initial value for a card of another definition",
            json!({
                "appId": app_id,
                "appVersion": 1,
                "initialValues": [{ "cardId": CARD_B, "value": "Paris" }],
            }),
            400,
            "ValidationException",
        ),
        (
            "a session id no reader could name",
            json!({ "appId": app_id, "appVersion": 1, "sessionId": "session-one" }),
            400,
            "ValidationException",
        ),
    ];

    for (name, request, expect_status, expect_code) in cases {
        let (status, body) = emulator.start_session(INSTANCE, request).await;
        assert_eq!(
            status, expect_status,
            "case {name:?}: expected {expect_status}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expect_code,
            "case {name:?}: expected {expect_code}, got {body}"
        );
    }

    let (status, body) = emulator
        .get_session(INSTANCE, "00000000-0000-4000-8000-000000000000")
        .await;
    assert_eq!(status, 404, "an unknown session is 404, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );
}

/// A caller-supplied session id is the id the session keeps.
#[tokio::test]
async fn a_supplied_session_id_is_the_session_that_is_read_back() {
    let emulator = Emulator::new();
    let app_id = created_app_id(&emulator, "Trip Planner", vec![text_card(CARD_A, "Destination")])
        .await;
    let session_id = "12345678-90ab-4cde-8f01-234567890abc";

    let (status, started) = emulator
        .start_session(
            INSTANCE,
            json!({
                "appId": app_id,
                "appVersion": 1,
                "sessionId": session_id,
                "initialValues": [{ "cardId": CARD_A, "value": "Lisbon" }],
            }),
        )
        .await;
    assert_eq!(status, 200, "start body {started}");
    assert_eq!(
        started["sessionId"].as_str(),
        Some(session_id),
        "expected the supplied sessionId, got {}",
        started["sessionId"]
    );

    let (status, session) = emulator.get_session(INSTANCE, session_id).await;
    assert_eq!(status, 200, "get session body {session}");
    assert_eq!(
        session["cardStatus"][CARD_A]["currentValue"], "Lisbon",
        "expected the supplied value, got {}",
        session["cardStatus"][CARD_A]["currentValue"]
    );
}

/// Restarting a session against another app moves it: it belongs to the app it
/// now runs, so deleting the app it used to run leaves it alone.
#[tokio::test]
async fn a_session_restarted_against_another_app_belongs_to_that_app() {
    let emulator = Emulator::new();
    let first = created_app_id(&emulator, "First app", vec![text_card(CARD_A, "Topic")]).await;
    let second = created_app_id(&emulator, "Second app", vec![text_card(CARD_B, "Topic")]).await;
    let session_id = "0fedcba9-8765-4321-b987-6543210fedcb";

    for (case, app_id, card_id, value) in [
        ("started on the first app", &first, CARD_A, "one"),
        ("restarted on the second app", &second, CARD_B, "two"),
    ] {
        let (status, body) = emulator
            .start_session(
                INSTANCE,
                json!({
                    "appId": app_id,
                    "appVersion": 1,
                    "sessionId": session_id,
                    "initialValues": [{ "cardId": card_id, "value": value }],
                }),
            )
            .await;
        assert_eq!(status, 200, "case {case:?}: start body {body}");
    }

    // The session now runs the second app's definition.
    let (status, session) = emulator.get_session(INSTANCE, session_id).await;
    assert_eq!(status, 200, "get session body {session}");
    assert_eq!(
        session["cardStatus"][CARD_B]["currentValue"], "two",
        "expected the restarted session's own values, got {}",
        session["cardStatus"]
    );

    // Deleting the app it used to run must not take it with it.
    let (status, body) = emulator.delete(INSTANCE, &first).await;
    assert_eq!(status, 200, "delete the first app: {status} {body}");
    let (status, session) = emulator.get_session(INSTANCE, session_id).await;
    assert_eq!(
        status, 200,
        "expected the moved session to survive the old app's delete, got {status} {session}"
    );

    // Deleting the app it now runs does take it.
    let (status, body) = emulator.delete(INSTANCE, &second).await;
    assert_eq!(status, 200, "delete the second app: {status} {body}");
    let (status, session) = emulator.get_session(INSTANCE, session_id).await;
    assert_eq!(
        status, 404,
        "expected the session to go with the app it ran, got {status} {session}"
    );
}

/// DeleteQApp removes the app and the sessions that ran it, and a second delete
/// of the same app is 404.
#[tokio::test]
async fn delete_removes_the_app_its_sessions_and_its_listing() {
    let emulator = Emulator::new();
    let kept = created_app_id(&emulator, "Kept", vec![text_card(CARD_A, "Topic")]).await;
    let doomed = created_app_id(&emulator, "Doomed", vec![text_card(CARD_B, "Topic")]).await;

    let (status, started) = emulator
        .start_session(INSTANCE, json!({ "appId": doomed, "appVersion": 1 }))
        .await;
    assert_eq!(status, 200, "start body {started}");
    let session_id = started["sessionId"].as_str().unwrap_or_default().to_string();

    let (status, body) = emulator.delete(INSTANCE, &doomed).await;
    assert_eq!(status, 200, "expected the delete to succeed, got {status} {body}");

    let (status, body) = emulator.get(INSTANCE, &doomed).await;
    assert_eq!(status, 404, "expected the deleted app to be gone, got {status} {body}");
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );

    let (status, body) = emulator.get_session(INSTANCE, &session_id).await;
    assert_eq!(
        status, 404,
        "expected the deleted app's session to be gone, got {status} {body}"
    );

    let (status, body) = emulator.list(INSTANCE, "").await;
    assert_eq!(status, 200, "list body {body}");
    let listed: Vec<&str> = body["apps"]
        .as_array()
        .map(|apps| {
            apps.iter()
                .map(|app| app["appId"].as_str().unwrap_or_default())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed,
        vec![kept.as_str()],
        "expected only the surviving app to be listed, got {listed:?}"
    );

    let (status, body) = emulator.delete(INSTANCE, &doomed).await;
    assert_eq!(
        status, 404,
        "expected a second delete of the same app to be 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "ResourceNotFoundException",
        "expected ResourceNotFoundException, got {body}"
    );
}

/// Two apps created in a row get different ids: an id is minted, not fixed.
#[tokio::test]
async fn every_created_app_gets_its_own_id() {
    let emulator = Emulator::new();
    let mut ids: Vec<String> = Vec::new();
    // Enough ids that a minting routine which repeated a value, or which only
    // sometimes landed on the modelled version/variant nibbles, would show up.
    for index in 0..64 {
        let id = created_app_id(
            &emulator,
            &format!("App {index}"),
            vec![text_card(CARD_A, "Topic")],
        )
        .await;
        assert!(
            !ids.contains(&id),
            "case app {index}: id {id} was minted twice"
        );
        assert!(
            is_uuid_shaped(&id),
            "case app {index}: expected a UUID-shaped appId, got {id:?}"
        );
        ids.push(id);
    }
}
