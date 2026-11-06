// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the status summaries this service reports.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a configuration manager, and each configuration deployed from
//! one of its definitions, reports ONE status summary per value the modelled
//! `StatusType` enum publishes; each summary walks the modelled `Status` phases of its
//! own kind of work in order over the span the clock owes that work, and carries the
//! informational message its current status and status type call for.
//!
//! The rows of each table differ along what the rule generalises over — which operation
//! reports the record (the two Gets and the two listings), which kind of work the
//! summary describes, how many definitions a manager carries and how far each one fans
//! out, and where in the lifecycle the record is read (just submitted, rolling out,
//! settled, submitted again, being torn down).

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::ops::types::{STATUS_TYPE_VALUES, STATUS_VALUES};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    status_message, ASYNC_EXECUTION_PHASES, DEPLOYMENT_PHASES, STATUS_DELETING, STATUS_NONE,
    STATUS_SUCCEEDED, STATUS_TYPE_ASYNC_EXECUTIONS, STATUS_TYPE_DEPLOYMENT, TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for; a manager's own Region, and the one a
/// definition naming no target Region is deployed in.
const REGION: &str = "us-west-2";

/// Long enough that every phase the clock owes a freshly submitted deployment has come
/// due.
fn past_transition() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
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

/// The minimum a create needs: one definition whose `Type` the service publishes and
/// whose `Parameters` provide what a definition of that type must.
///
/// A definition that names neither the accounts nor the organizational units to deploy
/// to is refused, and so is one that names no Regions to deploy into, or one that leaves
/// out a parameter its own type's documented table marks `(Required)`; no case here is
/// about any of those rules, so the organizational unit, the Region and each of that
/// type's own required parameters are added wherever the row's own `parameters` leave
/// them out — which leaves the accounts and the Regions the definition fans out to
/// exactly what those parameters name.
fn definition(type_: &str, parameters: Value) -> Value {
    let mut parameters = parameters.as_object().cloned().unwrap_or_default();
    parameters
        .entry("TargetOrganizationalUnits".to_string())
        .or_insert_with(|| json!("ou-1"));
    parameters
        .entry("TargetRegions".to_string())
        .or_insert_with(|| json!("us-east-1"));
    let parameters = super::type_parameter_fixtures::with_required_parameters(
        type_,
        Value::Object(parameters),
    );
    // Every definition names the administration role a LOCAL deployment must name, since
    // a case here may target accounts and none of them is about that rule
    // (`tests/all/local_deployment_roles.rs`).
    json!({
        "Type": type_,
        "Parameters": parameters,
        "LocalDeploymentAdministrationRoleArn":
            super::local_deployment_roles::administration_role_arn(),
    })
}

/// Create a manager that must succeed, answering its ARN.
async fn create_ok(router: &MultiServiceRouter, case: &str, body: &Value) -> String {
    let (status, answer) = call(router, "POST", "/configurationManager", Some(body)).await;
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

/// Read the manager back, asserting the read succeeded.
async fn get_manager_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfigurationManager must succeed, got {status} / {answer:?}"
    );
    answer
}

/// Read one configuration back, asserting the read succeeded.
async fn get_configuration_ok(router: &MultiServiceRouter, case: &str, id: &str) -> Value {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/getConfiguration/{}", label(id)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: GetConfiguration must succeed, got {status} / {answer:?}"
    );
    answer
}

/// The `ConfigurationManagerSummary` `ListConfigurationManagers` reports for one ARN.
async fn listed_manager(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let (status, answer) = call(router, "POST", "/listConfigurationManagers", Some(&json!({}))).await;
    assert_eq!(
        status, 200,
        "case {case}: ListConfigurationManagers must succeed, got {status} / {answer:?}"
    );
    answer
        .get("ConfigurationManagersList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationManagersList, got {answer:?}"))
        .iter()
        .find(|row| row.get("ManagerArn").and_then(Value::as_str) == Some(manager_arn))
        .unwrap_or_else(|| panic!("case {case}: expected a row for {manager_arn}, got {answer:?}"))
        .clone()
}

