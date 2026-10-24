// SPDX-License-Identifier: CC0-1.0

//! The order `ListAvailableResourceMetrics` answers its two client refusals in,
//! end to end over the wire.
//!
//! RULE under test: which metrics a data source can be queried for is a property
//! of the engine family behind the `(ServiceType, Identifier)` pair the request
//! addresses — the `MetricTypes` member scopes every type it documents by engine
//! ("os (OS counter metrics) - All engines", the `db` types "All engines except
//! for Amazon DocumentDB") — so the pair is resolved and authorized BEFORE the
//! request's metric types are inspected: a request naming a data source this
//! account may not read is refused with the modelled `NotAuthorizedException`
//! (HTTP 400) whatever metric types it names, and the metric-type answer the same
//! request earns on a data source the account DOES monitor is unchanged by that
//! ordering.
//!
//! Both modelled client errors of this operation are HTTP 400, so the error code
//! is the only discriminator on the wire and the assertions below pin it.
//!
//! The table spans the class the rule generalises over: every published metric
//! type and both `ServiceType` values, metric lists that are a published type, a
//! type the requested service type does not publish, a value the service
//! publishes for no engine at all, a per-SQL type whose catalog this emulator
//! cannot know, and mixed lists of them. The metric type and service type of the
//! recorded unit (`DOCDB` with `db`) are ONE row among them.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator};

/// The answer a metric-type list earns on a data source this account monitors —
/// what the authorization gate must NOT replace, and must not have been replaced
/// by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Monitored {
    /// HTTP 200 with the published catalog of the requested types.
    Catalog,
    /// HTTP 400 `InvalidArgumentException`: a value this service publishes for no
    /// engine, or one the engine family behind the requested `ServiceType` does
    /// not publish.
    InvalidArgument,
    /// HTTP 501: a per-SQL catalog is the running engine's own, and this emulator
    /// monitors none.
    NotImplemented,
}

/// One metric-type list, with the answer it earns on a monitored data source
/// under each `ServiceType`.
///
/// SOURCE for which type is published for which engine: the `MetricTypes` member's
/// own documentation in this service's Smithy model
/// (`smithy_query(query="doc",shape="ListAvailableResourceMetricsRequest",member="MetricTypes")`)
/// — "os (OS counter metrics) - All engines", "db (DB load metrics) - All engines
/// except for Amazon DocumentDB", and the same scope for the two per-SQL types.
const ROWS: &[(&str, &[&str], Monitored, Monitored)] = &[
    (
        "the type published for all engines",
        &["os"],
        Monitored::Catalog,
        Monitored::Catalog,
    ),
    (
        "the DB load type, not published for DocumentDB",
        &["db"],
        Monitored::Catalog,
        Monitored::InvalidArgument,
    ),
    (
        "the per-SQL type, not published for DocumentDB",
        &["db.sql.stats"],
        Monitored::NotImplemented,
        Monitored::InvalidArgument,
    ),
    (
        "the per-SQL digest type, not published for DocumentDB",
        &["db.sql_tokenized.stats"],
        Monitored::NotImplemented,
        Monitored::InvalidArgument,
    ),
    (
        "a word that is no metric type",
        &["foo"],
        Monitored::InvalidArgument,
        Monitored::InvalidArgument,
    ),
    (
        "a metric NAME instead of a metric type",
        &["db.load"],
        Monitored::InvalidArgument,
        Monitored::InvalidArgument,
    ),
    (
        "a published type miscased",
        &["OS"],
        Monitored::InvalidArgument,
        Monitored::InvalidArgument,
    ),
    (
        "two published types",
        &["os", "db"],
        Monitored::Catalog,
        Monitored::InvalidArgument,
    ),
    (
        "a published type beside a per-SQL one",
        &["os", "db.sql.stats"],
        Monitored::NotImplemented,
        Monitored::InvalidArgument,
    ),
    (
        "a published type beside an unpublished value",
        &["os", "foo"],
        Monitored::InvalidArgument,
        Monitored::InvalidArgument,
    ),
    (
        "one type named twice",
        &["os", "os"],
        Monitored::Catalog,
        Monitored::Catalog,
    ),
    (
        "no metric type at all",
        &[],
        Monitored::Catalog,
        Monitored::Catalog,
    ),
];

