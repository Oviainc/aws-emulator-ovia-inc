// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the STAGE VARIABLES an integration's configuration names: which
//! backend a deployment reached through two different stages addresses.
//!
//! The rule under test: where an integration's configuration carries a value a stage may
//! differ in — the endpoint it addresses (`uri`), the VPC link a private connection runs
//! over (`connectionId`), the role the call is made under (`credentials`) — a reference
//! written `${stageVariables.<name>}` is replaced, before the integration is performed, by
//! the value the INVOKED stage's own `variables` map gives that name ("a map that defines
//! the stage variables for a Stage resource", `Stage$variables`). So one deployment served
//! by two stages addresses each stage's own backend, and a name the invoked stage defines
//! no variable for leaves the configuration with a hole in it: the call is answered through
//! the api's `API_CONFIGURATION_ERROR` gateway response rather than addressed at the text
//! left behind.
//!
//! The rows span what the rule generalises over: WHICH member of the configuration carries
//! the reference, WHERE in a value it stands (the host label, a whole path segment, part of
//! one, the whole tail), how many references one value carries (none, one, two), and
//! whether the invoked stage defines the name. The pair of stages is what each row asserts
//! over: the same deployment, the same integration, two variable maps, two answers.
//!
//! Every case drives the real wire path — an HTTP request through the multi-service router
//! into the mounted service, on the api's own invoke host — through this crate's existing
//! harness, so what is asserted is what a client of the published invoke URL sees. The
//! backends are `MOCK` methods of other apis of this emulator, reached through the
//! `HTTP_PROXY` pass-through the sibling `passed_through_integration` suite covers, because
//! that is the one backend whose answer is observable in process.

