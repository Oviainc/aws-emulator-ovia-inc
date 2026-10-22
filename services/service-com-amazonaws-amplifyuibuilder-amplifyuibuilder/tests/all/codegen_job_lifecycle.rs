// SPDX-License-Identifier: CC0-1.0

//! The Amplify UI Builder codegen job lifecycle, end to end over HTTP.
//!
//! Every assertion here goes through the router, so each case exercises routing,
//! deserialization, the handler, the persisted state and serialization — the same path a
//! client takes. The rules under test:
//!
//! 1. `StartCodegenJob` persists the configuration it was given and `GetCodegenJob`
//!    reports that record: whatever `StartCodegenJobData` carried comes back unchanged,
//!    under a fresh `Uuid`, for ANY configuration the model admits.
//! 2. A started job is reported `in_progress` and reaches a TERMINAL status
//!    (`failed`/`succeeded`) because the CLOCK made the transition due — never because
//!    somebody read it. A job that ended `succeeded` carries the asset its run
//!    published; this emulator performs no code generation run, so the status it owes is
//!    the modelled `failed` with a reason and no asset.
//! 3. `clientToken` is an `@idempotencyToken`: one token names one job.
//! 4. An id naming no job in the app environment addressed is the modelled
//!    `ResourceNotFoundException` (HTTP 404), not an empty success.

use std::collections::BTreeSet;
use std::sync::Arc;

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `CodegenJobStatus` values the model declares, spelled out here independently of the
/// constants `src/state.rs` carries: from
/// `smithy_query(query="shape",shape="CodegenJobStatus")` — `["in_progress", "failed",
/// "succeeded"]`.
const IN_PROGRESS: &str = "in_progress";
const FAILED: &str = "failed";
const SUCCEEDED: &str = "succeeded";

/// The two statuses a codegen job never leaves, out of the catalog above.
const TERMINAL_STATUSES: &[&str] = &[FAILED, SUCCEEDED];

/// A wait shorter than the span the emulator takes to make a job's terminal status due, so
/// a job read after it is still the transient `in_progress`. Kept well under that span so
/// several polls in a row still land before the due instant on a loaded machine.
const WITHIN_THE_DUE_INSTANT: std::time::Duration = std::time::Duration::from_millis(20);

/// A wait longer than that span, so the clock has made the terminal status due.
const PAST_THE_DUE_INSTANT: std::time::Duration = std::time::Duration::from_millis(800);

/// `JSModule`'s declared values, from
/// `smithy_query(query="shape",shape="JSModule")` — `["es2020", "esnext"]`.
const JS_MODULES: &[&str] = &["es2020", "esnext"];

/// `JSTarget`'s declared values (`smithy_query(query="shape",shape="JSTarget")`).
const JS_TARGETS: &[&str] = &["es2015", "es2020"];

/// `JSScript`'s declared values (`smithy_query(query="shape",shape="JSScript")`).
const JS_SCRIPTS: &[&str] = &["jsx", "tsx", "js"];

/// `CodegenJobGenericDataSourceType`'s only declared value
/// (`smithy_query(query="shape",shape="CodegenJobGenericDataSourceType")`).
const DATA_SOURCE_TYPE: &str = "DataStore";

/// A SigV4 credential scope naming this service: it is what routes a request to the
/// amplifyuibuilder service.
const SIGNED_FOR_AMPLIFYUIBUILDER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/amplifyuibuilder/aws4_request, \
     SignedHeaders=host, Signature=abc";

/// The app and backend environment the cases address, unless a case is about addressing
/// another one. `AppId` is `@length(min: 1, max: 20)` with pattern `^d[a-z0-9]+$`.
const APP_ID: &str = "d1234abcd";
const ENVIRONMENT: &str = "staging";

