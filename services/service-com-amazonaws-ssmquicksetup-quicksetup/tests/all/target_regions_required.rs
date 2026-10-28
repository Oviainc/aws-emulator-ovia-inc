// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that a stored configuration definition always names the
//! Regions it is deployed into.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a Quick Setup configuration definition is deployed into the
//! Regions its parameters name and into no others, so every request that writes a
//! definition's parameter map — the create that stores one and the update that replaces
//! one — is accepted only when that map provides a value for `TargetRegions`, and is
//! otherwise refused with a 400 `ValidationException` having written nothing; nothing
//! stands in for the Regions a definition did not name, least of all the Region the
//! request was signed for. The model's documentation for the member is where the rule is
//! stated: the parameter table documented for a configuration type marks
//! `TargetRegions` `(Required)` (`smithy_query(query="doc",
//! shape="ConfigurationDefinitionInput", member="Parameters")`).

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for: the Region a created manager owns, and so
/// the only Region whose endpoint serves it. Deliberately NOT a Region any accepted row
/// below targets, so a configuration reported in it could only come from the signature.
const REGION: &str = "us-west-2";

/// The sentence the service answers when a parameter map names no Region to deploy into.
const NO_REGION_MESSAGE: &str = "TargetRegions must be defined in parameters.";

/// The account this emulator serves, which a definition naming no `TargetAccounts` is
/// deployed in.
fn local_account() -> String {
    engine::emulator_account_id().to_string()
}

/// The parameters every manager below is created with, so each update starts from a
/// definition that DOES name the Regions to deploy into and a refused update is visibly
/// the one that left it alone.
fn baseline_parameters() -> Value {
    json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "us-east-1" })
}

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine),
    );
    router
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

/// `CreateConfigurationManager` with ONE definition of `type_` carrying `parameters`.
async fn create(
    router: &MultiServiceRouter,
    type_: &str,
    parameters: &Value,
) -> (u16, Value) {
    let body = json!({
        "Name": "regions",
        "ConfigurationDefinitions": [{ "Type": type_, "Parameters": parameters }],
    });
    call(router, "POST", "/configurationManager", Some(&body)).await
}

