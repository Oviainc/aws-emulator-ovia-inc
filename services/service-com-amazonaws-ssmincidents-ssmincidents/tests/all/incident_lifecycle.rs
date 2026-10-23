// SPDX-License-Identifier: CC0-1.0

//! The Incident Manager response-plan -> incident-record lifecycle, end to end over
//! the `restJson1` wire: `CreateResponsePlan`, `GetResponsePlan`, `StartIncident`,
//! `GetIncidentRecord`, `ListIncidentRecords`, `UpdateIncidentRecord` and
//! `DeleteIncidentRecord`.
//!
//! Every suite here drives the real router (`MultiServiceRouter::dispatch`), so each
//! assertion is about the HTTP behaviour a client sees, not about a handler's return
//! value. Each case is a row in a table spanning the class the rule generalises over;
//! the request the issue quoted is one row.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// This service's SigV4 signing name, which is what the router keys `can_handle` on.
const SIGNING_NAME: &str = "ssm-incidents";

/// The access key id every request here is "signed" with (authentication is off in
/// these tests); the service derives the caller principal from it.
const TEST_AKID: &str = "AKIAIOSFODNN7EXAMPLE";

/// The canonical placeholder account id every ARN the emulator mints carries.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The principal ARN the service records for [`TEST_AKID`].
fn expected_principal() -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{TEST_AKID}")
}

/// One dispatched request's answer.
///
/// `pub(crate)`, like [`Emulator`], so the other suites in this test target drive the
/// emulator through this one harness instead of each declaring its own.
pub(crate) struct Reply {
    pub(crate) status: u16,
    pub(crate) body: Value,
}

impl Reply {
    /// The restJson1 error code (`__type`), or `""` on a success body.
    pub(crate) fn error_code(&self) -> &str {
        self.body
            .get("__type")
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// A string at a JSON pointer, or `""` when absent.
    pub(crate) fn text(&self, pointer: &str) -> &str {
        self.body
            .pointer(pointer)
            .and_then(Value::as_str)
            .unwrap_or_default()
    }

    /// A number at a JSON pointer, or `None` when absent or not a number.
    pub(crate) fn number(&self, pointer: &str) -> Option<f64> {
        self.body.pointer(pointer).and_then(Value::as_f64)
    }

    /// A boolean at a JSON pointer, or `None` when absent or not a boolean.
    pub(crate) fn boolean(&self, pointer: &str) -> Option<bool> {
        self.body.pointer(pointer).and_then(Value::as_bool)
    }
}

/// The emulator under test: one engine, one service, one router — exactly what
/// `core/router` assembles in production.
///
/// `pub(crate)` because every suite in this test target shares it; the engine is kept
/// beside the router so a suite can inspect (or seed) the very store the handlers read.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    pub(crate) engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_ssmincidents_ssmincidents::EmulatedService::new(Arc::clone(
                &engine,
            )),
        );
        Self { router, engine }
    }

    pub(crate) async fn post(&self, path: &str, body: Value) -> Reply {
        self.call("POST", path, Some(body)).await
    }

    pub(crate) async fn get(&self, path: &str) -> Reply {
        self.call("GET", path, None).await
    }

    pub(crate) async fn call(&self, method: &str, path: &str, body: Option<Value>) -> Reply {
        let payload = body
            .map(|value| bytes::Bytes::from(serde_json::to_vec(&value).expect("serialize body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{SIGNING_NAME}.us-east-1.amazonaws.com{path}"))
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential={TEST_AKID}/20260101/us-east-1/\
                     {SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=0"
                ),
            )
            .body(payload)
            .expect("build request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let raw = response.into_body();
        let body = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or(Value::Null)
        };
        Reply { status, body }
    }

    /// Create a response plan and answer its ARN, failing the test if it was refused.
    async fn create_plan(&self, request: Value) -> String {
        let reply = self.post("/createResponsePlan", request.clone()).await;
        assert_eq!(
            reply.status, 201,
            "creating {request:?}: expected 201, got {} body {:?}",
            reply.status, reply.body
        );
        reply.text("/arn").to_string()
    }

    /// Start an incident and answer its ARN, failing the test if it was refused.
    async fn start_incident(&self, request: Value) -> String {
        let reply = self.post("/startIncident", request.clone()).await;
        assert_eq!(
            reply.status, 200,
            "starting {request:?}: expected 200, got {} body {:?}",
            reply.status, reply.body
        );
        reply.text("/incidentRecordArn").to_string()
    }

    async fn get_incident(&self, arn: &str) -> Reply {
        self.get(&format!("/getIncidentRecord?arn={}", query_encode(arn)))
            .await
    }
}

/// Percent-encode a value for a query string, so an ARN's `:` and `/` survive the
/// round trip. `NON_ALPHANUMERIC` over-encodes (it also escapes `-._~`, which RFC 3986
/// leaves unreserved); that is still a valid encoding of the same value, and it keeps
/// the test free of a hand-rolled escaper.
pub(crate) fn query_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, percent_encoding::NON_ALPHANUMERIC).to_string()
}

/// A member that is present and not null.
fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|found| !found.is_null())
}

/// Whole seconds since the epoch, for the timestamp conditions `ListIncidentRecords`
/// filters on.
fn epoch_secs() -> i64 {
    i64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("after the epoch")
            .as_secs(),
    )
    .expect("fits in i64")
}

/// Response plans spanning the shape of a `CreateResponsePlan` request: every
/// character class the `name` pattern (`^[a-zA-Z0-9-_]*$`) allows, each published
/// `impact` value (the model's `Impact` range is 1..5), a template with and without
/// each optional member, and the optional top-level members.
fn plan_cases() -> Vec<Value> {
    vec![
        json!({
            "name": "minimal-plan",
            "incidentTemplate": { "title": "DB down", "impact": 2 },
        }),
        json!({
            "name": "plan_with_everything",
            "displayName": "A display name may contain spaces",
            "incidentTemplate": {
                "title": "Cache cold",
                "impact": 5,
                "summary": "the cache was flushed",
                "dedupeString": "cache-cold",
                "notificationTargets": [
                    { "snsTopicArn": format!("arn:aws:sns::{ACCOUNT}:incident-updates") },
                ],
                "incidentTags": { "team": "sre" },
            },
            "chatChannel": { "chatbotSns": [format!("arn:aws:sns::{ACCOUNT}:chat")] },
            "engagements": [format!("arn:aws:ssm-contacts::{ACCOUNT}:contact/on-call")],
            "tags": { "env": "test" },
        }),
        json!({
            "name": "PLAN9",
            "incidentTemplate": {
                "title": "Critical impact",
                "impact": 1,
                "notificationTargets": [
                    { "snsTopicArn": format!("arn:aws:sns::{ACCOUNT}:one") },
                    { "snsTopicArn": format!("arn:aws:sns::{ACCOUNT}:two") },
                ],
            },
        }),
        json!({
            "name": "a",
            "incidentTemplate": { "title": "Shortest legal name", "impact": 3 },
        }),
        json!({
            "name": "impact-4-plan",
            "displayName": "Impact 4",
            "incidentTemplate": { "title": "Low", "impact": 4, "summary": "low impact" },
        }),
        // Explicitly EMPTY optional lists: a read must report them empty, not absent.
        json!({
            "name": "empty-lists-plan",
            "incidentTemplate": {
                "title": "Empty lists",
                "impact": 2,
                "notificationTargets": [],
            },
            "engagements": [],
            "actions": [],
            "integrations": [],
        }),
    ]
}

