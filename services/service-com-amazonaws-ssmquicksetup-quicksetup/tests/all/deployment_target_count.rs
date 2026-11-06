// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that the per-definition deployment bound is read off the
//! deployments a configuration definition actually fans out to.
//!
//! The rule under test: how many account-and-Region deployments one configuration
//! definition fans out to is the SIZE of the target set it is deployed to — the cross
//! product of the accounts its `TargetAccounts` names (the account this emulator serves
//! when it names none) and the Regions its `TargetRegions` names, each list read with
//! its padding trimmed, its empty
//! entries dropped and its repeats dropped once. One number: the bound admits exactly
//! the parameter sets whose deployed set fits within `MAX_DEPLOYMENT_TARGETS`, deploys
//! exactly that many configurations when it admits them, and states exactly that number
//! when it refuses — whichever of the two operations that write a definition carries the
//! parameters.
//!
//! Every case drives the real wire path (the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers), so what is asserted is
//! what a signed AWS SDK request observes.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::MAX_DEPLOYMENT_TARGETS;
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for: the Region a created manager owns, and so
/// the only Region whose endpoint serves it.
const REGION: &str = "us-west-2";

/// A configuration type the service publishes (`ListQuickSetupTypes`), so every create
/// below is refused only for the reason under test.
const TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// The request member `CreateConfigurationManager` carries a definition's parameters on,
/// the label its refusals name.
const CREATE_MEMBER: &str = "ConfigurationDefinitions.Parameters";

/// The same member on `UpdateConfigurationDefinition`.
const UPDATE_MEMBER: &str = "Parameters";

/// One emulator with an empty store, mounted exactly as the router mounts it, with the
/// administration role every local deployment below names already in the account
/// (`tests/all/local_deployment_roles.rs`, which owns that rule).
fn emulator() -> MultiServiceRouter {
    super::local_deployment_roles::emulator()
}

/// A request signed for `ssm-quicksetup` in [`REGION`], the signal the router routes on
/// and the handlers read the region from.
fn request(method: &str, uri: &str, body: Option<&Value>) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("ssm-quicksetup.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDQUICKSETUP/20260101/{REGION}/ssm-quicksetup/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        // An error `__type` may be namespaced (`com.amazonaws.ssmquicksetup#Foo`).
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// The `Message` a refusal carries.
fn error_message(body: &Value) -> String {
    body.get("Message")
        .and_then(Value::as_str)
        .unwrap_or("<no Message>")
        .to_string()
}

/// Percent-encode a value for use as one `@httpLabel` path segment, which is what an
/// SDK does: a non-greedy label may not carry a raw `/`.
fn label(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '/' => "%2F".to_string(),
            ' ' => "%20".to_string(),
            other => other.to_string(),
        })
        .collect()
}

