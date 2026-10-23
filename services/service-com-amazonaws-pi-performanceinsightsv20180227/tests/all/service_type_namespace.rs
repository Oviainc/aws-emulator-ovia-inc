// SPDX-License-Identifier: CC0-1.0

//! The `(ServiceType, Identifier)` NAMESPACE cross-check, end to end over the wire.
//!
//! RULE under test: Performance Insights serves a data source under the
//! `ServiceType` of the Amazon Web Services service that OWNS it, so a request whose
//! `ServiceType` is not the owner of the data source its `Identifier` names addresses
//! no data source and is refused with the modelled `InvalidArgumentException` (HTTP
//! 400) — never served, and never as an empty but valid answer. Ownership is read
//! from the record the minting service left in the shared data-source directory,
//! because the id's FORM cannot carry it: RDS and DocumentDB both mint `db-<TOKEN>`
//! for an instance and `cluster-<TOKEN>` for a cluster. An identifier the directory
//! holds no record of stays undecided and is served exactly as before.
//!
//! The rows below span what the rule generalises over: the owning service (both
//! values of the `ServiceType` enum), the kind of resource (instance and cluster
//! resource ids), the engine the resource runs (which the refusal names), and the
//! operations that name such a pair. A one-service emulator has no operation that
//! mints a DB resource, so each row registers its data source in the shared
//! directory the way `rds`/`docdb` `CreateDBInstance` / `CreateDBCluster` do; the
//! cross-service wiring itself is covered by
//! `core/router/tests/all/pi_data_source_ownership.rs`.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator_with_engine, Answer};

use service_com_amazonaws_pi_performanceinsightsv20180227::state;

/// 2026-01-01T00:00:00Z. `awsJson1_1` carries an `ISOTimestamp` as epoch seconds.
const T0: f64 = 1_767_225_600.0;

/// A pattern-valid `AnalysisReportId` no Create ever minted.
const UNMINTED_REPORT_ID: &str = "report-0123456789abcdef0";

/// The two values of the modelled `ServiceType` enum.
const SERVICE_TYPES: &[&str] = &["RDS", "DOCDB"];

/// One registered data source: the resource id a database service minted, the
/// monitoring `ServiceType` that owns it, and the engine it runs.
struct DataSource {
    case: &'static str,
    resource_id: &'static str,
    owner: &'static str,
    engine: &'static str,
}

/// The rows vary the owning service, the resource kind (a `db-` instance id and a
/// `cluster-` cluster id under BOTH owners, which is what makes the point that the
/// form decides nothing), the token length and the engine.
const REGISTERED: &[DataSource] = &[
    DataSource {
        case: "an RDS MySQL DB instance",
        resource_id: "db-MYSQL000000000000000001",
        owner: "RDS",
        engine: "mysql",
    },
    DataSource {
        case: "an RDS PostgreSQL DB instance",
        resource_id: "db-POSTGRES0000000000000002",
        owner: "RDS",
        engine: "postgres",
    },
    DataSource {
        case: "an RDS Aurora cluster",
        resource_id: "cluster-AURORAMYSQL1",
        owner: "RDS",
        engine: "aurora-mysql",
    },
    DataSource {
        case: "a DocumentDB cluster",
        resource_id: "cluster-DOCDB0000001",
        owner: "DOCDB",
        engine: "docdb",
    },
    DataSource {
        case: "a DocumentDB instance",
        resource_id: "db-DOCDBINSTANCE0001",
        owner: "DOCDB",
        engine: "docdb",
    },
];

/// Identifiers no database service minted here, so the directory holds no record of
/// them: the forms the authorization gate already serves.
const UNREGISTERED: &[(&str, &str)] = &[
    (
        "the DbiResourceId the model documents",
        "db-ABCDEFGHIJKLMNOPQRSTU1VW2X",
    ),
    ("a cluster resource id", "cluster-ABCDEFGHIJKL"),
];

/// Seed the shared data-source directory the way a database control plane does when
/// it mints a resource id.
fn register(engine: &engine::InMemoryEngine, source: &DataSource) {
    state::data_source_directory(engine)
        .expect("the shared data-source directory is reachable from the engine")
        .lock()
        .register(source.resource_id, source.owner, source.engine);
}

/// The requests that name a `(ServiceType, Identifier)` pair and are IMPLEMENTED, so
/// the cross-check is the only reason any of them could be refused with an argument
/// error.
fn requests(service_type: &str, identifier: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "GetResourceMetadata",
            json!({"ServiceType": service_type, "Identifier": identifier}),
        ),
        (
            "ListPerformanceAnalysisReports",
            json!({"ServiceType": service_type, "Identifier": identifier}),
        ),
        (
            "GetResourceMetrics",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
                "PeriodInSeconds": 300,
            }),
        ),
        (
            "CreatePerformanceAnalysisReport",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
            }),
        ),
        (
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "AnalysisReportId": UNMINTED_REPORT_ID,
            }),
        ),
        (
            "DeletePerformanceAnalysisReport",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "AnalysisReportId": UNMINTED_REPORT_ID,
            }),
        ),
    ]
}

