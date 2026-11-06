// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that the two operations which write a configuration
//! definition read its deployment targets from the same parameters.
//!
//! The rule under test: a configuration definition's deployment targets are the
//! `TargetRegions`, `TargetAccounts` and `TargetOrganizationalUnits` entries of its
//! parameter map, so the operation that creates a definition and the operation that
//! updates one deploy the same set for one parameter map and refuse the same parameter
//! maps, with the same statement of the rule. Both drive the shared accessor in
//! `service_..._quicksetup::state`; every case below goes through the real wire path
//! (the production `MultiServiceRouter`, restJson1 serialization, the
//! modelled-constraint walk and the handlers), so what is asserted is what a signed AWS
//! SDK request observes.

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

/// The (`Account`, `Region`) pair of every configuration deployed for one manager, read
/// through `ListConfigurations` and paged to the end, since the fan-out may exceed one
/// page.
async fn deployed_pairs(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeSet<(String, String)> {
    let mut pairs = BTreeSet::new();
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
            let account = row["Account"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an Account, got {row:?}"));
            let region = row["Region"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a Region, got {row:?}"));
            pairs.insert((account.to_string(), region.to_string()));
        }
        // Every page carries the token; the empty string is the one that says this page
        // was the last.
        match page.get("NextToken").and_then(Value::as_str) {
            Some(next) if !next.is_empty() => token = Some(next.to_string()),
            _ => break,
        }
    }
    pairs
}

/// The account this emulator serves, which a definition naming no `TargetAccounts`
/// deploys in.
fn local_account() -> String {
    engine::emulator_account_id().to_string()
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

/// What one parameter map must produce, whichever operation writes it.
#[derive(Debug)]
enum Expected {
    /// Deployed to exactly these (account, Region) pairs.
    Targets(Vec<(String, String)>),
    /// Refused with a 400 `ValidationException`, nothing written.
    Refused,
}

/// Every (account, Region) pair the cross product of `accounts` and `regions` names.
fn product(accounts: &[String], regions: &[String]) -> Vec<(String, String)> {
    accounts
        .iter()
        .flat_map(|account| {
            regions
                .iter()
                .map(move |region| (account.clone(), region.clone()))
        })
        .collect()
}

/// The `Parameters` an update sends to put a definition on exactly the deployment
/// targets `parameters` names: the row's own entries, plus an EMPTY value for every
/// target key the row leaves out.
///
/// An update writes the entries its `Parameters` names into the map the definition holds
/// and leaves every other stored entry alone, so a stored target key the row does not
/// name is dropped by naming it with no value: a key that provides no value names no
/// target, exactly as a key left out of a create's parameter map does. That is what
/// makes the two operations comparable row by row — both end up with a definition whose
/// parameters name the row's targets and nothing else.
fn update_entries(parameters: &Value) -> Value {
    let mut entries = parameters.as_object().cloned().unwrap_or_default();
    for key in [
        "TargetAccounts",
        "TargetOrganizationalUnits",
        "TargetRegions",
    ] {
        entries.entry(key.to_string()).or_insert_with(|| json!(""));
    }
    Value::Object(entries)
}

/// One row: the case, the definition's `Parameters`, and what both operations must do
/// with them.
type ParameterCase = (&'static str, Value, Expected);

/// The table: parameter maps spanning what the rule generalises over — whether each
/// target key is present at all, how many entries it names, how the comma-separated
/// value is written (padding, empty entries, repeats), a parameter map that names no
/// deployment target at all, one that names no Region to deploy into, a target set
/// outside what may be deployed, the deployment
/// bound itself (at it and one past it), and a target list far longer than that bound.
fn cases(local: &str) -> Vec<ParameterCase> {
    let other = "111122223333".to_string();
    let third = "444455556666".to_string();
    // `MAX_DEPLOYMENT_TARGETS` is the published bound of the fan-out
    // (`crate::state::MAX_DEPLOYMENT_TARGETS`), so the last two rows sit on it and one
    // past it; the product is split across accounts so neither list alone decides it.
    let bound_accounts: Vec<String> = (0..4).map(|index| format!("{:012}", index + 1)).collect();
    let bound_regions: Vec<String> = (0..MAX_DEPLOYMENT_TARGETS / 4)
        .map(|index| format!("eu-west-{}", index + 1))
        .collect();
    vec![
        (
            "neither target key",
            json!({}),
            Expected::Refused,
        ),
        (
            "only the organizational units, no Region",
            json!({ "TargetOrganizationalUnits": "ou-1" }),
            Expected::Refused,
        ),
        (
            "one targeted Region, the accounts left to the organizational units",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "eu-west-1" }),
            Expected::Targets(vec![(local.to_string(), "eu-west-1".to_string())]),
        ),
        (
            "one targeted account, no Region",
            json!({ "TargetAccounts": &other }),
            Expected::Refused,
        ),
        (
            "one targeted account in one Region",
            json!({ "TargetAccounts": &other, "TargetRegions": REGION }),
            Expected::Targets(vec![(other.clone(), REGION.to_string())]),
        ),
        (
            "two accounts and two Regions",
            json!({
                "TargetAccounts": format!("{other},{third}"),
                "TargetRegions": "eu-west-1,ap-south-1",
            }),
            Expected::Targets(product(
                &[other.clone(), third.clone()],
                &["eu-west-1".to_string(), "ap-south-1".to_string()],
            )),
        ),
        (
            "padding, empty entries and repeats in both lists",
            json!({
                "TargetAccounts": format!(" {other} ,,{other}"),
                "TargetRegions": " eu-west-1 ,, eu-west-1 , ap-south-1 ",
            }),
            Expected::Targets(product(
                std::slice::from_ref(&other),
                &["eu-west-1".to_string(), "ap-south-1".to_string()],
            )),
        ),
        (
            "both target lists written empty",
            json!({ "TargetAccounts": "", "TargetOrganizationalUnits": "", "TargetRegions": "" }),
            Expected::Refused,
        ),
        (
            // Only the target keys name a deployment target, so a map of other
            // parameters names nowhere to deploy to.
            "a parameter that names no target",
            json!({ "UpdateSSMAgent": "true" }),
            Expected::Refused,
        ),
        (
            "an account that is not an account id",
            json!({ "TargetAccounts": "my-account", "TargetRegions": "eu-west-1" }),
            Expected::Refused,
        ),
        (
            "exactly the deployment bound, split across accounts and Regions",
            json!({
                "TargetAccounts": accounts(4),
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS / 4),
            }),
            Expected::Targets(product(&bound_accounts, &bound_regions)),
        ),
        (
            "one account past the deployment bound",
            json!({
                "TargetAccounts": accounts(5),
                "TargetRegions": regions(MAX_DEPLOYMENT_TARGETS / 4),
            }),
            Expected::Refused,
        ),
        (
            // Far more accounts than may be deployed to, with one entry that is not an
            // account id among them: a list this long names more deployments than this
            // service serves whatever its further entries say, so both operations refuse
            // it on the fan-out and state the same rule.
            "far more accounts than the deployment bound, one of them malformed",
            json!({
                "TargetAccounts": format!(
                    "{},my-account,{}",
                    accounts(MAX_DEPLOYMENT_TARGETS * 2),
                    accounts(1)
                ),
                "TargetRegions": "eu-west-1",
            }),
            Expected::Refused,
        ),
    ]
}