/// `CreateConfigurationManager` with ONE definition carrying `parameters`.
///
/// Every definition names the administration role a LOCAL deployment must name, since a
/// case here may target accounts and none of them is about that rule
/// (`tests/all/local_deployment_roles.rs`).
async fn create(router: &MultiServiceRouter, name: &str, parameters: &Value) -> (u16, Value) {
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{
            "Type": TYPE,
            "Parameters": parameters,
            "LocalDeploymentAdministrationRoleArn":
                super::local_deployment_roles::administration_role_arn(),
        }],
    });
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// A create that must succeed, answering the manager ARN it minted.
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    parameters: &Value,
) -> String {
    let (status, answer) = create(router, name, parameters).await;
    assert_eq!(
        status, 200,
        "case {case}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The service-assigned id of the manager's single configuration definition.
async fn definition_id(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> String {
    let (status, manager) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {manager:?}"
    );
    manager["ConfigurationDefinitions"][0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected a definition Id, got {manager:?}"))
        .to_string()
}

/// `PUT /configurationDefinition/{ManagerArn}/{Id}` writing `parameters`.
async fn update_parameters(
    router: &MultiServiceRouter,
    manager_arn: &str,
    id: &str,
    parameters: &Value,
) -> (u16, Value) {
    call(
        router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(manager_arn),
            label(id)
        ),
        Some(&json!({ "Parameters": parameters })),
    )
    .await
}

/// Every deployment of one manager, read through `ListConfigurations` and paged to the
/// end, as (`Id`, `Account`, `Region`) — a fan-out at the bound holds more rows than one
/// page.
async fn deployments(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeSet<(String, String, String)> {
    let mut rows: BTreeSet<(String, String, String)> = BTreeSet::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = serde_json::Map::new();
        body.insert("ManagerArn".to_string(), json!(manager_arn));
        if let Some(token) = &token {
            body.insert("StartingToken".to_string(), json!(token));
        }
        let (status, page) =
            call(router, "POST", "/listConfigurations", Some(&Value::Object(body))).await;
        assert_eq!(
            status, 200,
            "case {case}: ListConfigurations must succeed, got {status} / {page:?}"
        );
        for row in page["ConfigurationsList"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {page:?}"))
        {
            let id = row["Id"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an Id, got {row:?}"));
            let account = row["Account"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an Account, got {row:?}"));
            let region = row["Region"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a Region, got {row:?}"));
            rows.insert((id.to_string(), account.to_string(), region.to_string()));
        }
        // Every page carries the token; the empty string is the one that says this page
        // was the last.
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) if !next.is_empty() => token = Some(next.to_string()),
            _ => break,
        }
    }
    rows
}

/// The (account, Region) pairs of a deployed set, without the ids.
fn pairs(rows: &BTreeSet<(String, String, String)>) -> BTreeSet<(String, String)> {
    rows.iter()
        .map(|(_, account, region)| (account.clone(), region.clone()))
        .collect()
}

/// A comma-separated list of `count` distinct Region names.
fn regions(count: usize) -> String {
    (0..count)
        .map(|index| format!("eu-west-{}", index + 1))
        .collect::<Vec<String>>()
        .join(",")
}

/// A comma-separated list of `count` distinct account ids.
fn accounts(count: usize) -> String {
    (0..count)
        .map(|index| format!("{:012}", index + 1))
        .collect::<Vec<String>>()
        .join(",")
}

/// One row of the table: the case, the definition's `Parameters`, and how many
/// account-and-Region deployments that parameter set fans out to.
///
/// A row whose fan-out is within `MAX_DEPLOYMENT_TARGETS` must be admitted and deploy
/// exactly that many configurations; one past it must be refused, stating exactly that
/// number.
type FanOutCase = (&'static str, Value, usize);

/// The table: parameter sets spanning what the rule generalises over — which target
/// lists are named at all, how many entries each names, how the product is split
/// between them, how the lists are written (padding, empty entries, repeats), and
/// whether the fan-out lands under, on or past the bound.
fn cases() -> Vec<FanOutCase> {
    // The service reads at most one entry past `MAX_DEPLOYMENT_TARGETS` out of one
    // target list (`crate::state::MAX_PARAMETER_LIST_ENTRIES`), so no row below writes a
    // longer list than that: the bound, not the cap, is what each row exercises.
    vec![
        (
            "one Region and the organizational units: the local account in that Region",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": regions(1) }),
            1,
        ),
        (
            "one account and one Region",
            json!({ "TargetAccounts": accounts(1), "TargetRegions": regions(1) }),
            1,
        ),
        (
            "three accounts in one Region",
            json!({ "TargetAccounts": accounts(3), "TargetRegions": regions(1) }),
            3,
        ),
        (
            "two accounts and five Regions",
            json!({ "TargetAccounts": accounts(2), "TargetRegions": regions(5) }),
            10,
        ),
        (
            // Padding, empty entries and repeats name no further deployment, so the
            // fan-out is the size of the target set and not the number of entries
            // written: three region entries naming two Regions, two account entries
            // naming one account.
            "padding, an empty entry and a repeat in both lists",
            json!({
                "TargetAccounts": format!(" {} ,,{}", accounts(1), accounts(1)),
                "TargetRegions": " eu-west-1 ,, eu-west-1 , ap-south-1 ",
            }),
            2,
        ),
        (
            "one Region under the bound",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS - 1),
            }),
            MAX_DEPLOYMENT_TARGETS - 1,
        ),
        (
            "exactly the bound, in Regions",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS),
            }),
            MAX_DEPLOYMENT_TARGETS,
        ),
        (
            // One entry past the bound in entries written, but a repeat of one already
            // named: the set stays at the bound, so this is admitted.
            "a repeated Region past the bound keeps the set at the bound",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": format!("{},eu-west-1", regions(MAX_DEPLOYMENT_TARGETS)),
            }),
            MAX_DEPLOYMENT_TARGETS,
        ),
        (
            "exactly the bound, split across accounts and Regions",
            json!({
                "TargetAccounts": accounts(4),
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS / 4),
            }),
            MAX_DEPLOYMENT_TARGETS,
        ),
        (
            "one Region past the bound",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS + 1),
            }),
            MAX_DEPLOYMENT_TARGETS + 1,
        ),
        (
            "one account past the bound, in one Region",
            json!({
                "TargetAccounts": accounts(MAX_DEPLOYMENT_TARGETS + 1),
                "TargetRegions": regions(1),
            }),
            MAX_DEPLOYMENT_TARGETS + 1,
        ),
        (
            "one account past the bound, the product split across accounts and Regions",
            json!({
                "TargetAccounts": accounts(5),
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS / 4),
            }),
            5 * (MAX_DEPLOYMENT_TARGETS / 4),
        ),
        (
            "two Regions past the bound, the product split across accounts and Regions",
            json!({
                "TargetAccounts": accounts(3),
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS / 2),
            }),
            3 * (MAX_DEPLOYMENT_TARGETS / 2),
        ),
    ]
}

