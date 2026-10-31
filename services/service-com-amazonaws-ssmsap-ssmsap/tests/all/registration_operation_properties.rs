// SPDX-License-Identifier: CC0-1.0

//! What the operation record a registration files reports as its `Properties`, driven
//! over the wire.
//!
//! RULE: an operation record's `Properties` are the parameters of the call that filed it —
//! every member the request carried beyond the resource the record already names as
//! `ResourceId`/`ResourceArn` — so a client reading the record back learns how the work
//! was asked for; a member the call omitted, and a list it carried empty, name no
//! property, and a `@sensitive` member is named by what identifies the thing it belongs to
//! rather than by its secret.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they do
//! in the emulator binary. Nothing reaches into `crate::state`: what a record holds is
//! only believed when `GetOperation` and `ListOperations` report it back.
//!
//! The table spans what the rule generalises over: the `ApplicationType` carrying the
//! registration, which optional members the call carries (none, some, every one of them),
//! a list member omitted versus carried EMPTY, how many entries a list member carries (one,
//! several, the published `ComponentInfoList` maximum), repeated declarations of one thing,
//! and the form of the Amazon EC2 instance id the registration names.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::COMPONENT_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_DECLARED_COMPONENTS;

use crate::application_lifecycle::{rendered, settle, tag_pairs, Emulator, Reply};

/// The single `CredentialType` the model defines
/// (`service_com_amazonaws_ssmsap_ssmsap::ops::types::CREDENTIAL_TYPE_VALUES`).
const ADMIN: &str = "ADMIN";

/// An Amazon EC2 instance id in the 17-character form, which `InstanceId` is modelled to
/// admit (`^i-[\w\d]{8}$|^i-[\w\d]{17}$`).
const LONG_INSTANCE: &str = "i-0123456789abcdef0";

/// The same member in the 8-character form the same pattern admits, so one row names its
/// instance the other way.
const SHORT_INSTANCE: &str = "i-0abcd123";

/// One `ApplicationCredential` a registration declares.
#[derive(Debug, Clone)]
struct Credential {
    database: &'static str,
    credential_type: &'static str,
    secret: &'static str,
}

/// One `ComponentInfo` a registration declares.
#[derive(Debug, Clone)]
struct Component {
    component_type: String,
    sid: &'static str,
    ec2_instance_id: &'static str,
}

/// One registration, and therefore one record: the members it carries beyond the
/// `ApplicationId`.
///
/// A list member is `None` when the call OMITS it and `Some` when it carries it, so a row
/// can carry one empty — which the rule treats as asking for nothing, exactly as an
/// omitted member does.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    application_type: &'static str,
    instances: Vec<&'static str>,
    sid: Option<&'static str>,
    sap_instance_number: Option<&'static str>,
    credentials: Option<Vec<Credential>>,
    database_arn: Option<&'static str>,
    components: Option<Vec<Component>>,
}

impl Row {
    /// The smallest row: only the modelled `@required` members, which is what a
    /// registration of a type that requires nothing conditionally
    /// (`service_com_amazonaws_ssmsap_ssmsap::state::CONDITIONALLY_REQUIRED_MEMBERS`)
    /// carries.
    fn required_only(case: &'static str, application_type: &'static str) -> Self {
        Self {
            case,
            application_type,
            instances: vec![LONG_INSTANCE],
            sid: None,
            sap_instance_number: None,
            credentials: None,
            database_arn: None,
            components: None,
        }
    }

    /// The `ApplicationId` this row registers under: the case name, which the modelled
    /// `ApplicationId` pattern (`^[\w\d\.-]+$`) admits.
    fn application_id(&self) -> String {
        self.case.to_string()
    }
}

/// One credential of the SAP HANA database `database`.
fn credential(database: &'static str, secret: &'static str) -> Credential {
    Credential {
        database,
        credential_type: ADMIN,
        secret,
    }
}

/// One component of the SAP system `sid`, of the type `component_type`, on `ec2`.
fn component(component_type: &str, sid: &'static str, ec2_instance_id: &'static str) -> Component {
    Component {
        component_type: component_type.to_string(),
        sid,
        ec2_instance_id,
    }
}

