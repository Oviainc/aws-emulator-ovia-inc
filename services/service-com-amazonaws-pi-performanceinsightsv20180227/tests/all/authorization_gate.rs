// SPDX-License-Identifier: CC0-1.0

//! The resource-authorization gate, end to end over the wire.
//!
//! RULE under test: Performance Insights serves only a data source this account
//! monitors — one addressed by the IMMUTABLE resource id its owning service
//! minted (`db-<TOKEN>` for a DB instance, `cluster-<TOKEN>` for a cluster, the
//! token drawn from the uppercase-alphanumeric alphabet those ids are minted
//! from) — so every operation naming a `(ServiceType, Identifier)` pair that is
//! not such a resource id, and every `AnalysisReportId` the addressed data
//! source does not hold, is refused with the modelled `NotAuthorizedException`
//! (HTTP 400) under ONE message that discloses nothing about whether the
//! resource or the report exists.
//!
//! Every row below is one input of that class: the table spans the forms an
//! identifier can take (a name, a bare word, a lowercase token, a bare token
//! with no kind prefix, an empty token, an uppercased prefix, a numeric string),
//! both service types, and every implemented operation that names a data source.
//! The recorded probe identifier is ONE row among them.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator, is_modelled_report_id};

/// 2026-01-01T00:00:00Z. `awsJson1_1` carries an `ISOTimestamp` as epoch
/// seconds.
const T0: f64 = 1_767_225_600.0;

/// A pattern-valid `AnalysisReportId` (`report-` + 17 lowercase hex characters)
/// that no Create ever minted, so the boundary validator passes it and the
/// handler is what answers.
const UNMINTED_REPORT_ID: &str = "report-0123456789abcdef0";

/// The one message every authorization refusal in this service carries. Nothing
/// in it comes from the request: it is the same sentence for an absent data
/// source, another account's, and a report the addressed data source does not
/// hold.
const UNIFORM_MESSAGE: &str =
    service_com_amazonaws_pi_performanceinsightsv20180227::state::NOT_AUTHORIZED_MESSAGE;

/// Identifiers that address NO data source: none of them is a resource id of
/// the form a database service mints. Every one is inside the modelled
/// `IdentifierString` (length 0..256, pattern `^[a-zA-Z0-9-]+$`), so the
/// boundary validator passes it and the handler is what refuses.
const UNMONITORED: &[(&str, &str)] = &[
    (
        "a DB instance name instead of its resource id",
        "my-production-database",
    ),
    ("a bare word", "database"),
    ("a single character", "d"),
    ("a name that merely starts with the db kind", "db-instance-1"),
    (
        "an instance kind prefix with a lowercase token",
        "db-abcdefghijkl",
    ),
    (
        "a cluster kind prefix with a lowercase token",
        "cluster-abcdef",
    ),
    (
        "a resource-id token with no kind prefix",
        "ABCDEFGHIJKLMNOPQRSTU1VW2X",
    ),
    ("an instance kind prefix with an empty token", "db-"),
    ("a cluster kind prefix with an empty token", "cluster-"),
    ("an uppercased kind prefix", "DB-ABCDEFGHIJKL"),
    ("an account id", "123456789012"),
    (
        "the data source the wire corpus records",
        "sim2real-probe-pi-dbinst8-e56fa096",
    ),
];

/// Identifiers that ARE an immutable DB resource id, so the gate serves them.
///
/// Sources for the form: the pi model's own example on every `Identifier`
/// member ("To use a DB instance as a data source, specify its DbiResourceId
/// value. For example, specify db-ABCDEFGHIJKLMNOPQRSTU1VW2X."), and the ids
/// this emulator's own database services mint — RDS
/// `state::mint_dbi_resource_id` (`db-` plus 26 uppercase base-36 symbols) and
/// DocumentDB/Neptune `CreateDBCluster` (`cluster-` plus a token of that
/// alphabet, 12 symbols in DocumentDB and 26 in Neptune). The rows vary the
/// kind, the token length and the alphabet used.
const MONITORED: &[(&str, &str)] = &[
    (
        "the DbiResourceId the model documents",
        "db-ABCDEFGHIJKLMNOPQRSTU1VW2X",
    ),
    ("a 12-symbol instance token", "db-ABCDEFGHIJKL"),
    ("a digits-only instance token", "db-0123456789"),
    (
        "a 26-symbol cluster token",
        "cluster-ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    ),
    ("a 6-symbol cluster token", "cluster-234567"),
    ("the shortest non-empty token", "db-A"),
];

/// The two values of the modelled `ServiceType` enum.
const SERVICE_TYPES: &[&str] = &["RDS", "DOCDB"];

/// One otherwise-valid request per implemented operation, for a
/// `(ServiceType, Identifier)` pair: the authorization gate is the only reason
/// any of them could be refused.
fn requests(service_type: &str, identifier: &str, report_id: &str) -> Vec<(&'static str, Value)> {
    vec![
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
            "ListPerformanceAnalysisReports",
            json!({"ServiceType": service_type, "Identifier": identifier}),
        ),
        (
            // `os` is the one metric type documented for "All engines", so this
            // row is otherwise valid under both service types.
            "ListAvailableResourceMetrics",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "MetricTypes": ["os"],
            }),
        ),
        (
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "AnalysisReportId": report_id,
            }),
        ),
        (
            "DeletePerformanceAnalysisReport",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "AnalysisReportId": report_id,
            }),
        ),
    ]
}

