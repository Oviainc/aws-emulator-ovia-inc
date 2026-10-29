// SPDX-License-Identifier: CC0-1.0

//! WHICH configuration members a stored `Stage` reports, and which it leaves ABSENT.
//!
//! The rule every case here holds the service to: **a stage reports a configuration
//! member only once a request has set it — a member the caller never supplied is absent
//! from every reader, and a documented default for it governs only how the stage BEHAVES,
//! never what is stored or reported.**
//!
//! `Stage$AutoDeploy` is the member that makes the rule observable, because it is the one
//! whose documented default ("Specifies whether updates to an API automatically trigger a
//! new deployment. The default value is false", `CreateStage$AutoDeploy`) describes
//! behaviour while the model gives the member no `@default` and no `@required`
//! (`smithy_query(query="operation", operation="UpdateStage")`: `AutoDeploy` targets
//! `__boolean`, `jsonName` `autoDeploy`). Presence is therefore the only signal that tells
//! "auto-deploy was never configured on this stage" from "auto-deploy was configured and
//! turned off", and a client that round-trips a read back into `UpdateStage` must not end
//! up writing a setting its user never made.
//!
//! The rows vary what the rule generalises over: the owning api's protocol type, the form
//! of the stage name, whether the create configured the member and to which value, whether
//! a later `UpdateStage` configured it, and whether that patch named the member at all.
//! Every row is then read through all of `CreateStage`'s own answer, `GetStage` and
//! `GetStages`, because the rule is about the RECORD and not about one reader. The last
//! test closes the loop the rule exists for: echoing a read document back through
//! `UpdateStage` leaves the member exactly as absent, or as set, as it was.

use serde_json::{json, Value};

use crate::api_lifecycle::{as_str, call, emulator};

/// The `UpdateStage` input members that carry a caller's CONFIGURATION — every member of
/// `UpdateStageRequest` except the two `@httpLabel`s that address the stage
/// (`smithy_query(query="operation", operation="UpdateStage")`), and except
/// `DeploymentId`, which an auto-deploying stage refuses on the way back in ("The
/// deployment identifier for the API stage. Can't be updated if autoDeploy is enabled",
/// `UpdateStage$DeploymentId`). This is what a round-trip through the api sends back: the
/// readable configuration of the stage, nothing else.
const UPDATE_STAGE_CONFIGURATION_MEMBERS: &[&str] = &[
    "accessLogSettings",
    "autoDeploy",
    "clientCertificateId",
    "defaultRouteSettings",
    "description",
    "routeSettings",
    "stageVariables",
];

/// An HTTP api.
fn http_api(name: &str) -> Value {
    json!({ "name": name, "protocolType": "HTTP" })
}

/// A WebSocket api: `RouteSelectionExpression` "is required for WebSocket APIs"
/// (`CreateApi$RouteSelectionExpression`).
fn websocket_api(name: &str) -> Value {
    json!({
        "name": name,
        "protocolType": "WEBSOCKET",
        "routeSelectionExpression": "$request.body.action"
    })
}