/// A `CreateResponsePlan` mints `arn:aws:ssm-incidents::<account>:response-plan/<name>`
/// and STORES the plan, so `GetResponsePlan` on that ARN answers the same contents.
#[tokio::test]
async fn a_created_response_plan_is_stored_and_reads_back_unchanged() {
    let emulator = Emulator::new();

    for request in plan_cases() {
        let name = request["name"].as_str().expect("case names its plan");
        let arn = emulator.create_plan(request.clone()).await;
        assert_eq!(
            arn,
            format!("arn:aws:ssm-incidents::{ACCOUNT}:response-plan/{name}"),
            "case {name:?}: unexpected response-plan ARN, got {arn:?}"
        );

        let fetched = emulator
            .get(&format!("/getResponsePlan?arn={}", query_encode(&arn)))
            .await;
        assert_eq!(
            fetched.status, 200,
            "case {name:?}: expected 200 from GetResponsePlan, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.text("/arn"),
            arn,
            "case {name:?}: GetResponsePlan reported another ARN, got {:?}",
            fetched.body.get("arn")
        );
        assert_eq!(
            fetched.text("/name"),
            name,
            "case {name:?}: GetResponsePlan reported another name, got {:?}",
            fetched.body.get("name")
        );
        for field in ["displayName", "incidentTemplate", "chatChannel", "engagements", "actions", "integrations"] {
            assert_eq!(
                member(&fetched.body, field),
                member(&request, field),
                "case {name:?}: {field} did not round-trip, got {:?}",
                fetched.body.get(field)
            );
        }
    }
}

/// A name already in the store is a conflicting create (409), whatever the rest of the
/// request says; a replay carrying the same `clientToken` answers with the ARN the
/// first call minted instead of minting a second one.
#[tokio::test]
async fn re_creating_a_response_plan_conflicts_and_a_client_token_replay_does_not() {
    let emulator = Emulator::new();

    // Rows differ in WHICH member of the second request differs from the first.
    let cases: Vec<(&str, Value)> = vec![
        (
            "same-contents",
            json!({ "name": "conflict-1", "incidentTemplate": { "title": "T", "impact": 2 } }),
        ),
        (
            "different-title",
            json!({ "name": "conflict-2", "incidentTemplate": { "title": "Other", "impact": 2 } }),
        ),
        (
            "different-impact",
            json!({ "name": "conflict-3", "incidentTemplate": { "title": "T", "impact": 5 } }),
        ),
        (
            "extra-display-name",
            json!({
                "name": "conflict-4",
                "displayName": "added",
                "incidentTemplate": { "title": "T", "impact": 2 },
            }),
        ),
    ];

    for (case, second) in cases {
        let name = second["name"].as_str().expect("case names its plan");
        let first = json!({ "name": name, "incidentTemplate": { "title": "T", "impact": 2 } });
        emulator.create_plan(first).await;

        let again = emulator.post("/createResponsePlan", second.clone()).await;
        assert_eq!(
            again.status, 409,
            "case {case:?}: expected 409 for a duplicate name, got {} body {:?}",
            again.status, again.body
        );
        assert_eq!(
            again.error_code(),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {:?}",
            again.body
        );
    }

    // The idempotency token is what makes a repeat succeed.
    let token = "a-client-token";
    let request = json!({
        "name": "idempotent-plan",
        "clientToken": token,
        "incidentTemplate": { "title": "T", "impact": 2 },
    });
    let first = emulator.create_plan(request.clone()).await;
    let replay = emulator.post("/createResponsePlan", request).await;
    assert_eq!(
        replay.status, 201,
        "case \"client-token replay\": expected 201, got {} body {:?}",
        replay.status, replay.body
    );
    assert_eq!(
        replay.text("/arn"),
        first,
        "case \"client-token replay\": expected the first ARN, got {:?}",
        replay.body.get("arn")
    );
}

