// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteAccessLogSettings`.
//!
//! The rule these cases assert: **`DeleteAccessLogSettings` clears the access log
//! settings member of the stage its `(ApiId, StageName)` pair names and answers 204 with
//! an empty body, leaving every other member of that stage — and every other stage, of
//! that api and of any other — as it was; it answers 404 `NotFoundException` when no api
//! carries that id, or that api holds no stage of that name.** Clearing a member that is
//! already clear is the state the call asks for, so the operation is idempotent, and the
//! model declares no other error for it (`NotFoundException`, `TooManyRequestsException`
//! — `smithy_query(query="operation", operation="DeleteAccessLogSettings")`).
//!
//! The rows span what the rule generalises over: which `AccessLogSettings` members the
//! stage carries (both, each alone, neither), the length of the `Format` — including the
//! upper bound of `StringWithLengthBetween1And1024`, the shape `AccessLogSettings$Format`
//! targets — the number of `$context` variables it selects, which OTHER optional members
//! the stage carries, the form of the stage name (plain, hyphened, `$default`, the
//! 128-character bound), the parent api's protocol type, and how many stages and apis the
//! store holds. The recorded case that filed the issue — delete a stage's access log
//! settings and observe the configuration gone — is ONE row of the first table.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    invalid_api_identifier_message, invalid_identifier_message, STAGE_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code, instant};

/// The upper bound of `StringWithLengthBetween1And1024`, the shape
/// `AccessLogSettings$Format` targets.
const MAX_FORMAT_LEN: usize = 1024;

/// The upper bound of `StringWithLengthBetween1And128`, the shape
/// `CreateStage$StageName` targets.
const MAX_STAGE_NAME_LEN: usize = 128;

/// The `$context` variable every access log format must select: "The format must include
/// at least $context.requestId" (`AccessLogSettings$Format`).
const REQUEST_ID_VARIABLE: &str = "$context.requestId";

/// An access log format of exactly [`MAX_FORMAT_LEN`] characters that still selects the
/// request id, so the bound is exercised by a format the service accepts.
fn format_at_upper_bound() -> String {
    let filler = "x".repeat(MAX_FORMAT_LEN - REQUEST_ID_VARIABLE.len());
    format!("{REQUEST_ID_VARIABLE}{filler}")
}

/// One stage whose access log settings get deleted.
struct Row {
    case: &'static str,
    /// The `CreateApi` body the stage is created under.
    api: Value,
    /// The `CreateStage` body, carrying the `accessLogSettings` to be deleted.
    stage: Value,
}

fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "both members set, the shortest format that selects the request id",
            api: http_api("logged-api"),
            stage: json!({
                "stageName": "prod",
                "accessLogSettings": {
                    "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:api-logs",
                    "format": REQUEST_ID_VARIABLE
                }
            }),
        },
        Row {
            case: "a destination and no format at all, which configures no logging",
            api: http_api("destination-only-api"),
            stage: json!({
                "stageName": "staging",
                "accessLogSettings": {
                    "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:no-format"
                }
            }),
        },
        Row {
            case: "a format at the upper bound of its shape and no destination",
            api: http_api("long-format-api"),
            stage: json!({
                "stageName": "pre-prod_v2",
                "accessLogSettings": { "format": format_at_upper_bound() }
            }),
        },
        Row {
            case: "a format selecting several $context variables, beside every other \
                   optional member the stage stores",
            api: http_api("verbose-api"),
            stage: json!({
                "stageName": "observed",
                "description": "a stage with access logging and everything else",
                "autoDeploy": true,
                "defaultRouteSettings": {
                    "detailedMetricsEnabled": true,
                    "throttlingBurstLimit": 500,
                    "throttlingRateLimit": 12.5
                },
                "routeSettings": {
                    "GET /x": { "detailedMetricsEnabled": true }
                },
                "stageVariables": { "table_name": "items-v1" },
                "accessLogSettings": {
                    "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:verbose",
                    "format": "$context.requestId $context.identity.sourceIp $context.status \
                               $context.protocol $context.responseLength"
                },
                "tags": { "team": "platform", "env": "test" }
            }),
        },
        Row {
            case: "the $default stage of a WebSocket api, which also carries a client \
                   certificate",
            api: websocket_api("socket-api"),
            stage: json!({
                "stageName": "$default",
                "clientCertificateId": "abcd1234ef",
                "accessLogSettings": {
                    "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:sockets",
                    "format": "$context.requestId $context.eventType"
                }
            }),
        },
        Row {
            case: "a stage name at the upper bound of its shape: 128 characters",
            api: http_api("long-stage-name-api"),
            stage: json!({
                "stageName": "s".repeat(MAX_STAGE_NAME_LEN),
                "accessLogSettings": {
                    "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:long",
                    "format": REQUEST_ID_VARIABLE
                }
            }),
        },
    ]
}

/// Create one api and answer its `ApiId`.
async fn create_api(emulator: &MultiServiceRouter, case: &str, request: &Value) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Create one stage under `api_id` and answer the `StageName` it is keyed by.
async fn create_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    request: &Value,
) -> String {
    let (status, created) = call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(request.clone()),
    )
    .await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateStage, got {status} {created}"
    );
    as_str(&created, "stageName")
}