use serde_json::{json, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::VPC_LINK_PROVISION_SPAN;

use crate::execute_api_data_plane::{
    deploy, integration_response, invoke_on_host, json_template, method_response, mock_resource,
    put_integration, put_method, Invoked,
};
use crate::rest_api_lifecycle::{create_api, resource_under, string_member, Gateway};
use crate::test_invoke_method::test_invoke;

/// The Region every host in this suite is addressed in, which is the one the harness's own
/// control-plane calls are signed for.
const REGION: &str = "us-east-1";

/// The stage each backend api is deployed to.
const BACKEND_STAGE: &str = "back";

/// The two stages of the ONE deployment every row serves: the dev/prod pair a
/// configuration written over stage variables exists for.
const STAGES: [&str; 2] = ["dev", "prod"];

/// The label of the first backend api, which its answers name.
const ALPHA: &str = "alpha";

/// The label of the second backend api.
const BETA: &str = "beta";

/// The resource both backends serve.
const ITEMS: &str = "items";

/// The second resource the first backend serves, so a row can differ in the PATH rather
/// than in the api.
const ORDERS: &str = "orders";

/// The status and the `message` an uncustomized `API_CONFIGURATION_ERROR` gateway response
/// carries, which is what a call whose integration cannot be performed is answered with.
const CONFIGURATION_ERROR: (u16, &str) = (500, "Internal server error");

/// A network load balancer of this account, as Elastic Load Balancing writes its ARN
/// (`loadbalancer/net/{name}/{id}`), which is what a VPC link is provisioned against.
const LOAD_BALANCER_ARN: &str =
    "arn:aws:elasticloadbalancing:us-east-1:000000000000:loadbalancer/net/stage-variable/0a1b2c3d";

/// A role ARN a stage variable may carry into `Integration$credentials`: "To specify an IAM
/// Role for API Gateway to assume, use the role's Amazon Resource Name (ARN)".
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/apigateway-stage-variable";

/// The api's own invoke host, which names the api.
fn api_host(api: &str) -> String {
    format!("{api}.execute-api.{REGION}.amazonaws.com")
}

/// The body the `MOCK` method of backend `label` at `path` answers: the pair that says
/// WHICH backend served the call, which is what per-stage isolation is read off.
fn served_body(label: &str, path: &str) -> String {
    format!("{{\"backend\":\"{label}\",\"path\":\"{path}\"}}")
}

/// A backend api carrying one `MOCK` `GET` per entry of `paths`, each answering
/// [`served_body`], deployed to [`BACKEND_STAGE`] — as its api id.
async fn backend(gateway: &Gateway, label: &str, paths: &[&str]) -> String {
    let (api, root) = create_api(gateway, &format!("stage-variable-backend-{label}")).await;
    for path in paths {
        mock_resource(
            gateway,
            &api,
            &root,
            path,
            "GET",
            "200",
            json_template(&served_body(label, path)),
        )
        .await;
    }
    deploy(gateway, &api, BACKEND_STAGE).await;
    api
}

/// `CreateDeployment` with no stage of its own, as its deployment id: the one snapshot both
/// stages below point at, so a row varies the STAGE and nothing else.
async fn deployment_of(gateway: &Gateway, api: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/deployments"),
            Some(json!({ "description": "the snapshot both stages serve" })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateDeployment({api}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateDeployment")
}

/// `CreateStage` for `stage` over `deployment`, carrying `variables`.
async fn create_stage(
    gateway: &Gateway,
    api: &str,
    deployment: &str,
    stage: &str,
    variables: &Value,
) {
    let answer = gateway
        .call(
            "POST",
            &format!("/restapis/{api}/stages"),
            Some(json!({
                "stageName": stage,
                "deploymentId": deployment,
                "variables": variables,
            })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateStage({stage}, {variables}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
}

/// A front api whose root `GET` carries `integration`, served by the two [`STAGES`] of ONE
/// deployment with `variables` as each stage's own map — as its api id. The method
/// authorizes nobody, so the call under test is the anonymous one.
async fn front(gateway: &Gateway, name: &str, integration: Value, variables: &[Value; 2]) -> String {
    let (api, root) = create_api(gateway, name).await;
    put_method(
        gateway,
        &api,
        &root,
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(gateway, &api, &root, "GET", integration).await;
    let deployment = deployment_of(gateway, &api).await;
    for (stage, carried) in STAGES.iter().zip(variables.iter()) {
        create_stage(gateway, &api, &deployment, stage, carried).await;
    }
    api
}

/// One invoke of `stage` of the front api `api`.
async fn invoke_stage(gateway: &Gateway, api: &str, stage: &str) -> Invoked {
    invoke_on_host(gateway, &api_host(api), "GET", &format!("/{stage}/"), &[]).await
}

/// What a row expects one stage of the front api to answer.
#[derive(Debug)]
enum Expect {
    /// The request was passed through to a backend, which answered this body.
    Served(String),
    /// The integration could not be performed, so the api's `API_CONFIGURATION_ERROR`
    /// gateway response came back and no backend was reached.
    NotPerformed,
}

/// Assert the answer `served` is what `expect` says, naming the case and the stage.
fn assert_served(case: &str, stage: &str, served: &Invoked, expect: &Expect) {
    match expect {
        Expect::Served(body) => {
            assert_eq!(
                served.status, 200,
                "case {case:?} at stage {stage:?}: expected the backend's own 200, got {} \
                 with {:?}",
                served.status, served.body
            );
            assert_eq!(
                &served.body, body,
                "case {case:?} at stage {stage:?}: expected the backend {body:?} the \
                 stage's variables name, got {:?}",
                served.body
            );
        }
        Expect::NotPerformed => {
            let (status, message) = CONFIGURATION_ERROR;
            assert_eq!(
                served.status, status,
                "case {case:?} at stage {stage:?}: expected {status} with no request passed \
                 on, got {} with {:?}",
                served.status, served.body
            );
            assert_eq!(
                served.message().as_deref(),
                Some(message),
                "case {case:?} at stage {stage:?}: expected the API_CONFIGURATION_ERROR \
                 body, got {:?}",
                served.body
            );
        }
    }
}

/// One row: the case, the integration both stages share, each stage's own variables and
/// what each stage must answer. An alias because the tuple is complex enough to name once.
type Row<'a> = (&'a str, Value, [Value; 2], [Expect; 2]);

/// Drive every row of `rows` against a fresh front api and assert both stages.
async fn assert_rows(gateway: &Gateway, rows: Vec<Row<'_>>) {
    for (index, (case, integration, variables, expected)) in rows.into_iter().enumerate() {
        let api = front(
            gateway,
            &format!("stage-variable-front-{index}"),
            integration,
            &variables,
        )
        .await;
        for (stage, expect) in STAGES.iter().zip(expected.iter()) {
            let served = invoke_stage(gateway, &api, stage).await;
            assert_served(case, stage, &served, expect);
        }
    }
}

#[tokio::test]
async fn a_stage_variable_in_the_integration_uri_addresses_that_stages_own_backend() {
    let gateway = Gateway::new();
    let alpha = backend(&gateway, ALPHA, &[ITEMS, ORDERS]).await;
    let beta = backend(&gateway, BETA, &[ITEMS]).await;
    let alpha_items = served_body(ALPHA, ITEMS);
    let alpha_orders = served_body(ALPHA, ORDERS);
    let beta_items = served_body(BETA, ITEMS);

    // Rows differ along where in the `uri` the reference stands and how many there are:
    // the host label that names the api, a whole path segment, part of a segment, the
    // whole tail after the host, two references in one value, and none at all.
    let rows: Vec<Row<'_>> = vec![
        (
            "the api the host names comes from a variable",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://${{stageVariables.backend}}.execute-api.{REGION}.amazonaws.com\
                     /{BACKEND_STAGE}/{ITEMS}"
                ),
            }),
            [json!({ "backend": alpha }), json!({ "backend": beta })],
            [
                Expect::Served(alpha_items.clone()),
                Expect::Served(beta_items.clone()),
            ],
        ),
        (
            "a whole path segment comes from a variable",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://{host}/{BACKEND_STAGE}/${{stageVariables.resource}}",
                    host = api_host(&alpha)
                ),
            }),
            [
                json!({ "resource": ITEMS }),
                json!({ "resource": ORDERS }),
            ],
            [
                Expect::Served(alpha_items.clone()),
                Expect::Served(alpha_orders.clone()),
            ],
        ),
        (
            "a variable standing beside literal text inside one segment",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://{host}/{BACKEND_STAGE}/${{stageVariables.kind}}s",
                    host = api_host(&alpha)
                ),
            }),
            [json!({ "kind": "item" }), json!({ "kind": "order" })],
            [
                Expect::Served(alpha_items.clone()),
                Expect::Served(alpha_orders.clone()),
            ],
        ),
        (
            "the whole tail after the host comes from one variable",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://{host}/${{stageVariables.target}}",
                    host = api_host(&alpha)
                ),
            }),
            [
                json!({ "target": format!("{BACKEND_STAGE}/{ITEMS}") }),
                json!({ "target": format!("{BACKEND_STAGE}/{ORDERS}") }),
            ],
            [
                Expect::Served(alpha_items.clone()),
                Expect::Served(alpha_orders.clone()),
            ],
        ),
        (
            "two variables in one uri, the api and the resource",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://${{stageVariables.backend}}.execute-api.{REGION}.amazonaws.com\
                     /{BACKEND_STAGE}/${{stageVariables.resource}}"
                ),
            }),
            [
                json!({ "backend": beta, "resource": ITEMS }),
                json!({ "backend": alpha, "resource": ORDERS }),
            ],
            [
                Expect::Served(beta_items.clone()),
                Expect::Served(alpha_orders.clone()),
            ],
        ),
        (
            "a uri naming no variable at all addresses one backend from both stages",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://{host}/{BACKEND_STAGE}/{ITEMS}",
                    host = api_host(&alpha)
                ),
            }),
            [
                json!({ "backend": beta, "resource": ORDERS }),
                json!({ "backend": alpha, "resource": ITEMS }),
            ],
            [
                Expect::Served(alpha_items.clone()),
                Expect::Served(alpha_items.clone()),
            ],
        ),
    ];
    assert_rows(&gateway, rows).await;
}

