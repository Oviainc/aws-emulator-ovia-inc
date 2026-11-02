// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for ApiGatewayV2 `DeleteRouteSettings`.
//!
//! The rule these cases assert: **a DELETE on
//! `/v2/apis/{ApiId}/stages/{StageName}/routesettings/{RouteKey}` removes EXACTLY the one
//! entry its route key addresses from the stored stage's route settings map and answers
//! 204 with no body, leaving every other entry — and every other member of that stage,
//! and every other stage — as it was; each label of that path addresses a resource, so a
//! level naming something the store does not hold is the modelled `NotFoundException`
//! (404) and changes nothing.** The operation models `NotFoundException` and
//! `TooManyRequestsException` only (`smithy_query(query="operation",
//! operation="DeleteRouteSettings")`), and its route key is a non-greedy label, so a key
//! carrying a space or a slash arrives percent-encoded inside that one path segment (each
//! case below names the wire label beside the route key it stands for).
//!
//! The rows span what the rule generalises over: how many entries the stage carries, the
//! position of the deleted key in the stored (key) order, the form of the route key
//! (`$default`, the WebSocket `$connect`/`$disconnect` keys, `METHOD /path`, a greedy
//! proxy path, a multi-segment path), which `RouteSettings` members the entry carries,
//! which OTHER members the stage carries, the form of the stage name (plain, `$default`,
//! the 128-character bound) and the parent api's protocol type. The recorded case that
//! filed the issue — a stage's route settings deleted and the stage read back — is ONE
//! row.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::{
    invalid_api_identifier_message, invalid_identifier_message, STAGE_RESOURCE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, emulator, error_code, instant};

/// The upper bound of `StringWithLengthBetween1And128`, the shape
/// `CreateStage$StageName` targets.
const MAX_STAGE_NAME_LEN: usize = 128;

/// AWS's sentence for a `RouteKey` the addressed stage holds no route settings under: the
/// kind that resolved to nothing, with the route key the request addressed it with quoted
/// inside it. Written out here rather than taken from the service, so the cases assert the
/// rule.
fn no_such_route_settings(route_key: &str) -> String {
    format!("The settings for the Route with key '{route_key}' does not exist")
}