/// The refusal a fan-out of `fan_out` deployments draws on `member`.
fn past_the_bound_message(member: &str, fan_out: usize) -> String {
    format!(
        "{member} names {fan_out} account and Region deployments, more than the \
         {MAX_DEPLOYMENT_TARGETS} this service deploys for one configuration definition"
    )
}

/// One count behind both the bound and the deployment, for the operation that CREATES a
/// configuration definition: a parameter set within the bound deploys exactly as many
/// configurations as it fans out to, and one past it is the modelled 400 naming that
/// same number.
#[tokio::test]
async fn a_create_deploys_and_bounds_one_fan_out_count() {
    for (case, parameters, fan_out) in cases() {
        // A store of its own per row, so what one definition fans out to is measured
        // against an empty one.
        let router = emulator();
        let (status, answer) = create(&router, "counted", &parameters).await;

        if fan_out > MAX_DEPLOYMENT_TARGETS {
            assert_eq!(
                status, 400,
                "case {case}: a fan-out of {fan_out} is past the bound, expected 400, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer:?}"
            );
            assert_eq!(
                error_message(&answer),
                past_the_bound_message(CREATE_MEMBER, fan_out),
                "case {case}: expected the refusal to state {fan_out} deployments, got {answer:?}"
            );
            continue;
        }

        let manager_arn = answer["ManagerArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {answer:?}"))
            .to_string();
        let deployed = deployments(&router, case, &manager_arn).await;
        assert_eq!(
            deployed.len(),
            fan_out,
            "case {case}: expected {fan_out} deployed configurations, got {}",
            deployed.len()
        );
        assert_eq!(
            pairs(&deployed).len(),
            fan_out,
            "case {case}: expected one deployment per account and Region pair, got {:?}",
            pairs(&deployed)
        );
    }
}

/// The same single count behind the bound and the deployment, for the operation that
/// UPDATES a configuration definition: the parameter set an update writes deploys
/// exactly as many configurations as it fans out to, one past the bound is the modelled
/// 400 naming that same number, and a refused update leaves the definition's deployed
/// set exactly as it was.
#[tokio::test]
async fn an_update_deploys_and_bounds_one_fan_out_count() {
    // The parameters every update below starts from: one targeted Region in the local
    // account, so a refused update is visibly the one that wrote nothing.
    let baseline = json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" });

    for (case, parameters, fan_out) in cases() {
        let router = emulator();
        let manager_arn = create_ok(&router, case, "counted", &baseline).await;
        let definition = definition_id(&router, case, &manager_arn).await;
        let before = deployments(&router, case, &manager_arn).await;
        assert_eq!(
            before.len(),
            1,
            "case {case}: the baseline definition fans out to one deployment, got {before:?}"
        );

        let (status, answer) =
            update_parameters(&router, &manager_arn, &definition, &parameters).await;

        if fan_out > MAX_DEPLOYMENT_TARGETS {
            assert_eq!(
                status, 400,
                "case {case}: a fan-out of {fan_out} is past the bound, expected 400, got {status} / {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ValidationException",
                "case {case}: expected ValidationException, got {answer:?}"
            );
            assert_eq!(
                error_message(&answer),
                past_the_bound_message(UPDATE_MEMBER, fan_out),
                "case {case}: expected the refusal to state {fan_out} deployments, got {answer:?}"
            );
            let after = deployments(&router, case, &manager_arn).await;
            assert_eq!(
                after, before,
                "case {case}: a refused update must leave the deployed set alone, got {after:?}"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case}: a fan-out of {fan_out} is within the bound, expected 200, got {status} / {answer:?}"
        );
        let deployed = deployments(&router, case, &manager_arn).await;
        assert_eq!(
            deployed.len(),
            fan_out,
            "case {case}: expected {fan_out} deployed configurations, got {}",
            deployed.len()
        );
        assert_eq!(
            pairs(&deployed).len(),
            fan_out,
            "case {case}: expected one deployment per account and Region pair, got {:?}",
            pairs(&deployed)
        );
    }
}
