// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the common parameters a listing reports.
//!
//! The rule under test: the `FirstClassParameters` of a summary carries only the
//! entries of a configuration definition's parameter map whose keys are common to a
//! configuration definition whatever Quick Setup type it deploys — the keys that say
//! where it is deployed — while the options one type's own template understands stay
//! out of it, so the member is a subset of the full `Parameters` map the single-resource
//! reads answer. The rule is one accessor in
//! `service_..._quicksetup::state`, so `ListConfigurations` (through
//! `ConfigurationSummary`) and `ListConfigurationManagers` (through
//! `ConfigurationDefinitionSummary`) report the same subset of one definition.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    COMMON_PARAMETER_KEYS, PARAMETER_TARGET_ACCOUNTS, PARAMETER_TARGET_ORGANIZATIONAL_UNITS,
    PARAMETER_TARGET_REGIONS,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for, and so the Region the managers it creates
/// are served from.
const REGION: &str = "us-west-2";

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
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
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

/// `CreateConfigurationManager` with ONE definition of `type_` carrying `parameters`,
/// answering the manager ARN it minted.
///
/// Every definition names the administration role a LOCAL deployment must name, since a
/// case here may target accounts and none of them is about that rule
/// (`tests/all/local_deployment_roles.rs`).
async fn create_ok(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    type_: &str,
    parameters: &Value,
) -> String {
    let body = json!({
        "Name": name,
        "ConfigurationDefinitions": [{
            "Type": type_,
            "Parameters": parameters,
            "LocalDeploymentAdministrationRoleArn":
                super::local_deployment_roles::administration_role_arn(),
        }],
    });
    let (status, answer) = call(router, "POST", "/configurationManager", Some(&body)).await;
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

/// The single `ConfigurationsList` row `ListConfigurations` reports for one manager.
async fn only_configuration(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurations",
        Some(&json!({ "ManagerArn": manager_arn })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListConfigurations must succeed, got {status} / {answer:?}"
    );
    let rows = answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"));
    assert_eq!(
        rows.len(),
        1,
        "case {case}: this definition targets one account and Region, so one configuration \
         was deployed, got {rows:?}"
    );
    rows[0].clone()
}

/// The `ConfigurationManagersList` entry `ListConfigurationManagers` reports for one
/// manager ARN.
async fn listed_manager(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "POST",
        "/listConfigurationManagers",
        Some(&json!({})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationManagersList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: expected a ConfigurationManagersList, got {answer:?}")
        })
        .iter()
        .find(|entry| entry.get("ManagerArn").and_then(Value::as_str) == Some(manager_arn))
        .cloned()
        .unwrap_or_else(|| panic!("case {case}: expected an entry for {manager_arn}, got {answer:?}"))
}

/// `GET /getConfiguration/{ConfigurationId}`, which must succeed.
async fn get_configuration_ok(
    router: &MultiServiceRouter,
    case: &str,
    configuration_id: &str,
) -> Value {
    let (status, got) = call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(configuration_id)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfiguration must succeed, got {status} / {got:?}"
    );
    got
}

/// `GET /configurationManager/{ManagerArn}`, which must succeed.
async fn get_manager_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, got) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {got:?}"
    );
    got
}