/// A create that must succeed, answering the manager ARN it minted.
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    type_: &str,
    parameters: &Value,
) -> String {
    let (status, answer) = create(router, type_, parameters).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// The id and the `Parameters` of the manager's single stored configuration definition,
/// read back through `GetConfigurationManager`.
async fn stored_definition(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> (String, Value) {
    let (status, manager) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetConfigurationManager must succeed, got {status} / {manager:?}"
    );
    let definition = manager["ConfigurationDefinitions"][0].clone();
    let id = definition["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a definition Id, got {manager:?}"))
        .to_string();
    let parameters = definition
        .get("Parameters")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: expected Parameters, got {manager:?}"));
    (id, parameters)
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
/// through `ListConfigurations`.
async fn deployed_pairs(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> BTreeSet<(String, String)> {
    let (status, page) = call(
        router,
        "POST",
        "/listConfigurations",
        Some(&json!({ "ManagerArn": manager_arn })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConfigurations must succeed, got {status} / {page:?}"
    );
    page["ConfigurationsList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a ConfigurationsList, got {page:?}"))
        .iter()
        .map(|row| {
            let account = row["Account"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected an Account, got {row:?}"));
            let region = row["Region"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a Region, got {row:?}"));
            (account.to_string(), region.to_string())
        })
        .collect()
}

/// How many configuration managers the store holds.
async fn stored_managers(router: &MultiServiceRouter, case: &str) -> usize {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurationManagers",
        Some(&json!({ "MaxItems": 100 })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
    );
    answer["ConfigurationManagersList"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a manager listing, got {answer:?}"))
        .len()
}

/// One row: the case, the configuration type the definition deploys, its parameter map,
/// and — when the map may be written — the Regions the definition must then be deployed
/// into. `None` is a row the service must refuse.
type RegionCase = (&'static str, &'static str, Value, Option<Vec<&'static str>>);

/// The table: parameter maps spanning what the rule generalises over — whether
/// `TargetRegions` is present at all, how many Regions it names, how that
/// comma-separated value is written (padding, empty entries, repeats), whether the key
/// is present but provides no value, the spelling of the key, which configuration type
/// the definition deploys (one per published table, including types the reported case
/// did not name), and which other parameters are named beside it.
///
/// The configuration types are entries of the catalog `ListQuickSetupTypes` publishes
/// (`crate::state::QUICK_SETUP_TYPE_CATALOG`), so no row is refused for naming a type
/// the service does not offer.
fn cases() -> Vec<RegionCase> {
    vec![
        (
            // Both target keys at once, so the only thing the map leaves out is the
            // Regions.
            "both target keys but no Regions",
            "AWSQuickSetupType-DHMC",
            json!({
                "TargetAccounts": "111122223333",
                "TargetOrganizationalUnits": "ou-1",
            }),
            None,
        ),
        (
            "a deployment target but no Regions",
            "AWSQuickSetupType-DHMC",
            json!({ "TargetOrganizationalUnits": "ou-1" }),
            None,
        ),
        (
            "a targeted account but no Regions",
            "AWSQuickSetupType-SSMHostMgmt",
            json!({ "TargetAccounts": "111122223333" }),
            None,
        ),
        (
            "type-specific options and a target, but no Regions",
            "AWSQuickSetupType-Scheduler",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetTagKey": "Environment",
                "TargetTagValue": "production",
                "ICalendarString": "BEGIN:VCALENDAR",
            }),
            None,
        ),
        (
            "the Regions written empty",
            "AWSQuickSetupType-CFGRecording",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "" }),
            None,
        ),
        (
            "the Regions written as blanks",
            "AWSQuickSetupType-CFGCPacks",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "   " }),
            None,
        ),
        (
            "the Regions written as separators only",
            "AWSQuickSetupType-DevOpsGuru",
            json!({ "TargetAccounts": "111122223333", "TargetRegions": " , ," }),
            None,
        ),
        (
            // Parameter keys are matched as the model writes them, so a differently
            // cased key is a different parameter and names no Region.
            "the Regions key in lower case",
            "AWSQuickSetupType-SSMOpsCenter",
            json!({ "TargetOrganizationalUnits": "ou-1", "targetregions": "eu-west-1" }),
            None,
        ),
        (
            // Singular: a key of this definition's own template, not the common one.
            "a key that only resembles the Regions key",
            "AWSQuickSetupType-PatchPolicy",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegion": "eu-west-1" }),
            None,
        ),
        (
            "one Region",
            "AWSQuickSetupType-DHMC",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "eu-west-1" }),
            Some(vec!["eu-west-1"]),
        ),
        (
            // A type whose own parameter table the model does not itemise: the common
            // parameters are common to every definition.
            "one Region on the default configuration type",
            "AWSQuickSetupType-SSM",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": "ap-south-1" }),
            Some(vec!["ap-south-1"]),
        ),
        (
            "three Regions",
            "AWSQuickSetupType-ResourceExplorer",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": "eu-west-1,eu-central-1,ap-southeast-2",
                "SelectedAggregatorRegion": "eu-west-1",
                "ReplaceExistingAggregator": "false",
            }),
            Some(vec!["eu-west-1", "eu-central-1", "ap-southeast-2"]),
        ),
        (
            "a Region list with padding, an empty entry and a repeat",
            "AWSQuickSetupType-SSMChangeMgr",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": " eu-west-1 , ,eu-west-1, ap-south-1 ",
                "DelegatedAccountId": "111122223333",
                "JobFunction": "operators",
            }),
            Some(vec!["eu-west-1", "ap-south-1"]),
        ),
        (
            "the Region the request itself was signed for",
            "AWSQuickSetupType-Distributor",
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": REGION }),
            Some(vec![REGION]),
        ),
    ]
}

/// A create stores a definition only when its parameters name the Regions to deploy
/// into: it deploys into exactly those Regions, and refuses the rest with the modelled
/// 400 having stored nothing.
#[tokio::test]
async fn a_create_stores_a_definition_only_when_its_parameters_name_the_target_regions() {
    let router = emulator();
    let local = local_account();
    let mut stored = 0_usize;

    for (case, type_, parameters, deployed) in cases() {
        let (status, answer) = create(&router, type_, &parameters).await;
        match &deployed {
            Some(regions) => {
                stored += 1;
                assert_eq!(
                    status, 200,
                    "case {case:?}: CreateConfigurationManager must accept {parameters:?}, \
                     got {status} / {answer:?}"
                );
                let manager_arn = answer["ManagerArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"));
                // Round trip: the stored definition carries the map the create named.
                let (_, written) = stored_definition(&router, case, manager_arn).await;
                assert_eq!(
                    written, parameters,
                    "case {case:?}: the stored definition must carry the parameters the create \
                     named, got {written:?}"
                );
                let want: BTreeSet<(String, String)> = regions
                    .iter()
                    .map(|region| (local.clone(), (*region).to_string()))
                    .collect();
                let got = deployed_pairs(&router, case, manager_arn).await;
                assert_eq!(
                    got, want,
                    "case {case:?}: the definition must be deployed into exactly the Regions it \
                     names, expected {want:?}, got {got:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: CreateConfigurationManager must refuse {parameters:?}, \
                     got {status} / {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {answer:?}"
                );
                assert_eq!(
                    error_message(&answer),
                    NO_REGION_MESSAGE,
                    "case {case:?}: expected the refusal to state the rule, got {answer:?}"
                );
            }
        }
        // A refused create stores nothing, so the manager count only moves on a create
        // that succeeded.
        let held = stored_managers(&router, case).await;
        assert_eq!(
            held, stored,
            "case {case:?}: expected {stored} stored managers, got {held}"
        );
    }
}