/// `StartIncident` inherits `title`, `impact`, `summary`, `dedupeString`,
/// `notificationTargets` and `chatChannel` from the plan's template, and the request's
/// own `title` / `impact` override the inherited ones. The incident is persisted, so
/// `GetIncidentRecord` answers every required member from state.
#[tokio::test]
async fn a_started_incident_inherits_the_template_and_the_request_overrides_it() {
    let emulator = Emulator::new();

    let plan_arn = emulator
        .create_plan(json!({
            "name": "inheritance-plan",
            "incidentTemplate": {
                "title": "Template title",
                "impact": 3,
                "summary": "template summary",
                "notificationTargets": [
                    { "snsTopicArn": format!("arn:aws:sns::{ACCOUNT}:updates") },
                ],
            },
            "chatChannel": { "chatbotSns": [format!("arn:aws:sns::{ACCOUNT}:chat")] },
        }))
        .await;

    // Rows: which of `title` / `impact` the request overrides, and whether it carries
    // `triggerDetails` (which names the incident's source) or not (a manual start).
    let cases: Vec<(&str, Value, &str, i64, &str)> = vec![
        (
            "no overrides",
            json!({ "responsePlanArn": plan_arn }),
            "Template title",
            3,
            "aws.ssm-incidents",
        ),
        (
            "title only",
            json!({ "responsePlanArn": plan_arn, "title": "Request title" }),
            "Request title",
            3,
            "aws.ssm-incidents",
        ),
        (
            "impact only",
            json!({ "responsePlanArn": plan_arn, "impact": 1 }),
            "Template title",
            1,
            "aws.ssm-incidents",
        ),
        (
            "both, lowest impact",
            json!({ "responsePlanArn": plan_arn, "title": "Both", "impact": 5 }),
            "Both",
            5,
            "aws.ssm-incidents",
        ),
        (
            "triggered by a detector",
            json!({
                "responsePlanArn": plan_arn,
                "triggerDetails": {
                    "source": "aws.cloudwatch",
                    "timestamp": 1_700_000_000,
                    "triggerArn": format!("arn:aws:cloudwatch::{ACCOUNT}:alarm/cpu"),
                },
            }),
            "Template title",
            3,
            "aws.cloudwatch",
        ),
        (
            "with related items",
            json!({
                "responsePlanArn": plan_arn,
                "title": "Related",
                "relatedItems": [{
                    "identifier": { "value": { "url": "https://example.test/runbook" }, "type": "OTHER" },
                    "title": "Runbook",
                }],
            }),
            "Related",
            3,
            "aws.ssm-incidents",
        ),
    ];

    let mut seen_arns: Vec<String> = Vec::new();
    for (case, request, expected_title, expected_impact, expected_source) in cases {
        let arn = emulator.start_incident(request).await;
        let prefix = format!("arn:aws:ssm-incidents::{ACCOUNT}:incident-record/inheritance-plan/");
        assert!(
            arn.starts_with(&prefix),
            "case {case:?}: expected an incident-record ARN under {prefix:?}, got {arn:?}"
        );
        assert!(
            !seen_arns.contains(&arn),
            "case {case:?}: StartIncident reused an incident ARN, got {arn:?}"
        );
        seen_arns.push(arn.clone());

        let fetched = emulator.get_incident(&arn).await;
        assert_eq!(
            fetched.status, 200,
            "case {case:?}: expected 200 from GetIncidentRecord, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.text("/incidentRecord/arn"),
            arn,
            "case {case:?}: the record reported another ARN, got {:?}",
            fetched.body
        );
        assert_eq!(
            fetched.text("/incidentRecord/title"),
            expected_title,
            "case {case:?}: wrong title, got {:?}",
            fetched.body.pointer("/incidentRecord/title")
        );
        assert_eq!(
            fetched.number("/incidentRecord/impact"),
            Some(expected_impact as f64),
            "case {case:?}: wrong impact, got {:?}",
            fetched.body.pointer("/incidentRecord/impact")
        );
        assert_eq!(
            fetched.text("/incidentRecord/status"),
            "OPEN",
            "case {case:?}: a new incident must be OPEN, got {:?}",
            fetched.body.pointer("/incidentRecord/status")
        );
        assert_eq!(
            fetched.text("/incidentRecord/summary"),
            "template summary",
            "case {case:?}: summary must be inherited from the template, got {:?}",
            fetched.body.pointer("/incidentRecord/summary")
        );
        assert_eq!(
            fetched.text("/incidentRecord/incidentRecordSource/source"),
            expected_source,
            "case {case:?}: wrong incidentRecordSource.source, got {:?}",
            fetched.body.pointer("/incidentRecord/incidentRecordSource")
        );
        assert_eq!(
            fetched.text("/incidentRecord/incidentRecordSource/createdBy"),
            expected_principal(),
            "case {case:?}: wrong createdBy, got {:?}",
            fetched.body.pointer("/incidentRecord/incidentRecordSource/createdBy")
        );
        assert_eq!(
            fetched.text("/incidentRecord/lastModifiedBy"),
            expected_principal(),
            "case {case:?}: wrong lastModifiedBy, got {:?}",
            fetched.body.pointer("/incidentRecord/lastModifiedBy")
        );
        // Every `@required` member of `IncidentRecord` must be present.
        for required in [
            "arn",
            "title",
            "status",
            "impact",
            "creationTime",
            "lastModifiedTime",
            "lastModifiedBy",
            "incidentRecordSource",
            "dedupeString",
        ] {
            assert!(
                member(
                    fetched
                        .body
                        .pointer("/incidentRecord")
                        .unwrap_or(&Value::Null),
                    required
                )
                .is_some(),
                "case {case:?}: required member {required} is absent, got {:?}",
                fetched.body
            );
        }
        assert!(
            fetched.number("/incidentRecord/creationTime").unwrap_or(0.0) > 0.0,
            "case {case:?}: creationTime must be a real instant, got {:?}",
            fetched.body.pointer("/incidentRecord/creationTime")
        );
        assert!(
            member(
                fetched
                    .body
                    .pointer("/incidentRecord")
                    .unwrap_or(&Value::Null),
                "resolvedTime"
            )
            .is_none(),
            "case {case:?}: an OPEN incident has no resolvedTime, got {:?}",
            fetched.body.pointer("/incidentRecord/resolvedTime")
        );
        assert_eq!(
            fetched.body.pointer("/incidentRecord/notificationTargets"),
            Some(&json!([{ "snsTopicArn": format!("arn:aws:sns::{ACCOUNT}:updates") }])),
            "case {case:?}: notificationTargets must be inherited, got {:?}",
            fetched.body.pointer("/incidentRecord/notificationTargets")
        );
        assert_eq!(
            fetched.body.pointer("/incidentRecord/chatChannel"),
            Some(&json!({ "chatbotSns": [format!("arn:aws:sns::{ACCOUNT}:chat")] })),
            "case {case:?}: chatChannel must be inherited from the plan, got {:?}",
            fetched.body.pointer("/incidentRecord/chatChannel")
        );
    }

    // A template without a dedupeString leaves each incident its own, so no incident
    // deduplicates into another.
    let mut dedupes: Vec<String> = Vec::new();
    for arn in &seen_arns {
        let record = emulator.get_incident(arn).await;
        let dedupe = record.text("/incidentRecord/dedupeString").to_string();
        assert!(
            !dedupe.is_empty(),
            "incident {arn}: dedupeString must be populated, got {:?}",
            record.body
        );
        assert!(
            !dedupes.contains(&dedupe),
            "incident {arn}: a generated dedupeString must be unique, got {dedupe:?}"
        );
        dedupes.push(dedupe);
    }
}

/// A template that names a `dedupeString` folds a later incident into the open
/// incident already carrying it; once that incident is RESOLVED, the next start
/// creates a new one.
#[tokio::test]
async fn a_template_dedupe_string_folds_a_later_incident_into_the_open_one() {
    let emulator = Emulator::new();

    // Rows differ in the dedupe string itself: two plans with distinct strings must
    // not fold into each other.
    for dedupe in ["db-down", "cache_cold-2", "a"] {
        let plan_arn = emulator
            .create_plan(json!({
                "name": format!("dedupe-{}", dedupe.replace(['_'], "-")),
                "incidentTemplate": {
                    "title": "Deduped",
                    "impact": 2,
                    "dedupeString": dedupe,
                },
            }))
            .await;

        let first = emulator
            .start_incident(json!({ "responsePlanArn": plan_arn }))
            .await;
        let second = emulator
            .start_incident(json!({ "responsePlanArn": plan_arn }))
            .await;
        assert_eq!(
            second, first,
            "case {dedupe:?}: a duplicate dedupeString must fold into the open incident"
        );

        let stored = emulator.get_incident(&first).await;
        assert_eq!(
            stored.text("/incidentRecord/dedupeString"),
            dedupe,
            "case {dedupe:?}: the template's dedupeString must be stored, got {:?}",
            stored.body
        );

        let resolved = emulator
            .post(
                "/updateIncidentRecord",
                json!({ "arn": first, "status": "RESOLVED" }),
            )
            .await;
        assert_eq!(
            resolved.status, 204,
            "case {dedupe:?}: expected 204 from UpdateIncidentRecord, got {} body {:?}",
            resolved.status, resolved.body
        );

        let third = emulator
            .start_incident(json!({ "responsePlanArn": plan_arn }))
            .await;
        assert_ne!(
            third, first,
            "case {dedupe:?}: with no OPEN duplicate, a new incident must be created"
        );
    }
}