/// The `ConfigurationSummary` rows `ListConfigurations` reports for one manager.
async fn listed_configurations(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> Vec<Value> {
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
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .clone();
    assert!(
        !rows.is_empty(),
        "case {case}: a created manager deploys at least one configuration, got {answer:?}"
    );
    rows
}

/// The ids of the configurations one manager deployed.
async fn configuration_ids(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Vec<String> {
    listed_configurations(router, case, manager_arn)
        .await
        .iter()
        .map(|row| {
            row.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a configuration carries its Id, got {row:?}"))
                .to_string()
        })
        .collect()
}

/// Update one definition of a manager, asserting the update succeeded.
async fn update_definition_ok(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
    id: &str,
    body: &Value,
) {
    let (status, answer) = call(
        router,
        "PUT",
        &format!(
            "/configurationDefinition/{}/{}",
            label(manager_arn),
            label(id)
        ),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateConfigurationDefinition must succeed, got {status} / {answer:?}"
    );
}

/// Delete one manager, asserting the delete was accepted.
async fn delete_ok(router: &MultiServiceRouter, case: &str, manager_arn: &str) {
    let (status, answer) = call(
        router,
        "DELETE",
        &format!("/configurationManager/{}", label(manager_arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteConfigurationManager must succeed, got {status} / {answer:?}"
    );
}

/// The `StatusSummaries` of one reported record, keyed by `StatusType`.
///
/// Asserts the rule's shape as it reads them: every summary names a modelled
/// `StatusType` and a modelled `Status`, carries the `@required` `LastUpdatedAt`, and
/// there is exactly ONE summary for each value the `StatusType` enum publishes (the
/// generated `STATUS_TYPE_VALUES` catalog).
fn summaries_by_type(case: &str, record: &Value) -> BTreeMap<String, Value> {
    let list = record
        .get("StatusSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {record:?}"));
    let mut by_type: BTreeMap<String, Value> = BTreeMap::new();
    for summary in list {
        let status_type = summary
            .get("StatusType")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: StatusType is required, got {summary:?}"));
        assert!(
            STATUS_TYPE_VALUES.contains(&status_type),
            "case {case}: StatusType {status_type:?} is not in the modelled catalog \
             {STATUS_TYPE_VALUES:?}"
        );
        let status = summary
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a Status, got {summary:?}"));
        assert!(
            STATUS_VALUES.contains(&status),
            "case {case}: Status {status:?} is not in the modelled catalog {STATUS_VALUES:?}"
        );
        assert!(
            summary.get("LastUpdatedAt").is_some(),
            "case {case}: LastUpdatedAt is required, got {summary:?}"
        );
        assert!(
            by_type
                .insert(status_type.to_string(), summary.clone())
                .is_none(),
            "case {case}: two summaries report the StatusType {status_type:?}, got {list:?}"
        );
    }
    let reported: Vec<&str> = by_type.keys().map(String::as_str).collect();
    let mut modelled: Vec<&str> = STATUS_TYPE_VALUES.to_vec();
    modelled.sort_unstable();
    assert_eq!(
        reported, modelled,
        "case {case}: expected one summary per modelled StatusType {modelled:?}, got {list:?}"
    );
    by_type
}

/// The summary of `status_type` of one reported record.
fn summary_of(case: &str, record: &Value, status_type: &str) -> Value {
    summaries_by_type(case, record)
        .remove(status_type)
        .unwrap_or_else(|| panic!("case {case}: expected a {status_type} summary, got {record:?}"))
}

/// The `Status` of the summary of `status_type` of one reported record.
fn status_of(case: &str, record: &Value, status_type: &str) -> String {
    summary_of(case, record, status_type)
        .get("Status")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a Status, got {record:?}"))
        .to_string()
}

/// Assert every summary of one reported record carries the message its own status and
/// status type call for: the informational text while there is something to say beyond
/// the status, and no `StatusMessage` member at all where there is not.
fn assert_messages(case: &str, record: &Value) {
    for (status_type, summary) in summaries_by_type(case, record) {
        let status = summary
            .get("Status")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case}: expected a Status, got {summary:?}"))
            .to_string();
        let reported = summary
            .get("StatusMessage")
            .and_then(Value::as_str)
            .map(str::to_string);
        assert_eq!(
            reported,
            status_message(&status_type, &status),
            "case {case}: the {status_type} summary in {status} must carry the message its \
             status and type call for, got {summary:?}"
        );
        // The non-deployment summary always has something to report beyond its bare
        // status, and so does a deployment that is still in flight: the member is
        // documented as the informational message relevant to the current status and
        // status type.
        if status_type == STATUS_TYPE_ASYNC_EXECUTIONS || status != STATUS_SUCCEEDED {
            let message = reported.unwrap_or_default();
            assert!(
                !message.trim().is_empty(),
                "case {case}: the {status_type} summary in {status} must carry an \
                 informational message, got {summary:?}"
            );
        }
    }
}

/// Assert the two kinds of work of one record are reported consistently: the
/// asynchronous executions a deployment runs have not run while that deployment is
/// still in flight, and report their outcome once it has rolled out.
fn assert_async_follows_deployment(case: &str, record: &Value) {
    let deployment = status_of(case, record, STATUS_TYPE_DEPLOYMENT);
    let asynchronous = status_of(case, record, STATUS_TYPE_ASYNC_EXECUTIONS);
    let expected = if deployment == STATUS_SUCCEEDED {
        ASYNC_EXECUTION_PHASES[ASYNC_EXECUTION_PHASES.len() - 1]
    } else {
        ASYNC_EXECUTION_PHASES[0]
    };
    assert_eq!(
        asynchronous, expected,
        "case {case}: with the deployment in {deployment}, the asynchronous executions must \
         report {expected}, got {record:?}"
    );
}

/// Every reporting operation of this service answers one summary per modelled
/// `StatusType`, for a manager and for every configuration it deployed, however many
/// definitions that manager carries and however far each one fans out.
#[tokio::test]
async fn every_reported_record_carries_one_summary_per_modelled_status_type() {
    let router = emulator();
    // The rows differ along what the rule generalises over: the number of definitions,
    // the shape of each one's deployment fan-out (none, several Regions, several
    // accounts, both) and the configuration types named, which are entries of the
    // published catalog (`crate::state::QUICK_SETUP_TYPE_CATALOG`).
    let cases: Vec<(&str, Value)> = vec![
        (
            "one definition naming no target Region at all",
            json!({
                "Name": "no-targets",
                "ConfigurationDefinitions": [definition("AWSQuickSetupType-SSM", json!({}))],
            }),
        ),
        (
            "one definition fanning out over three Regions",
            json!({
                "Name": "three-regions",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-PatchPolicy",
                    json!({ "TargetRegions": "us-east-1,us-west-2,eu-west-1" }),
                )],
            }),
        ),
        (
            "one definition fanning out over two accounts and two Regions",
            json!({
                "Name": "four-targets",
                "ConfigurationDefinitions": [definition(
                    "AWSQuickSetupType-SSMHostMgmt",
                    json!({
                        "TargetAccounts": "000000000000,111111111111",
                        "TargetRegions": "us-east-1,eu-central-1",
                    }),
                )],
            }),
        ),
        (
            "three definitions of three types, each targeted differently",
            json!({
                "Name": "three-definitions",
                "ConfigurationDefinitions": [
                    definition("AWSQuickSetupType-DHMC", json!({ "TargetRegions": "us-east-1" })),
                    definition(
                        "AWSQuickSetupType-Scheduler",
                        json!({ "TargetRegions": "us-west-2,ap-south-1" }),
                    ),
                    definition(
                        "AWSQuickSetupType-ResourceExplorer",
                        json!({ "TargetAccounts": "000000000000" }),
                    ),
                ],
            }),
        ),
    ];

    for (case, body) in cases {
        let manager_arn = create_ok(&router, case, &body).await;

        // The manager, as each of the two operations that report it answers it.
        let fetched = get_manager_ok(&router, case, &manager_arn).await;
        summaries_by_type(&format!("{case} / GetConfigurationManager"), &fetched);
        let listed = listed_manager(&router, case, &manager_arn).await;
        summaries_by_type(&format!("{case} / ListConfigurationManagers"), &listed);

        // And every configuration it deployed, as each of the two that report those
        // answers it.
        for row in listed_configurations(&router, case, &manager_arn).await {
            let row_case = format!("{case} / ListConfigurations");
            summaries_by_type(&row_case, &row);
            let id = row
                .get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {row_case}: a configuration carries its Id, got {row:?}"));
            let configuration = get_configuration_ok(&router, case, id).await;
            summaries_by_type(&format!("{case} / GetConfiguration"), &configuration);
        }
    }
}

