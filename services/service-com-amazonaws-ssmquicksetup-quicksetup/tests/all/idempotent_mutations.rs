// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for what this service's `@idempotent` mutations leave behind
//! when they store nothing new.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — so what is
//! asserted is what a signed AWS SDK request observes.
//!
//! The rule under test: a mutation writes the values its request carries, and what it
//! leaves behind is decided by the values it STORED — so a request whose members all
//! carry what the record already holds (a replay of one already served, or a body
//! carrying none of the optional members) changes nothing a later read can see: no
//! `LastModifiedAt` moves, no deployment is submitted again, and no status summary of
//! the record or of anything deployed from it rewinds.
//!
//! The rows of each table differ along what the rule generalises over — WHICH members
//! the request carries, the form their values take (a type version spelled with and
//! without its minor), how large the record's parameter set is, how far the definition
//! fans out, and whether the request stores a new value at all (the rows that DO are
//! the control: for them the stamp moves and the deployment restarts, and the replay
//! that follows them still changes nothing).

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_ssmquicksetup_quicksetup::state::{
    quick_setup_type_entry, STATUS_INITIALIZING, STATUS_SUCCEEDED, STATUS_TYPE_DEPLOYMENT,
    TRANSITION_SPAN,
};
use smithy_server_runtime::MultiServiceRouter;

/// The region every request below signs for.
const REGION: &str = "us-west-2";

/// The Quick Setup type every definition below deploys. Its published version history
/// is what the `TypeVersion` rows are resolved against.
const BASELINE_TYPE: &str = "AWSQuickSetupType-SSMHostMgmt";

/// The version every definition below is created at: a version
/// [`BASELINE_TYPE`] publishes which is NOT its newest, so the `LATEST` alias names a
/// different version and is a row that stores a new value.
const BASELINE_VERSION: &str = "1.0";

/// The organizational unit every definition below deploys to — a parameter set naming
/// neither accounts nor organizational units is refused, and no case here is about that
/// rule.
const TARGET_OU: &str = "ou-1";

/// The execution role name every definition below is created with.
const BASELINE_EXECUTION_ROLE: &str = "QuickSetupExecution";

/// The administration role ARN every definition below is created with.
const BASELINE_ADMINISTRATION_ROLE: &str = "arn:aws:iam::000000000000:role/QuickSetupAdministration";

/// Long enough that every phase the clock owes a freshly submitted deployment has come
/// due, so a restart of that deployment is observable against a settled record.
fn past_transition() -> Duration {
    TRANSITION_SPAN + Duration::from_millis(400)
}

/// Long enough that a stamp a call moves is observably later than the one before it.
fn tick() -> Duration {
    Duration::from_millis(20)
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

/// The ids of the configurations one manager has deployed, in a stable order.
async fn configuration_ids(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
) -> Vec<String> {
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
    let mut ids: Vec<String> = answer
        .get("ConfigurationsList")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected a ConfigurationsList, got {answer:?}"))
        .iter()
        .map(|row| {
            row.get("Id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {case}: a configuration carries its Id, got {row:?}"))
                .to_string()
        })
        .collect();
    ids.sort();
    assert!(
        !ids.is_empty(),
        "case {case}: a created manager deploys at least one configuration"
    );
    ids
}

/// Everything the service reports about one manager and about each configuration
/// deployed from its definitions: the stored definitions, both timestamps and every
/// status summary, read through the operations a caller has.
///
/// Comparing two of these is how a case asserts that a call changed NOTHING a later
/// read can see — not only the members the case happens to name.
async fn reported_state(router: &MultiServiceRouter, case: &str, manager_arn: &str) -> Value {
    let manager = get_manager_ok(router, case, manager_arn).await;
    let mut configurations: Vec<Value> = Vec::new();
    for id in configuration_ids(router, case, manager_arn).await {
        let (status, answer) = call(
            router,
            "GET",
            &format!("/getConfiguration/{}", label(&id)),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: GetConfiguration must succeed, got {status} / {answer:?}"
        );
        configurations.push(answer);
    }
    json!({ "manager": manager, "configurations": configurations })
}

