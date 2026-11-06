// SPDX-License-Identifier: CC0-1.0

//! What a stage variable's value may be — and that the invoke URL this control plane
//! ANNOUNCES is one of them.
//!
//! "A map that defines the stage variables for a Stage. Variable names can have
//! alphanumeric and underscore characters, and the values must match
//! `[A-Za-z0-9-._~:/?#&=,]+`" (`Stage$StageVariables`), bounded by the 2048 characters of
//! the map's value shape (`StringWithLengthBetween0And2048`). So the rules under test are:
//!
//! * a value drawn from that documented character set is ACCEPTED by `CreateStage` and
//!   `UpdateStage` and round-trips through `GetStage`; a value carrying a character outside
//!   it, or no character at all, is refused 400 `BadRequestException` and stores nothing;
//! * a variable NAME outside the alphanumeric-and-underscore set is refused the same way;
//! * the documented use of a stage variable is interpolation into an `HTTP_PROXY`
//!   integration's URI host, so the host-and-path of the invoke URL this service announces
//!   for an api it just created (`Api$ApiEndpoint`, "the stage name is typically appended to
//!   this URI to form a complete path to a deployed API stage") has to be an acceptable
//!   value — the announced URL and the accepted character set cannot disagree, whichever of
//!   them is wrong.
//!
//! The rows span what the rules generalise over: the length of the value (one character,
//! the published bound, one past it), which of the set's punctuation it carries, where the
//! offending character sits, the form of the name, and — for the announced invoke URL — a
//! `$default` stage, a named stage, and an api in another Region, since the host carries
//! both the api id this service minted and the Region the request was signed for.

use serde_json::{json, Value};
use service_com_amazonaws_apigatewayv2_apigatewayv2::state::MAX_STAGE_VARIABLE_VALUE_LEN;
use smithy_server_runtime::MultiServiceRouter;

use crate::api_lifecycle::{as_str, call, call_in_region, emulator, error_code};

/// The stage that serves an api's root, which is reached with no stage segment in the path
/// at all (`crate::state::DEFAULT_STAGE_NAME`).
const DEFAULT_STAGE: &str = "$default";

/// The variable every row below sets, named the way the documented HTTP-proxy use case
/// names one.
const VARIABLE: &str = "backendUrl";

/// Create one HTTP api in `region` and answer `(ApiId, ApiEndpoint)` — the endpoint the
/// control plane ANNOUNCES for it, never a URL built here.
async fn create_api(emulator: &MultiServiceRouter, region: &str, name: &str) -> (String, String) {
    let (status, created) = call_in_region(
        emulator,
        region,
        "POST",
        "/v2/apis",
        Some(json!({ "name": name, "protocolType": "HTTP" })),
    )
    .await;
    assert_eq!(
        status, 201,
        "CreateApi must answer 201, got {status} with {created:?}"
    );
    (as_str(&created, "apiId"), as_str(&created, "apiEndpoint"))
}

/// The host-and-path portion of the invoke URL of the stage `stage_name` of the api whose
/// announced endpoint is `api_endpoint`.
///
/// The scheme is dropped, because what an `HTTP_PROXY` `IntegrationUri` interpolates a
/// stage variable into is the host (and path) of the backend, and the stage segment is
/// appended for a stage other than the one that serves the api's root.
fn announced_host_and_path(api_endpoint: &str, stage_name: &str) -> String {
    let host_and_path = api_endpoint
        .split_once("://")
        .map_or(api_endpoint, |(_, rest)| rest);
    if stage_name == DEFAULT_STAGE {
        return host_and_path.to_string();
    }
    format!("{host_and_path}/{stage_name}")
}

/// `CreateStage` for `stage_name` with the one stage variable `value`.
async fn create_stage(
    emulator: &MultiServiceRouter,
    api_id: &str,
    stage_name: &str,
    name: &str,
    value: &str,
) -> (u16, Value) {
    call(
        emulator,
        "POST",
        &format!("/v2/apis/{api_id}/stages"),
        Some(json!({
            "stageName": stage_name,
            "stageVariables": { name: value },
        })),
    )
    .await
}