/// `CodegenJob$id` targets the model's `Uuid` shape. Checked here against the textual form
/// of a version 4 UUID rather than through the crate's own minting code, so an id that is
/// not a UUID cannot make this test pass.
fn is_uuid_v4(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() != 36 {
        return false;
    }
    for (index, byte) in bytes.iter().enumerate() {
        let ok = match index {
            8 | 13 | 18 | 23 => *byte == b'-',
            _ => byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase(),
        };
        if !ok {
            return false;
        }
    }
    // Version nibble, then the variant nibble the RFC pins to one of 8, 9, a, b.
    bytes[14] == b'4' && matches!(bytes[19], b'8' | b'9' | b'a' | b'b')
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
    /// `GetCodegenJob` both bind their `CodegenJob` to `@httpPayload`, so the body IS the
    /// job.
    fn id(&self) -> String {
        self.body["id"].as_str().unwrap_or_default().to_string()
    }

    /// The `status` member, or the empty string.
    fn job_status(&self) -> String {
        self.body["status"].as_str().unwrap_or_default().to_string()
    }

    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
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

fn jobs_uri(app_id: &str, environment: &str) -> String {
    format!("/app/{app_id}/environment/{environment}/codegen-jobs")
}

async fn start_in(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
) -> Answer {
    call(router, "POST", &jobs_uri(app_id, environment), Some(data)).await
}

async fn start(router: &MultiServiceRouter, data: Value) -> Answer {
    start_in(router, APP_ID, ENVIRONMENT, data).await
}

async fn start_in_with_token(
    router: &MultiServiceRouter,
    app_id: &str,
    environment: &str,
    data: Value,
    token: &str,
) -> Answer {
    let uri = format!("{}?clientToken={}", jobs_uri(app_id, environment), token);
    call(router, "POST", &uri, Some(data)).await
}

async fn start_with_token(router: &MultiServiceRouter, data: Value, token: &str) -> Answer {
    start_in_with_token(router, APP_ID, ENVIRONMENT, data, token).await
}

async fn get_in(router: &MultiServiceRouter, app_id: &str, environment: &str, id: &str) -> Answer {
    let uri = format!("{}/{id}", jobs_uri(app_id, environment));
    call(router, "GET", &uri, None).await
}

async fn get(router: &MultiServiceRouter, id: &str) -> Answer {
    get_in(router, APP_ID, ENVIRONMENT, id).await
}

/// True when a JSON value is a `date-time` string, the wire form the model pins on
/// `CodegenJob$createdAt` and `$modifiedAt`.
fn is_date_time(value: &Value) -> bool {
    value
        .as_str()
        .is_some_and(|text| DateTime::from_str(text, Format::DateTime).is_ok())
}

/// The instant a `date-time` member names, in seconds since the epoch. Panics on anything
/// that is not such a string, so a wrong wire form fails the case that reads it.
fn epoch_seconds(value: &Value) -> f64 {
    let text = value
        .as_str()
        .unwrap_or_else(|| panic!("expected a date-time string, got {value:?}"));
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|error| panic!("{text:?} is not an ISO-8601 date-time ({error})"))
        .as_secs_f64()
}

/// `renderConfig.react` as the service reports it back: `ReactStartCodegenJobData`
/// declares `renderTypeDeclarations` and `inlineSourceMap` with `@default(false)`
/// (`smithy_query(query="shape",shape="ReactStartCodegenJobData")`), so a request that
/// omits one is read as carrying `false` and the stored job reports it.
fn with_react_defaults(render_config: &Value) -> Value {
    let mut react = render_config["react"].clone();
    let members = react
        .as_object_mut()
        .expect("the react variant of renderConfig is an object");
    for defaulted in ["renderTypeDeclarations", "inlineSourceMap"] {
        members
            .entry(defaulted.to_string())
            .or_insert_with(|| json!(false));
    }
    json!({"react": react})
}

/// The smallest `StartCodegenJobData` the model admits: `renderConfig` is its only
/// `@required` member, and `CodegenJobRenderConfig` is a union with one variant.
fn minimal_data() -> Value {
    json!({"renderConfig": {"react": {}}})
}