/// The `Status` the `Deployment` summary of one reported record carries.
fn deployment_status(case: &str, record: &Value) -> String {
    record
        .get("StatusSummaries")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected StatusSummaries, got {record:?}"))
        .iter()
        .find(|summary| {
            summary.get("StatusType").and_then(Value::as_str) == Some(STATUS_TYPE_DEPLOYMENT)
        })
        .and_then(|summary| summary.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: expected a {STATUS_TYPE_DEPLOYMENT} summary, got {record:?}"))
        .to_string()
}

/// A modelled timestamp member as seconds since the epoch, so two stamps can be
/// ordered. The member carries `@timestampFormat("date-time")`, so only the RFC 3339
/// string is on the wire; `tests/all/date_time_timestamps.rs` pins that form in its own
/// right.
fn epoch_seconds(case: &str, value: &Value, member: &str) -> f64 {
    let stamp = value
        .get(member)
        .unwrap_or_else(|| panic!("case {case}: expected {member}, got {value:?}"));
    match stamp {
        Value::String(text) => {
            aws_smithy_types::DateTime::from_str(text, aws_smithy_types::date_time::Format::DateTime)
                .unwrap_or_else(|e| panic!("case {case}: {member} {text:?} is not a date-time: {e}"))
                .as_secs_f64()
        }
        other => panic!("case {case}: {member} must be an RFC 3339 date-time string, got {other:?}"),
    }
}

/// Every record of a reported state, each with the case label that names it.
fn records(case: &str, state: &Value) -> Vec<(String, Value)> {
    let mut out = vec![(
        format!("{case} / manager"),
        state["manager"].clone(),
    )];
    for (index, configuration) in state["configurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected the configurations, got {state:?}"))
        .iter()
        .enumerate()
    {
        out.push((
            format!("{case} / configuration {index}"),
            configuration.clone(),
        ));
    }
    out
}

/// The newest version [`BASELINE_TYPE`] publishes — what the modelled `LATEST` alias
/// names. Source: the catalog the service publishes through `ListQuickSetupTypes`.
fn latest_version() -> &'static str {
    quick_setup_type_entry(BASELINE_TYPE)
        .unwrap_or_else(|| panic!("{BASELINE_TYPE:?} must be a published Quick Setup type"))
        .latest_version()
}

/// One starting point a row is exercised against: how large the definition's parameter
/// set is and how far it fans out.
struct Baseline {
    /// What the case label says about this starting point.
    name: &'static str,
    /// The `TargetRegions` value, which is also the fan-out: one configuration per
    /// targeted Region.
    regions: &'static str,
    /// The parameters the definition carries BESIDE its deployment target and Regions.
    extra: Value,
}

/// The starting points every row below is exercised against: a definition carrying
/// nothing but its deployment target deployed into one Region, and one carrying a
/// larger parameter set deployed into three.
fn baselines() -> Vec<Baseline> {
    vec![
        Baseline {
            name: "one target, no further parameters",
            regions: "us-east-1",
            extra: json!({}),
        },
        Baseline {
            name: "three targets, a larger parameter set",
            regions: "us-east-1,us-west-2,eu-west-1",
            extra: json!({ "TargetType": "*", "UpdateSSMAgent": "true" }),
        },
    ]
}

impl Baseline {
    /// The parameter set this starting point's definition is created with — the set an
    /// update replaying it must carry to store nothing new.
    fn parameters(&self) -> Value {
        let mut parameters = self.extra.as_object().cloned().unwrap_or_default();
        parameters.insert("TargetOrganizationalUnits".to_string(), json!(TARGET_OU));
        parameters.insert("TargetRegions".to_string(), json!(self.regions));
        Value::Object(parameters)
    }

    /// The create this starting point begins with: ONE definition, carrying every
    /// optional member, so an update naming one is observably leaving a STORED value
    /// alone rather than leaving an absent one absent.
    fn create_body(&self) -> Value {
        json!({
            "Name": "idempotent-mutations",
            "Description": "the record every row below starts from",
            "ConfigurationDefinitions": [{
                "Type": BASELINE_TYPE,
                "TypeVersion": BASELINE_VERSION,
                "Parameters": self.parameters(),
                "LocalDeploymentExecutionRoleName": BASELINE_EXECUTION_ROLE,
                "LocalDeploymentAdministrationRoleArn": BASELINE_ADMINISTRATION_ROLE,
            }],
        })
    }
}

/// One update under test.
struct Row {
    /// What the case is about.
    name: &'static str,
    /// The body the update sends.
    update: Value,
    /// Whether that body stores a value the stored record does not already hold.
    stores_a_new_value: bool,
}