/// A deployment is reported in every phase the modelled `Status` enum gives it, in
/// order, before it reports its outcome — and the asynchronous executions it runs
/// follow it: nothing has run while it is still in flight.
///
/// The phases are read by polling, so what is asserted is the sequence a caller
/// observes without any further call: each distinct status in the order it appeared,
/// for the manager and for the configuration it deployed.
#[tokio::test]
async fn a_deployment_is_reported_in_every_modelled_phase_before_its_outcome() {
    let case = "phases";
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        case,
        &json!({
            "Name": "walking",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMOpsCenter",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let ids = configuration_ids(&router, case, &manager_arn).await;
    let id = ids[0].clone();

    let mut manager_phases: Vec<String> = Vec::new();
    let mut configuration_phases: Vec<String> = Vec::new();
    // Generous: the whole walk is owed within one transition span, so a deadline of
    // several spans fails loudly rather than hanging if a phase never comes due.
    let deadline = Instant::now() + TRANSITION_SPAN * 20;
    loop {
        let manager = get_manager_ok(&router, case, &manager_arn).await;
        let configuration = get_configuration_ok(&router, case, &id).await;
        for (record_case, record, phases) in [
            ("manager", &manager, &mut manager_phases),
            ("configuration", &configuration, &mut configuration_phases),
        ] {
            let case = format!("{case} / {record_case}");
            assert_async_follows_deployment(&case, record);
            assert_messages(&case, record);
            let status = status_of(&case, record, STATUS_TYPE_DEPLOYMENT);
            if phases.last() != Some(&status) {
                phases.push(status);
            }
        }
        let settled = |phases: &Vec<String>| {
            phases.last().map(String::as_str) == Some(DEPLOYMENT_PHASES[DEPLOYMENT_PHASES.len() - 1])
        };
        if settled(&manager_phases) && settled(&configuration_phases) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the deployment never reported its outcome; manager saw \
             {manager_phases:?}, configuration saw {configuration_phases:?}"
        );
        tokio::time::sleep(TRANSITION_SPAN / 20).await;
    }

    let modelled: Vec<String> = DEPLOYMENT_PHASES.iter().map(|s| s.to_string()).collect();
    assert_eq!(
        manager_phases, modelled,
        "case {case}: the manager's deployment must be reported in every modelled phase in \
         order, got {manager_phases:?}"
    );
    assert_eq!(
        configuration_phases, modelled,
        "case {case}: the configuration's deployment must be reported in every modelled phase \
         in order, got {configuration_phases:?}"
    );
}

/// Each summary carries the informational message its own status and status type call
/// for, at every point of the lifecycle and from every operation that reports it: just
/// submitted, settled, submitted again by an update, and being torn down.
#[tokio::test]
async fn each_summary_carries_the_message_its_status_and_type_call_for() {
    let case = "messages";
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        case,
        &json!({
            "Name": "messaged",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-CFGRecording",
                json!({ "TargetRegions": "us-east-1,us-west-2" }),
            )],
        }),
    )
    .await;
    let ids = configuration_ids(&router, case, &manager_arn).await;
    let definition_id = get_manager_ok(&router, case, &manager_arn).await["ConfigurationDefinitions"]
        [0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a stored definition carries its Id"))
        .to_string();

    // Each point of the lifecycle in turn, with the phase the deployment is in there.
    assert_stage(
        &router,
        case,
        "just submitted",
        &manager_arn,
        &ids,
        DEPLOYMENT_PHASES[0],
    )
    .await;

    tokio::time::sleep(past_transition()).await;
    assert_stage(
        &router,
        case,
        "settled",
        &manager_arn,
        &ids,
        STATUS_SUCCEEDED,
    )
    .await;

    // An update that stores no new value asks for no deployment, so the parameter set
    // this one writes genuinely differs from the stored one; it targets the same
    // accounts and Regions, so the configurations it redeploys are the same records.
    update_definition_ok(
        &router,
        case,
        &manager_arn,
        &definition_id,
        &json!({ "Parameters": {
            "TargetOrganizationalUnits": "ou-1",
            "TargetRegions": "us-east-1,us-west-2",
            "TargetType": "*",
        } }),
    )
    .await;
    assert_stage(
        &router,
        case,
        "submitted again",
        &manager_arn,
        &ids,
        DEPLOYMENT_PHASES[0],
    )
    .await;

    tokio::time::sleep(past_transition()).await;
    delete_ok(&router, case, &manager_arn).await;
    assert_stage(
        &router,
        case,
        "being torn down",
        &manager_arn,
        &ids,
        STATUS_DELETING,
    )
    .await;
}