/// A `CodegenJobGenericDataSchema` with one model, one enum and one non-model. Every
/// member of the shape and of `CodegenGenericDataField` is `@required`, so this is the
/// smallest well-formed schema that is not empty.
fn generic_data_schema() -> Value {
    json!({
        "dataSourceType": DATA_SOURCE_TYPE,
        "models": {
            "Todo": {
                "fields": {
                    "id": {
                        "dataType": "ID",
                        "dataTypeValue": "ID",
                        "required": true,
                        "readOnly": true,
                        "isArray": false
                    },
                    "owner": {
                        "dataType": "Model",
                        "dataTypeValue": "User",
                        "required": false,
                        "readOnly": false,
                        "isArray": false,
                        "relationship": {
                            "type": "BELONGS_TO",
                            "relatedModelName": "User"
                        }
                    }
                },
                "isJoinTable": false,
                "primaryKeys": ["id"]
            }
        },
        "enums": {"Priority": {"values": ["LOW", "HIGH"]}},
        "nonModels": {
            "Address": {
                "fields": {
                    "street": {
                        "dataType": "String",
                        "dataTypeValue": "String",
                        "required": true,
                        "readOnly": false,
                        "isArray": false
                    }
                }
            }
        }
    })
}

/// The start bodies the round-trip cases span. Rows differ along what the rule generalises
/// over: which members of `StartCodegenJobData` are present, which `renderConfig` values
/// and which `apiConfiguration` variant the React config carries, and how many tags and
/// dependencies the request supplies. The enum values are drawn from the catalogs declared
/// at the top of this file, including entries the issue never cited.
fn start_cases() -> Vec<(&'static str, Value)> {
    vec![
        ("only the required renderConfig", minimal_data()),
        (
            "every JSModule/JSTarget/JSScript at its first declared value",
            json!({"renderConfig": {"react": {
                "module": JS_MODULES[0],
                "target": JS_TARGETS[0],
                "script": JS_SCRIPTS[0],
                "renderTypeDeclarations": true,
                "inlineSourceMap": true
            }}}),
        ),
        (
            "the last declared value of each of those three enums",
            json!({"renderConfig": {"react": {
                "module": JS_MODULES[JS_MODULES.len() - 1],
                "target": JS_TARGETS[JS_TARGETS.len() - 1],
                "script": JS_SCRIPTS[JS_SCRIPTS.len() - 1]
            }}}),
        ),
        (
            "the graphQLConfig apiConfiguration variant",
            json!({"renderConfig": {"react": {
                "script": JS_SCRIPTS[1],
                "apiConfiguration": {"graphQLConfig": {
                    "typesFilePath": "src/api/types.ts",
                    "queriesFilePath": "src/api/queries.ts",
                    "mutationsFilePath": "src/api/mutations.ts",
                    "subscriptionsFilePath": "src/api/subscriptions.ts",
                    "fragmentsFilePath": "src/api/fragments.ts"
                }}
            }}}),
        ),
        (
            "the dataStoreConfig variant with two dependencies",
            json!({"renderConfig": {"react": {
                "apiConfiguration": {"dataStoreConfig": {}},
                "dependencies": {"@aws-amplify/ui-react": "^5.0.0", "aws-amplify": "^6.0.0"}
            }}}),
        ),
        (
            "the noApiConfig variant with a generic data schema and forms",
            json!({
                "renderConfig": {"react": {"apiConfiguration": {"noApiConfig": {}}}},
                "genericDataSchema": generic_data_schema(),
                "autoGenerateForms": true,
                "features": {"isRelationshipSupported": true, "isNonModelSupported": false}
            }),
        ),
        (
            "forms off and both feature flags off",
            json!({
                "renderConfig": {"react": {"script": JS_SCRIPTS[2]}},
                "autoGenerateForms": false,
                "features": {"isRelationshipSupported": false, "isNonModelSupported": false}
            }),
        ),
        (
            "no tags at all",
            json!({"renderConfig": {"react": {}}, "tags": {}}),
        ),
        (
            "three tags",
            json!({"renderConfig": {"react": {}}, "tags": {
                "team": "ui", "env": ENVIRONMENT, "owner": "codegen"
            }}),
        ),
    ]
}