/// `UpdateIncidentRecord` mutates only the members it is given, bumps
/// `lastModifiedTime`/`lastModifiedBy`, and keeps `resolvedTime` present exactly while
/// the record is RESOLVED. A following `GetIncidentRecord` observes all of it.
#[tokio::test]
async fn an_update_mutates_only_the_supplied_members_and_tracks_resolution() {
    let emulator = Emulator::new();

    let plan_arn = emulator
        .create_plan(json!({
            "name": "update-plan",
            "incidentTemplate": { "title": "Before", "impact": 3, "summary": "before" },
            "chatChannel": { "chatbotSns": [format!("arn:aws:sns::{ACCOUNT}:chat")] },
        }))
        .await;

    // Rows differ in WHICH members the update supplies.
    let cases: Vec<(&str, Value)> = vec![
        ("title only", json!({ "title": "After" })),
        ("summary only", json!({ "summary": "after" })),
        ("impact at the low bound", json!({ "impact": 1 })),
        ("impact at the high bound", json!({ "impact": 5 })),
        ("status only", json!({ "status": "RESOLVED" })),
        (
            "every member at once",
            json!({
                "title": "After",
                "summary": "after",
                "impact": 4,
                "status": "RESOLVED",
                "notificationTargets": [
                    { "snsTopicArn": format!("arn:aws:sns::{ACCOUNT}:after") },
                ],
            }),
        ),
        ("clear the chat channel", json!({ "chatChannel": { "empty": {} } })),
    ];

    for (case, update) in cases {
        let arn = emulator
            .start_incident(json!({ "responsePlanArn": plan_arn }))
            .await;
        let before = emulator.get_incident(&arn).await;

        let mut body = update.clone();
        body["arn"] = json!(arn);
        let applied = emulator.post("/updateIncidentRecord", body).await;
        assert_eq!(
            applied.status, 204,
            "case {case:?}: expected 204, got {} body {:?}",
            applied.status, applied.body
        );

        let after = emulator.get_incident(&arn).await;
        let record = after
            .body
            .pointer("/incidentRecord")
            .cloned()
            .unwrap_or(Value::Null);

        // Supplied members changed to the supplied value...
        for (field, wanted) in update.as_object().expect("an update object") {
            if field == "chatChannel" {
                // `empty` means REMOVE the chat channel.
                assert!(
                    member(&record, "chatChannel").is_none(),
                    "case {case:?}: an empty chatChannel must clear the member, got {:?}",
                    record.get("chatChannel")
                );
                continue;
            }
            assert_eq!(
                member(&record, field),
                Some(wanted),
                "case {case:?}: {field} was not applied, got {:?}",
                record.get(field)
            );
        }
        // ...and the members it did not name kept their values.
        for field in ["title", "summary", "impact", "status", "notificationTargets"] {
            if update.get(field).is_some() {
                continue;
            }
            assert_eq!(
                member(&record, field),
                member(
                    before.body.pointer("/incidentRecord").unwrap_or(&Value::Null),
                    field
                ),
                "case {case:?}: {field} changed although the update did not name it, got {:?}",
                record.get(field)
            );
        }

        assert_eq!(
            record.get("lastModifiedBy").and_then(Value::as_str),
            Some(expected_principal().as_str()),
            "case {case:?}: lastModifiedBy must name the caller, got {:?}",
            record.get("lastModifiedBy")
        );
        assert!(
            record.get("lastModifiedTime").and_then(Value::as_f64)
                >= record.get("creationTime").and_then(Value::as_f64),
            "case {case:?}: lastModifiedTime must not precede creationTime, got {record:?}"
        );

        let resolved = record.get("status").and_then(Value::as_str) == Some("RESOLVED");
        assert_eq!(
            member(&record, "resolvedTime").is_some(),
            resolved,
            "case {case:?}: resolvedTime must be present exactly while RESOLVED, got {record:?}"
        );

        if resolved {
            // Reopening clears the resolution instant again.
            let reopened = emulator
                .post(
                    "/updateIncidentRecord",
                    json!({ "arn": arn, "status": "OPEN" }),
                )
                .await;
            assert_eq!(
                reopened.status, 204,
                "case {case:?}: expected 204 reopening, got {} body {:?}",
                reopened.status, reopened.body
            );
            let after_reopen = emulator.get_incident(&arn).await;
            assert_eq!(
                after_reopen.text("/incidentRecord/status"),
                "OPEN",
                "case {case:?}: reopening must set the status back to OPEN, got {:?}",
                after_reopen.body
            );
            assert!(
                after_reopen.body.pointer("/incidentRecord/resolvedTime").is_none(),
                "case {case:?}: reopening must clear resolvedTime, got {:?}",
                after_reopen.body.pointer("/incidentRecord/resolvedTime")
            );
        }
    }
}

/// A replayed `UpdateIncidentRecord` `clientToken` is a no-op: it does not apply the
/// update a second time.
#[tokio::test]
async fn a_replayed_update_client_token_does_not_apply_the_update_twice() {
    let emulator = Emulator::new();
    let plan_arn = emulator
        .create_plan(json!({
            "name": "update-idempotency",
            "incidentTemplate": { "title": "T", "impact": 3 },
        }))
        .await;

    for (case, token) in [("short token", "t1"), ("uuid-shaped token", "6f1b-4c2a-token")] {
        let arn = emulator
            .start_incident(json!({ "responsePlanArn": plan_arn }))
            .await;
        let update = json!({ "arn": arn, "clientToken": token, "title": "Applied once" });

        let first = emulator.post("/updateIncidentRecord", update.clone()).await;
        assert_eq!(
            first.status, 204,
            "case {case:?}: expected 204, got {} body {:?}",
            first.status, first.body
        );
        let once = emulator.get_incident(&arn).await;
        let stamp = once.number("/incidentRecord/lastModifiedTime");

        let replay = emulator.post("/updateIncidentRecord", update).await;
        assert_eq!(
            replay.status, 204,
            "case {case:?}: expected 204 on replay, got {} body {:?}",
            replay.status, replay.body
        );
        let twice = emulator.get_incident(&arn).await;
        assert_eq!(
            twice.number("/incidentRecord/lastModifiedTime"),
            stamp,
            "case {case:?}: a replay must not bump lastModifiedTime, got {:?}",
            twice.body
        );
        assert_eq!(
            twice.text("/incidentRecord/title"),
            "Applied once",
            "case {case:?}: the first update must still hold, got {:?}",
            twice.body
        );

        // The same token against a DIFFERENT incident is not a replay: it applies.
        let other = emulator
            .start_incident(json!({ "responsePlanArn": plan_arn }))
            .await;
        let applied = emulator
            .post(
                "/updateIncidentRecord",
                json!({ "arn": other, "clientToken": token, "title": "Applied elsewhere" }),
            )
            .await;
        assert_eq!(
            applied.status, 204,
            "case {case:?}: expected 204 for another incident, got {} body {:?}",
            applied.status, applied.body
        );
        let elsewhere = emulator.get_incident(&other).await;
        assert_eq!(
            elsewhere.text("/incidentRecord/title"),
            "Applied elsewhere",
            "case {case:?}: one token reused across incidents must update each, got {:?}",
            elsewhere.body
        );

        // A replayed token against a record that has since been deleted is a 404, not
        // a success for a resource that is gone.
        emulator
            .post("/deleteIncidentRecord", json!({ "arn": other }))
            .await;
        let after_delete = emulator
            .post(
                "/updateIncidentRecord",
                json!({ "arn": other, "clientToken": token, "title": "Applied elsewhere" }),
            )
            .await;
        assert_eq!(
            after_delete.status, 404,
            "case {case:?}: expected 404 replaying against a deleted record, got {} body {:?}",
            after_delete.status, after_delete.body
        );
        assert_eq!(
            after_delete.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            after_delete.body
        );
    }
}