/// Every assertion one row of a table makes: a create, then the two listings against
/// the two single-resource reads.
///
/// `expected` is the subset the two summaries must report; `parameters` is the whole
/// map the create supplied, which the two single-resource reads must still answer in
/// full — so a caller comparing a summary against a full read observes exactly the
/// distinction the two modelled members draw.
async fn assert_summaries_report(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    type_: &str,
    parameters: &Value,
    expected: &Value,
) {
    let manager_arn = create_ok(router, case, name, type_, parameters).await;

    let configuration = only_configuration(router, case, &manager_arn).await;
    assert_eq!(
        configuration.get("FirstClassParameters"),
        Some(expected),
        "case {case}: ListConfigurations must report only the common parameters \
         {expected:?}, got {configuration:?}"
    );

    let manager = listed_manager(router, case, &manager_arn).await;
    let definition_summaries = manager
        .get("ConfigurationDefinitionSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case}: expected ConfigurationDefinitionSummaries, got {manager:?}")
        });
    assert_eq!(
        definition_summaries.len(),
        1,
        "case {case}: the manager owns one definition, got {definition_summaries:?}"
    );
    assert_eq!(
        definition_summaries[0].get("FirstClassParameters"),
        Some(expected),
        "case {case}: ListConfigurationManagers must report only the common parameters \
         {expected:?}, got {:?}",
        definition_summaries[0]
    );

    // The full map is what the single-resource reads answer, so the summary's subset is
    // observably a subset of it and nothing the create supplied was dropped from state.
    let configuration_id = configuration
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected an Id, got {configuration:?}"));
    let got = get_configuration_ok(router, case, configuration_id).await;
    assert_eq!(
        got.get("Parameters"),
        Some(parameters),
        "case {case}: GetConfiguration must report the full parameter map, got {got:?}"
    );
    let manager_read = get_manager_ok(router, case, &manager_arn).await;
    assert_eq!(
        manager_read["ConfigurationDefinitions"][0].get("Parameters"),
        Some(parameters),
        "case {case}: GetConfigurationManager must report the full parameter map, got \
         {manager_read:?}"
    );
    for (key, value) in expected
        .as_object()
        .unwrap_or_else(|| panic!("case {case}: the expected subset is an object"))
    {
        assert_eq!(
            parameters.get(key),
            Some(value),
            "case {case}: the reported common parameter {key} must be an entry of the \
             stored map {parameters:?}"
        );
    }
}

/// One row of the subset table: the case, the manager name, the configuration type, the
/// parameters the create supplies and the common subset the summaries must report.
type SubsetCase = (&'static str, &'static str, &'static str, Value, Value);

/// A summary reports the common parameters of a definition and leaves the options of
/// its configuration type out.
///
/// Each row differs along what the rule generalises over: which configuration type the
/// definition deploys, which of the common keys it names, how many type-specific
/// options sit beside them, whether there are any at all, and how close a key that is
/// NOT common comes to the spelling of one that is. The type-specific keys are taken
/// from the per-type parameter tables documented on
/// `ConfigurationDefinitionInput$Parameters` (`smithy_query(query="doc",
/// shape="ConfigurationDefinitionInput", member="Parameters")`).
#[tokio::test]
async fn a_summary_reports_only_the_common_parameters_of_a_definition() {
    let router = emulator();

    let cases: Vec<SubsetCase> = vec![
        (
            "host management with its own options",
            "host-mgmt",
            "AWSQuickSetupType-SSMHostMgmt",
            json!({
                "TargetOrganizationalUnits": "ou-1",
                "TargetRegions": REGION,
                "UpdateSSMAgent": "true",
                "CollectInventory": "true",
                "ScanInstances": "true",
            }),
            json!({ "TargetOrganizationalUnits": "ou-1", "TargetRegions": REGION }),
        ),
        (
            "conformance packs targeted by account",
            "cpacks",
            "AWSQuickSetupType-CFGCPacks",
            json!({
                "TargetAccounts": "111122223333",
                "TargetRegions": "eu-west-1",
                "DelegatedAccountId": "111122223333",
                "CPackNames": "pack-a,pack-b",
                "RemediationSchedule": "rate(30 days)",
            }),
            json!({ "TargetAccounts": "111122223333", "TargetRegions": "eu-west-1" }),
        ),
        (
            "resource scheduler naming every common key",
            "scheduler",
            "AWSQuickSetupType-Scheduler",
            json!({
                "TargetAccounts": "111122223333",
                "TargetOrganizationalUnits": "ou-2",
                "TargetRegions": "us-east-1",
                "TargetTagKey": "Environment",
                "TargetTagValue": "production",
                "ICalendarString": "BEGIN:VCALENDAR",
            }),
            json!({
                "TargetAccounts": "111122223333",
                "TargetOrganizationalUnits": "ou-2",
                "TargetRegions": "us-east-1",
            }),
        ),
        (
            "a definition carrying nothing but common keys",
            "dhmc",
            "AWSQuickSetupType-DHMC",
            json!({ "TargetOrganizationalUnits": "ou-3", "TargetRegions": "us-east-2" }),
            json!({ "TargetOrganizationalUnits": "ou-3", "TargetRegions": "us-east-2" }),
        ),
        (
            "config recording with many options",
            "recording",
            "AWSQuickSetupType-CFGRecording",
            json!({
                "TargetOrganizationalUnits": "ou-4",
                "TargetRegions": "eu-central-1",
                "RecordAllResources": "true",
                "ResourceTypesToRecord": "AWS::EC2::Instance",
                "RecordGlobalResourceTypes": "false",
                "DeliveryBucketName": "recording-bucket",
                "NotificationOptions": "NoStreaming",
            }),
            json!({ "TargetOrganizationalUnits": "ou-4", "TargetRegions": "eu-central-1" }),
        ),
        (
            "keys that only resemble a common one",
            "near-misses",
            "AWSQuickSetupType-SSMHostMgmt",
            json!({
                "TargetOrganizationalUnits": "ou-5",
                "TargetRegions": "ap-south-1",
                "TargetRegion": "ap-south-1",
                "targetregions": "ap-south-1",
                "Target Regions": "ap-south-1",
            }),
            json!({ "TargetOrganizationalUnits": "ou-5", "TargetRegions": "ap-south-1" }),
        ),
    ];

    for (case, name, type_, parameters, expected) in cases {
        assert_summaries_report(&router, case, name, type_, &parameters, &expected).await;
    }
}

/// The value a common parameter key is given in the test below: a targeted account is
/// named by its account id, a targeted Region by a Region name, a targeted
/// organizational unit by an OU id. A key the crate publishes and this list does not
/// know is a case to be written, not a value to be guessed.
fn common_value(key: &str) -> &'static str {
    match key {
        PARAMETER_TARGET_ACCOUNTS => "111122223333",
        PARAMETER_TARGET_REGIONS => "us-east-1",
        PARAMETER_TARGET_ORGANIZATIONAL_UNITS => "ou-9",
        other => panic!("no value is known for the common parameter key {other:?}"),
    }
}