/// `StartCodegenJob` persists what it was given and `GetCodegenJob` reports that record:
/// every member the start carried comes back with the value it was stored with, under a
/// fresh UUID, and the two bodies are equal.
#[tokio::test]
async fn a_started_codegen_job_is_read_back_from_what_was_stored() {
    let router = emulator();
    let mut minted: BTreeSet<String> = BTreeSet::new();

    for (case, data) in start_cases() {
        let started = start(&router, data.clone()).await;
        assert_eq!(
            started.status, 200,
            "case {case:?}: StartCodegenJob must answer 200, got {} with {:?}",
            started.status, started.body
        );

        let id = started.id();
        assert!(
            is_uuid_v4(&id),
            "case {case:?}: expected CodegenJob$id to be a version 4 UUID, got {id:?}"
        );
        assert!(
            minted.insert(id.clone()),
            "case {case:?}: {id:?} was already minted for an earlier job"
        );

        assert_eq!(
            started.body["appId"],
            json!(APP_ID),
            "case {case:?}: expected appId {APP_ID:?}, got {:?}",
            started.body["appId"]
        );
        assert_eq!(
            started.body["environmentName"],
            json!(ENVIRONMENT),
            "case {case:?}: expected environmentName {ENVIRONMENT:?}, got {:?}",
            started.body["environmentName"]
        );

        // Every member `StartCodegenJobData` carried is reported back as given; `tags` is
        // reported for every job, so an absent one reads as `{}`.
        let expected_render_config = with_react_defaults(&data["renderConfig"]);
        assert_eq!(
            started.body["renderConfig"], expected_render_config,
            "case {case:?}: expected renderConfig {expected_render_config:?}, got {:?}",
            started.body["renderConfig"]
        );
        for member in ["genericDataSchema", "autoGenerateForms", "features"] {
            if let Some(supplied) = data.get(member) {
                assert_eq!(
                    &started.body[member], supplied,
                    "case {case:?}: expected {member} {supplied:?}, got {:?}",
                    started.body[member]
                );
            }
        }
        let expected_tags = data.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            started.body["tags"], expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            started.body["tags"]
        );

        // A job the clock has not settled yet is the transient status, with no outcome
        // reported for a run that has not ended.
        assert_eq!(
            started.job_status(),
            IN_PROGRESS,
            "case {case:?}: a freshly started job must be {IN_PROGRESS:?}, got {:?}",
            started.body["status"]
        );
        assert!(
            started.body.get("statusMessage").is_none(),
            "case {case:?}: an unfinished job reports no statusMessage, got {:?}",
            started.body["statusMessage"]
        );
        assert!(
            started.body.get("asset").is_none(),
            "case {case:?}: an unfinished job publishes no asset, got {:?}",
            started.body["asset"]
        );

        assert!(
            is_date_time(&started.body["createdAt"]),
            "case {case:?}: createdAt must be a date-time string, got {:?}",
            started.body["createdAt"]
        );
        assert_eq!(
            started.body["createdAt"], started.body["modifiedAt"],
            "case {case:?}: a job nothing has changed reports createdAt and modifiedAt at \
             the same instant, got {:?} against {:?}",
            started.body["createdAt"], started.body["modifiedAt"]
        );

        let read = get(&router, &id).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: GetCodegenJob must answer 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body, started.body,
            "case {case:?}: GetCodegenJob must report the record StartCodegenJob stored, \
             got {:?} against {:?}",
            read.body, started.body
        );
    }
}