/// `DeleteIncidentRecord` removes the record from state: `GetIncidentRecord` then
/// answers 404 and `ListIncidentRecords` no longer lists it. It is `@idempotent`, so
/// deleting an absent ARN still succeeds.
#[tokio::test]
async fn deleting_an_incident_removes_it_from_every_read_path_and_is_idempotent() {
    let emulator = Emulator::new();
    let plan_arn = emulator
        .create_plan(json!({
            "name": "delete-plan",
            "incidentTemplate": { "title": "T", "impact": 3 },
        }))
        .await;

    let kept = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn }))
        .await;

    // Rows: a record deleted while OPEN, one deleted while RESOLVED, and an ARN that
    // was never stored at all.
    let open = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn }))
        .await;
    let resolved = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn }))
        .await;
    emulator
        .post(
            "/updateIncidentRecord",
            json!({ "arn": resolved, "status": "RESOLVED" }),
        )
        .await;
    let absent =
        format!("arn:aws:ssm-incidents::{ACCOUNT}:incident-record/delete-plan/never-existed");

    for (case, arn, was_stored) in [
        ("open record", open.clone(), true),
        ("resolved record", resolved.clone(), true),
        ("absent record", absent.clone(), false),
    ] {
        for attempt in 1..=2 {
            let deleted = emulator
                .post("/deleteIncidentRecord", json!({ "arn": arn }))
                .await;
            assert_eq!(
                deleted.status, 204,
                "case {case:?} attempt {attempt}: expected 204, got {} body {:?}",
                deleted.status, deleted.body
            );
        }

        let fetched = emulator.get_incident(&arn).await;
        assert_eq!(
            fetched.status, 404,
            "case {case:?}: expected 404 after delete, got {} body {:?}",
            fetched.status, fetched.body
        );
        assert_eq!(
            fetched.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            fetched.body
        );
        let _ = was_stored;
    }

    let listed = emulator.post("/listIncidentRecords", json!({})).await;
    let arns: Vec<&str> = listed
        .body
        .pointer("/incidentRecordSummaries")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("arn").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        arns,
        vec![kept.as_str()],
        "only the undeleted record may still be listed, got {:?}",
        listed.body
    );
}

/// `ListIncidentRecords` projects every stored record and pages with `maxResults` /
/// `nextToken`: each page is at most `maxResults` long, the pages partition the stored
/// records exactly once, and the last page carries no token.
#[tokio::test]
async fn listing_pages_over_every_stored_record_and_terminates() {
    let emulator = Emulator::new();
    let plan_arn = emulator
        .create_plan(json!({
            "name": "paging-plan",
            "incidentTemplate": { "title": "T", "impact": 3 },
        }))
        .await;

    let mut expected: Vec<String> = Vec::new();
    for index in 0..7 {
        expected.push(
            emulator
                .start_incident(json!({
                    "responsePlanArn": plan_arn,
                    "title": format!("Incident {index}"),
                }))
                .await,
        );
    }
    expected.sort();

    // Rows: page sizes below, equal to and above the record count, plus each published
    // bound of `maxResults` (the model's range is 1..100), and "no maxResults at all".
    for page_size in [Some(1), Some(2), Some(3), Some(7), Some(100), None] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({});
            if let Some(size) = page_size {
                request["maxResults"] = json!(size);
            }
            if let Some(ref value) = token {
                request["nextToken"] = json!(value);
            }
            let reply = emulator.post("/listIncidentRecords", request.clone()).await;
            assert_eq!(
                reply.status, 200,
                "case {page_size:?}: expected 200, got {} body {:?}",
                reply.status, reply.body
            );
            let page = reply
                .body
                .pointer("/incidentRecordSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(size) = page_size {
                assert!(
                    page.len() <= size as usize,
                    "case {page_size:?}: a page exceeded maxResults, got {} items",
                    page.len()
                );
            }
            for summary in &page {
                // Every required `IncidentRecordSummary` member comes from the record.
                for required in ["arn", "title", "status", "impact", "creationTime", "incidentRecordSource"] {
                    assert!(
                        member(summary, required).is_some(),
                        "case {page_size:?}: summary is missing {required}, got {summary:?}"
                    );
                }
                seen.push(
                    summary
                        .get("arn")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                );
            }
            pages += 1;
            assert!(
                pages <= 20,
                "case {page_size:?}: paging did not terminate after {pages} pages"
            );
            token = reply
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        seen.sort();
        assert_eq!(
            seen, expected,
            "case {page_size:?}: paging must visit every stored record exactly once"
        );
    }
}

/// A `nextToken` names the record a page ended on, not an index, so deleting a record
/// from an already-returned page does not make the next page skip or repeat another.
#[tokio::test]
async fn paging_survives_a_delete_between_two_pages() {
    let emulator = Emulator::new();
    let plan_arn = emulator
        .create_plan(json!({
            "name": "stable-paging-plan",
            "incidentTemplate": { "title": "T", "impact": 3 },
        }))
        .await;

    let mut created: Vec<String> = Vec::new();
    for index in 0..5 {
        created.push(
            emulator
                .start_incident(json!({
                    "responsePlanArn": plan_arn,
                    "title": format!("Incident {index}"),
                }))
                .await,
        );
    }

    // Rows: delete the first, the middle and the last record of the first page.
    for victim_index in [0usize, 1, 0] {
        let first = emulator
            .post("/listIncidentRecords", json!({ "maxResults": 2 }))
            .await;
        let page_one: Vec<String> = arns_of(&first.body);
        assert_eq!(
            page_one.len(),
            2,
            "case {victim_index}: expected a 2-item first page, got {:?}",
            first.body
        );
        let token = first
            .body
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| panic!("case {victim_index}: expected a nextToken"));

        let victim = page_one[victim_index].clone();
        let deleted = emulator
            .post("/deleteIncidentRecord", json!({ "arn": victim }))
            .await;
        assert_eq!(
            deleted.status, 204,
            "case {victim_index}: expected 204 deleting mid-walk, got {} body {:?}",
            deleted.status, deleted.body
        );
        created.retain(|arn| *arn != victim);

        // Resuming must return exactly the records after the first page, none of which
        // the first page already showed.
        let mut rest: Vec<String> = Vec::new();
        let mut token = Some(token);
        while let Some(current) = token {
            let reply = emulator
                .post(
                    "/listIncidentRecords",
                    json!({ "maxResults": 2, "nextToken": current }),
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {victim_index}: expected 200 resuming, got {} body {:?}",
                reply.status, reply.body
            );
            rest.extend(arns_of(&reply.body));
            token = reply
                .body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        for shown in &page_one {
            assert!(
                !rest.contains(shown),
                "case {victim_index}: {shown} was returned twice across pages"
            );
        }
        let mut walked: Vec<String> = page_one
            .iter()
            .filter(|arn| created.contains(arn))
            .cloned()
            .chain(rest)
            .collect();
        walked.sort();
        let mut expected = created.clone();
        expected.sort();
        assert_eq!(
            walked, expected,
            "case {victim_index}: the walk must cover every surviving record once"
        );
    }
}