/// One lifecycle point of [`each_summary_carries_the_message_its_status_and_type_call_for`]:
/// the manager and each of its configurations report `expected_deployment`, from both
/// operations that report them, with every summary's message matching its status and
/// type.
async fn assert_stage(
    router: &MultiServiceRouter,
    case: &str,
    stage: &str,
    manager_arn: &str,
    ids: &[String],
    expected_deployment: &str,
) {
    let case = format!("{case} / {stage}");
    let mut records: Vec<(String, Value)> = vec![
        (
            format!("{case} / GetConfigurationManager"),
            get_manager_ok(router, &case, manager_arn).await,
        ),
        (
            format!("{case} / ListConfigurationManagers"),
            listed_manager(router, &case, manager_arn).await,
        ),
    ];
    for id in ids {
        records.push((
            format!("{case} / GetConfiguration {id}"),
            get_configuration_ok(router, &case, id).await,
        ));
    }
    for row in listed_configurations(router, &case, manager_arn).await {
        records.push((format!("{case} / ListConfigurations"), row));
    }

    for (record_case, record) in &records {
        assert_eq!(
            status_of(record_case, record, STATUS_TYPE_DEPLOYMENT),
            expected_deployment,
            "case {record_case}: expected the deployment to report {expected_deployment}, got \
             {record:?}"
        );
        assert_messages(record_case, record);
    }
}

