// SPDX-License-Identifier: CC0-1.0

//! What `StopApplication$StopConnectedEntity` ACTS on: the database management system
//! the stopped application is connected to.
//!
//! Every case goes through the real router (the harness `tests/all/application_lifecycle.rs`
//! owns), so the restJson1 routing, the boundary constraint walk, the handler and the
//! response serialization all run exactly as they do in the emulator binary. Nothing
//! reaches into `crate::state` directly: the DBMS is only believed to have stopped when a
//! `GetDatabase` call reports it out of what the stop persisted.
//!
//! The rule under test: a stop reaches beyond the application itself only to what the
//! call NAMES, so the connected DBMS is stopped exactly by a stop that names it as the
//! connected entity to stop — every database of that application then reports the
//! stopping DBMS's status — while a stop that names none leaves the DBMS as it was, and a
//! start of the application brings it back up because that call carries no switch to
//! leave it down.
//!
//! The tables span what the rule generalises over: how many databases a registration
//! gives rise to and how it named them (by credential, by `DatabaseArn`, by both),
//! database type, application type, every value the model's `ConnectedEntityType` enum
//! admits, whether the OTHER switch of the same call is set, and a DBMS stopped twice
//! over.

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::{
    CONNECTED_ENTITY_TYPE_VALUES, DATABASE_STATUS_VALUES,
};

use crate::application_lifecycle::{settle, Emulator, Reply};

/// A `DatabaseStatus` of a DBMS that is up, and the one every freshly registered
/// application's databases report once the clock has settled the registration. One of the
/// model's `DatabaseStatus` values
/// (`smithy_query(query="shape", shape="DatabaseStatus")`).
const RUNNING: &str = "RUNNING";

/// The `DatabaseStatus` of a DBMS that is being stopped, and of one being started.
const STOPPING: &str = "STOPPING";
const STARTING: &str = "STARTING";

/// The `DatabaseStatus` of a DBMS that has stopped.
const STOPPED: &str = "STOPPED";

/// The `DatabaseStatus` of a database of an application whose registration has not
/// settled: nothing is established about what it runs yet.
const UNKNOWN: &str = "UNKNOWN";

/// One registration under test, and the databases it gives rise to.
#[derive(Debug)]
struct Registration {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    sid: Option<&'static str>,
    sap_instance_number: Option<&'static str>,
    /// The `DatabaseName` of each `ApplicationCredential` the registration carries.
    credentials: &'static [&'static str],
    /// The `DatabaseArn` the registration carries, when it names a database by ARN.
    database_arn: Option<&'static str>,
    /// The `DatabaseId` of every database the registration gives rise to.
    databases: &'static [&'static str],
}

impl Registration {
    /// The `RegisterApplication` body for this row: the modelled `@required` members plus
    /// the ones this `ApplicationType` requires conditionally.
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
        if !self.credentials.is_empty() {
            body.insert(
                "Credentials".into(),
                Value::Array(
                    self.credentials
                        .iter()
                        .map(|database_name| {
                            json!({
                                "DatabaseName": database_name,
                                "CredentialType": "ADMIN",
                                "SecretId": format!("sap/{database_name}/admin"),
                            })
                        })
                        .collect(),
                ),
            );
        }
        if let Some(arn) = self.database_arn {
            body.insert("DatabaseArn".into(), json!(arn));
        }
        Value::Object(body)
    }
}