/// The ARNs of one `ListIncidentRecords` page, in the order it returned them.
fn arns_of(body: &Value) -> Vec<String> {
    body.pointer("/incidentRecordSummaries")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("arn").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A summary is derived from the same record `GetIncidentRecord` serves, so a change
/// made through `UpdateIncidentRecord` shows up in both.
#[tokio::test]
async fn a_summary_reports_the_same_state_the_record_does() {
    let emulator = Emulator::new();
    let plan_arn = emulator
        .create_plan(json!({
            "name": "summary-plan",
            "incidentTemplate": { "title": "Before", "impact": 3 },
        }))
        .await;
    let arn = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn }))
        .await;

    for (case, update) in [
        ("title", json!({ "title": "Renamed" })),
        ("impact", json!({ "impact": 2 })),
        ("status", json!({ "status": "RESOLVED" })),
    ] {
        let mut body = update;
        body["arn"] = json!(arn);
        emulator.post("/updateIncidentRecord", body).await;

        let record = emulator.get_incident(&arn).await;
        let listed = emulator.post("/listIncidentRecords", json!({})).await;
        let summary = listed
            .body
            .pointer("/incidentRecordSummaries/0")
            .cloned()
            .unwrap_or(Value::Null);
        for field in ["arn", "title", "status", "impact", "creationTime", "resolvedTime"] {
            assert_eq!(
                member(&summary, field),
                member(
                    record.body.pointer("/incidentRecord").unwrap_or(&Value::Null),
                    field
                ),
                "case {case:?}: the summary's {field} disagrees with the record, got {summary:?}"
            );
        }
    }
}

/// Each key `ListIncidentRecordsInput$filters` documents is applied; several filters
/// intersect, several values within one filter union, and a key or condition the
/// service does not filter on is refused with `ValidationException`.
#[tokio::test]
async fn listing_applies_each_documented_filter_key() {
    let emulator = Emulator::new();
    let plan_arn = emulator
        .create_plan(json!({
            "name": "filter-plan",
            "incidentTemplate": { "title": "T", "impact": 3 },
        }))
        .await;

    // Two records created before a midpoint second, two after it, with distinct
    // impacts and statuses.
    let early_open = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn, "impact": 1 }))
        .await;
    let early_resolved = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn, "impact": 2 }))
        .await;
    emulator
        .post(
            "/updateIncidentRecord",
            json!({ "arn": early_resolved, "status": "RESOLVED" }),
        )
        .await;

    // A whole second strictly between the two groups' creation instants.
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let midpoint = epoch_secs();

    let late_a = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn, "impact": 4 }))
        .await;
    let late_b = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn, "impact": 5 }))
        .await;

    let all = {
        let mut arns = vec![
            early_open.clone(),
            early_resolved.clone(),
            late_a.clone(),
            late_b.clone(),
        ];
        arns.sort();
        arns
    };

    let cases: Vec<(&str, Value, Vec<String>)> = vec![
        ("no filters", json!([]), all.clone()),
        (
            "status OPEN",
            json!([{ "key": "status", "condition": { "equals": { "stringValues": ["OPEN"] } } }]),
            sorted(vec![early_open.clone(), late_a.clone(), late_b.clone()]),
        ),
        (
            "status RESOLVED",
            json!([{ "key": "status", "condition": { "equals": { "stringValues": ["RESOLVED"] } } }]),
            vec![early_resolved.clone()],
        ),
        (
            "status OPEN or RESOLVED unions the values",
            json!([{
                "key": "status",
                "condition": { "equals": { "stringValues": ["OPEN", "RESOLVED"] } },
            }]),
            all.clone(),
        ),
        (
            "impact 1",
            json!([{ "key": "impact", "condition": { "equals": { "integerValues": [1] } } }]),
            vec![early_open.clone()],
        ),
        (
            "impact 4 or 5",
            json!([{ "key": "impact", "condition": { "equals": { "integerValues": [4, 5] } } }]),
            sorted(vec![late_a.clone(), late_b.clone()]),
        ),
        (
            "impact 3 matches nothing",
            json!([{ "key": "impact", "condition": { "equals": { "integerValues": [3] } } }]),
            Vec::new(),
        ),
        (
            "createdBy the caller",
            json!([{
                "key": "createdBy",
                "condition": { "equals": { "stringValues": [expected_principal()] } },
            }]),
            all.clone(),
        ),
        (
            "createdBy someone else",
            json!([{
                "key": "createdBy",
                "condition": {
                    "equals": { "stringValues": [format!("arn:aws:iam::{ACCOUNT}:user/other")] },
                },
            }]),
            Vec::new(),
        ),
        (
            "created after the midpoint",
            json!([{ "key": "creationTime", "condition": { "after": midpoint } }]),
            sorted(vec![late_a.clone(), late_b.clone()]),
        ),
        (
            "created before the midpoint",
            json!([{ "key": "creationTime", "condition": { "before": midpoint } }]),
            sorted(vec![early_open.clone(), early_resolved.clone()]),
        ),
        (
            "two filters intersect",
            json!([
                { "key": "creationTime", "condition": { "after": midpoint } },
                { "key": "impact", "condition": { "equals": { "integerValues": [5] } } },
            ]),
            vec![late_b.clone()],
        ),
    ];

    for (case, filters, expected) in cases {
        let reply = emulator
            .post(
                "/listIncidentRecords",
                json!({ "filters": filters, "maxResults": 100 }),
            )
            .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200, got {} body {:?}",
            reply.status, reply.body
        );
        let mut got: Vec<String> = reply
            .body
            .pointer("/incidentRecordSummaries")
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.get("arn").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        got.sort();
        assert_eq!(got, expected, "case {case:?}: wrong records matched");
    }

    // A key the service does not filter on, and a condition that cannot apply to a
    // key, are both bad requests.
    let refusals: Vec<(&str, Value)> = vec![
        (
            "unknown key",
            json!([{ "key": "title", "condition": { "equals": { "stringValues": ["T"] } } }]),
        ),
        (
            "equals on creationTime",
            json!([{ "key": "creationTime", "condition": { "equals": { "stringValues": ["now"] } } }]),
        ),
        (
            "before on impact",
            json!([{ "key": "impact", "condition": { "before": midpoint } }]),
        ),
        (
            "integerValues on status",
            json!([{ "key": "status", "condition": { "equals": { "integerValues": [1] } } }]),
        ),
    ];
    for (case, filters) in refusals {
        let reply = emulator
            .post("/listIncidentRecords", json!({ "filters": filters }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }

    // A token this service never issued is a bad request — including a bare number,
    // which an offset-shaped token would have silently accepted.
    for (case, token) in [
        ("not a token", "not-a-token"),
        ("zero", "0"),
        ("a large offset", "999999999"),
        ("hex that is not a token", "6162"),
        ("an empty token", ""),
    ] {
        let bad_token = emulator
            .post("/listIncidentRecords", json!({ "nextToken": token }))
            .await;
        assert_eq!(
            bad_token.status, 400,
            "case {case:?}: expected 400 for nextToken {token:?}, got {} body {:?}",
            bad_token.status, bad_token.body
        );
        assert_eq!(
            bad_token.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            bad_token.body
        );
    }

    // More filters than the model's `FilterList` allows is a bad request.
    let too_many: Vec<Value> = (0..6)
        .map(|_| json!({ "key": "status", "condition": { "equals": { "stringValues": ["OPEN"] } } }))
        .collect();
    let refused = emulator
        .post("/listIncidentRecords", json!({ "filters": too_many }))
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"six filters\": expected 400, got {} body {:?}",
        refused.status, refused.body
    );
}

