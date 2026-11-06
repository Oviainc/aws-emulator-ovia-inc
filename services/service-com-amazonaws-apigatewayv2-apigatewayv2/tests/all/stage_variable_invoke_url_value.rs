// SPDX-License-Identifier: CC0-1.0

//! A stage variable may carry the host-and-path portion of the invoke URL this service
//! ANNOUNCES for a stage.
//!
//! RULE: a stage variable's value has to match `[A-Za-z0-9-._~:/?#&=,]+`
//! (`Stage$StageVariables`, "the values must match [A-Za-z0-9-._~:/?#&=,]+"), and every
//! invoke URL this service announces for a stage of an api is built from characters that
//! pattern admits — the `ApiEndpoint` it reports, plus the stage name for a NAMED stage,
//! since the reserved `$default` stage is the one that serves the api's root and is reached
//! with no stage segment at all. Interpolating such a URL into a stage variable is therefore
//! always accepted. That is the documented HTTP-proxy use case: a stage variable holding the
//! backend host and path that an `HTTP_PROXY` `IntegrationUri` is assembled from per stage.
//!
//! The two halves of the rule are asserted together, so neither the announced URL nor the
//! validator can drift out of the other's reach: the value every row sends is READ OFF the
//! api this test just created rather than written out here, and the stage it creates round
//! trips through `GetStage`. The second test pins the pattern's far edge on the one
//! character that decides the rule — `$`, which the pattern does not admit and which is
//! therefore what no announced invoke URL may put in a path a client interpolates.
//!
//! The rows span what the rule generalises over: the api's `ProtocolType` (`HTTP` and
//! `WEBSOCKET`, whose announced endpoints carry different schemes), the stage the URL is
//! built for (the reserved `$default` served at the api's root, and named stages spelled
//! with a hyphen and an underscore), and the FORM of the value taken from that URL (the
//! invoke URL's authority-and-path, and the same with a resource path and a query after it).

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::DEFAULT_STAGE_NAME;

use crate::api_lifecycle::{as_str, call, emulator, error_code};

/// One row: the api to create and the stage the variable's URL is built for.
struct Row {
    case: &'static str,
    protocol_type: &'static str,
    stage_name: &'static str,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "an HTTP api's $default stage, served at the api's root",
            protocol_type: "HTTP",
            stage_name: DEFAULT_STAGE_NAME,
        },
        Row {
            case: "an HTTP api's named stage",
            protocol_type: "HTTP",
            stage_name: "prod",
        },
        Row {
            case: "an HTTP api's stage name carrying a hyphen and an underscore",
            protocol_type: "HTTP",
            stage_name: "blue_green-2",
        },
        Row {
            case: "a WEBSOCKET api's named stage",
            protocol_type: "WEBSOCKET",
            stage_name: "live",
        },
    ]
}

/// The host-and-path portion of `endpoint`: everything after the scheme delimiter, which is
/// what a client interpolates into an `HTTP_PROXY` `IntegrationUri`.
fn host_and_path(endpoint: &str) -> &str {
    endpoint
        .split_once("://")
        .map_or(endpoint, |(_scheme, rest)| rest)
}

/// The `CreateApi` body for one row.
fn create_api_body(name: &str, protocol_type: &str) -> Value {
    let mut request = json!({ "name": name, "protocolType": protocol_type });
    if protocol_type == "WEBSOCKET" {
        // A WEBSOCKET api dispatches by this expression, which its create requires.
        request["routeSelectionExpression"] = json!("$request.body.action");
    }
    request
}

#[tokio::test]
async fn a_stage_variable_may_carry_an_announced_invoke_url_host_and_path() {
    let emulator = emulator();

    for (index, row) in rows().into_iter().enumerate() {
        let case = row.case;
        let (status, api) = call(
            &emulator,
            "POST",
            "/v2/apis",
            Some(create_api_body(
                &format!("stage-variable-url-{index}"),
                row.protocol_type,
            )),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateApi, got {status} {api}"
        );
        let api_id = as_str(&api, "apiId");

        // The URL this service ANNOUNCED for the api, not one written out here.
        let announced = as_str(&api, "apiEndpoint");
        assert!(
            !announced.is_empty(),
            "case {case:?}: expected CreateApi to announce an ApiEndpoint, got {api}"
        );
        // The stage's own invoke URL, as a client builds it: the announced endpoint, with
        // the stage name appended for a named stage and nothing appended for the reserved
        // stage that serves the api's root.
        let authority = if row.stage_name == DEFAULT_STAGE_NAME {
            host_and_path(&announced).to_string()
        } else {
            format!("{}/{}", host_and_path(&announced), row.stage_name)
        };

        // The forms a client builds from that URL: the authority and path themselves, the
        // same with a resource path, and the same with a query string.
        let values: Vec<(&str, String)> = vec![
            ("the stage's own invoke URL", authority.clone()),
            (
                "that URL with a resource path",
                format!("{authority}/items"),
            ),
            (
                "that URL with a resource path and a query",
                format!("{authority}/items?tier=gold&page=2"),
            ),
        ];
        let variables: serde_json::Map<String, Value> = values
            .iter()
            .enumerate()
            .map(|(position, (_form, value))| (format!("backendUrl{position}"), json!(value)))
            .collect();

        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({
                "stageName": row.stage_name,
                "stageVariables": Value::Object(variables.clone()),
            })),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateStage for the stage variables {:?}, got \
             {status} {created}",
            values
                .iter()
                .map(|(form, value)| format!("{form}: {value}"))
                .collect::<Vec<String>>()
        );
        assert_eq!(
            created.get("stageVariables"),
            Some(&Value::Object(variables.clone())),
            "case {case:?}: expected the stage to report the variables it was created with, \
             got {created}"
        );

        // The round trip: the reader reports the values the create persisted, character for
        // character.
        let (status, read) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{}", row.stage_name),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} {read}"
        );
        assert_eq!(
            read.get("stageVariables"),
            Some(&Value::Object(variables)),
            "case {case:?}: expected GetStage to report the stored stage variables, got {read}"
        );
    }
}

#[tokio::test]
async fn a_value_carrying_a_character_the_pattern_excludes_is_refused() {
    let emulator = emulator();

    let (status, api) = call(
        &emulator,
        "POST",
        "/v2/apis",
        Some(create_api_body("stage-variable-pattern", "HTTP")),
    )
    .await;
    assert_eq!(
        status, 201,
        "setup: expected 201 from CreateApi, got {status} {api}"
    );
    let api_id = as_str(&api, "apiId");
    let authority = host_and_path(&as_str(&api, "apiEndpoint")).to_string();

    // `$` is the one character the issue's hypothesis turned on, and the pattern
    // `[A-Za-z0-9-._~:/?#&=,]+` does not admit it — wherever in the value it sits. A
    // service that appended the reserved stage name to an announced invoke URL would be
    // handing clients a value of exactly this shape.
    for (position, (case, value)) in [
        (
            "the reserved stage name appended to the announced invoke URL",
            format!("{authority}/{DEFAULT_STAGE_NAME}"),
        ),
        (
            "the reserved stage name in the middle of a path",
            format!("{authority}/{DEFAULT_STAGE_NAME}/items"),
        ),
        ("the excluded character alone", "$".to_string()),
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(json!({
                "stageName": format!("excluded{position}"),
                "stageVariables": { "backendUrl": value },
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?} ({value}): expected 400 from CreateStage, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?} ({value}): expected BadRequestException, got {body}"
        );
    }
}