/// The registrations the rule is asserted over.
fn rows() -> Vec<Row> {
    vec![
        // A type that requires nothing beyond the modelled `@required` members, so the
        // record reports exactly those two.
        Row::required_only("ABAP-REQUIRED-ONLY", "SAP_ABAP"),
        // The members a HANA application is required to carry as well.
        Row {
            case: "HANA-CONDITIONALLY-REQUIRED",
            application_type: "HANA",
            instances: vec![LONG_INSTANCE],
            sid: Some("HDB"),
            sap_instance_number: Some("00"),
            credentials: Some(vec![credential("HDB", "sap/hdb/admin")]),
            database_arn: None,
            components: None,
        },
        // Every member the operation models at once, with several entries in each list
        // member and the instance named in the other modelled form.
        Row {
            case: "HANA-EVERY-MEMBER",
            application_type: "HANA",
            instances: vec![SHORT_INSTANCE],
            sid: Some("PRD"),
            sap_instance_number: Some("01"),
            credentials: Some(vec![
                credential("SYSTEMDB", "sap/prd/system"),
                credential("PRD", "sap/prd/tenant"),
            ]),
            database_arn: Some(
                "arn:aws:ssm-sap:us-east-1:000000000000:database/PRD-DB/SYSTEMDB",
            ),
            components: Some(vec![
                component("WD", "PRD", LONG_INSTANCE),
                component("ASCS", "PRD", SHORT_INSTANCE),
            ]),
        },
        // The list members carried EMPTY rather than omitted: the call asked for nothing
        // with either, so neither names a property.
        Row {
            case: "ABAP-EMPTY-LISTS",
            application_type: "SAP_ABAP",
            instances: vec![LONG_INSTANCE],
            sid: None,
            sap_instance_number: None,
            credentials: Some(Vec::new()),
            database_arn: None,
            components: Some(Vec::new()),
        },
        // The most components one `ComponentsInfo` may declare
        // (`ComponentInfoList` is `@length(min: 0, max: 5)`), each of a type the model's
        // own `ComponentType` catalogue names.
        Row {
            case: "ABAP-MAX-COMPONENTS",
            application_type: "SAP_ABAP",
            instances: vec![LONG_INSTANCE],
            sid: Some("DEV"),
            sap_instance_number: Some("02"),
            credentials: None,
            database_arn: None,
            components: Some(
                COMPONENT_TYPE_VALUES
                    .iter()
                    .take(MAX_DECLARED_COMPONENTS)
                    .map(|component_type| component(component_type, "DEV", LONG_INSTANCE))
                    .collect(),
            ),
        },
        // Two declarations of ONE credential and two of ONE component: what the record
        // reports is what the call CARRIED, declaration for declaration, whatever the
        // application is left holding.
        Row {
            case: "HANA-REPEATED-DECLARATIONS",
            application_type: "HANA",
            instances: vec![LONG_INSTANCE],
            sid: Some("QAS"),
            sap_instance_number: Some("03"),
            credentials: Some(vec![
                credential("QAS", "sap/qas/first"),
                credential("QAS", "sap/qas/second"),
            ]),
            database_arn: None,
            components: Some(vec![
                component("WD", "QAS", LONG_INSTANCE),
                component("WD", "QAS", SHORT_INSTANCE),
            ]),
        },
    ]
}

/// The `RegisterApplication` body `row` is registered with.
fn registration_body(row: &Row) -> Value {
    let mut body = Map::new();
    body.insert("ApplicationId".into(), json!(row.application_id()));
    body.insert("ApplicationType".into(), json!(row.application_type));
    body.insert("Instances".into(), json!(row.instances));
    if let Some(sid) = row.sid {
        body.insert("Sid".into(), json!(sid));
    }
    if let Some(instance_number) = row.sap_instance_number {
        body.insert("SapInstanceNumber".into(), json!(instance_number));
    }
    if let Some(credentials) = row.credentials.as_ref() {
        body.insert(
            "Credentials".into(),
            Value::Array(
                credentials
                    .iter()
                    .map(|credential| {
                        json!({
                            "DatabaseName": credential.database,
                            "CredentialType": credential.credential_type,
                            "SecretId": credential.secret,
                        })
                    })
                    .collect(),
            ),
        );
    }
    if let Some(database_arn) = row.database_arn {
        body.insert("DatabaseArn".into(), json!(database_arn));
    }
    if let Some(components) = row.components.as_ref() {
        body.insert(
            "ComponentsInfo".into(),
            Value::Array(
                components
                    .iter()
                    .map(|component| {
                        json!({
                            "ComponentType": component.component_type,
                            "Sid": component.sid,
                            "Ec2InstanceId": component.ec2_instance_id,
                        })
                    })
                    .collect(),
            ),
        );
    }
    Value::Object(body)
}