/// The page size is CLAMPED to the modelled ceiling, not merely defaulted from it: no
/// number a request carries sizes a page or the allocation that builds it.
#[test]
fn the_page_size_is_clamped_to_the_modelled_ceiling() {
    use service_com_amazonaws_ssmincidents_ssmincidents::state::{
        page_size, MAX_INCIDENT_RECORDS_PER_PAGE,
    };

    let ceiling = MAX_INCIDENT_RECORDS_PER_PAGE as usize;
    // Rows: absent, each published bound of `maxResults` (1..100), one past the top,
    // non-positive values, and the extreme a malformed client could send.
    let cases = [
        (None, ceiling),
        (Some(1), 1),
        (Some(50), 50),
        (Some(MAX_INCIDENT_RECORDS_PER_PAGE), ceiling),
        (Some(MAX_INCIDENT_RECORDS_PER_PAGE + 1), ceiling),
        (Some(0), ceiling),
        (Some(-1), ceiling),
        (Some(i32::MAX), ceiling),
        (Some(i32::MIN), ceiling),
    ];
    for (requested, expected) in cases {
        let got = page_size(requested);
        assert_eq!(
            got, expected,
            "case {requested:?}: expected a page size of {expected}, got {got}"
        );
        assert!(
            got <= ceiling && got > 0,
            "case {requested:?}: a page size must be within 1..={ceiling}, got {got}"
        );
    }
}

/// The idempotency map is bounded with oldest-first eviction, so a caller sending a
/// fresh `clientToken` on every request cannot grow it without limit.
#[test]
fn the_idempotency_map_is_bounded_and_evicts_the_oldest_token() {
    use service_com_amazonaws_ssmincidents_ssmincidents::state::{
        ComAmazonawsSsmincidentsSsmincidentsState, MAX_IDEMPOTENCY_ENTRIES,
    };

    // Rows: the operations that mint through a token, driven past the bound.
    for operation in ["CreateResponsePlan", "StartIncident", "UpdateIncidentRecord"] {
        let mut state = ComAmazonawsSsmincidentsSsmincidentsState::default();
        let overflow = MAX_IDEMPOTENCY_ENTRIES + 25;
        for index in 0..overflow {
            let token = format!("token-{index}");
            state.remember_arn(operation, Some(&token), &format!("arn-{index}"));
        }
        assert_eq!(
            state.idempotency.len(),
            MAX_IDEMPOTENCY_ENTRIES,
            "case {operation:?}: the map must stay at its bound, got {}",
            state.idempotency.len()
        );
        assert_eq!(
            state.idempotency_order.len(),
            MAX_IDEMPOTENCY_ENTRIES,
            "case {operation:?}: the eviction queue must stay at its bound, got {}",
            state.idempotency_order.len()
        );
        assert!(
            state.replayed_arn(operation, Some("token-0")).is_none(),
            "case {operation:?}: the oldest token must have been evicted"
        );
        let newest = format!("token-{}", overflow - 1);
        assert_eq!(
            state.replayed_arn(operation, Some(&newest)),
            Some(format!("arn-{}", overflow - 1)),
            "case {operation:?}: the newest token must still replay"
        );
        // An empty or absent token is never remembered at all.
        state.remember_arn(operation, None, "arn-none");
        state.remember_arn(operation, Some(""), "arn-empty");
        assert_eq!(
            state.idempotency.len(),
            MAX_IDEMPOTENCY_ENTRIES,
            "case {operation:?}: an absent or empty token must add no entry, got {}",
            state.idempotency.len()
        );
    }
}

fn sorted(mut arns: Vec<String>) -> Vec<String> {
    arns.sort();
    arns
}

/// A filter whose key or condition the service does not support is refused WHATEVER
/// the store holds — an empty store included, where a per-record check would never run.
#[tokio::test]
async fn a_bad_filter_is_refused_even_when_nothing_is_stored() {
    let emulator = Emulator::new();

    let cases: Vec<(&str, Value)> = vec![
        (
            "unknown key",
            json!([{ "key": "title", "condition": { "equals": { "stringValues": ["T"] } } }]),
        ),
        (
            "equals on creationTime",
            json!([{ "key": "creationTime", "condition": { "equals": { "stringValues": ["now"] } } }]),
        ),
        (
            "before on impact",
            json!([{ "key": "impact", "condition": { "before": 1_700_000_000 } }]),
        ),
        (
            "after on status",
            json!([{ "key": "status", "condition": { "after": 1_700_000_000 } }]),
        ),
        (
            "integerValues on createdBy",
            json!([{ "key": "createdBy", "condition": { "equals": { "integerValues": [1] } } }]),
        ),
        (
            "a good filter beside a bad one",
            json!([
                { "key": "status", "condition": { "equals": { "stringValues": ["OPEN"] } } },
                { "key": "title", "condition": { "equals": { "stringValues": ["T"] } } },
            ]),
        ),
    ];

    for (case, filters) in cases {
        let reply = emulator
            .post("/listIncidentRecords", json!({ "filters": filters }))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400 on an empty store, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }

    // A SUPPORTED filter on an empty store is simply an empty page, not a refusal.
    let empty = emulator
        .post(
            "/listIncidentRecords",
            json!({
                "filters": [
                    { "key": "status", "condition": { "equals": { "stringValues": ["OPEN"] } } },
                ],
            }),
        )
        .await;
    assert_eq!(
        empty.status, 200,
        "case \"supported filter, empty store\": expected 200, got {} body {:?}",
        empty.status, empty.body
    );
    assert_eq!(
        empty.body.pointer("/incidentRecordSummaries"),
        Some(&json!([])),
        "case \"supported filter, empty store\": expected an empty page, got {:?}",
        empty.body
    );
}

/// An operation naming a resource that is not in the store answers
/// `ResourceNotFoundException` (404) — not a success, and not a 501.
#[tokio::test]
async fn an_unknown_resource_is_reported_as_not_found() {
    let emulator = Emulator::new();
    let plan = format!("arn:aws:ssm-incidents::{ACCOUNT}:response-plan/nope");
    let record = format!("arn:aws:ssm-incidents::{ACCOUNT}:incident-record/nope/1");

    let cases: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "GetResponsePlan",
            "GET",
            format!("/getResponsePlan?arn={}", query_encode(&plan)),
            None,
        ),
        (
            "GetIncidentRecord",
            "GET",
            format!("/getIncidentRecord?arn={}", query_encode(&record)),
            None,
        ),
        (
            "StartIncident",
            "POST",
            "/startIncident".to_string(),
            Some(json!({ "responsePlanArn": plan })),
        ),
        (
            "UpdateIncidentRecord",
            "POST",
            "/updateIncidentRecord".to_string(),
            Some(json!({ "arn": record, "status": "RESOLVED" })),
        ),
    ];

    for (case, method, path, body) in cases {
        let reply = emulator.call(method, &path, body).await;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            reply.body
        );
    }
}

