// SPDX-License-Identifier: CC0-1.0

//! `GetDatabase` over the wire: the ONE SAP HANA database a request names.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` directly: a database is only believed when a `GetDatabase`
//! call reports it back out of what a `RegisterApplication` persisted.
//!
//! The rule under test: the databases there are to read are the ones the registrations of
//! the Region the request was signed for named, `DatabaseArn` or `DatabaseId` names which
//! of them to read while `ApplicationId`/`ComponentId` narrow where that reference is
//! resolved, and the `Database` reported for it carries the ids, name, type and ARN the
//! listing reports for that same record plus what the registration declared about it (the
//! credentials that name it) and what the store knows about the application it belongs to
//! (the status that application is in, the instant its record last changed, and the ARN of
//! the component the database belongs to); a reference no database of this account and
//! Region answers is refused, and nothing the registration did not state is reported.
//!
//! The table spans what that rule generalises over: how the database was named by the
//! registration (by credential, by `DatabaseArn`, by both), how the request names it (by
//! id under its application, by id alone, by ARN, narrowed by component), database count
//! and type, credential count per database, application type, and whether the
//! registration named a System ID or declared the HANA component at all.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    DATABASE_STATUS_VALUES, DATABASE_TYPE_VALUES,
};

use crate::application_lifecycle::{settle, tag_pairs, Emulator, Reply, REGION};
use crate::list_databases::{expected_arn, listed};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// A second regional endpoint, so a database can be asked for at a Region its application
/// was not registered in.
const OTHER_REGION: &str = "us-west-2";

/// The account every emulated ARN carries, from the core's single source of truth.
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// The `DatabaseStatus` a database of an application that is up must report, which is
/// what every row of the table below expects once the clock has settled the registration.
/// One of the model's `DatabaseStatus` values
/// (`smithy_query(query="shape", shape="DatabaseStatus")`).
const RUNNING: &str = "RUNNING";

/// The members `Database` carries only from discovery on the HANA instance: neither may
/// be reported for a database this emulator was merely told about. Source:
/// `smithy_query(query="shape", shape="Database")`.
const UNDISCOVERED_MEMBERS: &[&str] = &["PrimaryHost", "SQLPort"];

/// One SAP HANA database of a registration, and what `GetDatabase` must report for it.
#[derive(Debug)]
struct Expected {
    database_id: &'static str,
    database_name: &'static str,
    /// A `DatabaseType` value; `SYSTEM` for the HANA system database, `TENANT` otherwise.
    database_type: &'static str,
    /// The ARN the database must carry when it is NOT the one the registration's
    /// `DatabaseArn` named: the sub-resource ARN of the (application, database) pair.
    /// `Some` is the ARN the registration gave it, reported as registered.
    arn: Option<&'static str>,
    /// The `ComponentId` the database must report, or `None` when it must report none.
    component_id: Option<&'static str>,
    /// The `(DatabaseName, SecretId)` of every credential the database must report, in
    /// the order the registration listed them; empty when it must report none.
    credentials: &'static [(&'static str, &'static str)],
    /// The `ComponentId` of every component whose ARN the database must report in
    /// `ConnectedComponentArns`; empty when it must report none.
    connected_components: &'static [&'static str],
}

/// One registration and the databases `GetDatabase` must read back out of it.
#[derive(Debug)]
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    sid: Option<&'static str>,
    sap_instance_number: Option<&'static str>,
    /// The `ComponentsInfo` entries, as (`ComponentType`, `Sid`, `Ec2InstanceId`).
    components_info: &'static [(&'static str, &'static str, &'static str)],
    /// The `(DatabaseName, SecretId)` of each `ApplicationCredential` the registration
    /// carries.
    credentials: &'static [(&'static str, &'static str)],
    /// The `DatabaseArn` the registration carries, when it names its database by ARN.
    database_arn: Option<&'static str>,
    expected: &'static [Expected],
}