/// The stage document `GetStage` reports.
async fn get_stage(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage_name: &str,
) -> Value {
    let (status, fetched) = call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetStage, got {status} {fetched}"
    );
    fetched
}

/// Send the delete and assert the modelled success: 204 with an empty body.
async fn delete_settings(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage_name: &str,
) {
    let (status, body) = call(
        emulator,
        "DELETE",
        &format!("/v2/apis/{api_id}/stages/{stage_name}/accesslogsettings"),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected 204 from DeleteAccessLogSettings, got {status} {body}"
    );
    assert_eq!(
        body,
        Value::Null,
        "case {case:?}: a 204 carries no body, got {body}"
    );
}

/// The stage document with `members` removed, so two documents can be compared on
/// everything the delete must not decide.
fn without(record: &Value, members: &[&str]) -> Value {
    let mut stripped = record.clone();
    if let Some(object) = stripped.as_object_mut() {
        for member in members {
            object.remove(*member);
        }
    }
    stripped
}

/// The stage was rewritten no earlier than it was created and no earlier than the last
/// time it was read: `LastUpdatedDate` is "the timestamp when the stage was last
/// updated" (`Stage$LastUpdatedDate`), so it never moves backwards and never predates
/// `CreatedDate`.
fn assert_last_updated_not_before(case: &str, before: &Value, after: &Value) {
    let previous = instant(case, before, "lastUpdatedDate");
    let current = instant(case, after, "lastUpdatedDate");
    let created = instant(case, after, "createdDate");
    assert!(
        current >= previous,
        "case {case:?}: expected LastUpdatedDate not to move backwards, got {current} \
         after {previous}"
    );
    assert!(
        current >= created,
        "case {case:?}: expected LastUpdatedDate not to predate CreatedDate {created}, \
         got {current}"
    );
}

#[tokio::test]
async fn delete_access_log_settings_clears_the_settings_and_keeps_the_rest_of_the_stage() {
    let emulator = emulator();

    for row in rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let stage_name = create_stage(&emulator, row.case, &api_id, &row.stage).await;
        let before = get_stage(&emulator, row.case, &api_id, &stage_name).await;
        assert!(
            before.get("accessLogSettings").is_some(),
            "case {:?}: the stage must carry the settings it was created with, got {before}",
            row.case
        );

        delete_settings(&emulator, row.case, &api_id, &stage_name).await;

        let after = get_stage(&emulator, row.case, &api_id, &stage_name).await;
        assert_eq!(
            after.get("accessLogSettings"),
            None,
            "case {:?}: the access log settings must be gone, got {after}",
            row.case
        );
        assert_eq!(
            without(&after, &["accessLogSettings", "lastUpdatedDate"]),
            without(&before, &["accessLogSettings", "lastUpdatedDate"]),
            "case {:?}: the delete must touch nothing but the access log settings",
            row.case
        );
        assert_last_updated_not_before(row.case, &before, &after);

        // Deleting what is already deleted asks for the state the stage is already in,
        // so it answers exactly as the first call did and clears nothing further.
        delete_settings(&emulator, row.case, &api_id, &stage_name).await;
        let again = get_stage(&emulator, row.case, &api_id, &stage_name).await;
        assert_eq!(
            without(&again, &["lastUpdatedDate"]),
            without(&after, &["lastUpdatedDate"]),
            "case {:?}: the repeated delete changed the stage",
            row.case
        );
        assert_last_updated_not_before(row.case, &after, &again);
    }
}

#[tokio::test]
async fn delete_access_log_settings_on_a_stage_carrying_none_changes_nothing() {
    let emulator = emulator();

    // A stage that never had access log settings is already in the state the call asks
    // for, and the operation models no error for it.
    let cases = [
        (
            "a stage of an HTTP api created without accessLogSettings",
            http_api("unlogged-http-api"),
            json!({ "stageName": "prod" }),
        ),
        (
            "a stage of a WebSocket api carrying other members but no accessLogSettings",
            websocket_api("unlogged-socket-api"),
            json!({
                "stageName": "sockets",
                "description": "no access logging here",
                "stageVariables": { "endpoint": "https://example.com/path" }
            }),
        ),
    ];

    for (case, api, stage) in cases {
        let api_id = create_api(&emulator, case, &api).await;
        let stage_name = create_stage(&emulator, case, &api_id, &stage).await;
        let before = get_stage(&emulator, case, &api_id, &stage_name).await;
        assert_eq!(
            before.get("accessLogSettings"),
            None,
            "case {case:?}: this stage must carry no access log settings, got {before}"
        );

        delete_settings(&emulator, case, &api_id, &stage_name).await;

        let after = get_stage(&emulator, case, &api_id, &stage_name).await;
        assert_eq!(
            without(&after, &["lastUpdatedDate"]),
            without(&before, &["lastUpdatedDate"]),
            "case {case:?}: the delete must leave a stage with no settings untouched"
        );
        assert_last_updated_not_before(case, &before, &after);
    }
}

