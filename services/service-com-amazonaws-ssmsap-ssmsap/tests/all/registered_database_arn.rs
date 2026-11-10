// SPDX-License-Identifier: CC0-1.0

//! The SAP HANA database a registration names by `DatabaseArn`, driven over the wire.
//!
//! RULE: a database a registration NAMES is one of the registered application's
//! databases, and a registration names one in either of two places — the `DatabaseArn`
//! member and each `ApplicationCredential$DatabaseName` — so the application's database
//! set is the union of both, one entry per distinct database. A database the
//! registration named by ARN is reported under THAT ARN, which is the database's own;
//! one named only by a credential keeps the sub-resource ARN minted from the application
//! and database ids.
//!
//! Every case goes through the real router (the harness
//! `tests/all/application_lifecycle.rs` owns), so the restJson1 routing, the boundary
//! constraint walk, the handler and the response serialization all run exactly as they
//! do in the emulator binary. Nothing reaches into `crate::state`: the registered ARN is
//! only believed when a `ListDatabases` call reports it back.
//!
//! The table spans what the rule generalises over: the ARN's form (a database ARN of
//! another application, of this one, one whose resource is `:`-separated, one with a
//! trailing separator, one whose last segment carries characters the modelled
//! `DatabaseId` does not admit), whether credentials name the same database or another
//! one or none at all, the application type, and whether the registration named a System
//! ID. The issue's `ABAP-DB` registration is ONE row.

use serde_json::{json, Map, Value};
use service_com_amazonaws_ssmsap_ssmsap::ops::types::DATABASE_TYPE_VALUES;
use service_com_amazonaws_ssmsap_ssmsap::state::MAX_DATABASE_ID_LENGTH;

use crate::application_lifecycle::{settle, valid_registration, Emulator, Reply};
use crate::list_databases::{credentials, expected_arn, listed, next_token, Summary};
use crate::validation_message_rendering::{message, VALIDATION_EXCEPTION};

/// Where the ARN a listing reports for a database has to come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArnSource {
    /// The ARN the registration's `DatabaseArn` named: the database's own.
    AsRegistered,
    /// The sub-resource ARN of a database the registration named only by credential.
    Minted,
}

/// One registration carrying a `DatabaseArn`, and the databases `ListDatabases` must
/// report for it.
struct Case {
    /// How an assertion names the row.
    name: &'static str,
    application_id: &'static str,
    /// An `ApplicationType` value the registration carries.
    application_type: &'static str,
    /// The `Sid` the registration carries, or `None` for a registration naming none.
    sid: Option<&'static str>,
    /// The `DatabaseName` of each `ApplicationCredential` the registration carries.
    database_names: &'static [&'static str],
    /// The `DatabaseArn` the registration carries.
    database_arn: String,
    /// The `ComponentId` every reported database must carry, or `None` when it must
    /// carry none.
    expected_component: Option<&'static str>,
    /// The `(DatabaseId, DatabaseType, where its ARN comes from)` of every database the
    /// listing must report, in the order it must report them.
    expected: Vec<(&'static str, &'static str, ArnSource)>,
}

