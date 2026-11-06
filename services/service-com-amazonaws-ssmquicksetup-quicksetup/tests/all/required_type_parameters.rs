// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests that a stored configuration definition provides every
//! parameter its own configuration type requires of it.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a Quick Setup configuration definition deploys the
//! service-owned template its `Type` names, with the parameters its `Parameters` map
//! provides, so every request that writes a definition's parameter map — the create
//! that stores one and the update that writes entries into one — is accepted only when
//! the map the definition then HOLDS provides a value for every parameter the table
//! documented for THAT type marks `(Required)`, under the condition that table states
//! for it, and is otherwise refused with a 400 `ValidationException` having written
//! nothing. The model's documentation for the member is where the rule is stated: one
//! parameter table per configuration type, each entry marked `(Required)` or
//! `(Optional)` (`smithy_query(query="doc", shape="ConfigurationDefinitionInput",
//! member="Parameters")`); the crate publishes those markers per type as
//! `QuickSetupTypeEntry::required_parameters`, so the per-type rows below are driven
//! off the catalog rather than written out once more here.

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    RequiredWhen, NO_DEPLOYMENT_REGION_MESSAGE, PARAMETER_TARGET_REGIONS,
    QUICK_SETUP_TYPE_CATALOG,
};
use smithy_server_runtime::MultiServiceRouter;

use super::type_parameter_fixtures::{documented_value, required_keys};

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The one Region every definition below deploys into, so each deploys exactly one
/// configuration and no case here turns on the fan-out: the value the shared fixture
/// documents for that parameter.
fn target_region() -> &'static str {
    documented_value("TargetRegions")
}

/// The sentence the service answers when a definition's parameter map provides no value
/// for a parameter it must provide: the service's own wording, the same template it
/// answers for the Regions every type requires ("TargetRegions must be defined in
/// parameters.") and for the either/or between the two deployment-target keys.
fn must_be_defined(key: &str) -> String {
    format!("{key} must be defined in parameters.")
}

/// The parameters a definition of `type_` must provide and nothing else: the Regions to
/// deploy into, somewhere to deploy to, and every parameter that type's own table
/// requires — each with the value the shared fixture documents for it
/// ([`documented_value`]), so this suite and the suites that are about other rules send
/// the same forms.
///
/// The deployment target is the organizational unit, which every published table either
/// marks `(Required)` or offers as one of the two keys a definition may name, so one
/// complete set is built the same way for every type.
fn complete_parameters(type_: &str) -> Value {
    let mut parameters = Map::new();
    for key in ["TargetOrganizationalUnits", "TargetRegions"] {
        parameters.insert(key.to_string(), json!(documented_value(key)));
    }
    // Read against the map as it grows, so a key required only while another carries a
    // value is added once that value is there.
    while let Some(key) = required_keys(type_, &Value::Object(parameters.clone()))
        .into_iter()
        .find(|key| !parameters.contains_key(*key))
    {
        parameters.insert(key.to_string(), json!(documented_value(key)));
    }
    Value::Object(parameters)
}

/// `parameters` with `entries` written over it, the way an update writes the entries it
/// names into the map a definition holds and leaves the rest alone.
fn with_entries(parameters: &Value, entries: &Value) -> Value {
    let mut map = parameters.as_object().cloned().unwrap_or_default();
    for (key, value) in entries.as_object().cloned().unwrap_or_default() {
        map.insert(key, value);
    }
    Value::Object(map)
}

/// `parameters` without the entry `key` names.
fn without(parameters: &Value, key: &str) -> Value {
    let mut map = parameters.as_object().cloned().unwrap_or_default();
    map.remove(key);
    Value::Object(map)
}

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