/// An update replaces a definition's whole parameter map, so it is accepted only when
/// the map it writes still names the Regions to deploy into — and a refused one leaves
/// the stored definition, deployments and all, exactly as it was.
#[tokio::test]
async fn an_update_replaces_a_parameter_map_only_when_it_still_names_the_target_regions() {
    let local = local_account();
    let baseline_pairs: BTreeSet<(String, String)> = [(local.clone(), "us-east-1".to_string())]
        .into_iter()
        .collect();

    for (case, type_, parameters, deployed) in cases() {
        // One manager per row, so no row observes another row's writes.
        let router = emulator();
        let manager_arn = create_ok(&router, case, type_, &baseline_parameters()).await;
        let (id, before) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            before,
            baseline_parameters(),
            "case {case:?}: the baseline create must store the baseline parameters, got {before:?}"
        );

        let (status, answer) = update_parameters(&router, &manager_arn, &id, &parameters).await;
        let (_, after) = stored_definition(&router, case, &manager_arn).await;
        let pairs = deployed_pairs(&router, case, &manager_arn).await;
        match &deployed {
            Some(regions) => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateConfigurationDefinition must accept {parameters:?}, \
                     got {status} / {answer:?}"
                );
                assert_eq!(
                    after, parameters,
                    "case {case:?}: the stored definition must carry the parameters the update \
                     wrote, got {after:?}"
                );
                let want: BTreeSet<(String, String)> = regions
                    .iter()
                    .map(|region| (local.clone(), (*region).to_string()))
                    .collect();
                assert_eq!(
                    pairs, want,
                    "case {case:?}: the update must redeploy into exactly the Regions it names, \
                     expected {want:?}, got {pairs:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: UpdateConfigurationDefinition must refuse {parameters:?}, \
                     got {status} / {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {answer:?}"
                );
                assert_eq!(
                    error_message(&answer),
                    NO_REGION_MESSAGE,
                    "case {case:?}: expected the refusal to state the rule, got {answer:?}"
                );
                assert_eq!(
                    after,
                    baseline_parameters(),
                    "case {case:?}: a refused update must leave the stored parameters alone, \
                     got {after:?}"
                );
                assert_eq!(
                    pairs, baseline_pairs,
                    "case {case:?}: a refused update must leave the deployments alone, \
                     got {pairs:?}"
                );
            }
        }
    }
}

/// The Region a request is signed for never stands in for the Regions a definition
/// names: every account and Region pair a manager deploys to comes from the parameter
/// map, so a manager created in one Region and targeting others reports none of its
/// configurations in the Region it was created through.
#[tokio::test]
async fn the_signed_region_is_never_a_deployment_region_of_its_own() {
    let case = "signed region is not a target";
    let router = emulator();
    let local = local_account();
    let other = "111122223333".to_string();

    let manager_arn = create_ok(
        &router,
        case,
        "AWSQuickSetupType-SSMHostMgmt",
        &json!({
            "TargetAccounts": format!("{local},{other}"),
            "TargetRegions": "eu-west-1,ap-south-1",
        }),
    )
    .await;

    let want: BTreeSet<(String, String)> = [
        (local.clone(), "eu-west-1".to_string()),
        (local.clone(), "ap-south-1".to_string()),
        (other.clone(), "eu-west-1".to_string()),
        (other.clone(), "ap-south-1".to_string()),
    ]
    .into_iter()
    .collect();
    let got = deployed_pairs(&router, case, &manager_arn).await;
    assert_eq!(
        got, want,
        "case {case:?}: the deployments must be the cross product of the named accounts and \
         Regions, expected {want:?}, got {got:?}"
    );
    assert!(
        got.iter().all(|(_, region)| region != REGION),
        "case {case:?}: no configuration may be reported in the Region the request was signed \
         for, got {got:?}"
    );
}