/// A started job is non-terminal, so the CLOCK owes it a terminal status: reading it
/// before the transition is due reports `in_progress` however often it is polled, and
/// after the due instant it reports a terminal status that then never moves again. This
/// emulator runs no code generator, so the status it owes is the modelled `failed`,
/// carrying the reason and publishing no asset.
#[tokio::test]
async fn a_codegen_job_reaches_a_terminal_status_the_clock_owes() {
    let router = emulator();

    // Rows differ in the configuration started and in how often the job is polled before
    // the transition falls due: neither changes when the status moves.
    let cases: Vec<(&str, Value, usize)> = vec![
        ("the minimal configuration, polled once", minimal_data(), 1),
        (
            "a schema-driven configuration, polled four times",
            json!({
                "renderConfig": {"react": {"script": JS_SCRIPTS[1], "module": JS_MODULES[1]}},
                "genericDataSchema": generic_data_schema(),
                "autoGenerateForms": true
            }),
            4,
        ),
        (
            "a tagged configuration, never polled before the due instant",
            json!({"renderConfig": {"react": {}}, "tags": {"team": "ui"}}),
            0,
        ),
    ];

    for (case, data, polls) in cases {
        let started = start(&router, data).await;
        let id = started.id();
        assert_eq!(
            started.job_status(),
            IN_PROGRESS,
            "case {case:?}: StartCodegenJob must report {IN_PROGRESS:?}, got {:?}",
            started.body["status"]
        );

        for poll in 0..polls {
            tokio::time::sleep(WITHIN_THE_DUE_INSTANT).await;
            let polled = get(&router, &id).await;
            assert_eq!(
                polled.job_status(),
                IN_PROGRESS,
                "case {case:?}: poll {poll} before the due instant must still report \
                 {IN_PROGRESS:?}, got {:?}",
                polled.body["status"]
            );
            assert_eq!(
                polled.body["modifiedAt"], started.body["modifiedAt"],
                "case {case:?}: poll {poll} must not move modifiedAt, got {:?} against {:?}",
                polled.body["modifiedAt"], started.body["modifiedAt"]
            );
        }

        tokio::time::sleep(PAST_THE_DUE_INSTANT).await;
        let settled = get(&router, &id).await;
        let status = settled.job_status();
        assert!(
            TERMINAL_STATUSES.contains(&status.as_str()),
            "case {case:?}: past the due instant the job must hold one of \
             {TERMINAL_STATUSES:?}, got {status:?}"
        );

        // The transition is a change to the resource, so it moves `modifiedAt` past
        // `createdAt` — and it was due at the instant the clock made it due, not at the
        // instant of the read.
        let created_at = epoch_seconds(&settled.body["createdAt"]);
        let modified_at = epoch_seconds(&settled.body["modifiedAt"]);
        assert_eq!(
            settled.body["createdAt"], started.body["createdAt"],
            "case {case:?}: the transition must not move createdAt, got {:?} against {:?}",
            settled.body["createdAt"], started.body["createdAt"]
        );
        assert!(
            modified_at > created_at,
            "case {case:?}: a settled job carries a modifiedAt after its createdAt, got \
             {modified_at} against {created_at}"
        );

        if status == SUCCEEDED {
            // Unreachable against this emulator, and asserted so it stays that way: a
            // `succeeded` job is one whose RUN published an artifact, so it may only be
            // reported with the asset that run produced.
            let download_url = settled.body["asset"]["downloadUrl"].as_str();
            assert!(
                download_url.is_some_and(|url| !url.is_empty()),
                "case {case:?}: a {SUCCEEDED:?} job publishes the asset its run produced, \
                 got {:?}",
                settled.body["asset"]
            );
        }

        // This emulator runs no Amplify UI code generator, so the terminal status it owes
        // is the modelled failure, carrying the reason and publishing no asset — never a
        // success with a download URL nothing serves.
        assert_eq!(
            status, FAILED,
            "case {case:?}: a job whose code generation run was never performed must end \
             {FAILED:?}, got {status:?}"
        );
        let reason = settled.body["statusMessage"].as_str();
        assert!(
            reason.is_some_and(|text| !text.is_empty()),
            "case {case:?}: a {FAILED:?} job says why it failed, got {:?}",
            settled.body["statusMessage"]
        );
        assert!(
            settled.body.get("asset").is_none(),
            "case {case:?}: a {FAILED:?} job published no asset, got {:?}",
            settled.body["asset"]
        );

        // Reading a settled job changes nothing: the status a reader sees is the one the
        // clock set, and a later reader sees exactly the same record.
        let again = get(&router, &id).await;
        assert_eq!(
            again.body, settled.body,
            "case {case:?}: a second read must report the same record, got {:?} against {:?}",
            again.body, settled.body
        );
    }
}