#[tokio::test]
async fn a_variable_the_invoked_stage_does_not_define_is_an_integration_that_cannot_be_performed()
{
    let gateway = Gateway::new();
    let alpha = backend(&gateway, ALPHA, &[ITEMS]).await;
    let alpha_items = served_body(ALPHA, ITEMS);
    let alpha_uri = format!(
        "https://{host}/{BACKEND_STAGE}/{ITEMS}",
        host = api_host(&alpha)
    );

    // Rows differ along WHICH member carries the reference the second stage leaves
    // undefined, and how it is undefined: a map that defines another name, and an empty
    // map. The first stage of every row defines it, so each row shows the same
    // configuration served at one stage and unperformable at the other.
    let rows: Vec<Row<'_>> = vec![
        (
            "the uri names a variable the second stage does not define",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://${{stageVariables.backend}}.execute-api.{REGION}.amazonaws.com\
                     /{BACKEND_STAGE}/{ITEMS}"
                ),
            }),
            [
                json!({ "backend": alpha }),
                json!({ "elsewhere": alpha }),
            ],
            [
                Expect::Served(alpha_items.clone()),
                Expect::NotPerformed,
            ],
        ),
        (
            "the credentials name a variable the second stage leaves out of an empty map",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": alpha_uri.clone(),
                "credentials": "${stageVariables.role}",
            }),
            [json!({ "role": ROLE_ARN }), json!({})],
            [
                Expect::Served(alpha_items.clone()),
                Expect::NotPerformed,
            ],
        ),
        (
            "one of two references in the uri is left undefined",
            json!({
                "type": "HTTP_PROXY",
                "httpMethod": "GET",
                "uri": format!(
                    "https://${{stageVariables.backend}}.execute-api.{REGION}.amazonaws.com\
                     /{BACKEND_STAGE}/${{stageVariables.resource}}"
                ),
            }),
            [
                json!({ "backend": alpha, "resource": ITEMS }),
                json!({ "backend": alpha }),
            ],
            [
                Expect::Served(alpha_items.clone()),
                Expect::NotPerformed,
            ],
        ),
    ];
    assert_rows(&gateway, rows).await;
}