/// An update submits the definition for deployment again, so EVERY kind of work the
/// record reports starts over from its first modelled phase and walks the rest on the
/// clock — not only the deployment.
#[tokio::test]
async fn submitting_a_deployment_again_restarts_every_kind_of_work() {
    let case = "restart";
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        case,
        &json!({
            "Name": "restarted",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-Distributor",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let ids = configuration_ids(&router, case, &manager_arn).await;
    let definition_id = get_manager_ok(&router, case, &manager_arn).await["ConfigurationDefinitions"]
        [0]["Id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a stored definition carries its Id"))
        .to_string();

    tokio::time::sleep(past_transition()).await;
    let settled = get_manager_ok(&router, case, &manager_arn).await;
    assert_eq!(
        status_of(case, &settled, STATUS_TYPE_ASYNC_EXECUTIONS),
        ASYNC_EXECUTION_PHASES[ASYNC_EXECUTION_PHASES.len() - 1],
        "case {case}: the asynchronous executions must report their outcome once the \
         deployment has rolled out, got {settled:?}"
    );

    // A deployment is submitted again for an update that stores a NEW value, so this
    // parameter set differs from the stored one while naming the same account and
    // Region to deploy to.
    update_definition_ok(
        &router,
        case,
        &manager_arn,
        &definition_id,
        &json!({ "Parameters": {
            "TargetOrganizationalUnits": "ou-1",
            "TargetRegions": "us-east-1",
            "TargetType": "*",
        } }),
    )
    .await;

    for (record_case, record) in [
        (
            "manager",
            get_manager_ok(&router, case, &manager_arn).await,
        ),
        (
            "configuration",
            get_configuration_ok(&router, case, &ids[0]).await,
        ),
    ] {
        let record_case = format!("{case} / {record_case}");
        assert_eq!(
            status_of(&record_case, &record, STATUS_TYPE_DEPLOYMENT),
            DEPLOYMENT_PHASES[0],
            "case {record_case}: the re-submitted deployment must report its first phase, got \
             {record:?}"
        );
        assert_eq!(
            status_of(&record_case, &record, STATUS_TYPE_ASYNC_EXECUTIONS),
            ASYNC_EXECUTION_PHASES[0],
            "case {record_case}: the asynchronous executions must start over with the \
             deployment, got {record:?}"
        );
    }

    tokio::time::sleep(past_transition()).await;
    for (record_case, record) in [
        (
            "manager",
            get_manager_ok(&router, case, &manager_arn).await,
        ),
        (
            "configuration",
            get_configuration_ok(&router, case, &ids[0]).await,
        ),
    ] {
        let record_case = format!("{case} / {record_case}");
        assert_eq!(
            status_of(&record_case, &record, STATUS_TYPE_DEPLOYMENT),
            STATUS_SUCCEEDED,
            "case {record_case}: the re-submitted deployment must report its outcome, got \
             {record:?}"
        );
        assert_eq!(
            status_of(&record_case, &record, STATUS_TYPE_ASYNC_EXECUTIONS),
            ASYNC_EXECUTION_PHASES[ASYNC_EXECUTION_PHASES.len() - 1],
            "case {record_case}: the asynchronous executions must report their outcome too, got \
             {record:?}"
        );
    }
}

/// Accepting a delete ends the work that was in flight: no phase of any other kind of
/// work comes due after it, so the asynchronous executions a teardown interrupted keep
/// the status they had reached while the `Deployment` summary reports the teardown.
#[tokio::test]
async fn accepting_a_delete_ends_the_work_that_was_in_flight() {
    let case = "interrupted";
    let router = emulator();
    let manager_arn = create_ok(
        &router,
        case,
        &json!({
            "Name": "interrupted",
            "ConfigurationDefinitions": [definition(
                "AWSQuickSetupType-SSMChangeMgr",
                json!({ "TargetRegions": "us-east-1" }),
            )],
        }),
    )
    .await;
    let ids = configuration_ids(&router, case, &manager_arn).await;

    // Delete while the create's work is still in flight: half the span in, nothing the
    // clock owes the asynchronous executions has come due yet.
    tokio::time::sleep(TRANSITION_SPAN / 2).await;
    let in_flight = get_manager_ok(&router, case, &manager_arn).await;
    assert_eq!(
        status_of(case, &in_flight, STATUS_TYPE_ASYNC_EXECUTIONS),
        STATUS_NONE,
        "case {case}: the create's asynchronous executions must still be in flight when the \
         delete is accepted, got {in_flight:?}"
    );
    delete_ok(&router, case, &manager_arn).await;

    // Past the instant the interrupted work would have come due (one span after the
    // create, a quarter span before this read) and before the teardown reclaims the
    // records (one span after the delete, a quarter span after it).
    tokio::time::sleep(TRANSITION_SPAN * 3 / 4).await;
    for (record_case, record) in [
        (
            "manager",
            get_manager_ok(&router, case, &manager_arn).await,
        ),
        (
            "configuration",
            get_configuration_ok(&router, case, &ids[0]).await,
        ),
    ] {
        let record_case = format!("{case} / {record_case}");
        assert_eq!(
            status_of(&record_case, &record, STATUS_TYPE_DEPLOYMENT),
            STATUS_DELETING,
            "case {record_case}: an accepted delete must be reported as the teardown, got \
             {record:?}"
        );
        assert_eq!(
            status_of(&record_case, &record, STATUS_TYPE_ASYNC_EXECUTIONS),
            STATUS_NONE,
            "case {record_case}: the asynchronous executions the delete ended must not come due \
             after it, got {record:?}"
        );
        assert_messages(&record_case, &record);
    }
}