impl Case {
    /// The `RegisterApplication` body for this row.
    fn request(&self) -> Value {
        let mut body = Map::new();
        body.insert("ApplicationId".into(), json!(self.application_id));
        body.insert("ApplicationType".into(), json!(self.application_type));
        body.insert("Instances".into(), json!(["i-0123456789abcdef0"]));
        if let Some(sid) = self.sid {
            body.insert("Sid".into(), json!(sid));
        }
        if let Some(number) = self.sap_instance_number {
            body.insert("SapInstanceNumber".into(), json!(number));
        }
        if !self.components_info.is_empty() {
            body.insert(
                "ComponentsInfo".into(),
                Value::Array(
                    self.components_info
                        .iter()
                        .map(|(component_type, sid, instance)| {
                            json!({
                                "ComponentType": component_type,
                                "Sid": sid,
                                "Ec2InstanceId": instance,
                            })
                        })
                        .collect(),
                ),
            );
        }
        if !self.credentials.is_empty() {
            body.insert("Credentials".into(), credential_list(self.credentials));
        }
        if let Some(arn) = self.database_arn {
            body.insert("DatabaseArn".into(), json!(arn));
        }
        Value::Object(body)
    }
}

/// The `Credentials` list of a registration carrying each `(DatabaseName, SecretId)`
/// pair.
///
/// `tests/all/list_databases.rs` builds its credentials from the database names alone;
/// the rows here have to say which credential belongs to which database, so the secret id
/// is part of the row. `CredentialType` is the model's single `CredentialType` value.
fn credential_list(credentials: &[(&str, &str)]) -> Value {
    Value::Array(
        credentials
            .iter()
            .map(|(database_name, secret_id)| {
                json!({
                    "DatabaseName": database_name,
                    "CredentialType": "ADMIN",
                    "SecretId": secret_id,
                })
            })
            .collect(),
    )
}