/// The updates each starting point is exercised with.
fn definition_rows(baseline: &Baseline) -> Vec<Row> {
    vec![
        Row {
            // Only the two `@httpLabel` members are `@required`, so a body carrying
            // none of the four optional ones is a valid request that asks for nothing.
            name: "an empty body",
            update: json!({}),
            stores_a_new_value: false,
        },
        Row {
            name: "the stored parameter set again",
            update: json!({ "Parameters": baseline.parameters() }),
            stores_a_new_value: false,
        },
        Row {
            name: "the stored type version again",
            update: json!({ "TypeVersion": BASELINE_VERSION }),
            stores_a_new_value: false,
        },
        Row {
            // `^\d{1,3}(\.\d{1,3})?$|^LATEST$` admits a major with no minor, which
            // numbers the same version as `<major>.0`: a different request text naming
            // the version already stored.
            name: "the stored type version spelled without its minor",
            update: json!({ "TypeVersion": "1" }),
            stores_a_new_value: false,
        },
        Row {
            name: "the stored execution role name again",
            update: json!({ "LocalDeploymentExecutionRoleName": BASELINE_EXECUTION_ROLE }),
            stores_a_new_value: false,
        },
        Row {
            name: "the stored administration role ARN again",
            update: json!({
                "LocalDeploymentAdministrationRoleArn": BASELINE_ADMINISTRATION_ROLE,
            }),
            stores_a_new_value: false,
        },
        Row {
            name: "every member carrying the value already stored",
            update: json!({
                "TypeVersion": BASELINE_VERSION,
                "Parameters": baseline.parameters(),
                "LocalDeploymentExecutionRoleName": BASELINE_EXECUTION_ROLE,
                "LocalDeploymentAdministrationRoleArn": BASELINE_ADMINISTRATION_ROLE,
            }),
            stores_a_new_value: false,
        },
        Row {
            name: "a parameter the definition did not carry",
            update: json!({ "Parameters": {
                "TargetOrganizationalUnits": TARGET_OU,
                "TargetRegions": baseline.regions,
                "TargetTagKey": "Env",
            } }),
            stores_a_new_value: true,
        },
        Row {
            name: "another published type version",
            update: json!({ "TypeVersion": "1.1" }),
            stores_a_new_value: true,
        },
        Row {
            // The alias names this type's newest published version, which is not the
            // one the baseline stored.
            name: "the LATEST alias",
            update: json!({ "TypeVersion": "LATEST" }),
            stores_a_new_value: latest_version() != BASELINE_VERSION,
        },
        Row {
            name: "another execution role name",
            update: json!({ "LocalDeploymentExecutionRoleName": "OtherExecutionRole" }),
            stores_a_new_value: true,
        },
        Row {
            name: "another administration role ARN",
            update: json!({
                "LocalDeploymentAdministrationRoleArn":
                    "arn:aws:iam::123456789012:role/OtherAdministration",
            }),
            stores_a_new_value: true,
        },
    ]
}

/// The definition id of the one definition a baseline's manager holds.
fn only_definition_id(case: &str, manager: &Value) -> String {
    let definitions = manager
        .get("ConfigurationDefinitions")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: expected ConfigurationDefinitions, got {manager:?}"));
    assert_eq!(
        definitions.len(),
        1,
        "case {case}: expected one stored definition, got {definitions:?}"
    );
    definitions[0]
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: a stored definition carries its Id, got {manager:?}"))
        .to_string()
}

/// `UpdateConfigurationDefinition` that must succeed.
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

/// An update of a configuration definition changes what a later read reports exactly
/// when it stored a new value: one that stored none leaves every reported value — both
/// stamps and every status summary, of the manager and of every configuration deployed
/// from the definition — exactly as it was, and one that stored a value submits the
/// definition for deployment again. Replaying either, as a client retrying a timed-out
/// request does, changes nothing further.
#[tokio::test]
async fn an_update_that_stores_no_new_value_changes_nothing_a_read_reports() {
    for baseline in baselines() {
        for row in definition_rows(&baseline) {
            let case = format!("{} / {}", baseline.name, row.name);
            // One manager per row, so no row observes another row's writes.
            let router = emulator();
            let manager_arn = create_ok(&router, &case, &baseline.create_body()).await;

            // From a SETTLED record, so a deployment submitted again is observable as a
            // status that rewound rather than as one that never moved on.
            tokio::time::sleep(past_transition()).await;
            let before = reported_state(&router, &case, &manager_arn).await;
            let definition_id = only_definition_id(&case, &before["manager"]);
            for (record_case, record) in records(&case, &before) {
                assert_eq!(
                    deployment_status(&record_case, &record),
                    STATUS_SUCCEEDED,
                    "case {record_case}: the deployment must have settled before the \
                     update, got {record:?}"
                );
            }

            tokio::time::sleep(tick()).await;
            update_definition_ok(&router, &case, &manager_arn, &definition_id, &row.update).await;
            let after = reported_state(&router, &case, &manager_arn).await;

            if row.stores_a_new_value {
                for (record_case, record) in records(&case, &after) {
                    assert_eq!(
                        deployment_status(&record_case, &record),
                        STATUS_INITIALIZING,
                        "case {record_case}: an update that stored a new value submits the \
                         definition for deployment again, got {record:?}"
                    );
                }
                let moved = epoch_seconds(&case, &after["manager"], "LastModifiedAt");
                let stood = epoch_seconds(&case, &before["manager"], "LastModifiedAt");
                assert!(
                    moved > stood,
                    "case {case}: an update that stored a new value must move \
                     LastModifiedAt past {stood}, got {moved}"
                );
            } else {
                assert_eq!(
                    after, before,
                    "case {case}: an update that stored no new value must leave every \
                     reported value as it was, expected {before:?}, got {after:?}"
                );
            }

            // The retry the `@idempotent` trait exists for: the very same request again,
            // from the state the first one left once it had settled.
            tokio::time::sleep(past_transition()).await;
            let settled = reported_state(&router, &case, &manager_arn).await;
            tokio::time::sleep(tick()).await;
            update_definition_ok(&router, &case, &manager_arn, &definition_id, &row.update).await;
            let replayed = reported_state(&router, &case, &manager_arn).await;
            assert_eq!(
                replayed, settled,
                "case {case}: a replay of the same update must change nothing a read \
                 reports, expected {settled:?}, got {replayed:?}"
            );
        }
    }
}