/// The rows: one registration each, spanning database count, how the databases were
/// named, database type and application type.
const REGISTRATIONS: &[Registration] = &[
    // The smallest HANA registration: one tenant database, named by one credential.
    Registration {
        name: "one database named by one credential",
        application_id: "HDB-TEST",
        application_type: "HANA",
        sid: Some("HDB"),
        sap_instance_number: Some("00"),
        credentials: &["SYSTEMDB"],
        database_arn: None,
        databases: &["SYSTEMDB"],
    },
    // A whole HANA system: the system database and two tenants, every one of them a
    // database of the ONE DBMS the stop names.
    Registration {
        name: "the system database and two tenants",
        application_id: "hdb-multi",
        application_type: "HANA",
        sid: Some("PRD"),
        sap_instance_number: Some("01"),
        credentials: &["TNT2", "SYSTEMDB", "HDB"],
        database_arn: None,
        databases: &["HDB", "SYSTEMDB", "TNT2"],
    },
    // An ABAP registration whose connected HANA database is named by ARN alone: it has
    // no credential and no component, and it is still the DBMS the stop reaches.
    Registration {
        name: "an ABAP application whose database is named by ARN only",
        application_id: "ABAP-DB",
        application_type: "SAP_ABAP",
        sid: None,
        sap_instance_number: None,
        credentials: &[],
        database_arn: Some("arn:aws:ssm-sap:us-east-1:000000000000:database/HDB-SOURCE/HDB"),
        databases: &["HDB"],
    },
    // A HANA registration that names one database by ARN and another by credential: both
    // belong to the same connected DBMS.
    Registration {
        name: "one database by ARN beside one by credential",
        application_id: "hdb-arn-and-credential",
        application_type: "HANA",
        sid: Some("MIX"),
        sap_instance_number: Some("02"),
        credentials: &["TNT7"],
        database_arn: Some(
            "arn:aws:ssm-sap:us-east-1:000000000000:database/hdb-arn-and-credential/SYSTEMDB",
        ),
        databases: &["SYSTEMDB", "TNT7"],
    },
];

/// Register `registration` and wait out the clock, so the application is ACTIVATED and
/// nothing is outstanding on it.
async fn register_settled(emulator: &Emulator, registration: &Registration) {
    let reply = emulator
        .send("/register-application", registration.request())
        .await;
    assert_eq!(
        reply.status, 200,
        "case {:?}: expected 200 from RegisterApplication, got {reply:?}",
        registration.name
    );
    settle().await;
}

/// `POST /stop-application` for `application_id`, naming `connected_entity` and
/// `include_ec2_instance_shutdown` when the row asks for them.
async fn stop(
    emulator: &Emulator,
    application_id: &str,
    connected_entity: Option<&str>,
    include_ec2_instance_shutdown: Option<bool>,
) -> Reply {
    let mut body = Map::new();
    body.insert("ApplicationId".into(), json!(application_id));
    if let Some(entity) = connected_entity {
        body.insert("StopConnectedEntity".into(), json!(entity));
    }
    if let Some(shutdown) = include_ec2_instance_shutdown {
        body.insert("IncludeEc2InstanceShutdown".into(), json!(shutdown));
    }
    emulator.send("/stop-application", Value::Object(body)).await
}

/// The `Status` `GetDatabase` reports for one database of `application_id`.
async fn database_status(emulator: &Emulator, application_id: &str, database_id: &str) -> String {
    let reply = emulator
        .send(
            "/get-database",
            json!({"ApplicationId": application_id, "DatabaseId": database_id}),
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {database_id:?}: expected 200 from GetDatabase, got {reply:?}"
    );
    let status = reply
        .member("Database")
        .and_then(|database| database.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        DATABASE_STATUS_VALUES.contains(&status.as_str()),
        "case {database_id:?}: Status {status:?} is not one of {DATABASE_STATUS_VALUES:?} \
         in {reply:?}"
    );
    status
}

/// Assert every database of `registration` reports `want`.
async fn assert_every_database(
    emulator: &Emulator,
    registration: &Registration,
    step: &str,
    want: &str,
) {
    for database_id in registration.databases {
        let status = database_status(emulator, registration.application_id, database_id).await;
        assert_eq!(
            status, want,
            "case {:?} step {step:?}: expected database {database_id} Status {want:?}, got \
             {status:?}",
            registration.name
        );
    }
}