/// RULE: a request addressing a data source this account does not monitor is
/// refused with `NotAuthorizedException` (HTTP 400) — never served, and never
/// under another error identity — on every operation that names one.
#[tokio::test]
async fn an_unmonitored_data_source_is_refused_as_not_authorized() {
    let emulator = emulator();

    for (form, identifier) in UNMONITORED {
        for service_type in SERVICE_TYPES {
            for (operation, input) in requests(service_type, identifier, UNMINTED_REPORT_ID) {
                let case = format!("{form} ({identifier:?}) / {service_type} / {operation}");
                let answer = call(&emulator, operation, input).await;

                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected 400, got {} {:?}",
                    answer.status, answer.body
                );
                assert_eq!(
                    answer.error_code(),
                    "NotAuthorizedException",
                    "case {case}: expected NotAuthorizedException, got {:?}",
                    answer.body
                );
                assert_eq!(
                    answer.error_message(),
                    UNIFORM_MESSAGE,
                    "case {case}: expected the service's uniform authorization message, got {:?}",
                    answer.body
                );
                // The refusal discloses nothing about the resource or the
                // report it was asked about. (The containment check is made for
                // the rows long enough for it to mean anything — a one- or
                // two-character identifier is a substring of almost any
                // sentence; the equality above is what pins those.)
                assert!(
                    !answer.error_message().contains(UNMINTED_REPORT_ID)
                        && (identifier.len() < 8 || !answer.error_message().contains(*identifier)),
                    "case {case}: expected a message naming neither the resource nor the report, \
                     got {:?}",
                    answer.body
                );
                // A refused Create mints nothing.
                assert!(
                    answer.member("AnalysisReportId").is_null(),
                    "case {case}: expected no AnalysisReportId in a refusal, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE: the same gate SERVES a data source addressed by a resource id, and the
/// report it mints there round-trips through Get and List until Delete removes
/// it.
#[tokio::test]
async fn a_data_source_addressed_by_a_resource_id_is_served_and_round_trips() {
    let emulator = emulator();

    for (form, identifier) in MONITORED {
        for service_type in SERVICE_TYPES {
            let case = format!("{form} ({identifier:?}) / {service_type}");

            let metrics = call(
                &emulator,
                "GetResourceMetrics",
                json!({
                    "ServiceType": service_type,
                    "Identifier": identifier,
                    "MetricQueries": [{"Metric": "db.load.avg"}],
                    "StartTime": T0,
                    "EndTime": T0 + 3_600.0,
                    "PeriodInSeconds": 300,
                }),
            )
            .await;
            assert_eq!(
                metrics.status, 200,
                "case {case} / GetResourceMetrics: expected 200, got {} {:?}",
                metrics.status, metrics.body
            );

            let created = call(
                &emulator,
                "CreatePerformanceAnalysisReport",
                json!({
                    "ServiceType": service_type,
                    "Identifier": identifier,
                    "StartTime": T0,
                    "EndTime": T0 + 3_600.0,
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case} / Create: expected 200, got {} {:?}",
                created.status, created.body
            );
            let report_id = created
                .member("AnalysisReportId")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert!(
                is_modelled_report_id(&report_id),
                "case {case} / Create: expected a minted report id, got {:?}",
                created.body
            );

            let read_back = call(
                &emulator,
                "GetPerformanceAnalysisReport",
                json!({
                    "ServiceType": service_type,
                    "Identifier": identifier,
                    "AnalysisReportId": report_id,
                }),
            )
            .await;
            assert_eq!(
                read_back.status, 200,
                "case {case} / Get: expected 200, got {} {:?}",
                read_back.status, read_back.body
            );
            assert_eq!(
                read_back
                    .member("AnalysisReport")
                    .get("Identifier")
                    .and_then(Value::as_str),
                Some(*identifier),
                "case {case} / Get: expected the created resource echoed, got {:?}",
                read_back.body
            );

            let listed = call(
                &emulator,
                "ListPerformanceAnalysisReports",
                json!({"ServiceType": service_type, "Identifier": identifier}),
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
                "case {case} / List: expected exactly the created report, got {:?}",
                listed.body
            );

            let deleted = call(
                &emulator,
                "DeletePerformanceAnalysisReport",
                json!({
                    "ServiceType": service_type,
                    "Identifier": identifier,
                    "AnalysisReportId": report_id,
                }),
            )
            .await;
            assert_eq!(
                deleted.status, 200,
                "case {case} / Delete: expected 200, got {} {:?}",
                deleted.status, deleted.body
            );

            // Deleted, so the data source no longer holds it: the same uniform
            // refusal as a data source the caller may not see.
            let gone = call(
                &emulator,
                "GetPerformanceAnalysisReport",
                json!({
                    "ServiceType": service_type,
                    "Identifier": identifier,
                    "AnalysisReportId": report_id,
                }),
            )
            .await;
            assert_eq!(
                gone.error_code(),
                "NotAuthorizedException",
                "case {case} / Get after Delete: expected NotAuthorizedException, got {} {:?}",
                gone.status,
                gone.body
            );
        }
    }
}

/// RULE: an `AnalysisReportId` the addressed data source does not hold is the
/// same uniform `NotAuthorizedException`, and its message names neither the
/// report nor the resource — the disclosure the single error exists to prevent.
#[tokio::test]
async fn a_report_the_data_source_does_not_hold_discloses_nothing() {
    let emulator = emulator();

    // Two monitored data sources, each holding one report, so a row can ask one
    // of them for the other's report.
    let owner = ("RDS", "db-ABCDEFGHIJKLMNOPQRSTU1VW2X");
    let other = ("DOCDB", "cluster-ABCDEFGHIJKL");
    let mut minted: Vec<String> = Vec::new();
    for (service_type, identifier) in [owner, other] {
        let created = call(
            &emulator,
            "CreatePerformanceAnalysisReport",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "StartTime": T0,
            }),
        )
        .await;
        minted.push(
            created
                .member("AnalysisReportId")
                .as_str()
                .unwrap_or_else(|| panic!("create answers with an id, got {:?}", created.body))
                .to_string(),
        );
    }

    // Each row is a (ServiceType, Identifier, AnalysisReportId) triple whose
    // data source is monitored and whose report id is pattern-valid, but which
    // no stored report answers to.
    let cases: Vec<(&str, &str, String, &str)> = vec![
        (
            owner.0,
            owner.1,
            UNMINTED_REPORT_ID.to_string(),
            "an id no Create ever minted",
        ),
        (
            owner.0,
            owner.1,
            minted[1].clone(),
            "another data source's report",
        ),
        (
            other.0,
            other.1,
            minted[0].clone(),
            "the first data source's report under the second",
        ),
        (
            owner.0,
            "db-NOSUCHRESOURCE",
            minted[0].clone(),
            "a monitored resource id that holds nothing",
        ),
        (
            other.0,
            owner.1,
            minted[0].clone(),
            "the right resource id under the wrong service type",
        ),
    ];

    for (service_type, identifier, report_id, label) in cases {
        for operation in [
            "GetPerformanceAnalysisReport",
            "DeletePerformanceAnalysisReport",
        ] {
            let case = format!("{label} / {operation}");
            let answer = call(
                &emulator,
                operation,
                json!({
                    "ServiceType": service_type,
                    "Identifier": identifier,
                    "AnalysisReportId": report_id,
                }),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {case}: expected NotAuthorizedException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_message(),
                UNIFORM_MESSAGE,
                "case {case}: expected the service's uniform authorization message, got {:?}",
                answer.body
            );
            assert!(
                !answer.error_message().contains(identifier)
                    && !answer.error_message().contains(&report_id),
                "case {case}: expected a message disclosing neither the resource nor the report, \
                 got {:?}",
                answer.body
            );
        }
    }

    // The reports that WERE created are untouched by the refusals above.
    for ((service_type, identifier), report_id) in [owner, other].iter().zip(&minted) {
        let answer = call(
            &emulator,
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": service_type,
                "Identifier": identifier,
                "AnalysisReportId": report_id,
            }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case the surviving report of {identifier}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
    }
}

/// RULE: the gate decides authorization for a request whose VALUES hold
/// together; a request that breaks a modelled constraint or a documented
/// cross-member rule is still answered as the argument error it is, so the gate
/// never hides a client-side input bug.
#[tokio::test]
async fn argument_validity_is_decided_before_the_authorization_gate() {
    let emulator = emulator();

    // An inverted window on an UNMONITORED data source: documented on
    // `StartTime` ("The value for StartTime must be earlier than the value for
    // EndTime"), and the service's own `InvalidArgumentException`.
    for (form, identifier) in UNMONITORED {
        let inverted: Vec<(&str, Value)> = vec![
            (
                "GetResourceMetrics",
                json!({
                    "ServiceType": "RDS",
                    "Identifier": identifier,
                    "MetricQueries": [{"Metric": "db.load.avg"}],
                    "StartTime": T0,
                    "EndTime": T0 - 1.0,
                }),
            ),
            (
                "CreatePerformanceAnalysisReport",
                json!({
                    "ServiceType": "RDS",
                    "Identifier": identifier,
                    "StartTime": T0,
                    "EndTime": T0,
                }),
            ),
        ];
        for (operation, input) in inverted {
            let case = format!("{form} ({identifier:?}) / {operation} with an inverted window");
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
        }
    }

    // A value outside a modelled constraint is refused at the boundary, before
    // any handler runs — so it is never answered as an authorization outcome.
    // (Which argument error the boundary reports is not this rule's business;
    // that it is NOT the authorization one is.)
    let malformed: Vec<(&str, &str, Value)> = vec![
        (
            "an Identifier outside the modelled pattern",
            "GetResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db ABCDEFGH!",
                "MetricQueries": [{"Metric": "db.load.avg"}],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
            }),
        ),
        (
            "a ServiceType outside the modelled enum",
            "ListPerformanceAnalysisReports",
            json!({"ServiceType": "AURORA", "Identifier": "my-production-database"}),
        ),
        (
            "an absent required Identifier",
            "ListPerformanceAnalysisReports",
            json!({"ServiceType": "RDS"}),
        ),
        (
            "an AnalysisReportId outside the modelled pattern",
            "GetPerformanceAnalysisReport",
            json!({
                "ServiceType": "RDS",
                "Identifier": "my-production-database",
                "AnalysisReportId": "report-NOTHEX0000000000",
            }),
        ),
    ];

    for (label, operation, input) in malformed {
        let answer = call(&emulator, operation, input).await;
        assert_eq!(
            answer.status, 400,
            "case {label}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_ne!(
            answer.error_code(),
            "NotAuthorizedException",
            "case {label}: expected an argument error from the boundary, got {:?}",
            answer.body
        );
    }
}

/// RULE: the gate answers before the emulator considers whether it could
/// compute the series at all — a dimension-grouped or filtered query, which this
/// emulator refuses with 501 for a data source it serves, is an authorization
/// refusal for one it does not.
#[tokio::test]
async fn the_gate_precedes_what_the_emulator_cannot_compute() {
    let emulator = emulator();

    let queries: Vec<(&str, Value)> = vec![
        (
            "GroupBy",
            json!({"Metric": "db.load.avg", "GroupBy": {"Group": "db.sql"}}),
        ),
        (
            "Filter",
            json!({"Metric": "db.load.avg", "Filter": {"db.user.name": "postgres"}}),
        ),
    ];

    for (label, query) in queries {
        for (form, identifier) in UNMONITORED {
            let case = format!("{label} on {form} ({identifier:?})");
            let answer = call(
                &emulator,
                "GetResourceMetrics",
                json!({
                    "ServiceType": "RDS",
                    "Identifier": identifier,
                    "MetricQueries": [query.clone()],
                    "StartTime": T0,
                    "EndTime": T0 + 3_600.0,
                    "PeriodInSeconds": 300,
                }),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected the authorization refusal, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {case}: expected NotAuthorizedException, got {:?}",
                answer.body
            );
        }

        // The same query on a monitored data source still answers 501: the gate
        // replaced no honest refusal.
        let monitored = call(
            &emulator,
            "GetResourceMetrics",
            json!({
                "ServiceType": "RDS",
                "Identifier": "db-ABCDEFGHIJKLMNOPQRSTU1VW2X",
                "MetricQueries": [query],
                "StartTime": T0,
                "EndTime": T0 + 3_600.0,
                "PeriodInSeconds": 300,
            }),
        )
        .await;
        assert_eq!(
            monitored.status, 501,
            "case {label} on a monitored data source: expected 501, got {} {:?}",
            monitored.status, monitored.body
        );
    }
}