/// One row: the api to create the stage under, the `CreateStage` body, an optional
/// `UpdateStage` patch, and the `AutoDeploy` every reader must report afterwards.
struct Row {
    case: &'static str,
    api: Value,
    create: Value,
    patch: Option<Value>,
    /// `None` when no request configured the member, so no reader reports it.
    expected: Option<bool>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the recorded case: a WebSocket stage created with no AutoDeploy",
            api: websocket_api("ws-unset-api"),
            create: json!({ "stageName": "prod" }),
            patch: None,
            expected: None,
        },
        Row {
            case: "an HTTP stage created with no AutoDeploy",
            api: http_api("http-unset-api"),
            create: json!({ "stageName": "prod" }),
            patch: None,
            expected: None,
        },
        Row {
            case: "a stage that configured AutoDeploy off",
            api: http_api("http-off-api"),
            create: json!({ "stageName": "prod", "autoDeploy": false }),
            patch: None,
            expected: Some(false),
        },
        Row {
            case: "a stage that configured AutoDeploy on",
            api: http_api("http-on-api"),
            create: json!({ "stageName": "prod", "autoDeploy": true }),
            patch: None,
            expected: Some(true),
        },
        Row {
            case: "the $default stage name, with no AutoDeploy",
            api: http_api("http-default-stage-api"),
            create: json!({ "stageName": "$default" }),
            patch: None,
            expected: None,
        },
        Row {
            case: "a stage name carrying hyphens and underscores, carrying other \
                   configuration but no AutoDeploy",
            api: websocket_api("ws-named-api"),
            create: json!({
                "stageName": "pre-prod_v2",
                "description": "the staging stage",
                "stageVariables": { "table": "orders" }
            }),
            patch: None,
            expected: None,
        },
        Row {
            case: "a patch that names another member leaves AutoDeploy unconfigured",
            api: http_api("http-other-patch-api"),
            create: json!({ "stageName": "prod" }),
            patch: Some(json!({ "description": "patched elsewhere" })),
            expected: None,
        },
        Row {
            case: "a patch that names no member at all leaves AutoDeploy unconfigured",
            api: websocket_api("ws-empty-patch-api"),
            create: json!({ "stageName": "prod" }),
            patch: Some(json!({})),
            expected: None,
        },
        Row {
            case: "a patch that configures AutoDeploy off on a stage that never had it",
            api: http_api("http-patch-off-api"),
            create: json!({ "stageName": "prod" }),
            patch: Some(json!({ "autoDeploy": false })),
            expected: Some(false),
        },
        Row {
            case: "a patch that configures AutoDeploy on on a stage that never had it",
            api: http_api("http-patch-on-api"),
            create: json!({ "stageName": "prod" }),
            patch: Some(json!({ "autoDeploy": true })),
            expected: Some(true),
        },
        Row {
            case: "a patch that turns a configured AutoDeploy off",
            api: websocket_api("ws-patch-flip-api"),
            create: json!({ "stageName": "prod", "autoDeploy": true }),
            patch: Some(json!({ "autoDeploy": false })),
            expected: Some(false),
        },
    ]
}

/// Create one api and answer its `ApiId`.
async fn create_api(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    case: &str,
    request: &Value,
) -> String {
    let (status, created) = call(emulator, "POST", "/v2/apis", Some(request.clone())).await;
    assert_eq!(
        status, 201,
        "case {case:?}: expected 201 from CreateApi, got {status} {created}"
    );
    as_str(&created, "apiId")
}

/// Assert `document` reports `expected`, naming the reader and the actual document.
///
/// A member that is absent and one serialized as JSON `null` are both read as "not
/// present", which is what a restJson1 client sees.
fn assert_auto_deploy(case: &str, reader: &str, document: &Value, expected: Option<bool>) {
    let actual = document.get("autoDeploy");
    assert_eq!(
        actual,
        expected.map(|flag| json!(flag)).as_ref(),
        "case {case:?}: expected {reader} to report AutoDeploy {expected:?}, got {actual:?} \
         in {document}"
    );
}

#[tokio::test]
async fn a_stage_reports_auto_deploy_only_once_a_request_configured_it() {
    let emulator = emulator();

    for row in rows() {
        let case = row.case;
        let api_id = create_api(&emulator, case, &row.api).await;

        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(row.create.clone()),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateStage, got {status} {created}"
        );
        let stage_name = as_str(&created, "stageName");

        // The create's own answer reports what the create configured, before any patch.
        let configured_by_create = row.create.get("autoDeploy").and_then(Value::as_bool);
        assert_auto_deploy(case, "CreateStage", &created, configured_by_create);

        // The documented default still governs BEHAVIOUR: a stage that configured no
        // auto-deploy behaves exactly like one that configured it off — the service
        // releases no deployment to it — while one that configured it on has a deployment
        // released by the service ("Specifies whether a deployment was automatically
        // released", `Deployment$AutoDeployed`).
        let released = created.get("deploymentId").and_then(Value::as_str);
        if configured_by_create == Some(true) {
            assert!(
                released.is_some(),
                "case {case:?}: an auto-deploying stage must serve a deployment the \
                 service released, got {created}"
            );
        } else {
            assert_eq!(
                released, None,
                "case {case:?}: a stage that did not configure auto-deploy on must serve \
                 no deployment, got {created}"
            );
        }

        if let Some(patch) = &row.patch {
            let (status, patched) = call(
                &emulator,
                "PATCH",
                &format!("/v2/apis/{api_id}/stages/{stage_name}"),
                Some(patch.clone()),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 from UpdateStage, got {status} {patched}"
            );
            assert_auto_deploy(case, "UpdateStage", &patched, row.expected);
        }

        // Every reader of the record agrees: the member is not a shape one reader invents.
        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} {fetched}"
        );
        assert_auto_deploy(case, "GetStage", &fetched, row.expected);

        let (status, page) = call(&emulator, "GET", &format!("/v2/apis/{api_id}/stages"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStages, got {status} {page}"
        );
        let items = page["items"].as_array().cloned().unwrap_or_default();
        let listed = items
            .iter()
            .find(|item| as_str(item, "stageName") == stage_name)
            .unwrap_or_else(|| panic!("case {case:?}: expected {stage_name} to be listed, got {page}"));
        assert_auto_deploy(case, "GetStages", listed, row.expected);

        // A read changes nothing a later caller sees, so the member does not appear on the
        // second read either.
        let (_, refetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            refetched, fetched,
            "case {case:?}: GetStage changed the record it reported"
        );
    }
}