/// `CreateVpcLink` over [`LOAD_BALANCER_ARN`], as its id.
async fn vpc_link(gateway: &Gateway, name: &str) -> String {
    let answer = gateway
        .call(
            "POST",
            "/vpclinks",
            Some(json!({ "name": name, "targetArns": [LOAD_BALANCER_ARN] })),
        )
        .await;
    assert_eq!(
        answer.status, 202,
        "case CreateVpcLink({name:?}): expected 202, got {} with {:?}",
        answer.status, answer.body
    );
    string_member(&answer.body, "id", "CreateVpcLink")
}

/// A stage variable in `connectionId` names the VPC LINK a private integration is carried
/// over, so one stage's private connection is a link this account holds and another's is
/// not — the same member the rest of the suite asserts over `uri` and `credentials`.
#[tokio::test]
async fn a_stage_variable_names_the_vpc_link_the_private_integration_is_carried_over() {
    let gateway = Gateway::new();
    let alpha = backend(&gateway, ALPHA, &[ITEMS]).await;
    let link = vpc_link(&gateway, "stage-variable-link").await;
    // A created link is PENDING until its provisioning span has passed; the margin is for
    // a slow machine.
    tokio::time::sleep(VPC_LINK_PROVISION_SPAN + std::time::Duration::from_millis(400)).await;

    let rows: Vec<Row<'_>> = vec![(
        "the connectionId comes from a variable",
        json!({
            "type": "HTTP_PROXY",
            "httpMethod": "GET",
            "connectionType": "VPC_LINK",
            "connectionId": "${stageVariables.link}",
            "uri": format!(
                "https://{host}/{BACKEND_STAGE}/{ITEMS}",
                host = api_host(&alpha)
            ),
        }),
        [
            json!({ "link": link }),
            json!({ "link": "no-such-link" }),
        ],
        [
            Expect::Served(served_body(ALPHA, ITEMS)),
            Expect::NotPerformed,
        ],
    )];
    assert_rows(&gateway, rows).await;
}