/// The stage variables `GetStage` reports for `stage_name`, and the status it answered.
async fn read_stage(
    emulator: &MultiServiceRouter,
    api_id: &str,
    stage_name: &str,
) -> (u16, Value) {
    call(
        emulator,
        "GET",
        &format!("/v2/apis/{api_id}/stages/{stage_name}"),
        None,
    )
    .await
}

#[tokio::test]
async fn a_stage_variable_value_inside_the_documented_character_set_is_stored_and_read_back() {
    let emulator = emulator();
    let (api_id, api_endpoint) = create_api(&emulator, "eu-west-1", "accepting-api").await;

    // (case, the variable name, the value it carries)
    let rows: Vec<(String, String, String)> = vec![
        (
            "the host and path the control plane announces for a $default stage".to_string(),
            VARIABLE.to_string(),
            announced_host_and_path(&api_endpoint, DEFAULT_STAGE),
        ),
        (
            "the host and path announced for a NAMED stage".to_string(),
            VARIABLE.to_string(),
            announced_host_and_path(&api_endpoint, "prod"),
        ),
        (
            "that announced URL with its scheme still on it".to_string(),
            VARIABLE.to_string(),
            api_endpoint.clone(),
        ),
        (
            "a backend host with a path, a query and a fragment".to_string(),
            VARIABLE.to_string(),
            format!(
                "{}/items?filter=a,b&page=2#top",
                announced_host_and_path(&api_endpoint, "prod")
            ),
        ),
        (
            "one character".to_string(),
            VARIABLE.to_string(),
            "a".to_string(),
        ),
        (
            "every punctuation character the set admits".to_string(),
            VARIABLE.to_string(),
            "aZ09-._~:/?#&=,".to_string(),
        ),
        (
            format!("a value at the published {MAX_STAGE_VARIABLE_VALUE_LEN}-character bound"),
            VARIABLE.to_string(),
            "v".repeat(MAX_STAGE_VARIABLE_VALUE_LEN),
        ),
        (
            "a name carrying an underscore and digits".to_string(),
            "backend_2".to_string(),
            announced_host_and_path(&api_endpoint, "prod"),
        ),
    ];

    for (index, (case, name, value)) in rows.into_iter().enumerate() {
        let stage_name = format!("accepted{index}");
        let (status, body) = create_stage(&emulator, &api_id, &stage_name, &name, &value).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateStage, got {status} with {body:?}"
        );

        // The round trip: the stored stage reports the value the request named.
        let (status, read_back) = read_stage(&emulator, &api_id, &stage_name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} with {read_back:?}"
        );
        let stored = read_back.get("stageVariables").cloned().unwrap_or(Value::Null);
        assert_eq!(
            stored.get(&name),
            Some(&json!(value)),
            "case {case:?}: expected the stored stage to report {name}={value:?}, got {stored:?}"
        );
        assert_eq!(
            stored.as_object().map(serde_json::Map::len),
            Some(1),
            "case {case:?}: expected exactly the one variable the request set, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn the_invoke_url_this_control_plane_announces_is_an_acceptable_stage_variable_value() {
    let emulator = emulator();
    // The rows vary the two things the announced host carries — the api id this service
    // minted and the Region the request was signed for — and the stage whose invoke URL is
    // taken, since a stage other than the api's root contributes a path segment of its own.
    let rows = [
        ("the emulator's default Region", "us-east-1"),
        ("another Region", "ap-southeast-2"),
        ("a Region whose label is long", "eu-central-1"),
    ];
    for (case, region) in rows {
        let (api_id, api_endpoint) =
            create_api(&emulator, region, &format!("backend-{region}")).await;
        for stage in [DEFAULT_STAGE, "prod", "stage_2"] {
            let value = announced_host_and_path(&api_endpoint, stage);
            assert!(
                value.contains(&api_id),
                "case {case:?} stage {stage:?}: the announced invoke URL must name the api, \
                 got {value:?}"
            );
            // A frontend api whose own stage interpolates that backend into an HTTP_PROXY
            // URI: the value is handed to CreateStage exactly as announced.
            let (frontend, _) =
                create_api(&emulator, region, &format!("frontend-{region}-{stage}")).await;
            let (status, body) =
                create_stage(&emulator, &frontend, "front", VARIABLE, &value).await;
            assert_eq!(
                status, 201,
                "case {case:?} stage {stage:?}: a stage variable carrying the announced \
                 invoke URL {value:?} must be accepted, got {status} with {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_resolves_a_stage_variable_by_the_same_rules_as_a_create() {
    let emulator = emulator();
    let (api_id, api_endpoint) = create_api(&emulator, "eu-west-1", "updating-api").await;
    let host = announced_host_and_path(&api_endpoint, DEFAULT_STAGE);
    let (status, body) = create_stage(&emulator, &api_id, "prod", VARIABLE, &host).await;
    assert_eq!(
        status, 201,
        "the stage under test must be created, got {status} with {body:?}"
    );

    // (case, the value the update names, accepted?) — one value from the documented set and
    // the forms outside it, so the two writers of a stage cannot drift apart.
    let announced = announced_host_and_path(&api_endpoint, "prod");
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "the host and path announced for another stage",
            announced.clone(),
            true,
        ),
        (
            "a character the set does not admit",
            format!("{host}/{DEFAULT_STAGE}"),
            false,
        ),
        (
            "a value one character past the published bound",
            "v".repeat(MAX_STAGE_VARIABLE_VALUE_LEN + 1),
            false,
        ),
    ];

    for (case, value, accepted) in rows {
        let (status, body) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}/stages/prod"),
            Some(json!({ "stageVariables": { VARIABLE: value.clone() } })),
        )
        .await;
        if accepted {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from UpdateStage, got {status} with {body:?}"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 from UpdateStage, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "BadRequestException",
                "case {case:?}: expected BadRequestException, got {body:?}"
            );
        }
        // Whatever the update answered, the stage reports the last value it ACCEPTED: a
        // refused update stores nothing.
        let (status, read_back) = read_stage(&emulator, &api_id, "prod").await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} with {read_back:?}"
        );
        let stored = read_back.get("stageVariables").cloned().unwrap_or(Value::Null);
        let expected = if accepted { &value } else { &announced };
        assert_eq!(
            stored.get(VARIABLE),
            Some(&json!(expected)),
            "case {case:?}: expected the stored value {expected:?}, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn a_stage_variable_outside_the_documented_sets_is_refused_and_stores_nothing() {
    let emulator = emulator();
    let (api_id, api_endpoint) = create_api(&emulator, "eu-west-1", "refusing-api").await;
    let host = announced_host_and_path(&api_endpoint, DEFAULT_STAGE);

    // (case, the variable name, its value)
    let rows: Vec<(String, String, String)> = vec![
        (
            "a value carrying a character the set does not admit".to_string(),
            VARIABLE.to_string(),
            format!("{host}/{DEFAULT_STAGE}"),
        ),
        (
            "a value carrying a space".to_string(),
            VARIABLE.to_string(),
            format!("{host} /items"),
        ),
        (
            "a percent-encoded value, which the published set does not admit".to_string(),
            VARIABLE.to_string(),
            format!("{host}/a%20b"),
        ),
        (
            "a value carrying a bracket".to_string(),
            VARIABLE.to_string(),
            format!("{host}/[items]"),
        ),
        (
            "an empty value, which the pattern's + admits nothing of".to_string(),
            VARIABLE.to_string(),
            String::new(),
        ),
        (
            format!("a value one character past the {MAX_STAGE_VARIABLE_VALUE_LEN} bound"),
            VARIABLE.to_string(),
            "v".repeat(MAX_STAGE_VARIABLE_VALUE_LEN + 1),
        ),
        (
            "a name carrying a hyphen, which the name set does not admit".to_string(),
            "backend-url".to_string(),
            host.clone(),
        ),
        (
            "a name carrying a dot".to_string(),
            "backend.url".to_string(),
            host.clone(),
        ),
        (
            "an empty name".to_string(),
            String::new(),
            host.clone(),
        ),
    ];

    for (index, (case, name, value)) in rows.into_iter().enumerate() {
        let stage_name = format!("refused{index}");
        let (status, body) = create_stage(&emulator, &api_id, &stage_name, &name, &value).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 from CreateStage, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "BadRequestException",
            "case {case:?}: expected BadRequestException, got {body:?}"
        );
        let (status, read_back) = read_stage(&emulator, &api_id, &stage_name).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store no stage, GetStage answered {status} \
             with {read_back:?}"
        );
    }
}