/// Input outside what the model declares is refused with `ValidationException` (400):
/// an absent `@required` member, a value outside a `@range`, one outside an enum's
/// value set, and one failing a `@pattern`. The `impact` and `maxResults` rows walk
/// each published bound and one past it.
#[tokio::test]
async fn input_outside_the_model_is_refused_with_a_validation_exception() {
    let emulator = Emulator::new();
    let plan_arn = emulator
        .create_plan(json!({
            "name": "validation-plan",
            "incidentTemplate": { "title": "T", "impact": 3 },
        }))
        .await;
    let record_arn = emulator
        .start_incident(json!({ "responsePlanArn": plan_arn }))
        .await;

    let cases: Vec<(&str, &str, Value)> = vec![
        (
            "CreateResponsePlan without a name",
            "/createResponsePlan",
            json!({ "incidentTemplate": { "title": "T", "impact": 3 } }),
        ),
        (
            "CreateResponsePlan without an incidentTemplate",
            "/createResponsePlan",
            json!({ "name": "no-template" }),
        ),
        (
            "CreateResponsePlan with a space in the name",
            "/createResponsePlan",
            json!({ "name": "has a space", "incidentTemplate": { "title": "T", "impact": 3 } }),
        ),
        (
            "CreateResponsePlan with an empty name",
            "/createResponsePlan",
            json!({ "name": "", "incidentTemplate": { "title": "T", "impact": 3 } }),
        ),
        (
            "CreateResponsePlan with impact 0 (one below the range)",
            "/createResponsePlan",
            json!({ "name": "impact-zero", "incidentTemplate": { "title": "T", "impact": 0 } }),
        ),
        (
            "CreateResponsePlan with impact 6 (one above the range)",
            "/createResponsePlan",
            json!({ "name": "impact-six", "incidentTemplate": { "title": "T", "impact": 6 } }),
        ),
        (
            "StartIncident without a responsePlanArn",
            "/startIncident",
            json!({ "title": "no plan" }),
        ),
        (
            "StartIncident with an ARN failing the Arn pattern",
            "/startIncident",
            json!({ "responsePlanArn": "not-an-arn" }),
        ),
        (
            "StartIncident with impact 9",
            "/startIncident",
            json!({ "responsePlanArn": plan_arn, "impact": 9 }),
        ),
        (
            "StartIncident with impact 0",
            "/startIncident",
            json!({ "responsePlanArn": plan_arn, "impact": 0 }),
        ),
        (
            "UpdateIncidentRecord with a status outside the enum",
            "/updateIncidentRecord",
            json!({ "arn": record_arn, "status": "CLOSED" }),
        ),
        (
            "UpdateIncidentRecord with impact 6",
            "/updateIncidentRecord",
            json!({ "arn": record_arn, "impact": 6 }),
        ),
        (
            "UpdateIncidentRecord without an arn",
            "/updateIncidentRecord",
            json!({ "status": "RESOLVED" }),
        ),
        (
            "DeleteIncidentRecord with an ARN failing the pattern",
            "/deleteIncidentRecord",
            json!({ "arn": "nope" }),
        ),
        (
            "ListIncidentRecords with maxResults 0 (one below the range)",
            "/listIncidentRecords",
            json!({ "maxResults": 0 }),
        ),
        (
            "ListIncidentRecords with maxResults 101 (one above the range)",
            "/listIncidentRecords",
            json!({ "maxResults": 101 }),
        ),
    ];

    for (case, path, body) in cases {
        let reply = emulator.post(path, body).await;
        assert_eq!(
            reply.status, 400,
            "case {case:?}: expected 400, got {} body {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_code(),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            reply.body
        );
    }

    // And each published bound of the ranges above is ACCEPTED.
    for impact in 1..=5 {
        let reply = emulator
            .post(
                "/updateIncidentRecord",
                json!({ "arn": record_arn, "impact": impact }),
            )
            .await;
        assert_eq!(
            reply.status, 204,
            "case \"impact {impact}\": a value inside the range must be accepted, got {} body {:?}",
            reply.status, reply.body
        );
    }
    for max_results in [1, 100] {
        let reply = emulator
            .post("/listIncidentRecords", json!({ "maxResults": max_results }))
            .await;
        assert_eq!(
            reply.status, 200,
            "case \"maxResults {max_results}\": a value inside the range must be accepted, \
             got {} body {:?}",
            reply.status, reply.body
        );
    }
}

/// The whole workflow the issue describes, in order, on one emulator.
#[tokio::test]
async fn the_documented_client_workflow_runs_end_to_end() {
    let emulator = Emulator::new();

    let plan_arn = emulator
        .create_plan(json!({
            "name": "my-plan",
            "incidentTemplate": { "title": "DB down", "impact": 2 },
        }))
        .await;
    assert_eq!(
        plan_arn,
        format!("arn:aws:ssm-incidents::{ACCOUNT}:response-plan/my-plan"),
        "step 2: unexpected response-plan ARN"
    );

    let incident_arn = emulator
        .start_incident(json!({
            "responsePlanArn": plan_arn,
            "title": "DB down now",
            "impact": 1,
        }))
        .await;

    let opened = emulator.get_incident(&incident_arn).await;
    assert_eq!(
        opened.text("/incidentRecord/status"),
        "OPEN",
        "step 4: expected an OPEN incident, got {:?}",
        opened.body
    );
    assert_eq!(
        opened.text("/incidentRecord/title"),
        "DB down now",
        "step 4: the request's title must override the template's, got {:?}",
        opened.body
    );
    assert_eq!(
        opened.number("/incidentRecord/impact"),
        Some(1.0),
        "step 4: the request's impact must override the template's, got {:?}",
        opened.body
    );

    let listed = emulator.post("/listIncidentRecords", json!({})).await;
    assert_eq!(
        listed.text("/incidentRecordSummaries/0/arn"),
        incident_arn,
        "step 5: the new incident must be listed, got {:?}",
        listed.body
    );

    let resolved = emulator
        .post(
            "/updateIncidentRecord",
            json!({
                "arn": incident_arn,
                "status": "RESOLVED",
                "summary": "failover complete",
            }),
        )
        .await;
    assert_eq!(
        resolved.status, 204,
        "step 6: expected 204, got {} body {:?}",
        resolved.status, resolved.body
    );

    let after = emulator.get_incident(&incident_arn).await;
    assert_eq!(
        after.text("/incidentRecord/status"),
        "RESOLVED",
        "step 6: expected RESOLVED, got {:?}",
        after.body
    );
    assert_eq!(
        after.text("/incidentRecord/summary"),
        "failover complete",
        "step 6: expected the new summary, got {:?}",
        after.body
    );
    assert!(
        after.number("/incidentRecord/resolvedTime").is_some(),
        "step 6: a RESOLVED incident carries a resolvedTime, got {:?}",
        after.body
    );
    assert!(
        after.number("/incidentRecord/lastModifiedTime")
            >= opened.number("/incidentRecord/lastModifiedTime"),
        "step 6: lastModifiedTime must be bumped, got {:?}",
        after.body
    );

    let deleted = emulator
        .post("/deleteIncidentRecord", json!({ "arn": incident_arn }))
        .await;
    assert_eq!(
        deleted.status, 204,
        "step 7: expected 204, got {} body {:?}",
        deleted.status, deleted.body
    );

    let gone = emulator.get_incident(&incident_arn).await;
    assert_eq!(
        gone.status, 404,
        "step 7: expected 404 after the delete, got {} body {:?}",
        gone.status, gone.body
    );
    let empty = emulator.post("/listIncidentRecords", json!({})).await;
    assert_eq!(
        empty.body.pointer("/incidentRecordSummaries"),
        Some(&json!([])),
        "step 7: the deleted incident must no longer be listed, got {:?}",
        empty.body
    );
}