/// `CreateConfigurationManager` with ONE definition of `type_` carrying `parameters`.
async fn create(router: &MultiServiceRouter, type_: &str, parameters: &Value) -> (u16, Value) {
    // Every definition names the administration role a LOCAL deployment must name, since
    // a case here may target accounts and none of them is about that rule
    // (`tests/all/local_deployment_roles.rs`).
    let body = json!({
        "Name": "required-parameters",
        "ConfigurationDefinitions": [{
            "Type": type_,
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
    type_: &str,
    parameters: &Value,
) -> String {
    let (status, answer) = create(router, type_, parameters).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateConfigurationManager must accept {parameters:?}, \
         got {status} / {answer:?}"
    );
    answer
        .get("ManagerArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a ManagerArn, got {answer:?}"))
        .to_string()
}

/// A create that must be refused for the parameter `key`, with nothing stored for it.
async fn assert_create_refused(
    router: &MultiServiceRouter,
    case: &str,
    type_: &str,
    parameters: &Value,
    key: &str,
) {
    let before = stored_managers(router, case).await;
    let (status, answer) = create(router, type_, parameters).await;
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
        must_be_defined(key),
        "case {case:?}: expected the refusal to name {key:?}, got {answer:?}"
    );
    let after = stored_managers(router, case).await;
    assert_eq!(
        after, before,
        "case {case:?}: a refused create must store nothing, expected {before} stored \
         managers, got {after}"
    );
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

/// A parameter set that provides every parameter its type requires is stored and
/// deployed, for EVERY type the catalog publishes — including the types whose tables
/// require nothing of their own, so the requirement never refuses a complete set.
///
/// Round trip: the stored definition is read back through `GetConfigurationManager` and
/// reports the map the create named, and the configuration it deployed is reported in
/// the account and Region that map names.
#[tokio::test]
async fn a_complete_parameter_set_is_stored_and_deployed_for_every_published_type() {
    assert!(
        !QUICK_SETUP_TYPE_CATALOG.is_empty(),
        "the published catalog must not be empty"
    );
    let local = engine::emulator_account_id().to_string();

    for entry in QUICK_SETUP_TYPE_CATALOG {
        // One manager per row, so no row observes another row's writes.
        let router = emulator();
        let case = entry.type_;
        let parameters = complete_parameters(entry.type_);
        let manager_arn = create_ok(&router, case, entry.type_, &parameters).await;

        let (_, stored) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            stored, parameters,
            "case {case:?}: the stored definition must carry the parameters the create \
             named, got {stored:?}"
        );
        let want: BTreeSet<(String, String)> = [(local.clone(), target_region().to_string())]
            .into_iter()
            .collect();
        let got = deployed_pairs(&router, case, &manager_arn).await;
        assert_eq!(
            got, want,
            "case {case:?}: the definition must be deployed where its parameters name, \
             expected {want:?}, got {got:?}"
        );
    }
}

/// How a parameter set can fail to PROVIDE a value for a key it names: the same reading
/// the deployment-target keys get, where a key written with no entries provides none.
/// `None` is the key left out of the map altogether.
const UNPROVIDED: [Option<&str>; 3] = [None, Some(""), Some("   ")];

/// Every parameter a published type's table requires of a definition is required of it:
/// a create whose map provides no value for one is refused with the modelled 400 naming
/// that parameter, having stored nothing.
///
/// Driven off the published catalog, so the rows span every type and every parameter its
/// own table requires — including the types and keys the report that prompted this suite
/// did not cite — crossed with each way a map can provide no value for a key it names.
#[tokio::test]
async fn a_create_is_refused_when_a_required_parameter_provides_no_value() {
    let router = emulator();
    let mut rows = 0_usize;

    for entry in QUICK_SETUP_TYPE_CATALOG {
        let complete = complete_parameters(entry.type_);
        for key in required_keys(entry.type_, &complete) {
            for written in UNPROVIDED {
                let case = format!(
                    "{} without a value for {key} (written {written:?})",
                    entry.type_
                );
                let parameters = match written {
                    None => without(&complete, key),
                    Some(value) => with_entries(&complete, &json!({ key: value })),
                };
                assert_create_refused(&router, &case, entry.type_, &parameters, key).await;
                rows += 1;
            }
        }
    }

    // The published tables require parameters of most of their types, so a run that
    // asserted nothing would be a silent pass.
    assert!(
        rows >= 3 * UNPROVIDED.len(),
        "the published catalog must require parameters of its types, got {rows} rows"
    );
}

/// One row: the case, the type, the parameter entries to write over a complete set, and
/// the parameter the service must refuse the create for — `None` for a row it must
/// accept.
type TargetRow = (&'static str, &'static str, Value, Option<&'static str>);

/// Which deployment-target key a definition must name is its own type's table's to
/// state: the tables that mark `TargetOrganizationalUnits` `(Required)` name no
/// `TargetAccounts` to stand in for it, while the tables that mark both `(Optional)` ask
/// for a value for either one.
#[tokio::test]
async fn the_deployment_target_a_type_must_name_is_the_one_its_own_table_requires() {
    let router = emulator();
    // The accounts written in place of the organizational unit, for each type: the four
    // tables that mark the OU `(Required)` (OpsCenter, Change Manager, Default Host
    // Management Configuration, Resource Explorer) and, beside them, tables that mark
    // both target keys `(Optional)` and so accept either.
    let rows: Vec<TargetRow> = vec![
        (
            "OpsCenter targeted by account alone",
            "AWSQuickSetupType-SSMOpsCenter",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            Some("TargetOrganizationalUnits"),
        ),
        (
            "Change Manager targeted by account alone",
            "AWSQuickSetupType-SSMChangeMgr",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            Some("TargetOrganizationalUnits"),
        ),
        (
            "Default Host Management Configuration targeted by account alone",
            "AWSQuickSetupType-DHMC",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            Some("TargetOrganizationalUnits"),
        ),
        (
            "Resource Explorer targeted by account alone",
            "AWSQuickSetupType-ResourceExplorer",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            Some("TargetOrganizationalUnits"),
        ),
        (
            // The OU written empty provides no value for it, so the requirement is the
            // one that refuses the set even with an account beside it.
            "Resource Explorer naming an account and an empty OU",
            "AWSQuickSetupType-ResourceExplorer",
            json!({
                "TargetAccounts": documented_value("TargetAccounts"),
                "TargetOrganizationalUnits": "",
            }),
            Some("TargetOrganizationalUnits"),
        ),
        (
            "Resource Scheduler targeted by account alone",
            "AWSQuickSetupType-Scheduler",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            None,
        ),
        (
            "Conformance Packs targeted by account alone",
            "AWSQuickSetupType-CFGCPacks",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            None,
        ),
        (
            "Host Management targeted by account alone",
            "AWSQuickSetupType-SSMHostMgmt",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            None,
        ),
        (
            // The default configuration type, whose own table the model does not
            // itemise: the either/or between the two target keys is what holds for it.
            "the default configuration type targeted by account alone",
            "AWSQuickSetupType-SSM",
            json!({ "TargetAccounts": documented_value("TargetAccounts") }),
            None,
        ),
    ];

    for (case, type_, entries, refused_for) in rows {
        let parameters = with_entries(
            &without(&complete_parameters(type_), "TargetOrganizationalUnits"),
            &entries,
        );
        match refused_for {
            Some(key) => assert_create_refused(&router, case, type_, &parameters, key).await,
            None => {
                let manager_arn = create_ok(&router, case, type_, &parameters).await;
                let (_, stored) = stored_definition(&router, case, &manager_arn).await;
                assert_eq!(
                    stored, parameters,
                    "case {case:?}: the stored definition must carry the parameters the \
                     create named, got {stored:?}"
                );
            }
        }
    }
}

/// A requirement whose table states a condition holds only while that condition does: a
/// definition that does not target instances the way the condition names may leave the
/// parameter out, and one that does may not.
///
/// The rows span both tables that state such a condition (Distributor, Patch Policy),
/// each of the two values it reads, the parameter the condition is stated on, and — as
/// the contrast — the table that marks those same three keys `(Optional)` for every
/// value of that parameter (Host Management).
#[tokio::test]
async fn a_conditional_requirement_holds_only_while_its_stated_condition_does() {
    let router = emulator();
    let rows: Vec<TargetRow> = vec![
        (
            "Distributor targeting instances by tag, with no tag key",
            "AWSQuickSetupType-Distributor",
            json!({ "TargetType": "Tags" }),
            Some("TargetTagKey"),
        ),
        (
            "Distributor targeting instances by tag, with a key and no value",
            "AWSQuickSetupType-Distributor",
            json!({ "TargetType": "Tags", "TargetTagKey": documented_value("TargetTagKey") }),
            Some("TargetTagValue"),
        ),
        (
            "Distributor targeting instances by tag, with both",
            "AWSQuickSetupType-Distributor",
            json!({
                "TargetType": "Tags",
                "TargetTagKey": documented_value("TargetTagKey"),
                "TargetTagValue": documented_value("TargetTagValue"),
            }),
            None,
        ),
        (
            "Distributor targeting a resource group, with no group name",
            "AWSQuickSetupType-Distributor",
            json!({ "TargetType": "ResourceGroups" }),
            Some("ResourceGroupName"),
        ),
        (
            "Distributor targeting a resource group, with its name",
            "AWSQuickSetupType-Distributor",
            json!({
                "TargetType": "ResourceGroups",
                "ResourceGroupName": documented_value("ResourceGroupName"),
            }),
            None,
        ),
        (
            // Neither condition holds, so none of the three keys is required: `*` is a
            // value its own table names for this parameter.
            "Distributor targeting every instance in the account",
            "AWSQuickSetupType-Distributor",
            json!({ "TargetType": "*" }),
            None,
        ),
        (
            "Distributor naming no TargetType at all",
            "AWSQuickSetupType-Distributor",
            json!({}),
            None,
        ),
        (
            "Patch Policy targeting instances by tag, with no tag key",
            "AWSQuickSetupType-PatchPolicy",
            json!({ "TargetType": "Tags" }),
            Some("TargetTagKey"),
        ),
        (
            "Patch Policy targeting a resource group, with no group name",
            "AWSQuickSetupType-PatchPolicy",
            json!({ "TargetType": "ResourceGroups" }),
            Some("ResourceGroupName"),
        ),
        (
            // Its own table marks all three of those keys `(Optional)`, so the same
            // parameter value requires nothing of it.
            "Host Management targeting instances by tag, with no tag key",
            "AWSQuickSetupType-SSMHostMgmt",
            json!({ "TargetType": "Tags" }),
            None,
        ),
        (
            "Host Management targeting a resource group, with no group name",
            "AWSQuickSetupType-SSMHostMgmt",
            json!({ "TargetType": "ResourceGroups" }),
            None,
        ),
    ];

    for (case, type_, entries, refused_for) in rows {
        // A complete set for the type with the row's own targeting entries written over
        // it, so the only thing in question is the conditional requirement.
        let parameters = with_entries(&complete_parameters(type_), &entries);
        match refused_for {
            Some(key) => {
                let parameters = without(&parameters, key);
                assert_create_refused(&router, case, type_, &parameters, key).await;
            }
            None => {
                let manager_arn = create_ok(&router, case, type_, &parameters).await;
                let (_, stored) = stored_definition(&router, case, &manager_arn).await;
                assert_eq!(
                    stored, parameters,
                    "case {case:?}: the stored definition must carry the parameters the \
                     create named, got {stored:?}"
                );
            }
        }
    }
}

/// One update row: the case, the type of the definition it writes into, the entries it
/// writes, and the parameter the service must refuse it for — `None` for a row it must
/// accept.
type UpdateRow = (&'static str, &'static str, Value, Option<&'static str>);

/// An update writes the entries its `Parameters` names into the map the definition
/// holds, so it is accepted only when the map it leaves behind still provides every
/// parameter the definition's own type requires — `Type` is not an input of the
/// operation, so the STORED type is what the merged map is judged against. A refused
/// update leaves the stored definition, deployments and all, exactly as it was.
#[tokio::test]
async fn an_update_is_refused_when_it_leaves_a_required_parameter_without_a_value() {
    let local = engine::emulator_account_id().to_string();
    let baseline_pairs: BTreeSet<(String, String)> =
        [(local, target_region().to_string())].into_iter().collect();

    let rows: Vec<UpdateRow> = vec![
        (
            "the Scheduler schedule written empty",
            "AWSQuickSetupType-Scheduler",
            json!({ "ICalendarString": "" }),
            Some("ICalendarString"),
        ),
        (
            "the Scheduler tag value written as blanks",
            "AWSQuickSetupType-Scheduler",
            json!({ "TargetTagValue": "   " }),
            Some("TargetTagValue"),
        ),
        (
            "the OpsCenter delegated administrator written empty",
            "AWSQuickSetupType-SSMOpsCenter",
            json!({ "DelegatedAccountId": "" }),
            Some("DelegatedAccountId"),
        ),
        (
            "the OpsCenter organizational units written empty",
            "AWSQuickSetupType-SSMOpsCenter",
            json!({ "TargetOrganizationalUnits": "" }),
            Some("TargetOrganizationalUnits"),
        ),
        (
            "the conformance pack list written empty",
            "AWSQuickSetupType-CFGCPacks",
            json!({ "CPackNames": "" }),
            Some("CPackNames"),
        ),
        (
            "the Resource Explorer aggregator Region written empty",
            "AWSQuickSetupType-ResourceExplorer",
            json!({ "SelectedAggregatorRegion": "" }),
            Some("SelectedAggregatorRegion"),
        ),
        (
            "the Change Manager job function written empty",
            "AWSQuickSetupType-SSMChangeMgr",
            json!({ "JobFunction": "" }),
            Some("JobFunction"),
        ),
        (
            "the Distributor package list written empty",
            "AWSQuickSetupType-Distributor",
            json!({ "PackagesToInstall": "" }),
            Some("PackagesToInstall"),
        ),
        (
            // The condition is met by the very entries this update writes, so the
            // requirement it brings with it is judged against the merged map.
            "a Distributor update that starts targeting instances by tag",
            "AWSQuickSetupType-Distributor",
            json!({ "TargetType": "Tags" }),
            Some("TargetTagKey"),
        ),
        (
            "a Scheduler update rewriting its schedule",
            "AWSQuickSetupType-Scheduler",
            json!({ "ICalendarString": "BEGIN:VCALENDAR;FREQ=DAILY" }),
            None,
        ),
        (
            "a Patch Policy update naming a type-specific option of its own",
            "AWSQuickSetupType-PatchPolicy",
            json!({ "RebootOption": "NoReboot" }),
            None,
        ),
        (
            "a Host Management update naming nothing its table requires",
            "AWSQuickSetupType-SSMHostMgmt",
            json!({ "UpdateSSMAgent": "false" }),
            None,
        ),
    ];

    for (case, type_, entries, refused_for) in rows {
        // One manager per row, so no row observes another row's writes.
        let router = emulator();
        let stored_before = complete_parameters(type_);
        let manager_arn = create_ok(&router, case, type_, &stored_before).await;
        let (id, before) = stored_definition(&router, case, &manager_arn).await;
        assert_eq!(
            before, stored_before,
            "case {case:?}: the baseline create must store the complete parameter set, \
             got {before:?}"
        );

        let (status, answer) = update_parameters(&router, &manager_arn, &id, &entries).await;
        let (_, after) = stored_definition(&router, case, &manager_arn).await;
        let pairs = deployed_pairs(&router, case, &manager_arn).await;
        match refused_for {
            Some(key) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: UpdateConfigurationDefinition must refuse {entries:?}, \
                     got {status} / {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {answer:?}"
                );
                assert_eq!(
                    error_message(&answer),
                    must_be_defined(key),
                    "case {case:?}: expected the refusal to name {key:?}, got {answer:?}"
                );
                assert_eq!(
                    after, stored_before,
                    "case {case:?}: a refused update must leave the stored parameters \
                     alone, got {after:?}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: UpdateConfigurationDefinition must accept {entries:?}, \
                     got {status} / {answer:?}"
                );
                let want = with_entries(&stored_before, &entries);
                assert_eq!(
                    after, want,
                    "case {case:?}: the stored definition must hold {want:?}, got {after:?}"
                );
            }
        }
        assert_eq!(
            pairs, baseline_pairs,
            "case {case:?}: the definition must stay deployed where its parameters name, \
             expected {baseline_pairs:?}, got {pairs:?}"
        );
    }
}

/// The published catalog states each requirement either outright or under the condition
/// its own documented table gives, and never requires a parameter of a type whose table
/// does not mark it `(Required)`: a conditional entry names the parameter its condition
/// reads, and that parameter is one the type's own table documents.
#[tokio::test]
async fn every_published_requirement_states_the_condition_its_table_gives() {
    // The service answers ONE sentence for a parameter a definition must provide,
    // whichever parameter it is, so the refusal it publishes for the Regions every type
    // requires is that same sentence for that key.
    assert_eq!(
        NO_DEPLOYMENT_REGION_MESSAGE,
        must_be_defined(PARAMETER_TARGET_REGIONS),
        "the published refusal for the Regions must be the sentence the service answers \
         for a parameter a definition must provide"
    );

    for entry in QUICK_SETUP_TYPE_CATALOG {
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        for required in entry.required_parameters {
            assert!(
                seen.insert(required.key),
                "case {:?}: the parameter {:?} is required twice",
                entry.type_,
                required.key
            );
            // Every required key must have a value this suite can send, so a catalog
            // entry cannot be added without a case for it here.
            assert!(
                !documented_value(required.key).is_empty(),
                "case {:?}: {:?} must have a value of its documented form",
                entry.type_,
                required.key
            );
            if let RequiredWhen::ParameterIsOneOf { key, values } = required.when {
                assert!(
                    !values.is_empty(),
                    "case {:?}: the condition on {:?} must name the values it holds for",
                    entry.type_,
                    required.key
                );
                assert!(
                    entry
                        .required_parameters
                        .iter()
                        .all(|other| other.key != key),
                    "case {:?}: the condition on {:?} reads {key:?}, which must not be \
                     required of the same type or no complete set could be built",
                    entry.type_,
                    required.key
                );
            }
        }
    }
}