/// A create and an update read one definition's deployment targets from the same
/// parameters: for every parameter map in the table, both operations deploy the same
/// account-and-Region set, and both refuse the same maps with a 400
/// `ValidationException` stating the same rule.
#[tokio::test]
async fn a_create_and_an_update_read_the_same_targets_from_one_parameter_map() {
    let router = emulator();
    let local = local_account();
    // The parameters every update below starts from: one targeted Region in the local
    // account, so a refused update is visibly the one that wrote nothing.
    let baseline = json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" });
    let baseline_pairs: BTreeSet<(String, String)> =
        [(local.clone(), "us-east-1".to_string())].into_iter().collect();

    for (case, parameters, expected) in cases(&local) {
        // The create path: one manager per row, carrying the row's parameters.
        let (create_status, created) = create(&router, "created", &parameters).await;

        // The update path: a manager created from the baseline, then updated onto the
        // row's parameters.
        let update_case = format!("{case} (update)");
        let manager_arn = create_ok(&router, &update_case, "updated", &baseline).await;
        let definition = definition_id(&router, &update_case, &manager_arn).await;
        assert_eq!(
            deployed_pairs(&router, &update_case, &manager_arn).await,
            baseline_pairs,
            "case {case}: the baseline create must deploy the baseline targets"
        );
        let (update_status, updated) =
            update_parameters(&router, &manager_arn, &definition, &update_entries(&parameters))
                .await;

        match expected {
            Expected::Targets(want) => {
                let want: BTreeSet<(String, String)> = want.into_iter().collect();
                assert_eq!(
                    create_status, 200,
                    "case {case}: CreateConfigurationManager must succeed, got {create_status} / {created:?}"
                );
                assert_eq!(
                    update_status, 200,
                    "case {case}: UpdateConfigurationDefinition must succeed, got {update_status} / {updated:?}"
                );
                let created_arn = created["ManagerArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a ManagerArn, got {created:?}"));
                let from_create = deployed_pairs(&router, case, created_arn).await;
                let from_update = deployed_pairs(&router, &update_case, &manager_arn).await;
                assert_eq!(
                    from_create, want,
                    "case {case}: expected the create to deploy {want:?}, got {from_create:?}"
                );
                assert_eq!(
                    from_update, want,
                    "case {case}: expected the update to deploy {want:?}, got {from_update:?}"
                );
            }
            Expected::Refused => {
                assert_eq!(
                    create_status, 400,
                    "case {case}: CreateConfigurationManager must refuse these parameters, got {create_status} / {created:?}"
                );
                assert_eq!(
                    update_status, 400,
                    "case {case}: UpdateConfigurationDefinition must refuse these parameters, got {update_status} / {updated:?}"
                );
                assert_eq!(
                    error_code(&created),
                    "ValidationException",
                    "case {case}: expected ValidationException from the create, got {created:?}"
                );
                assert_eq!(
                    error_code(&updated),
                    "ValidationException",
                    "case {case}: expected ValidationException from the update, got {updated:?}"
                );
                // One rule, one message: a refusal stating one of this service's own
                // bounds names the member the parameters arrived on, so the member is
                // taken off before the two are compared; a refusal stating a rule of
                // the API itself names no member and is compared as it stands.
                let from_create = error_message(&created);
                let from_update = error_message(&updated);
                let create_tail = from_create
                    .strip_prefix(CREATE_MEMBER)
                    .unwrap_or(&from_create);
                let update_tail = from_update
                    .strip_prefix(UPDATE_MEMBER)
                    .unwrap_or(&from_update);
                assert_eq!(
                    create_tail, update_tail,
                    "case {case}: both refusals must state one rule, got {from_create:?} and {from_update:?}"
                );
                // A refused update writes nothing: the definition keeps the baseline
                // targets it was deployed to.
                let after = deployed_pairs(&router, &update_case, &manager_arn).await;
                assert_eq!(
                    after, baseline_pairs,
                    "case {case}: a refused update must leave the deployed set alone, got {after:?}"
                );
            }
        }
    }
}