/// `clientToken` is an `@idempotencyToken`: the same token always names the job it first
/// started, distinct tokens name distinct jobs, a token is scoped to the app environment it
/// was used in, and no token at all starts a new job every time.
#[tokio::test]
async fn one_client_token_starts_one_codegen_job() {
    // Rows differ in token shape and in how often the start is retried.
    let cases: Vec<(&str, String, usize)> = vec![
        ("a plain token, retried once", "token-a".to_string(), 2),
        ("the same token, retried four times", "token-b".to_string(), 4),
        ("a one-character token", "x".to_string(), 3),
        ("a 64-character token", "t".repeat(64), 2),
    ];

    let router = emulator();
    let mut ids: BTreeSet<String> = BTreeSet::new();

    for (case, token, retries) in &cases {
        let first = start_with_token(&router, minimal_data(), token).await;
        let id = first.id();
        assert!(
            is_uuid_v4(&id),
            "case {case:?}: expected a UUID job id, got {id:?}"
        );
        for retry in 1..*retries {
            let repeated = start_with_token(&router, minimal_data(), token).await;
            assert_eq!(
                repeated.status, 200,
                "case {case:?}: retry {retry} must answer 200, got {} with {:?}",
                repeated.status, repeated.body
            );
            assert_eq!(
                repeated.id(),
                id,
                "case {case:?}: retry {retry} must report the job the token first started, \
                 got {:?}",
                repeated.id()
            );
            assert_eq!(
                repeated.body["createdAt"], first.body["createdAt"],
                "case {case:?}: retry {retry} must not restart the job, got createdAt {:?} \
                 against {:?}",
                repeated.body["createdAt"], first.body["createdAt"]
            );
        }
        assert!(
            ids.insert(id.clone()),
            "case {case:?}: token {token:?} must not name a job another token started"
        );
    }

    // The same token in another app environment addresses that environment's own job.
    let token = &cases[0].1;
    let other = start_in_with_token(&router, APP_ID, "production", minimal_data(), token).await;
    assert!(
        !ids.contains(&other.id()),
        "a token replayed against another environment must start that environment's own \
         job, got {:?}",
        other.id()
    );
    assert_eq!(
        get_in(&router, APP_ID, ENVIRONMENT, &other.id()).await.status,
        404,
        "a job started in another environment is not readable from {ENVIRONMENT:?}"
    );

    // With no token every start mints a new job.
    let untokened: Vec<String> = vec![
        start(&router, minimal_data()).await.id(),
        start(&router, minimal_data()).await.id(),
        start(&router, minimal_data()).await.id(),
    ];
    let distinct: BTreeSet<&String> = untokened.iter().collect();
    assert_eq!(
        distinct.len(),
        untokened.len(),
        "an untokened start must mint a new job every time, got {untokened:?}"
    );
}

/// An id that names no stored job in the app environment addressed is reported as missing:
/// `GetCodegenJob` answers the modelled `ResourceNotFoundException` (HTTP 404), never an
/// empty success.
#[tokio::test]
async fn an_id_naming_no_codegen_job_is_reported_as_missing() {
    let router = emulator();
    let id = start(&router, minimal_data()).await.id();

    // Rows differ in HOW the id fails to name a job in the addressed app environment.
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "a UUID nothing ever started",
            APP_ID.to_string(),
            ENVIRONMENT.to_string(),
            "11111111-1111-4111-8111-111111111111".to_string(),
        ),
        (
            "an id that is not a UUID at all",
            APP_ID.to_string(),
            ENVIRONMENT.to_string(),
            "not-a-job".to_string(),
        ),
        (
            "a real id read from another environment",
            APP_ID.to_string(),
            "production".to_string(),
            id.clone(),
        ),
        (
            "a real id read from another app",
            "d9999zzzz".to_string(),
            ENVIRONMENT.to_string(),
            id.clone(),
        ),
    ];

    for (case, app_id, environment, job_id) in cases {
        let answer = get_in(&router, &app_id, &environment, &job_id).await;
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected HTTP 404, got {} with {:?}",
            answer.status, answer.body
        );
        assert!(
            answer.error_code().contains("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            answer.error_code()
        );
    }

    // The job that does exist is still readable: a miss reports nothing about it.
    assert_eq!(
        get(&router, &id).await.status,
        200,
        "the started job must still be readable after the misses above"
    );
}