/// Identifiers that address NO data source this account monitors: none is a
/// resource id of the form a database service mints, and every one is inside the
/// modelled `IdentifierString` (length 0..256, pattern `^[a-zA-Z0-9-]+$`), so the
/// boundary validator passes it and the handler is what refuses. The rows vary
/// the form: a DB instance name, a lowercase token, a token with no kind prefix,
/// an empty token, and the identifier the recorded unit probes with.
const UNMONITORED: &[&str] = &[
    "my-production-database",
    "db-abcdefghijkl",
    "ABCDEFGHIJKLMNOPQRSTU1VW2X",
    "cluster-",
    "sim2real-probe-pi-dbinst8-e56fa096",
];

/// A data source addressed by an immutable DB resource id, so the gate serves it
/// (the form the model documents on every `Identifier` member: "specify
/// db-ABCDEFGHIJKLMNOPQRSTU1VW2X").
const MONITORED: &str = "db-ABCDEFGHIJKLMNOPQRSTU1VW2X";

/// The two values of the modelled `ServiceType` enum, with the answer column
/// each selects from a row.
const SERVICE_TYPES: &[&str] = &["RDS", "DOCDB"];

/// The one message every authorization refusal in this service carries.
const UNIFORM_MESSAGE: &str =
    service_com_amazonaws_pi_performanceinsightsv20180227::state::NOT_AUTHORIZED_MESSAGE;

fn list_input(service_type: &str, identifier: &str, metric_types: &[&str]) -> Value {
    json!({
        "ServiceType": service_type,
        "Identifier": identifier,
        "MetricTypes": metric_types,
    })
}

/// The answer a row expects for one `ServiceType`.
fn expected(row: &(&str, &[&str], Monitored, Monitored), service_type: &str) -> Monitored {
    if service_type == "DOCDB" {
        row.3
    } else {
        row.2
    }
}

/// RULE: a data source this account may not read is refused with
/// `NotAuthorizedException` whatever the request's `MetricTypes` say — including
/// metric types that would be an argument error, or unanswerable, on a data
/// source the account monitors.
#[tokio::test]
async fn an_unmonitored_data_source_is_not_authorized_whatever_metric_types_it_names() {
    let emulator = emulator();

    for identifier in UNMONITORED {
        for service_type in SERVICE_TYPES {
            for row in ROWS {
                let (label, metric_types, _, _) = row;
                let case =
                    format!("{identifier:?} / {service_type} / {label} {metric_types:?}");
                let answer = call(
                    &emulator,
                    "ListAvailableResourceMetrics",
                    list_input(service_type, identifier, metric_types),
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
                    answer.member("Metrics").is_null()
                        && answer.member("NextToken").is_null(),
                    "case {case}: expected no catalog in a refusal, got {:?}",
                    answer.body
                );
            }
        }
    }
}

/// RULE (the other side of the ordering): on a data source the account DOES
/// monitor, every row still earns the answer its metric types deserve — the
/// published catalog, the modelled argument error, or the honest 501 — so putting
/// the gate first replaced no answer of its own.
#[tokio::test]
async fn a_monitored_data_source_still_earns_its_metric_type_answer() {
    let emulator = emulator();

    for service_type in SERVICE_TYPES {
        for row in ROWS {
            let (label, metric_types, _, _) = row;
            let case = format!("{service_type} / {label} {metric_types:?}");
            let answer = call(
                &emulator,
                "ListAvailableResourceMetrics",
                list_input(service_type, MONITORED, metric_types),
            )
            .await;

            assert_ne!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {case}: expected the metric-type answer, not an authorization \
                 refusal, got {} {:?}",
                answer.status,
                answer.body
            );

            match expected(row, service_type) {
                Monitored::Catalog => {
                    assert_eq!(
                        answer.status, 200,
                        "case {case}: expected 200, got {} {:?}",
                        answer.status, answer.body
                    );
                    assert!(
                        answer.member("Metrics").is_array(),
                        "case {case}: expected a catalog, got {:?}",
                        answer.body
                    );
                }
                Monitored::InvalidArgument => {
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
                    assert_ne!(
                        answer.error_message(),
                        UNIFORM_MESSAGE,
                        "case {case}: expected an argument-error message, got {:?}",
                        answer.body
                    );
                }
                Monitored::NotImplemented => {
                    assert_eq!(
                        answer.status, 501,
                        "case {case}: expected 501, got {} {:?}",
                        answer.status, answer.body
                    );
                }
            }
        }
    }
}