/// The rows.
fn cases() -> Vec<Case> {
    vec![
        // The registration the issue records: a HANA database named by ARN alone.
        Case {
            name: "an ABAP application whose registration named its HANA database by ARN only",
            application_id: "ABAP-DB",
            application_type: "SAP_ABAP",
            sid: None,
            database_names: &[],
            database_arn: expected_arn("HDB-TEST", "HDB"),
            expected_component: None,
            expected: vec![("HDB", "TENANT", ArnSource::AsRegistered)],
        },
        // The ARN names the HANA SYSTEM database, and the registration names a System ID
        // so the database has a component to belong to.
        Case {
            name: "an ARN naming the HANA system database",
            application_id: "abap-systemdb-by-arn",
            application_type: "SAP_ABAP",
            sid: Some("ABC"),
            database_names: &[],
            database_arn: expected_arn("ABC-HDB", "SYSTEMDB"),
            expected_component: Some("ABC-HANA"),
            expected: vec![("SYSTEMDB", "SYSTEM", ArnSource::AsRegistered)],
        },
        // An ARN whose resource part is `:`-separated rather than `/`-separated: the
        // modelled `SsmSapArn` pattern admits both, so the rule cannot key on one form.
        Case {
            name: "an ARN whose resource is colon-separated",
            application_id: "abap-colon-arn",
            application_type: "SAP_ABAP",
            sid: Some("PRD"),
            database_names: &[],
            database_arn: "arn:aws:rds:us-east-1:000000000000:db:sap-prd".to_string(),
            expected_component: Some("PRD-HANA"),
            expected: vec![("SAP_PRD", "TENANT", ArnSource::AsRegistered)],
        },
        // A last segment carrying characters the modelled `DatabaseId` pattern
        // (`[\w\d]+$`) does not admit.
        Case {
            name: "an ARN whose last segment is not of the DatabaseId form",
            application_id: "abap-punctuated-arn",
            application_type: "SAP_ABAP",
            sid: None,
            database_names: &[],
            database_arn: expected_arn("abap-punctuated-arn", "prod db.1"),
            expected_component: None,
            expected: vec![("PROD_DB_1", "TENANT", ArnSource::AsRegistered)],
        },
        // A trailing separator leaves the database named by the last NON-EMPTY segment.
        Case {
            name: "an ARN with a trailing separator",
            application_id: "abap-trailing-separator",
            application_type: "SAP_ABAP",
            sid: None,
            database_names: &[],
            database_arn: format!("{}/", expected_arn("abap-trailing-separator", "TNT9")),
            expected_component: None,
            expected: vec![("TNT9", "TENANT", ArnSource::AsRegistered)],
        },
        // The ARN and a credential name the SAME database: one database, reported under
        // the ARN the registration gave it.
        Case {
            name: "a database named by ARN and by a credential is one database",
            application_id: "hdb-arn-and-credential",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["HDB"],
            database_arn: expected_arn("hdb-source", "HDB"),
            expected_component: Some("HDB-HANA"),
            expected: vec![("HDB", "TENANT", ArnSource::AsRegistered)],
        },
        // The ARN names one database and the credentials another two: the union, in
        // database id order, each under the ARN its own mention gives it.
        Case {
            name: "a database named by ARN beside databases named only by credentials",
            application_id: "hdb-arn-and-others",
            application_type: "HANA",
            sid: Some("HDB"),
            database_names: &["SYSTEMDB", "TNT1"],
            database_arn: expected_arn("hdb-arn-and-others", "AAA"),
            expected_component: Some("HDB-HANA"),
            expected: vec![
                ("AAA", "TENANT", ArnSource::AsRegistered),
                ("SYSTEMDB", "SYSTEM", ArnSource::Minted),
                ("TNT1", "TENANT", ArnSource::Minted),
            ],
        },
    ]
}

/// The `RegisterApplication` body for `case`.
fn registration(case: &Case) -> Value {
    let mut body: Map<String, Value> =
        valid_registration(case.application_id, case.application_type)
            .as_object()
            .cloned()
            .expect("a registration body is a JSON object");
    match case.sid {
        Some(sid) => {
            body.insert("Sid".into(), json!(sid));
        }
        None => {
            body.remove("Sid");
        }
    }
    body.insert("Credentials".into(), credentials(case.database_names));
    body.insert("DatabaseArn".into(), json!(case.database_arn));
    Value::Object(body)
}

/// The `Summary` list `case` must be reported as.
fn expected_summaries(case: &Case) -> Vec<Summary> {
    case.expected
        .iter()
        .map(|(database_id, database_type, arn_source)| Summary {
            application_id: Some(case.application_id.to_string()),
            component_id: case.expected_component.map(str::to_string),
            database_id: Some((*database_id).to_string()),
            database_type: Some((*database_type).to_string()),
            arn: Some(match arn_source {
                ArnSource::AsRegistered => case.database_arn.clone(),
                ArnSource::Minted => expected_arn(case.application_id, database_id),
            }),
            tagged: false,
        })
        .collect()
}

/// Register `body`, asserting the registration itself was accepted, and wait out the
/// clock so the discovery that registration runs has produced the application's
/// components — the components its databases are reported as connected to.
async fn register(emulator: &Emulator, name: &str, body: Value) {
    let reply = emulator.send("/register-application", body).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected 200 from RegisterApplication, got {reply:?}"
    );
    settle().await;
}

/// `ListDatabases` with `body`.
async fn list(emulator: &Emulator, body: Value) -> Reply {
    emulator.send("/list-databases", body).await
}