/// `RouteKey` is a NON-greedy label of this operation's URI, so the route key a client
/// addresses arrives percent-encoded INSIDE that one path segment: its space is `%20`,
/// its `/` is `%2F`, and the `{`/`}` of a path-variable key are `%7B`/`%7D`. Every row
/// below names the wire label, which is what a client puts on the wire, beside the route
/// key it stands for.
/// One stage whose route settings one entry is deleted from.
struct Row {
    case: &'static str,
    /// The `CreateApi` body the stage is created under.
    api: Value,
    /// The `CreateStage` body, carrying the `routeSettings` map the delete acts on.
    stage: Value,
    /// The percent-encoded `RouteKey` label the DELETE addresses.
    delete: &'static str,
    /// The `routeSettings` map a later `GetStage` must report.
    remaining: fn() -> Value,
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
            case: "the only entry of a stage, a METHOD /path key",
            api: http_api("single-setting-api"),
            stage: json!({
                "stageName": "prod",
                "routeSettings": { "GET /items": { "detailedMetricsEnabled": true } }
            }),
            delete: "GET%20%2Fitems",
            remaining: || json!({}),
        },
        Row {
            case: "the middle key of three in key order, beside the stage's other members",
            api: http_api("three-settings-api"),
            stage: json!({
                "stageName": "staging",
                "description": "a stage with settings for several routes",
                "defaultRouteSettings": {
                    "detailedMetricsEnabled": true,
                    "throttlingBurstLimit": 500,
                    "throttlingRateLimit": 12.5
                },
                "routeSettings": {
                    "ANY /{proxy+}": { "throttlingBurstLimit": 5 },
                    "GET /items": { "detailedMetricsEnabled": true },
                    "POST /items": { "throttlingRateLimit": 2.5 }
                },
                "stageVariables": { "table_name": "items-v1" },
                "tags": { "team": "platform" }
            }),
            // `ANY` < `GET` < `POST` in key order, so this is the middle entry: the
            // wire label of `GET /items`.
            delete: "GET%20%2Fitems",
            remaining: || {
                json!({
                    "ANY /{proxy+}": { "throttlingBurstLimit": 5 },
                    "POST /items": { "throttlingRateLimit": 2.5 }
                })
            },
        },
        Row {
            case: "the first key of three on a WebSocket stage, whose entries carry the \
                   members only that protocol supports",
            api: websocket_api("socket-api"),
            stage: json!({
                "stageName": "sockets",
                "routeSettings": {
                    "$connect": {
                        "dataTraceEnabled": true,
                        "loggingLevel": "INFO",
                        "throttlingBurstLimit": 10,
                        "throttlingRateLimit": 5.5
                    },
                    "$default": { "loggingLevel": "ERROR" },
                    "$disconnect": { "dataTraceEnabled": false, "loggingLevel": "OFF" }
                }
            }),
            delete: "$connect",
            remaining: || {
                json!({
                    "$default": { "loggingLevel": "ERROR" },
                    "$disconnect": { "dataTraceEnabled": false, "loggingLevel": "OFF" }
                })
            },
        },
        Row {
            case: "the last key of two, a multi-segment path carrying every throttling \
                   member, on the $default stage",
            api: http_api("default-stage-api"),
            stage: json!({
                "stageName": "$default",
                "routeSettings": {
                    "DELETE /orders/{id}": { "detailedMetricsEnabled": false },
                    "PUT /orders/{id}/lines/{line}": {
                        "detailedMetricsEnabled": true,
                        "throttlingBurstLimit": 1,
                        "throttlingRateLimit": 0.5
                    }
                },
                "stageVariables": { "endpoint": "https://example.com/path" }
            }),
            // The wire label of `PUT /orders/{id}/lines/{line}`.
            delete: "PUT%20%2Forders%2F%7Bid%7D%2Flines%2F%7Bline%7D",
            remaining: || json!({ "DELETE /orders/{id}": { "detailedMetricsEnabled": false } }),
        },
        Row {
            case: "the $default route key of a stage whose name is at the upper bound of \
                   its shape",
            api: http_api("long-stage-name-api"),
            stage: json!({
                "stageName": "s".repeat(MAX_STAGE_NAME_LEN),
                "routeSettings": {
                    "$default": { "throttlingBurstLimit": 2000, "throttlingRateLimit": 1000.0 },
                    "GET /health": {}
                }
            }),
            delete: "$default",
            remaining: || json!({ "GET /health": {} }),
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

/// The stage document `GetStage` reports: the store is what answers, so every assertion
/// about what the delete persisted reads the resource back.
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

/// The URI one `DeleteRouteSettings` call addresses. `route_label` is the route key as a
/// client percent-encodes it for the operation's last, non-greedy path label; a stage name
/// carries only characters a path segment takes as they are.
fn delete_uri(api_id: &str, stage_name: &str, route_label: &str) -> String {
    format!("/v2/apis/{api_id}/stages/{stage_name}/routesettings/{route_label}")
}

/// Send the delete and assert the modelled success: 204 with an empty body.
async fn delete_route_settings(
    emulator: &MultiServiceRouter,
    case: &str,
    api_id: &str,
    stage_name: &str,
    route_label: &str,
) {
    let (status, body) = call(
        emulator,
        "DELETE",
        &delete_uri(api_id, stage_name, route_label),
        None,
    )
    .await;
    assert_eq!(
        status, 204,
        "case {case:?}: expected 204 from DeleteRouteSettings, got {status} {body}"
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

/// A delete that removed an entry REWROTE the stage, so its `LastUpdatedDate` — "the
/// timestamp when the stage was last updated" (`Stage$LastUpdatedDate`) — is a strictly
/// later instant than the one the stage reported before the call, and still no earlier
/// than `CreatedDate`, which a rewrite does not move. `__timestampIso8601` carries
/// sub-second precision, so two instants of one test are distinguishable.
fn assert_last_updated_advanced(case: &str, before: &Value, after: &Value) {
    let previous = instant(case, before, "lastUpdatedDate");
    let current = instant(case, after, "lastUpdatedDate");
    let created = instant(case, after, "createdDate");
    assert!(
        current > previous,
        "case {case:?}: expected LastUpdatedDate to advance past {previous}, got {current}"
    );
    assert!(
        current >= created,
        "case {case:?}: expected LastUpdatedDate not to predate CreatedDate {created}, \
         got {current}"
    );
    assert_eq!(
        before.get("lastUpdatedDate"),
        before.get("createdDate"),
        "case {case:?}: nothing had rewritten the stage before the delete, so it must still \
         report its creation instant, got {before}"
    );
}

/// A call that rewrote nothing left `LastUpdatedDate` exactly as it was: the member is
/// compared as rendered, so neither a re-stamp nor a re-render passes.
fn assert_last_updated_unchanged(case: &str, before: &Value, after: &Value) {
    assert_eq!(
        after.get("lastUpdatedDate"),
        before.get("lastUpdatedDate"),
        "case {case:?}: expected LastUpdatedDate to be untouched, got {after}"
    );
}

#[tokio::test]
async fn deleting_route_settings_removes_exactly_the_addressed_entry() {
    let emulator = emulator();

    for row in rows() {
        let api_id = create_api(&emulator, row.case, &row.api).await;
        let stage_name = create_stage(&emulator, row.case, &api_id, &row.stage).await;
        let before = get_stage(&emulator, row.case, &api_id, &stage_name).await;
        assert_eq!(
            before.get("routeSettings"),
            row.stage.get("routeSettings"),
            "case {:?}: the stage must carry the settings it was created with, got {before}",
            row.case
        );

        delete_route_settings(&emulator, row.case, &api_id, &stage_name, row.delete).await;

        let after = get_stage(&emulator, row.case, &api_id, &stage_name).await;
        let expected = (row.remaining)();
        assert_eq!(
            after.get("routeSettings"),
            Some(&expected),
            "case {:?}: expected the remaining settings {expected}, got {after}",
            row.case
        );
        assert_eq!(
            without(&after, &["routeSettings", "lastUpdatedDate"]),
            without(&before, &["routeSettings", "lastUpdatedDate"]),
            "case {:?}: the delete must touch nothing but the addressed entry",
            row.case
        );
        assert_last_updated_advanced(row.case, &before, &after);
    }
}

#[tokio::test]
async fn route_settings_the_store_does_not_hold_are_not_found() {
    let emulator = emulator();

    let settings = json!({
        "GET /items": { "detailedMetricsEnabled": true },
        "$default": { "throttlingBurstLimit": 7 }
    });
    let api_id = create_api(&emulator, "the api that does exist", &http_api("present")).await;
    let stage_name = create_stage(
        &emulator,
        "the stage that does exist",
        &api_id,
        &json!({
            "stageName": "prod",
            "defaultRouteSettings": { "throttlingBurstLimit": 99 },
            "routeSettings": settings
        }),
    )
    .await;
    let before = get_stage(&emulator, "the stage that does exist", &api_id, &stage_name).await;

    // A second api, deleted, so its id is one that USED to name an api; and a third api
    // carrying a stage with route settings of its own, so that stage's name and its key
    // name something of ANOTHER api and nothing of the api under test.
    let gone_api = create_api(&emulator, "the api that gets deleted", &http_api("gone")).await;
    let (status, body) = call(&emulator, "DELETE", &format!("/v2/apis/{gone_api}"), None).await;
    assert_eq!(status, 204, "expected 204 from DeleteApi, got {status} {body}");
    let other_api = create_api(&emulator, "another api", &http_api("elsewhere")).await;
    let other_stage = create_stage(
        &emulator,
        "a stage of another api",
        &other_api,
        &json!({
            "stageName": "only-over-there",
            "routeSettings": { "GET /elsewhere": { "detailedMetricsEnabled": true } }
        }),
    )
    .await;

    // An unknown ApiId: the api is what resolved to nothing, so the api is what the
    // refusal names, account-qualified.
    for (case, id) in [
        ("an id no api was ever minted with", "zzzzzzzzzz".to_string()),
        ("a deleted api's id", gone_api),
        ("an id of another shape entirely", "NOT-AN-API-ID".to_string()),
        ("a one-character id", "a".to_string()),
    ] {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &delete_uri(&id, &stage_name, "GET%20%2Fitems"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteRouteSettings, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            invalid_api_identifier_message(&id),
            "case {case:?}: expected the api sentence, got {body}"
        );
    }

    // A known api and a stage it does not hold: the stage is what resolved to nothing.
    for (case, name) in [
        ("a name no stage of this api carries", "nosuchstage".to_string()),
        ("a name that names a stage of another api", other_stage),
        ("the reserved $default name, unused here", "$default".to_string()),
        (
            "a name at the upper bound of its shape",
            "s".repeat(MAX_STAGE_NAME_LEN),
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &delete_uri(&api_id, &name, "GET%20%2Fitems"),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteRouteSettings, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            invalid_identifier_message(STAGE_RESOURCE),
            "case {case:?}: expected the stage sentence, got {body}"
        );
    }

    // A known api and stage, and a key that stage holds no settings under: the key level
    // is what resolved to nothing, and the refusal quotes the key it was addressed with.
    // Each case names the route key and the wire label that carries it.
    for (case, key, label) in [
        (
            "a route key no entry is stored under",
            "GET /nope",
            "GET%20%2Fnope",
        ),
        (
            "the same method and a different path",
            "GET /items/{id}",
            "GET%20%2Fitems%2F%7Bid%7D",
        ),
        (
            "the same path and a different method",
            "POST /items",
            "POST%20%2Fitems",
        ),
        (
            "a key stored only on a stage of another api",
            "GET /elsewhere",
            "GET%20%2Felsewhere",
        ),
        (
            "the WebSocket $connect key on an HTTP stage",
            "$connect",
            "$connect",
        ),
        ("a key differing only in case", "get /items", "get%20%2Fitems"),
        (
            "a key differing only in trailing whitespace",
            "GET /items ",
            "GET%20%2Fitems%20",
        ),
        (
            "a greedy proxy key no entry is stored under",
            "ANY /{proxy+}",
            "ANY%20%2F%7Bproxy%2B%7D",
        ),
    ] {
        let (status, body) = call(
            &emulator,
            "DELETE",
            &delete_uri(&api_id, &stage_name, label),
            None,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from DeleteRouteSettings, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "NotFoundException",
            "case {case:?}: expected NotFoundException, got {body}"
        );
        assert_eq!(
            as_str(&body, "message"),
            no_such_route_settings(key),
            "case {case:?}: expected the route settings sentence naming {key:?}, got {body}"
        );
    }

    // None of those refusals removed anything, and none of them reached the stage's
    // `DefaultRouteSettings`, which no route key addresses.
    let kept = get_stage(&emulator, "the stage that does exist", &api_id, &stage_name).await;
    assert_eq!(
        kept.get("routeSettings"),
        Some(&settings),
        "a refused delete must not touch the settings of a stage that does exist, got {kept}"
    );
    assert_eq!(
        kept.get("defaultRouteSettings"),
        Some(&json!({ "detailedMetricsEnabled": false, "throttlingBurstLimit": 99 })),
        "a refused delete must not touch the stage's default route settings, got {kept}"
    );
    // A refusal rewrote no record, so the stage was not "last updated" by one.
    assert_last_updated_unchanged("the stage that does exist", &before, &kept);
}

#[tokio::test]
async fn deleting_the_same_route_settings_twice_is_not_found_the_second_time() {
    let emulator = emulator();

    let api_id = create_api(&emulator, "the owning api", &websocket_api("repeat-api")).await;
    let stage_name = create_stage(
        &emulator,
        "a stage with two entries",
        &api_id,
        &json!({
            "stageName": "sockets",
            "routeSettings": {
                "$connect": { "loggingLevel": "INFO" },
                "$default": { "loggingLevel": "ERROR" }
            }
        }),
    )
    .await;

    delete_route_settings(&emulator, "first delete", &api_id, &stage_name, "$connect").await;

    let (status, body) = call(
        &emulator,
        "DELETE",
        &delete_uri(&api_id, &stage_name, "$connect"),
        None,
    )
    .await;
    assert_eq!(
        status, 404,
        "second delete: expected 404, got {status} {body}"
    );
    assert_eq!(
        error_code(&body),
        "NotFoundException",
        "second delete: expected NotFoundException, got {body}"
    );
    assert_eq!(
        as_str(&body, "message"),
        no_such_route_settings("$connect"),
        "second delete: expected the route settings sentence naming the key, got {body}"
    );

    // The entry that was never deleted survived both calls.
    let fetched = get_stage(&emulator, "after both deletes", &api_id, &stage_name).await;
    assert_eq!(
        fetched.get("routeSettings"),
        Some(&json!({ "$default": { "loggingLevel": "ERROR" } })),
        "expected only the entry that was never addressed, got {fetched}"
    );
}

#[tokio::test]
async fn deleting_route_settings_touches_only_the_stage_it_names() {
    let emulator = emulator();

    // Two apis, several stages each, every stage carrying the SAME two route keys, so
    // "only the entry of the stage named" is asserted over a store full of candidates.
    // The wire label of the key each delete addresses, `GET /items`.
    let deleted_label = "GET%20%2Fitems";
    let settings = json!({
        "GET /items": { "detailedMetricsEnabled": true },
        "$default": { "throttlingBurstLimit": 11 }
    });
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
                    "routeSettings": settings
                }),
            )
            .await;
            stages.push((case, api_id.clone(), stage_name));
        }
    }

    // Delete one key from one stage at a time; every stage not visited yet must still
    // report both of the entries it was created with.
    for index in 0..stages.len() {
        let (case, api_id, stage_name) = stages[index].clone();
        delete_route_settings(&emulator, &case, &api_id, &stage_name, deleted_label).await;

        for (position, (other_case, other_api, other_stage)) in stages.iter().enumerate() {
            let fetched = get_stage(&emulator, other_case, other_api, other_stage).await;
            let expected = if position <= index {
                json!({ "$default": { "throttlingBurstLimit": 11 } })
            } else {
                settings.clone()
            };
            assert_eq!(
                fetched.get("routeSettings"),
                Some(&expected),
                "case {case:?}: wrong settings for {other_case:?} after the delete, got {fetched}"
            );
        }
    }
}