/// The rows: one registration each.
const CASES: &[Case] = &[
    // The smallest HANA registration: one database, named by one credential, belonging to
    // the HANA component the registration declared.
    Case {
        name: "one database named by one credential",
        application_id: "HDB-TEST",
        application_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("00"),
        components_info: &[("HANA", "HDB", "i-0123456789abcdef0")],
        credentials: &[("HDB", "sap/hdb/admin")],
        database_arn: None,
        expected: &[Expected {
            database_id: "HDB",
            database_name: "HDB",
            database_type: "TENANT",
            arn: None,
            component_id: Some("HDB-HANA"),
            credentials: &[("HDB", "sap/hdb/admin")],
            connected_components: &["HDB-HANA"],
        }],
    },
    // The system database beside two tenants: each is read on its own, and the type is
    // the system database's for exactly one of them.
    Case {
        name: "the system database and two tenants",
        application_id: "hdb-multi",
        application_type: "HANA",
        sid: Some("PRD"),
        sap_instance_number: Some("01"),
        components_info: &[("HANA", "PRD", "i-00000000000000001")],
        credentials: &[
            ("TNT2", "sap/prd/tnt2"),
            ("SYSTEMDB", "sap/prd/system"),
            ("HDB", "sap/prd/hdb"),
        ],
        database_arn: None,
        expected: &[
            Expected {
                database_id: "HDB",
                database_name: "HDB",
                database_type: "TENANT",
                arn: None,
                component_id: Some("PRD-HANA"),
                credentials: &[("HDB", "sap/prd/hdb")],
                connected_components: &["PRD-HANA"],
            },
            Expected {
                database_id: "SYSTEMDB",
                database_name: "SYSTEMDB",
                database_type: "SYSTEM",
                arn: None,
                component_id: Some("PRD-HANA"),
                credentials: &[("SYSTEMDB", "sap/prd/system")],
                connected_components: &["PRD-HANA"],
            },
            Expected {
                database_id: "TNT2",
                database_name: "TNT2",
                database_type: "TENANT",
                arn: None,
                component_id: Some("PRD-HANA"),
                credentials: &[("TNT2", "sap/prd/tnt2")],
                connected_components: &["PRD-HANA"],
            },
        ],
    },
    // Two credentials naming ONE database: one database, reporting both credentials that
    // reach it.
    Case {
        name: "two credentials naming one database",
        application_id: "hdb-two-credentials",
        application_type: "HANA",
        sid: Some("TWO"),
        sap_instance_number: Some("02"),
        components_info: &[("HANA", "TWO", "i-00000000000000002")],
        credentials: &[("TNT1", "sap/two/first"), ("TNT1", "sap/two/second")],
        database_arn: None,
        expected: &[Expected {
            database_id: "TNT1",
            database_name: "TNT1",
            database_type: "TENANT",
            arn: None,
            component_id: Some("TWO-HANA"),
            credentials: &[("TNT1", "sap/two/first"), ("TNT1", "sap/two/second")],
            connected_components: &["TWO-HANA"],
        }],
    },
    // An ABAP registration that named its HANA database by ARN alone and declared no
    // System ID: the database is read under the ARN it was registered with, and belongs
    // to no component, so it reports no component and no connected component.
    Case {
        name: "an ABAP application whose database is named by ARN only",
        application_id: "ABAP-DB",
        application_type: "SAP_ABAP",
        sid: None,
        sap_instance_number: None,
        components_info: &[],
        credentials: &[],
        database_arn: Some("arn:aws:ssm-sap:us-east-1:000000000000:database/HDB-SOURCE/HDB"),
        expected: &[Expected {
            database_id: "HDB",
            database_name: "HDB",
            database_type: "TENANT",
            arn: Some("arn:aws:ssm-sap:us-east-1:000000000000:database/HDB-SOURCE/HDB"),
            component_id: None,
            credentials: &[],
            connected_components: &[],
        }],
    },
    // A registration that named a System ID but declared no `ComponentsInfo`: its
    // database belongs to the HANA component those ids identify, and no component of this
    // application answers that id, so there is no connected component ARN to report.
    Case {
        name: "a database whose component the registration never declared",
        application_id: "hdb-no-components",
        application_type: "HANA",
        sid: Some("NOC"),
        sap_instance_number: Some("03"),
        components_info: &[],
        credentials: &[("SYSTEMDB", "sap/noc/system")],
        database_arn: None,
        expected: &[Expected {
            database_id: "SYSTEMDB",
            database_name: "SYSTEMDB",
            database_type: "SYSTEM",
            arn: None,
            component_id: Some("NOC-HANA"),
            credentials: &[("SYSTEMDB", "sap/noc/system")],
            connected_components: &[],
        }],
    },
    // The ARN and a credential naming the SAME database, beside a database named only by
    // a credential: the first keeps the ARN the registration gave it, the second the
    // minted sub-resource ARN, and each reports its own credential.
    Case {
        name: "a database named by ARN and by credential, beside one named by credential",
        application_id: "hdb-arn-and-credential",
        application_type: "HANA",
        sid: Some("MIX"),
        sap_instance_number: Some("04"),
        components_info: &[("HANA", "MIX", "i-00000000000000004")],
        credentials: &[("AAA", "sap/mix/aaa"), ("TNT7", "sap/mix/tnt7")],
        database_arn: Some("arn:aws:rds:us-east-1:000000000000:db:AAA"),
        expected: &[
            Expected {
                database_id: "AAA",
                database_name: "AAA",
                database_type: "TENANT",
                arn: Some("arn:aws:rds:us-east-1:000000000000:db:AAA"),
                component_id: Some("MIX-HANA"),
                credentials: &[("AAA", "sap/mix/aaa")],
                connected_components: &["MIX-HANA"],
            },
            Expected {
                database_id: "TNT7",
                database_name: "TNT7",
                database_type: "TENANT",
                arn: None,
                component_id: Some("MIX-HANA"),
                credentials: &[("TNT7", "sap/mix/tnt7")],
                connected_components: &["MIX-HANA"],
            },
        ],
    },
];

/// The ARN the component `component_id` of `application_id` carries in [`REGION`]: a
/// sub-resource ARN naming both ids, as a database's does.
fn component_arn(application_id: &str, component_id: &str) -> String {
    format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:component/{application_id}/{component_id}")
}

/// The ARN `expected` must be reported under for `case`.
fn expected_database_arn(case: &Case, expected: &Expected) -> String {
    match expected.arn {
        Some(arn) => arn.to_string(),
        None => expected_arn(case.application_id, expected.database_id),
    }
}