#[tokio::test]
async fn echoing_a_read_stage_back_through_update_stage_configures_nothing_new() {
    let emulator = emulator();

    // (the api, the CreateStage body, what every reader must report before and after the
    // echo). The echo is the round trip the presence of the member exists for: a tool that
    // reads a stage and writes the document back must not turn "never configured" into
    // "configured off".
    let rows: Vec<(&'static str, Value, Value, Option<bool>)> = vec![
        (
            "a WebSocket stage that configured no AutoDeploy",
            websocket_api("ws-roundtrip-api"),
            json!({ "stageName": "prod", "description": "round-tripped" }),
            None,
        ),
        (
            "an HTTP stage that configured no AutoDeploy",
            http_api("http-roundtrip-api"),
            json!({ "stageName": "prod" }),
            None,
        ),
        (
            "a stage that configured AutoDeploy off",
            http_api("http-roundtrip-off-api"),
            json!({ "stageName": "prod", "autoDeploy": false }),
            Some(false),
        ),
        (
            "a stage that configured AutoDeploy on",
            http_api("http-roundtrip-on-api"),
            json!({ "stageName": "prod", "autoDeploy": true }),
            Some(true),
        ),
    ];

    for (case, api, create, expected) in rows {
        let api_id = create_api(&emulator, case, &api).await;
        let (status, created) = call(
            &emulator,
            "POST",
            &format!("/v2/apis/{api_id}/stages"),
            Some(create),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateStage, got {status} {created}"
        );
        let stage_name = as_str(&created, "stageName");

        let (status, fetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} {fetched}"
        );
        assert_auto_deploy(case, "GetStage", &fetched, expected);

        // The document a client reads, projected onto the members `UpdateStage` accepts —
        // exactly what a round-trip writes back.
        let mut echo = serde_json::Map::new();
        for member in UPDATE_STAGE_CONFIGURATION_MEMBERS {
            if let Some(value) = fetched.get(member) {
                echo.insert((*member).to_string(), value.clone());
            }
        }
        assert_eq!(
            echo.contains_key("autoDeploy"),
            expected.is_some(),
            "case {case:?}: the echoed document must carry autoDeploy only when the stage \
             reports it, got {echo:?}"
        );
        let (status, patched) = call(
            &emulator,
            "PATCH",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            Some(Value::Object(echo)),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UpdateStage, got {status} {patched}"
        );
        assert_auto_deploy(case, "UpdateStage after the echo", &patched, expected);

        let (status, refetched) = call(
            &emulator,
            "GET",
            &format!("/v2/apis/{api_id}/stages/{stage_name}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetStage, got {status} {refetched}"
        );
        assert_auto_deploy(case, "GetStage after the echo", &refetched, expected);
        assert_eq!(
            refetched.get("autoDeploy"),
            fetched.get("autoDeploy"),
            "case {case:?}: the round trip changed the stage's auto-deploy setting, \
             got {refetched}"
        );
    }
}