/// The `Status` `GetApplication` reports for `application_id`.
async fn application_status(emulator: &Emulator, application_id: &str) -> String {
    let reply = emulator
        .send("/get-application", json!({"ApplicationId": application_id}))
        .await;
    assert_eq!(
        reply.status, 200,
        "case {application_id:?}: expected 200 from GetApplication, got {reply:?}"
    );
    reply
        .member("Application")
        .and_then(|application| application.get("Status"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// RULE: a stop stops the connected DBMS exactly when the call names it as the connected
/// entity to stop; a stop that names none leaves the DBMS as it was, and starting the
/// application brings it back up.
#[tokio::test]
async fn a_stop_stops_the_connected_dbms_only_when_it_names_it() {
    for registration in REGISTRATIONS {
        // Every value the model's `ConnectedEntityType` admits names the DBMS, so the
        // row is driven for each of them rather than for one spelling.
        for entity in CONNECTED_ENTITY_TYPE_VALUES {
            let emulator = Emulator::new();
            let id = registration.application_id;
            register_settled(&emulator, registration).await;
            assert_every_database(&emulator, registration, "registered and settled", RUNNING)
                .await;

            // A stop that names NO connected entity stops the application alone: the
            // DBMS its databases live in keeps running, while it runs and once it has
            // stopped.
            let plain = stop(&emulator, id, None, None).await;
            assert_eq!(
                plain.status, 200,
                "case {:?}: expected 200 from a stop naming no connected entity, got \
                 {plain:?}",
                registration.name
            );
            assert_every_database(&emulator, registration, "the application is stopping", RUNNING)
                .await;
            settle().await;
            let stopped = application_status(&emulator, id).await;
            assert_eq!(
                stopped, "STOPPED",
                "case {:?}: expected the application to be STOPPED, got {stopped:?}",
                registration.name
            );
            assert_every_database(&emulator, registration, "the application has stopped", RUNNING)
                .await;

            // A stop that NAMES the connected entity stops that DBMS: every database of
            // the application is in the transition at once and has stopped one span
            // later.
            let named = stop(&emulator, id, Some(entity), None).await;
            assert_eq!(
                named.status, 200,
                "case {:?}: expected 200 from a stop naming {entity}, got {named:?}",
                registration.name
            );
            assert_every_database(
                &emulator,
                registration,
                "the named DBMS is stopping",
                STOPPING,
            )
            .await;
            settle().await;
            assert_every_database(&emulator, registration, "the named DBMS has stopped", STOPPED)
                .await;

            // A read reports the same thing twice: reading a stopped DBMS changes
            // nothing a later caller sees.
            assert_every_database(&emulator, registration, "the stopped DBMS re-read", STOPPED)
                .await;

            // Starting the application brings the DBMS back up: the call carries no
            // switch to leave it down.
            let started = emulator
                .send("/start-application", json!({"ApplicationId": id}))
                .await;
            assert_eq!(
                started.status, 200,
                "case {:?}: expected 200 from StartApplication, got {started:?}",
                registration.name
            );
            assert_every_database(&emulator, registration, "the DBMS is starting", STARTING).await;
            settle().await;
            assert_every_database(&emulator, registration, "the DBMS has started", RUNNING).await;
        }
    }
}

/// One stop of the switch table: the two optional members the call may carry, and the
/// `DatabaseStatus` the connected DBMS must have settled at afterwards.
#[derive(Debug)]
struct SwitchCase {
    connected_entity: Option<&'static str>,
    include_ec2_instance_shutdown: Option<bool>,
    settled_database_status: &'static str,
}

/// RULE: of the two switches a stop carries, the connected-entity one alone decides
/// whether the connected DBMS stops — the Amazon EC2 switch reaches the instances and
/// nothing else — and the application's own status walk is the same whatever either of
/// them says.
#[tokio::test]
async fn the_connected_entity_switch_alone_decides_whether_the_dbms_stops() {
    let cases: &[SwitchCase] = &[
        SwitchCase {
            connected_entity: None,
            include_ec2_instance_shutdown: None,
            settled_database_status: RUNNING,
        },
        SwitchCase {
            connected_entity: None,
            include_ec2_instance_shutdown: Some(false),
            settled_database_status: RUNNING,
        },
        SwitchCase {
            connected_entity: None,
            include_ec2_instance_shutdown: Some(true),
            settled_database_status: RUNNING,
        },
        SwitchCase {
            connected_entity: Some("DBMS"),
            include_ec2_instance_shutdown: None,
            settled_database_status: STOPPED,
        },
        SwitchCase {
            connected_entity: Some("DBMS"),
            include_ec2_instance_shutdown: Some(false),
            settled_database_status: STOPPED,
        },
        SwitchCase {
            connected_entity: Some("DBMS"),
            include_ec2_instance_shutdown: Some(true),
            settled_database_status: STOPPED,
        },
    ];

    // The one registration every row stops: one HANA system with its system database.
    let registration = &REGISTRATIONS[0];
    for case in cases {
        let emulator = Emulator::new();
        let id = registration.application_id;
        register_settled(&emulator, registration).await;

        let reply = stop(
            &emulator,
            id,
            case.connected_entity,
            case.include_ec2_instance_shutdown,
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case:?}: expected 200 from StopApplication, got {reply:?}"
        );
        // The application's own status is the stop's, whatever the switches say.
        let during = application_status(&emulator, id).await;
        assert_eq!(
            during, "STOPPING",
            "case {case:?}: expected the application to be STOPPING, got {during:?}"
        );
        // The record the call files still reports the switches it carried.
        let operation_id = reply
            .member("OperationId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let record = emulator
            .send("/get-operation", json!({"OperationId": operation_id}))
            .await;
        for (member, expected) in [
            (
                "StopConnectedEntity",
                case.connected_entity.map(str::to_string),
            ),
            (
                "IncludeEc2InstanceShutdown",
                case.include_ec2_instance_shutdown
                    .map(|flag| flag.to_string()),
            ),
        ] {
            let reported = record
                .member("Operation")
                .and_then(|operation| operation.get("Properties"))
                .and_then(|properties| properties.get(member))
                .and_then(Value::as_str)
                .map(str::to_string);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected Properties {member} {expected:?}, got {reported:?} \
                 in {record:?}"
            );
        }

        settle().await;
        let settled = application_status(&emulator, id).await;
        assert_eq!(
            settled, "STOPPED",
            "case {case:?}: expected the application to be STOPPED, got {settled:?}"
        );
        assert_every_database(
            &emulator,
            registration,
            "the stop has settled",
            case.settled_database_status,
        )
        .await;
    }
}

/// RULE: stopping a DBMS that is already stopped stops it again rather than refusing or
/// reporting something else: the transition runs once more and settles at the same
/// status, and nothing a reporting call answers is established while the registration
/// itself is still under way.
#[tokio::test]
async fn a_dbms_may_be_stopped_again_and_reports_nothing_before_its_registration_settles() {
    let registration = &REGISTRATIONS[1];
    let emulator = Emulator::new();
    let id = registration.application_id;

    // Before the registration settles the service has established nothing about what the
    // application runs, so no database status follows from it — whatever the DBMS is.
    let reply = emulator
        .send("/register-application", registration.request())
        .await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from RegisterApplication, got {reply:?}"
    );
    assert_every_database(&emulator, registration, "the registration is running", UNKNOWN).await;
    settle().await;

    // Two stops in a row, each naming the DBMS: the second is accepted on a DBMS that has
    // already stopped and leaves it stopped.
    for round in ["the first stop", "the second stop"] {
        let stopped = stop(&emulator, id, Some("DBMS"), None).await;
        assert_eq!(
            stopped.status, 200,
            "case {round:?}: expected 200 from StopApplication, got {stopped:?}"
        );
        assert_every_database(&emulator, registration, round, STOPPING).await;
        settle().await;
        assert_every_database(&emulator, registration, round, STOPPED).await;
    }
}