/// The `Properties` the record of `row` must report: one entry per member the call
/// carried, the `ApplicationId` excepted — it is the record's own `ResourceId`.
///
/// A credential is named by the SAP HANA database it reaches and its `CredentialType`,
/// never by its `@sensitive` `SecretId`; a component by the id this service resolves a
/// component by (its System ID and type, the form `ListComponents` reports) and the Amazon
/// EC2 instance it was declared on.
fn expected_properties(row: &Row) -> BTreeMap<String, String> {
    let mut expected = BTreeMap::new();
    expected.insert(
        "ApplicationType".to_string(),
        row.application_type.to_string(),
    );
    expected.insert("Instances".to_string(), row.instances.join(", "));
    if let Some(sid) = row.sid {
        expected.insert("Sid".to_string(), sid.to_string());
    }
    if let Some(instance_number) = row.sap_instance_number {
        expected.insert(
            "SapInstanceNumber".to_string(),
            instance_number.to_string(),
        );
    }
    let credentials = row.credentials.clone().unwrap_or_default();
    if !credentials.is_empty() {
        expected.insert(
            "Credentials".to_string(),
            credentials
                .iter()
                .map(|credential| {
                    format!("{} ({})", credential.database, credential.credential_type)
                })
                .collect::<Vec<String>>()
                .join(", "),
        );
    }
    if let Some(database_arn) = row.database_arn {
        expected.insert("DatabaseArn".to_string(), database_arn.to_string());
    }
    let components = row.components.clone().unwrap_or_default();
    if !components.is_empty() {
        expected.insert(
            "ComponentsInfo".to_string(),
            components
                .iter()
                .map(|component| {
                    format!(
                        "{}-{} ({})",
                        component.sid, component.component_type, component.ec2_instance_id
                    )
                })
                .collect::<Vec<String>>()
                .join(", "),
        );
    }
    expected
}

/// Every `SecretId` `row` declared — none of which may appear in anything a reader of the
/// record answers.
fn secrets(row: &Row) -> Vec<&'static str> {
    row.credentials
        .clone()
        .unwrap_or_default()
        .iter()
        .map(|credential| credential.secret)
        .collect()
}

/// `GetOperation` for `id`.
async fn get_operation(emulator: &Emulator, id: &str) -> Reply {
    emulator
        .send("/get-operation", json!({"OperationId": id}))
        .await
}

/// The `Properties` of the record a `GetOperation` reply reports.
fn properties(reply: &Reply) -> BTreeMap<String, String> {
    tag_pairs(reply.op("Properties"))
}