/// Whether this answer serves the request rather than refusing it.
fn served(answer: &Answer) -> bool {
    answer.status == 200
}

/// RULE: a data source queried under a `ServiceType` that does not own it is refused
/// with `InvalidArgumentException` (HTTP 400) on every operation that names the pair,
/// and the refusal names the engine the addressed resource runs.
#[tokio::test]
async fn a_data_source_queried_under_the_wrong_service_type_is_an_argument_error() {
    let (emulator, engine) = emulator_with_engine();
    for source in REGISTERED {
        register(&engine, source);
    }

    for source in REGISTERED {
        for service_type in SERVICE_TYPES.iter().filter(|value| **value != source.owner) {
            for (operation, input) in requests(service_type, source.resource_id) {
                let case = format!(
                    "{} ({}) under {service_type} / {operation}",
                    source.case, source.resource_id
                );
                let answer = call(&emulator, operation, input).await;

                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected 400, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "InvalidArgumentException",
                    "case {case}: expected InvalidArgumentException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.error_message(),
                    state::wrong_service_type_message(source.engine),
                    "case {case}: expected the refusal to name the engine {:?}, got {:?}",
                    source.engine,
                    answer.body
                );
                // A refused Create mints nothing, and a refused read answers no
                // resource.
                assert!(
                    answer.member("AnalysisReportId").is_null()
                        && answer.member("Identifier").is_null(),
                    "case {case}: a refusal carries no resource, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE: the SAME data source under the `ServiceType` that owns it is served — the
/// cross-check refuses a mismatch and nothing else.
#[tokio::test]
async fn a_data_source_queried_under_its_owner_is_served() {
    let (emulator, engine) = emulator_with_engine();
    for source in REGISTERED {
        register(&engine, source);
    }

    for source in REGISTERED {
        for (operation, input) in requests(source.owner, source.resource_id) {
            let case = format!(
                "{} ({}) under its owner {} / {operation}",
                source.case, source.resource_id, source.owner
            );
            let answer = call(&emulator, operation, input).await;

            assert_ne!(
                answer.error_code(),
                "InvalidArgumentException",
                "case {case}: the owner's own ServiceType is never an argument error, got {} {:?}",
                answer.status,
                answer.body
            );
            // The two report-by-id rows name a report no Create minted, which is the
            // service's uniform authorization refusal; every other request is served.
            let expects_report = operation == "GetPerformanceAnalysisReport"
                || operation == "DeletePerformanceAnalysisReport";
            if expects_report {
                assert_eq!(
                    answer.error_code(),
                    "NotAuthorizedException",
                    "case {case}: an unminted report id is the uniform refusal, got {} {:?}",
                    answer.status,
                    answer.body
                );
            } else {
                assert!(
                    served(&answer),
                    "case {case}: expected 200, got {} {:?}",
                    answer.status,
                    answer.body
                );
            }
        }
    }
}

/// RULE: an identifier no database service recorded here is UNDECIDED, not refused —
/// the directory is a record of what this emulator minted, so a missing record may
/// not become a wrong answer. Such a pair is served under either `ServiceType`, as
/// the authorization gate alone decides.
#[tokio::test]
async fn an_unrecorded_identifier_is_served_under_either_service_type() {
    let (emulator, engine) = emulator_with_engine();
    // One row IS registered, so the suite proves the two kinds of identifier coexist
    // in one directory rather than that the directory is simply empty.
    register(&engine, &REGISTERED[0]);

    for (form, identifier) in UNREGISTERED {
        for service_type in SERVICE_TYPES {
            for (operation, input) in requests(service_type, identifier) {
                let case = format!("{form} ({identifier}) under {service_type} / {operation}");
                let answer = call(&emulator, operation, input).await;
                assert_ne!(
                    answer.error_code(),
                    "InvalidArgumentException",
                    "case {case}: an unrecorded identifier is not an argument error, got {} {:?}",
                    answer.status,
                    answer.body
                );
            }
        }
    }
}

/// RULE: `GetResourceMetadata` answers the metadata of the data source it was asked
/// about — the `Identifier` echoed back, and the features the emulator observes on it,
/// each status projected from the monitoring configuration the owning service recorded
/// (none here, so every feature is `UNKNOWN`) — and is subject to the same two gates as
/// its peers. The projection itself is `resource_features.rs`.
#[tokio::test]
async fn get_resource_metadata_reports_the_addressed_data_source() {
    let (emulator, engine) = emulator_with_engine();
    for source in REGISTERED {
        register(&engine, source);
    }

    for source in REGISTERED {
        let case = format!("{} ({})", source.case, source.resource_id);
        let answer = call(
            &emulator,
            "GetResourceMetadata",
            json!({"ServiceType": source.owner, "Identifier": source.resource_id}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.member("Identifier").as_str(),
            Some(source.resource_id),
            "case {case}: expected the addressed identifier echoed, got {:?}",
            answer.body
        );
        let features = answer.member("Features").as_object().cloned().unwrap_or_default();
        assert_eq!(
            features.len(),
            state::REPORTED_FEATURES.len(),
            "case {case}: expected one entry per reported feature, got {:?}",
            answer.body
        );
        for feature in state::REPORTED_FEATURES {
            assert!(
                features.contains_key(feature),
                "case {case}: expected the feature {feature} reported, got {:?}",
                answer.body
            );
        }
    }

    // The authorization gate still decides a value that addresses no data source at
    // all, under the one message every refusal in this service carries.
    let answer = call(
        &emulator,
        "GetResourceMetadata",
        json!({"ServiceType": "RDS", "Identifier": "my-production-database"}),
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_code().as_str()),
        (400, "NotAuthorizedException"),
        "case a DB instance name instead of a resource id: expected the uniform refusal, got \
         {} {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        answer.error_message(),
        state::NOT_AUTHORIZED_MESSAGE,
        "case a DB instance name instead of a resource id: expected the uniform message, got {:?}",
        answer.body
    );
}

/// RULE: the cross-check refuses a mismatch BEFORE anything is stored, and leaves the
/// owner's own view of the data source untouched — a report created under the owning
/// `ServiceType` still round-trips through Get and List.
#[tokio::test]
async fn a_refused_mismatch_changes_nothing_the_owner_sees() {
    let (emulator, engine) = emulator_with_engine();
    let source = &REGISTERED[0];
    register(&engine, source);
    let other = SERVICE_TYPES
        .iter()
        .find(|value| **value != source.owner)
        .expect("the enum has a second value");

    let created = call(
        &emulator,
        "CreatePerformanceAnalysisReport",
        json!({
            "ServiceType": source.owner,
            "Identifier": source.resource_id,
            "StartTime": T0,
            "EndTime": T0 + 3_600.0,
        }),
    )
    .await;
    let report_id = created
        .member("AnalysisReportId")
        .as_str()
        .unwrap_or_else(|| panic!("Create answers with a minted id, got {:?}", created.body))
        .to_string();

    // Every mismatched call is refused …
    for (operation, input) in requests(other, source.resource_id) {
        let answer = call(&emulator, operation, input).await;
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {operation} under the wrong ServiceType: expected InvalidArgumentException, \
             got {} {:?}",
            answer.status,
            answer.body
        );
    }

    // … and the report created under the owner is still exactly there.
    let listed = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": source.owner, "Identifier": source.resource_id}),
    )
    .await;
    let listed_ids: Vec<&str> = listed
        .member("AnalysisReports")
        .as_array()
        .map(|reports| {
            reports
                .iter()
                .filter_map(|report| report.get("AnalysisReportId").and_then(Value::as_str))
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        listed_ids,
        vec![report_id.as_str()],
        "case the owner's listing after the refusals: expected exactly the created report, got \
         {:?}",
        listed.body
    );

    let read_back = call(
        &emulator,
        "GetPerformanceAnalysisReport",
        json!({
            "ServiceType": source.owner,
            "Identifier": source.resource_id,
            "AnalysisReportId": report_id,
        }),
    )
    .await;
    assert_eq!(
        read_back
            .member("AnalysisReport")
            .get("Identifier")
            .and_then(Value::as_str),
        Some(source.resource_id),
        "case the owner's Get after the refusals: expected the stored report, got {:?}",
        read_back.body
    );
}

/// RULE: ownership is decided by the RECORD, not by the identifier's form — the same
/// `db-`/`cluster-` forms are served or refused purely according to which service
/// registered them, so no case is keyed to a prefix.
#[tokio::test]
async fn ownership_follows_the_record_and_not_the_identifiers_form() {
    let (emulator, engine) = emulator_with_engine();

    // Two ids of the SAME form, registered to DIFFERENT owners.
    let rds_instance = DataSource {
        case: "a db- id owned by RDS",
        resource_id: "db-SAMEFORM0000000000001",
        owner: "RDS",
        engine: "mysql",
    };
    let docdb_instance = DataSource {
        case: "a db- id owned by DocumentDB",
        resource_id: "db-SAMEFORM0000000000002",
        owner: "DOCDB",
        engine: "docdb",
    };
    register(&engine, &rds_instance);
    register(&engine, &docdb_instance);

    for (source, mismatch) in [
        (&rds_instance, "DOCDB"),
        (&docdb_instance, "RDS"),
    ] {
        let served = call(
            &emulator,
            "GetResourceMetadata",
            json!({"ServiceType": source.owner, "Identifier": source.resource_id}),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {}: its owner serves it, got {} {:?}",
            source.case, served.status, served.body
        );

        let refused = call(
            &emulator,
            "GetResourceMetadata",
            json!({"ServiceType": mismatch, "Identifier": source.resource_id}),
        )
        .await;
        assert_eq!(
            (refused.status, refused.error_code().as_str()),
            (400, "InvalidArgumentException"),
            "case {}: the other ServiceType refuses it, got {} {:?}",
            source.case,
            refused.status,
            refused.body
        );
        assert_eq!(
            refused.error_message(),
            state::wrong_service_type_message(source.engine),
            "case {}: the refusal names the engine of the registered resource, got {:?}",
            source.case,
            refused.body
        );
    }
}