/// RULE: the databases of an application are the ones its registration named, by
/// `DatabaseArn` as well as by credential — so a registration that named a database only
/// by ARN has that database, reported under the ARN it was registered with.
#[tokio::test]
async fn the_database_a_registration_named_by_arn_is_one_of_the_applications_databases() {
    let emulator = Emulator::new();

    for case in cases() {
        register(&emulator, case.name, registration(&case)).await;

        let reply = list(&emulator, json!({"ApplicationId": case.application_id})).await;
        assert_eq!(
            reply.status, 200,
            "case {:?}: expected 200 from ListDatabases, got {reply:?}",
            case.name
        );
        let reported = listed(&reply);
        let expected = expected_summaries(&case);
        assert_eq!(
            reported, expected,
            "case {:?}: expected {expected:?}, got {reported:?}",
            case.name
        );
        assert!(
            next_token(&reply).is_none(),
            "case {:?}: a listing that reported every database must carry no NextToken, got {:?}",
            case.name,
            next_token(&reply)
        );
        for database in &reported {
            // The modelled `DatabaseId` (pattern `[\w\d]+$`, length 1..300) and
            // `DatabaseType` enum, pinned however the ARN spelled the database.
            assert!(
                !database.id().is_empty()
                    && database.id().chars().count() <= MAX_DATABASE_ID_LENGTH
                    && database
                        .id()
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_'),
                "case {:?}: DatabaseId {:?} is not of the modelled form",
                case.name,
                database.id()
            );
            let database_type = database.database_type.clone().unwrap_or_default();
            assert!(
                DATABASE_TYPE_VALUES.contains(&database_type.as_str()),
                "case {:?}: DatabaseType {database_type:?} is not one of {DATABASE_TYPE_VALUES:?}",
                case.name
            );
        }
    }
}

/// RULE: the database set is the application's own, so the unnarrowed listing reports
/// every database every registration named — and listing is a READ, so a second call
/// reports exactly the same set.
#[tokio::test]
async fn every_registered_database_arn_is_reported_by_the_unnarrowed_listing() {
    let emulator = Emulator::new();

    let cases = cases();
    for case in &cases {
        register(&emulator, case.name, registration(case)).await;
    }

    let reply = list(&emulator, json!({"MaxResults": 50})).await;
    assert_eq!(
        reply.status, 200,
        "expected 200 from the unnarrowed ListDatabases, got {reply:?}"
    );
    let reported = listed(&reply);
    for case in &cases {
        for expected in expected_summaries(case) {
            assert!(
                reported.contains(&expected),
                "case {:?}: expected the unnarrowed listing to report {expected:?}, got \
                 {reported:?}",
                case.name
            );
        }
    }

    let again = listed(&list(&emulator, json!({"MaxResults": 50})).await);
    assert_eq!(
        again, reported,
        "a second listing of unchanged state must report the same databases {reported:?}, got \
         {again:?}"
    );
}

/// RULE: `DatabaseArn` keeps its modelled constraint — a value outside the `SsmSapArn`
/// pattern is refused as `ValidationException` (HTTP 400) and nothing is registered.
#[tokio::test]
async fn a_database_arn_outside_the_modelled_pattern_registers_nothing() {
    let emulator = Emulator::new();

    // Values past the modelled `SsmSapArn` pattern
    // (`^arn:(.+:){2,4}.+$|^arn:(.+:){1,3}.+\/.+$`): no `arn:` prefix at all, too few
    // separated parts, and the empty string.
    for (index, arn) in ["not-an-arn", "arn:aws:ssm-sap", ""].into_iter().enumerate() {
        let application_id = format!("bad-database-arn-{index}");
        let mut body: Map<String, Value> = valid_registration(&application_id, "SAP_ABAP")
            .as_object()
            .cloned()
            .expect("a registration body is a JSON object");
        body.insert("DatabaseArn".into(), json!(arn));

        let reply = emulator
            .send("/register-application", Value::Object(body))
            .await;
        assert_eq!(
            reply.status, 400,
            "case {arn:?}: expected 400 from RegisterApplication, got {reply:?}"
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

        // A refused registration left nothing behind: whichever refusal an unregistered
        // application is read with (another suite's rule), the read reports no
        // application at all.
        let fetched = emulator
            .send("/get-application", json!({"ApplicationId": application_id}))
            .await;
        assert!(
            fetched.status >= 400 && fetched.member("Application").is_none(),
            "case {arn:?}: a refused registration must leave no application, got {fetched:?}"
        );
    }
}