/// RULE: the record a registration files reports the parameters of that registration.
#[tokio::test]
async fn a_registration_record_reports_the_calls_own_parameters() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every row is registered first, so each record is read while its registration is
    // still INPROGRESS — one application's registration is no other's business, so the
    // calls do not conflict.
    let mut filed: Vec<(Row, String)> = Vec::new();
    for row in rows {
        let reply = emulator
            .send("/register-application", registration_body(&row))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
            row.case
        );
        let operation_id = reply
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert!(
            !operation_id.is_empty(),
            "case {:?}: the registration must report the operation it filed, got {reply:?}",
            row.case
        );
        filed.push((row, operation_id));
    }

    for (row, operation_id) in &filed {
        let running = get_operation(&emulator, operation_id).await;
        assert_eq!(
            running.status, 200,
            "case {:?}: expected 200 from GetOperation, got {running:?}",
            row.case
        );
        assert_eq!(
            running.op_str("Type"),
            "REGISTER_APPLICATION",
            "case {:?}: expected the registration's own operation type, got {:?}",
            row.case,
            running.op("Type")
        );
        assert_eq!(
            running.op_str("ResourceId"),
            row.application_id(),
            "case {:?}: expected the application as ResourceId, got {:?}",
            row.case,
            running.op("ResourceId")
        );
        assert_eq!(
            properties(&running),
            expected_properties(row),
            "case {:?}: expected the call's parameters as Properties, got {:?}",
            row.case,
            running.op("Properties")
        );

        // A `@sensitive` member is not what the record names a credential by.
        let record = rendered(&running);
        for secret in secrets(row) {
            assert!(
                !record.contains(secret),
                "case {:?}: a record must name no SecretId, got {record:?}",
                row.case
            );
        }

        // Reading the record changes nothing a later reader sees.
        let again = get_operation(&emulator, operation_id).await;
        assert_eq!(
            rendered(&again),
            record,
            "case {:?}: a second read must report the same record, got {again:?}",
            row.case
        );
    }

    // The parameters the record was filed with are the ones it keeps once the clock has
    // brought the registration to its terminal status, and the listing reports the same
    // record the reader does.
    settle().await;
    for (row, operation_id) in &filed {
        let done = get_operation(&emulator, operation_id).await;
        assert_eq!(
            done.op_str("Status"),
            "SUCCESS",
            "case {:?}: a due registration has succeeded, got {:?}",
            row.case,
            done.op("Status")
        );
        assert_eq!(
            properties(&done),
            expected_properties(row),
            "case {:?}: a settled record keeps the parameters it was filed with, got {:?}",
            row.case,
            done.op("Properties")
        );

        let listed = emulator
            .send(
                "/list-operations",
                json!({"ApplicationId": row.application_id()}),
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {:?}: expected 200 from ListOperations, got {listed:?}",
            row.case
        );
        let rows_listed = listed
            .member("Operations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let listed_record = rows_listed
            .iter()
            .find(|record| record.get("Id").and_then(Value::as_str) == Some(operation_id.as_str()))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            listed_record,
            done.member("Operation").cloned().unwrap_or(Value::Null),
            "case {:?}: the listed row must be the record the reader reports, got {rows_listed:?}",
            row.case
        );
    }
}

/// RULE, read back through the resource the parameters describe: the members the record
/// reports as `Properties` are the ones the registration was made with, so the application
/// the same call created reports them as its own state.
#[tokio::test]
async fn the_reported_parameters_are_the_ones_the_registration_was_made_with() {
    let emulator = Emulator::new();
    let row = Row {
        case: "HANA-ROUND-TRIP",
        application_type: "HANA",
        instances: vec![LONG_INSTANCE],
        sid: Some("HDB"),
        sap_instance_number: Some("00"),
        credentials: Some(vec![credential("HDB", "sap/hdb/admin")]),
        database_arn: None,
        components: Some(vec![component("WD", "HDB", SHORT_INSTANCE)]),
    };

    let reply = emulator
        .send("/register-application", registration_body(&row))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
        row.case
    );
    let operation_id = reply
        .member("OperationId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    settle().await;

    let record = get_operation(&emulator, &operation_id).await;
    let reported = properties(&record);
    assert_eq!(
        reported,
        expected_properties(&row),
        "case {:?}: expected the call's parameters as Properties, got {:?}",
        row.case,
        record.op("Properties")
    );

    let application = emulator
        .send(
            "/get-application",
            json!({"ApplicationId": row.application_id()}),
        )
        .await;
    assert_eq!(
        application.status, 200,
        "case {:?}: expected 200 from GetApplication, got {application:?}",
        row.case
    );
    // The same `ComponentsInfo` declaration the record names is the component the
    // application holds, named by the same id — and the `Sid` the record reports is the
    // System ID the application's own SAP HANA component is identified by.
    let components = application
        .app("Components")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let component_ids: Vec<String> = components
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect();
    for expected in ["HDB-WD", "HDB-HANA"] {
        assert!(
            component_ids.contains(&expected.to_string()),
            "case {:?}: expected {expected} among the application's components, got {component_ids:?}",
            row.case
        );
    }
    assert_eq!(
        reported.get("Sid").map(String::as_str),
        Some("HDB"),
        "case {:?}: the record reports the System ID the registration was made with, got {:?}",
        row.case,
        record.op("Properties")
    );
}