/// `UpdateConfigurationManager` that must succeed.
async fn update_manager_ok(
    router: &MultiServiceRouter,
    case: &str,
    manager_arn: &str,
    body: &Value,
) {
    let (status, answer) = call(
        router,
        "PUT",
        &format!("/configurationManager/{}", label(manager_arn)),
        Some(body),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: UpdateConfigurationManager must succeed, got {status} / {answer:?}"
    );
}

/// The same rule for the other `@idempotent` mutation of a stored manager: an update
/// naming nothing, or naming the values the manager already carries, leaves every
/// reported value as it was; one that stores a value moves the stamp and nothing else —
/// renaming a manager deploys nothing, so no status summary moves for it.
#[tokio::test]
async fn a_manager_update_that_stores_no_new_value_changes_nothing_a_read_reports() {
    let rows: Vec<Row> = vec![
        Row {
            name: "an empty body",
            update: json!({}),
            stores_a_new_value: false,
        },
        Row {
            name: "the stored name again",
            update: json!({ "Name": "idempotent-mutations" }),
            stores_a_new_value: false,
        },
        Row {
            name: "the stored description again",
            update: json!({ "Description": "the record every row below starts from" }),
            stores_a_new_value: false,
        },
        Row {
            name: "both members carrying the values already stored",
            update: json!({
                "Name": "idempotent-mutations",
                "Description": "the record every row below starts from",
            }),
            stores_a_new_value: false,
        },
        Row {
            name: "another name",
            update: json!({ "Name": "renamed" }),
            stores_a_new_value: true,
        },
        Row {
            name: "another description",
            update: json!({ "Description": "rewritten" }),
            stores_a_new_value: true,
        },
    ];

    for baseline in baselines() {
        for row in &rows {
            let case = format!("{} / {}", baseline.name, row.name);
            let router = emulator();
            let manager_arn = create_ok(&router, &case, &baseline.create_body()).await;

            tokio::time::sleep(past_transition()).await;
            let before = reported_state(&router, &case, &manager_arn).await;

            tokio::time::sleep(tick()).await;
            update_manager_ok(&router, &case, &manager_arn, &row.update).await;
            let after = reported_state(&router, &case, &manager_arn).await;

            if row.stores_a_new_value {
                let moved = epoch_seconds(&case, &after["manager"], "LastModifiedAt");
                let stood = epoch_seconds(&case, &before["manager"], "LastModifiedAt");
                assert!(
                    moved > stood,
                    "case {case}: an update that stored a new value must move \
                     LastModifiedAt past {stood}, got {moved}"
                );
                for (record_case, record) in records(&case, &after) {
                    assert_eq!(
                        deployment_status(&record_case, &record),
                        STATUS_SUCCEEDED,
                        "case {record_case}: renaming a manager deploys nothing, so the \
                         settled deployment must stay settled, got {record:?}"
                    );
                }
                assert_eq!(
                    after["configurations"], before["configurations"],
                    "case {case}: renaming a manager must leave the configurations \
                     deployed from its definitions as they were, expected {:?}, got {:?}",
                    before["configurations"], after["configurations"]
                );
            } else {
                assert_eq!(
                    after, before,
                    "case {case}: an update that stored no new value must leave every \
                     reported value as it was, expected {before:?}, got {after:?}"
                );
            }

            tokio::time::sleep(tick()).await;
            update_manager_ok(&router, &case, &manager_arn, &row.update).await;
            let replayed = reported_state(&router, &case, &manager_arn).await;
            assert_eq!(
                replayed, after,
                "case {case}: a replay of the same update must change nothing a read \
                 reports, expected {after:?}, got {replayed:?}"
            );
        }
    }
}