/// One method whose integration is `integration`, answering `body` through a `MOCK`
/// integration response, as the api id and the resource id a simulated invocation
/// addresses.
async fn simulated_method(
    gateway: &Gateway,
    name: &str,
    integration: Value,
    body: &str,
) -> (String, String) {
    let (api, root) = create_api(gateway, name).await;
    let resource = resource_under(gateway, &api, &root, ITEMS).await;
    put_method(
        gateway,
        &api,
        &resource,
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(gateway, &api, &resource, "GET", integration).await;
    method_response(gateway, &api, &resource, "GET", "200", json!({})).await;
    integration_response(
        gateway,
        &api,
        &resource,
        "GET",
        "200",
        json!({ "responseTemplates": json_template(body) }),
    )
    .await;
    (api, resource)
}

/// `TestInvokeMethod` reads the configuration against the stage variables the CALLER asked
/// to simulate — `stageVariables` is "a key-value map of stage variables to simulate an
/// invocation on a deployed Stage" — so a simulated invocation of a configuration naming
/// one resolves it exactly as the data plane resolves it at a stage whose map carries those
/// entries.
#[tokio::test]
async fn a_simulated_invocation_resolves_the_stage_variables_it_was_given() {
    let gateway = Gateway::new();
    let body = served_body(ALPHA, ITEMS);
    // A `MOCK` is the one integration type that needs no backend — "for testing without
    // actually invoking the backend" (`IntegrationType`) — so it is what shows a resolved
    // reference in the simulated RESPONSE rather than only in the execution log.
    let (mocked, mocked_resource) = simulated_method(
        &gateway,
        "stage-variable-simulated-mock",
        json!({ "type": "MOCK", "credentials": "${stageVariables.role}" }),
        &body,
    )
    .await;
    // A Lambda proxy integration, whose `uri` is the form a stage-variable-selected
    // function is written in. This emulator mounts no Lambda at all, so the invocation
    // reaches nothing whatever the variable resolves to — what the row asserts is WHY.
    let (proxied, proxied_resource) = simulated_method(
        &gateway,
        "stage-variable-simulated-proxy",
        json!({
            "type": "AWS_PROXY",
            "httpMethod": "POST",
            "uri": format!(
                "arn:aws:apigateway:{REGION}:lambda:path/2015-03-31/functions\
                 /arn:aws:lambda:{REGION}:000000000000:function:${{stageVariables.fn}}\
                 /invocations"
            ),
        }),
        &body,
    )
    .await;

    // (case, the method, the request, the simulated status, a sentence the log must carry).
    let rows: Vec<(&str, &str, &str, Value, i64, &str)> = vec![
        (
            "the simulated map defines the name the credentials reference",
            &mocked,
            &mocked_resource,
            json!({ "stageVariables": { "role": ROLE_ARN } }),
            200,
            "Successfully completed execution",
        ),
        (
            "the simulated map defines another name",
            &mocked,
            &mocked_resource,
            json!({ "stageVariables": { "other": ROLE_ARN } }),
            i64::from(CONFIGURATION_ERROR.0),
            "names the stage variable role",
        ),
        (
            "the request carries no simulated stage variables at all",
            &mocked,
            &mocked_resource,
            json!({}),
            i64::from(CONFIGURATION_ERROR.0),
            "names the stage variable role",
        ),
        (
            "the resolved uri names a function no Lambda of this emulator answers for",
            &proxied,
            &proxied_resource,
            json!({ "stageVariables": { "fn": "orders-handler" } }),
            i64::from(CONFIGURATION_ERROR.0),
            "did not answer the invocation",
        ),
        (
            "the uri names a function variable the simulated map leaves out",
            &proxied,
            &proxied_resource,
            json!({ "stageVariables": { "other": "orders-handler" } }),
            i64::from(CONFIGURATION_ERROR.0),
            "names the stage variable fn",
        ),
    ];

    for (case, api, resource, request, status, logged) in rows {
        let simulated = test_invoke(&gateway, api, resource, "GET", request.clone()).await;
        assert_eq!(
            simulated.http_status, 200,
            "case {case:?}: expected the modelled 200 from TestInvokeMethod, got {} with \
             {:?}",
            simulated.http_status, simulated.body
        );
        assert_eq!(
            simulated.status(),
            status,
            "case {case:?}: expected the simulated status {status}, got {} with {:?}",
            simulated.status(),
            simulated.body
        );
        let log = simulated.body["log"].as_str().unwrap_or_default();
        assert!(
            log.contains(logged),
            "case {case:?}: expected the execution log to carry {logged:?}, got {log:?}"
        );
        if status == 200 {
            assert_eq!(
                simulated.simulated_body(),
                body,
                "case {case:?}: expected the integration's own body, got {:?}",
                simulated.simulated_body()
            );
        }
    }
}