#[tokio::test]
async fn delete_access_log_settings_refuses_an_unknown_api_or_stage() {
    let emulator = emulator();

    let settings = json!({
        "destinationArn": "arn:aws:logs:eu-west-1:000000000000:log-group:kept",
        "format": REQUEST_ID_VARIABLE
    });
    let api_id = create_api(&emulator, "the api that does exist", &http_api("present")).await;
    let stage_name = create_stage(
        &emulator,
        "the stage that does exist",
        &api_id,
        &json!({ "stageName": "prod", "accessLogSettings": settings }),
    )
    .await;

    // A second api, deleted, so its id is one that USED to name an api; and a third api
    // carrying a stage of its own, so that stage's name is one that names a stage of
    // ANOTHER api and nothing of the api under test.
    let gone_api = create_api(&emulator, "the api that gets deleted", &http_api("gone")).await;
    let (status, body) = call(&emulator, "DELETE", &format!("/v2/apis/{gone_api}"), None).await;
    assert_eq!(status, 204, "expected 204 from DeleteApi, got {status} {body}");
    let other_api = create_api(&emulator, "another api", &http_api("elsewhere")).await;
    let other_stage = create_stage(
        &emulator,
        "a stage of another api",
        &other_api,
        &json!({ "stageName": "only-over-there" }),
    )
    .await;

    // An unknown ApiId: the api is what resolved to nothing, so the api is what the
    // refusal names, account-qualified.
    let unknown_apis = [
        ("an id no api was ever minted with", "zzzzzzzzzz".to_string()),
        ("a deleted api's id", gone_api),
        ("an id of another shape entirely", "NOT-AN-API-ID".to_string()),
        ("a one-character id", "a".to_string()),
    ];
    for (case, id) in unknown_apis {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/apis/{id}/stages/{stage_name}/accesslogsettings"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteAccessLogSettings, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(invalid_api_identifier_message(&id).as_str()),
            "case {case:?}: expected the api sentence, got {body}"
        );
    }

    // A known api and a stage it does not hold: the stage is what resolved to nothing.
    let unknown_stages = [
        ("a name no stage of this api carries", "nosuchstage".to_string()),
        ("a name that names a stage of another api", other_stage),
        ("the reserved $default name, unused here", "$default".to_string()),
        (
            "a name at the upper bound of its shape",
            "s".repeat(MAX_STAGE_NAME_LEN),
        ),
    ];
    for (case, name) in unknown_stages {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &format!("/v2/apis/{api_id}/stages/{name}/accesslogsettings"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteAccessLogSettings, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(invalid_identifier_message(STAGE_RESOURCE).as_str()),
            "case {case:?}: expected the stage sentence, got {body}"
        );
    }

    // Every refusal left the stage that does exist exactly as it was.
    let kept = get_stage(&emulator, "the stage that does exist", &api_id, &stage_name).await;
    assert_eq!(
        kept.get("accessLogSettings"),
        Some(&settings),
        "a refused delete must not touch the settings of a stage that does exist, got {kept}"
    );
}

#[tokio::test]
async fn delete_access_log_settings_touches_only_the_stage_it_names() {
    let emulator = emulator();

    // Two apis, several stages each, every stage with settings of its own, so "only the
    // stage named" is asserted over a store holding many candidates.
    let mut stages = Vec::new();
    for api_index in 0..2 {
        let case = format!("api #{api_index}");
        let api_id = create_api(&emulator, &case, &http_api(&format!("multi-{api_index}"))).await;
        for stage_index in 0..3 {
            let case = format!("api #{api_index} stage #{stage_index}");
            let stage_name = create_stage(
                &emulator,
                &case,
                &api_id,
                &json!({
                    "stageName": format!("stage-{stage_index}"),
                    "accessLogSettings": {
                        "destinationArn": format!(
                            "arn:aws:logs:eu-west-1:000000000000:log-group:logs-{api_index}-{stage_index}"
                        ),
                        "format": format!("{REQUEST_ID_VARIABLE} #{stage_index}")
                    }
                }),
            )
            .await;
            let created = get_stage(&emulator, &case, &api_id, &stage_name).await;
            stages.push((case, api_id.clone(), stage_name, created));
        }
    }

    // Delete one stage's settings at a time; every stage not visited yet must still
    // report exactly the settings it was created with.
    for index in 0..stages.len() {
        let (case, api_id, stage_name, _) = stages[index].clone();
        delete_settings(&emulator, &case, &api_id, &stage_name).await;

        for (other_case, other_api, other_stage, created) in &stages {
            let fetched = get_stage(&emulator, other_case, other_api, other_stage).await;
            let visited = stages
                .iter()
                .take(index + 1)
                .any(|(_, seen_api, seen_stage, _)| {
                    seen_api == other_api && seen_stage == other_stage
                });
            let expected = if visited {
                without(created, &["accessLogSettings"])
            } else {
                created.clone()
            };
            assert_eq!(
                without(&fetched, &["lastUpdatedDate"]),
                without(&expected, &["lastUpdatedDate"]),
                "case {case:?}: wrong record for {other_case:?} after the delete"
            );
        }
    }
}