/// A member of the response body's `Database`, treating JSON `null` as absent the way an
/// omitted optional member is.
fn database(reply: &Reply, name: &str) -> Option<Value> {
    reply
        .member("Database")
        .and_then(|value| value.get(name))
        .filter(|value| !value.is_null())
        .cloned()
}

/// A string member of the response body's `Database`.
fn database_str(reply: &Reply, name: &str) -> String {
    database(reply, name)
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// The `(DatabaseName, SecretId)` of every credential the reported database carries, in
/// the order it reported them.
fn reported_credentials(reply: &Reply) -> Vec<(String, String)> {
    database(reply, "Credentials")
        .and_then(|value| value.as_array().cloned())
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item.get("DatabaseName")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        item.get("SecretId")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `ConnectedComponentArns` the reported database carries, in the order it reported
/// them.
fn reported_connected_components(reply: &Reply) -> Vec<String> {
    database(reply, "ConnectedComponentArns")
        .and_then(|value| value.as_array().cloned())
        .map(|items| {
            items
                .iter()
                .map(|item| item.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// `GetDatabase` with `body`, at [`REGION`]'s endpoint.
async fn get_database(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("/get-database", body).await
}

/// Register every row, then wait out the clock ONCE so each application is up.
async fn register_all(emulator: &Emulator) {
    for case in CASES {
        let reply = emulator.send("/register-application", case.request()).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
            case.name
        );
    }
    settle().await;
}

/// Assert `reply` reports exactly the database `expected` of `case`.
fn assert_reported(case: &Case, expected: &Expected, how: &str, reply: &Reply) {
    assert_eq!(
        reply.status, 200,
        "case {:?} read {how}: expected 200 from GetDatabase, got {reply:?}",
        case.name
    );
    let arn = expected_database_arn(case, expected);
    for (member, want) in [
        ("ApplicationId", case.application_id.to_string()),
        ("DatabaseId", expected.database_id.to_string()),
        ("DatabaseName", expected.database_name.to_string()),
        ("DatabaseType", expected.database_type.to_string()),
        ("Arn", arn),
        ("Status", RUNNING.to_string()),
    ] {
        let actual = database_str(reply, member);
        assert_eq!(
            actual, want,
            "case {:?} read {how}: expected {member} {want:?}, got {actual:?} in {reply:?}",
            case.name
        );
    }
    assert!(
        DATABASE_TYPE_VALUES.contains(&database_str(reply, "DatabaseType").as_str()),
        "case {:?} read {how}: DatabaseType {:?} is not one of {DATABASE_TYPE_VALUES:?}",
        case.name,
        database_str(reply, "DatabaseType")
    );
    assert!(
        DATABASE_STATUS_VALUES.contains(&database_str(reply, "Status").as_str()),
        "case {:?} read {how}: Status {:?} is not one of {DATABASE_STATUS_VALUES:?}",
        case.name,
        database_str(reply, "Status")
    );
    let component =
        database(reply, "ComponentId").and_then(|value| value.as_str().map(str::to_string));
    assert_eq!(
        component.as_deref(),
        expected.component_id,
        "case {:?} read {how}: expected ComponentId {:?}, got {component:?}",
        case.name,
        expected.component_id
    );
    let credentials = reported_credentials(reply);
    let wanted_credentials: Vec<(String, String)> = expected
        .credentials
        .iter()
        .map(|(name, secret)| ((*name).to_string(), (*secret).to_string()))
        .collect();
    assert_eq!(
        credentials, wanted_credentials,
        "case {:?} read {how}: expected Credentials {wanted_credentials:?}, got {credentials:?}",
        case.name
    );
    let connected = reported_connected_components(reply);
    let wanted_connected: Vec<String> = expected
        .connected_components
        .iter()
        .map(|component_id| component_arn(case.application_id, component_id))
        .collect();
    assert_eq!(
        connected, wanted_connected,
        "case {:?} read {how}: expected ConnectedComponentArns {wanted_connected:?}, got \
         {connected:?}",
        case.name
    );
    // `LastUpdated` is the instant the application record last changed, serialized by
    // restJson1 as epoch seconds.
    let last_updated = database(reply, "LastUpdated").and_then(|value| value.as_f64());
    assert!(
        last_updated.is_some_and(|seconds| seconds > 0.0),
        "case {:?} read {how}: expected a LastUpdated instant, got {last_updated:?}",
        case.name
    );
    for member in UNDISCOVERED_MEMBERS {
        assert!(
            database(reply, member).is_none(),
            "case {:?} read {how}: nothing discovered {member}, got {:?}",
            case.name,
            database(reply, member)
        );
    }
}

/// RULE: the database a reference names is the record the registration persisted, read
/// back whichever of the modelled references names it — its `DatabaseId` under its
/// application, that id narrowed by the component it belongs to, its id alone, or its own
/// ARN.
#[tokio::test]
async fn the_database_a_reference_names_is_read_back_out_of_the_registration() {
    let emulator = Emulator::new();
    register_all(&emulator).await;

    for case in CASES {
        for expected in case.expected {
            let by_id = get_database(
                &emulator,
                json!({
                    "ApplicationId": case.application_id,
                    "DatabaseId": expected.database_id,
                }),
            )
            .await;
            assert_reported(case, expected, "by application and database id", &by_id);

            let by_arn = get_database(
                &emulator,
                json!({"DatabaseArn": expected_database_arn(case, expected)}),
            )
            .await;
            assert_reported(case, expected, "by database ARN", &by_arn);
            assert_eq!(
                by_arn.member("Database"),
                by_id.member("Database"),
                "case {:?}: a database read by ARN must be the database read by id, got \
                 {by_arn:?} and {by_id:?}",
                case.name
            );

            if let Some(component_id) = expected.component_id {
                let narrowed = get_database(
                    &emulator,
                    json!({
                        "ApplicationId": case.application_id,
                        "ComponentId": component_id,
                        "DatabaseId": expected.database_id,
                    }),
                )
                .await;
                assert_reported(case, expected, "narrowed by its component", &narrowed);
            }
        }
    }
}

/// RULE: the database read one by one is the database the listing reports, because both
/// report the same record: the ids, type and ARN agree member for member.
#[tokio::test]
async fn a_database_read_one_by_one_is_the_database_the_listing_reports() {
    let emulator = Emulator::new();
    register_all(&emulator).await;

    for case in CASES {
        let listing = emulator
            .send("/list-databases", json!({"ApplicationId": case.application_id}))
            .await;
        assert_eq!(
            listing.status, 200,
            "case {:?}: expected 200 from ListDatabases, got {listing:?}",
            case.name
        );
        let summaries = listed(&listing);
        assert_eq!(
            summaries.len(),
            case.expected.len(),
            "case {:?}: expected {} databases listed, got {summaries:?}",
            case.name,
            case.expected.len()
        );
        for summary in &summaries {
            let read = get_database(
                &emulator,
                json!({
                    "ApplicationId": case.application_id,
                    "DatabaseId": summary.id(),
                }),
            )
            .await;
            for (member, listed_value) in [
                ("ApplicationId", summary.application_id.clone()),
                ("ComponentId", summary.component_id.clone()),
                ("DatabaseId", summary.database_id.clone()),
                ("DatabaseType", summary.database_type.clone()),
                ("Arn", summary.arn.clone()),
            ] {
                let read_value =
                    database(&read, member).and_then(|value| value.as_str().map(str::to_string));
                assert_eq!(
                    read_value, listed_value,
                    "case {:?}: the listing reported {member} {listed_value:?} for {:?}, the \
                     read reported {read_value:?}",
                    case.name,
                    summary.id()
                );
            }
        }
    }
}

/// RULE: `ApplicationId` only NARROWS where a reference is resolved, so a `DatabaseId`
/// named on its own resolves among every application of the Region — and where more than
/// one application holds a database of that id, the lookup walks the applications in the
/// id order the listing pages them in, so the same call resolves the same database every
/// time.
#[tokio::test]
async fn a_database_id_named_on_its_own_resolves_in_application_id_order() {
    let emulator = Emulator::new();
    register_all(&emulator).await;

    // Each of these ids is held by exactly ONE of the registrations, so the bare id names
    // one database wherever the lookup starts.
    for (database_id, application_id) in [
        ("AAA", "hdb-arn-and-credential"),
        ("TNT7", "hdb-arn-and-credential"),
        ("TNT1", "hdb-two-credentials"),
        ("TNT2", "hdb-multi"),
    ] {
        let reply = get_database(&emulator, json!({"DatabaseId": database_id})).await;
        assert_eq!(
            reply.status, 200,
            "case {database_id:?}: expected 200 from GetDatabase, got {reply:?}"
        );
        let reported = database_str(&reply, "ApplicationId");
        assert_eq!(
            reported, application_id,
            "case {database_id:?}: expected the database of {application_id:?}, got \
             {reported:?} in {reply:?}"
        );
    }

    // Three of the registrations hold a database whose id is `HDB` (`ABAP-DB`, `HDB-TEST`
    // and `hdb-multi`). The applications are walked in id order, so the bare id resolves
    // to the first of them, and a repeated call resolves exactly the same database.
    let ambiguous = get_database(&emulator, json!({"DatabaseId": "HDB"})).await;
    let reported = database_str(&ambiguous, "ApplicationId");
    assert_eq!(
        reported, "ABAP-DB",
        "case \"a database id more than one application holds\": expected the first \
         application in id order, got {reported:?} in {ambiguous:?}"
    );
    let again = get_database(&emulator, json!({"DatabaseId": "HDB"})).await;
    assert_eq!(
        again.member("Database"),
        ambiguous.member("Database"),
        "case \"a repeated ambiguous read\": expected {ambiguous:?}, got {again:?}"
    );

    // Naming the application narrows that same id to THAT application's own database.
    let narrowed = get_database(
        &emulator,
        json!({"ApplicationId": "hdb-multi", "DatabaseId": "HDB"}),
    )
    .await;
    for (member, want) in [
        ("ApplicationId", "hdb-multi".to_string()),
        ("Arn", expected_arn("hdb-multi", "HDB")),
    ] {
        let actual = database_str(&narrowed, member);
        assert_eq!(
            actual, want,
            "case \"an ambiguous id narrowed by its application\": expected {member} \
             {want:?}, got {actual:?} in {narrowed:?}"
        );
    }
}

/// What a step of the status walk below does to the application before reading its
/// database back.
#[derive(Debug, Clone, Copy)]
enum Act {
    /// Read straight away, with the application as the previous step left it.
    Nothing,
    /// Wait out the change the clock owes the application.
    Settle,
    /// Make the asynchronous call this route addresses.
    Call(&'static str),
}

/// RULE: a database runs as part of its application, so the status it reports is the one
/// that follows the status its application is in — including the transient status an
/// asynchronous call gives the application until the clock settles it.
#[tokio::test]
async fn the_status_of_a_database_follows_the_application_it_belongs_to() {
    let emulator = Emulator::new();
    let application_id = "hdb-status";
    let body = Case {
        name: "hdb-status",
        application_id,
        application_type: "HANA",
        sid: Some("STA"),
        sap_instance_number: Some("05"),
        components_info: &[("HANA", "STA", "i-00000000000000005")],
        credentials: &[("SYSTEMDB", "sap/sta/system")],
        database_arn: None,
        expected: &[],
    }
    .request();
    let registered = emulator.send("/register-application", body).await;
    assert_eq!(
        registered.status, 200,
        "expected 200 from RegisterApplication, got {registered:?}"
    );

    // Each step is (what to do to the application, how an assertion names the step, the
    // status the database must report afterwards). A registration, a stop and a start are
    // asynchronous, so the read straight after each call sees the transient status and the
    // read after the clock has settled sees the status the application moved to.
    let reference = json!({"ApplicationId": application_id, "DatabaseId": "SYSTEMDB"});
    let steps: &[(Act, &str, &str)] = &[
        // Registering: the service has not established what the application runs yet.
        (Act::Nothing, "just registered", "UNKNOWN"),
        (Act::Settle, "registered and settled", "RUNNING"),
        (Act::Call("/stop-application"), "stopping", "STOPPING"),
        (Act::Settle, "stopped", "STOPPED"),
        (Act::Call("/start-application"), "starting", "STARTING"),
        (Act::Settle, "started and settled", "RUNNING"),
    ];
    for (act, step, want) in steps {
        match act {
            Act::Nothing => {}
            Act::Settle => settle().await,
            Act::Call(path) => {
                let reply = emulator
                    .send(path, json!({"ApplicationId": application_id}))
                    .await;
                assert_eq!(
                    reply.status, 200,
                    "case {step:?}: expected 200 from {path}, got {reply:?}"
                );
            }
        }
        let reply = get_database(&emulator, reference.clone()).await;
        let status = database_str(&reply, "Status");
        assert_eq!(
            status, *want,
            "case {step:?}: expected Status {want:?}, got {status:?} in {reply:?}"
        );
        assert!(
            DATABASE_STATUS_VALUES.contains(&status.as_str()),
            "case {step:?}: Status {status:?} is not one of {DATABASE_STATUS_VALUES:?}"
        );
    }
}

/// RULE: a reference no database of this account and Region answers is refused with the
/// modelled `ValidationException` (HTTP 400) and reports no database at all — this
/// operation models no not-found error, so the reference itself is what is refused.
#[tokio::test]
async fn a_reference_no_database_answers_is_refused() {
    let emulator = Emulator::new();
    register_all(&emulator).await;

    // (how an assertion names the row, the request, a fragment the refusal must name).
    let rows: Vec<(&str, Value, String)> = vec![
        (
            "no identifier at all",
            json!({}),
            "databaseId".to_string(),
        ),
        (
            "an application but no database",
            json!({"ApplicationId": "HDB-TEST"}),
            "databaseId".to_string(),
        ),
        (
            "a database id no registration named",
            json!({"ApplicationId": "HDB-TEST", "DatabaseId": "NOSUCHDB"}),
            "NOSUCHDB".to_string(),
        ),
        (
            "a database id of another application",
            json!({"ApplicationId": "HDB-TEST", "DatabaseId": "TNT7"}),
            "TNT7".to_string(),
        ),
        (
            "an application nothing registered",
            json!({"ApplicationId": "never-registered", "DatabaseId": "HDB"}),
            "never-registered".to_string(),
        ),
        (
            "a component the database does not belong to",
            json!({
                "ApplicationId": "HDB-TEST",
                "ComponentId": "PRD-HANA",
                "DatabaseId": "HDB",
            }),
            "HDB".to_string(),
        ),
        (
            "an ARN no registration named",
            json!({"DatabaseArn": expected_arn("HDB-TEST", "GHOST")}),
            expected_arn("HDB-TEST", "GHOST"),
        ),
        (
            "an application ARN rather than a database ARN",
            json!({"DatabaseArn": format!("arn:aws:ssm-sap:{REGION}:{ACCOUNT}:application/HDB-TEST")}),
            "HDB-TEST".to_string(),
        ),
    ];

    for (row, request, fragment) in rows {
        let reply = get_database(&emulator, request).await;
        assert_eq!(
            reply.status, 400,
            "case {row:?}: expected 400 from GetDatabase, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {row:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        assert!(
            reply.member("Database").is_none(),
            "case {row:?}: a refusal must report no database, got {reply:?}"
        );
        let stated = message(&reply);
        assert!(
            stated.contains(&fragment),
            "case {row:?}: expected the refusal to name {fragment:?}, got {stated:?}"
        );
    }
}

/// RULE: a `DatabaseArn` outside the modelled `SsmSapArn` pattern is refused at the
/// boundary, before any lookup, and the refusal names the member.
#[tokio::test]
async fn a_database_arn_outside_the_modelled_pattern_is_refused() {
    let emulator = Emulator::new();

    // Values past the modelled `SsmSapArn` pattern
    // (`^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$`): no `arn:` prefix at all, too few
    // separated parts, and the empty string.
    for arn in ["not-an-arn", "arn:aws:ssm-sap", ""] {
        let reply = get_database(&emulator, json!({"DatabaseArn": arn})).await;
        assert_eq!(
            reply.status, 400,
            "case {arn:?}: expected 400 from GetDatabase, got {reply:?}"
        );
        assert_eq!(
            reply.code(),
            VALIDATION_EXCEPTION,
            "case {arn:?}: expected {VALIDATION_EXCEPTION}, got {reply:?}"
        );
        let stated = message(&reply);
        assert!(
            stated.contains("databaseArn"),
            "case {arn:?}: expected the refusal to name the member, got {stated:?}"
        );
    }
}

/// RULE: the lookup runs in the Region the request was signed for, so a database
/// registered at one regional endpoint does not resolve through another.
#[tokio::test]
async fn a_database_of_one_region_does_not_resolve_through_another() {
    let emulator = Emulator::new();
    register_all(&emulator).await;

    for request in [
        json!({"ApplicationId": "HDB-TEST", "DatabaseId": "HDB"}),
        json!({"DatabaseArn": expected_arn("HDB-TEST", "HDB")}),
    ] {
        let elsewhere = emulator
            .send_in_region(OTHER_REGION, "/get-database", request.clone())
            .await;
        assert_eq!(
            elsewhere.status, 400,
            "case {request:?}: expected 400 at the {OTHER_REGION} endpoint, got {elsewhere:?}"
        );
        assert!(
            elsewhere.member("Database").is_none(),
            "case {request:?}: expected no database at the {OTHER_REGION} endpoint, got \
             {elsewhere:?}"
        );

        // The same reference still resolves at the endpoint it was registered at, so what
        // the other Region refused is the scoping and not the reference.
        let here = get_database(&emulator, request.clone()).await;
        assert_eq!(
            here.status, 200,
            "case {request:?}: expected 200 at the {REGION} endpoint, got {here:?}"
        );
    }
}

/// RULE: the tags reported for a database are the ones held for that database's OWN ARN,
/// so a database nobody has tagged reports none and its application's tags are not
/// reported in their place; and reading a database changes nothing a later caller sees.
#[tokio::test]
async fn a_database_reports_its_own_tags_and_reading_it_changes_nothing() {
    let emulator = Emulator::new();
    let application_id = "hdb-tagged-application";
    let mut body = Case {
        name: "hdb-tagged-application",
        application_id,
        application_type: "HANA",
        sid: Some("TAG"),
        sap_instance_number: Some("06"),
        components_info: &[("HANA", "TAG", "i-00000000000000006")],
        credentials: &[("HDB", "sap/tag/hdb"), ("SYSTEMDB", "sap/tag/system")],
        database_arn: None,
        expected: &[],
    }
    .request();
    body.as_object_mut()
        .expect("a registration body is a JSON object")
        .insert("Tags".into(), json!({"env": "dev"}));
    let registered = emulator.send("/register-application", body).await;
    assert_eq!(
        registered.status, 200,
        "expected 200 from RegisterApplication, got {registered:?}"
    );
    settle().await;

    // The registration's tags are the APPLICATION's, held for the application's ARN.
    let application = emulator
        .send("/get-application", json!({"ApplicationId": application_id}))
        .await;
    let application_tags = tag_pairs(application.member("Tags"));
    let wanted: BTreeMap<String, String> = [("env", "dev")]
        .into_iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect();
    assert_eq!(
        application_tags, wanted,
        "case \"a tagged application\": expected {wanted:?}, got {application_tags:?}"
    );

    // Neither database of that application is tagged, so neither reports a tag set, and
    // the application's own tags are not reported for them.
    for database_id in ["HDB", "SYSTEMDB"] {
        let reference = json!({"ApplicationId": application_id, "DatabaseId": database_id});
        let read = get_database(&emulator, reference.clone()).await;
        assert_eq!(
            read.status, 200,
            "case {database_id:?}: expected 200 from GetDatabase, got {read:?}"
        );
        assert!(
            read.member("Tags").is_none(),
            "case {database_id:?}: nothing tagged the database, expected no Tags, got {:?}",
            tag_pairs(read.member("Tags"))
        );

        // Reading is a READ: the second call reports exactly what the first did.
        let again = get_database(&emulator, reference).await;
        assert_eq!(
            again.member("Database"),
            read.member("Database"),
            "case {database_id:?}: a repeated read must report {read:?}, got {again:?}"
        );
    }
}