/// Every key the crate publishes as common is reported, driven off the published set
/// rather than a list written out here, so a key added to it is covered too.
///
/// Each case adds ONE common key to a definition that already names where it deploys,
/// beside a type-specific option, and the summary must report the common keys and only
/// those.
#[tokio::test]
async fn every_published_common_key_is_reported_and_an_option_is_not() {
    let router = emulator();
    assert!(
        !COMMON_PARAMETER_KEYS.is_empty(),
        "the published common parameter keys must not be empty, got {COMMON_PARAMETER_KEYS:?}"
    );

    for (index, key) in COMMON_PARAMETER_KEYS.iter().enumerate() {
        let case = format!("common key {key}");
        let mut expected = Map::new();
        // Somewhere to deploy to and the Region to deploy into, so the create is
        // accepted whichever key is under test.
        expected.insert(
            PARAMETER_TARGET_ORGANIZATIONAL_UNITS.to_string(),
            json!(common_value(PARAMETER_TARGET_ORGANIZATIONAL_UNITS)),
        );
        expected.insert(
            PARAMETER_TARGET_REGIONS.to_string(),
            json!(common_value(PARAMETER_TARGET_REGIONS)),
        );
        expected.insert((*key).to_string(), json!(common_value(key)));

        let mut parameters = expected.clone();
        // A type-specific option of the configuration type below, which no summary
        // reports.
        parameters.insert("UpdateSSMAgent".to_string(), json!("true"));

        assert_summaries_report(
            &router,
            &case,
            &format!("published-{index}"),
            "AWSQuickSetupType-SSMHostMgmt",
            &Value::Object(parameters),
            &Value::Object(expected),
        )
        .await;
    }
}
