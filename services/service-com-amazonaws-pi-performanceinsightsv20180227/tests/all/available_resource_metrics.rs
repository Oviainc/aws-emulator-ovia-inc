// SPDX-License-Identifier: CC0-1.0

//! `ListAvailableResourceMetrics`, end to end over the wire.
//!
//! RULE under test: the operation answers the published CATALOG of the metric
//! types the request names — every entry carrying the metric's full name,
//! description and unit — for a data source this account monitors; a metric type
//! Performance Insights does not publish, and one the requested `ServiceType`
//! does not publish, are refused with the modelled `InvalidArgumentException`; a
//! data source the account does not monitor is refused with the modelled
//! `NotAuthorizedException`; and because availability is a published property
//! rather than a measurement, the answer is the same every time and `MaxResults`
//! /`NextToken` page it so every metric is visited exactly once.
//!
//! Each table below spans the class the rule generalises over — both
//! `ServiceType` values, every published metric type, requests naming one type,
//! several types and a repeated type, and every page size on and around the
//! modelled `MaxResults` bounds. The metric type the recorded unit asks for is
//! ONE row among them.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator, is_modelled_next_token};

/// A data source addressed by an immutable DB resource id, so the service's
/// authorization gate serves it (the form the model documents on every
/// `Identifier` member: "specify db-ABCDEFGHIJKLMNOPQRSTU1VW2X").
const IDENTIFIER: &str = "db-ABCDEFGHIJKLMNOPQRSTU1VW2X";

/// A second monitored data source, for the rows that check a cursor is bound to
/// the result set it was handed out for.
const OTHER_IDENTIFIER: &str = "cluster-ABCDEFGHIJKL";

/// The one message every authorization refusal in this service carries.
const UNIFORM_MESSAGE: &str =
    service_com_amazonaws_pi_performanceinsightsv20180227::state::NOT_AUTHORIZED_MESSAGE;

/// The metric types with a catalog this emulator publishes, and the
/// `ServiceType` values each is published for.
///
/// SOURCE: the `MetricTypes` member's own documentation — "os (OS counter
/// metrics) - All engines", "db (DB load metrics) - All engines except for Amazon
/// DocumentDB". The per-SQL types that documentation also lists carry an
/// engine-specific catalog and are covered by their own test below.
const CATALOGUED_TYPES: &[(&str, &[&str], &str)] =
    &[("os", &["RDS", "DOCDB"], "os."), ("db", &["RDS"], "db.")];

/// The per-SQL metric types the same documentation lists: "db.sql.stats (per-SQL
/// metrics)" and "db.sql_tokenized.stats (per-SQL digest metrics)", both "All
/// engines except for Amazon DocumentDB".
const PER_SQL_TYPES: &[&str] = &["db.sql.stats", "db.sql_tokenized.stats"];

/// The two values of the modelled `ServiceType` enum.
const SERVICE_TYPES: &[&str] = &["RDS", "DOCDB"];

fn list_input(
    service_type: &str,
    identifier: &str,
    metric_types: &[&str],
    max_results: Option<i64>,
    next_token: Option<&str>,
) -> Value {
    let mut input = json!({
        "ServiceType": service_type,
        "Identifier": identifier,
        "MetricTypes": metric_types,
    });
    if let Some(max_results) = max_results {
        input["MaxResults"] = json!(max_results);
    }
    if let Some(next_token) = next_token {
        input["NextToken"] = json!(next_token);
    }
    input
}

/// The `Metric` of every entry of a successful answer, in the order answered.
fn metric_names(answer: &Value) -> Vec<String> {
    answer
        .get("Metrics")
        .and_then(Value::as_array)
        .map(|metrics| {
            metrics
                .iter()
                .map(|metric| {
                    metric
                        .get("Metric")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The whole catalog of `metric_types` for one data source, unpaged.
async fn whole_catalog(
    emulator: &smithy_server_runtime::MultiServiceRouter,
    service_type: &str,
    metric_types: &[&str],
) -> Vec<String> {
    let answer = call(
        emulator,
        "ListAvailableResourceMetrics",
        list_input(service_type, IDENTIFIER, metric_types, None, None),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "case {service_type}/{metric_types:?}: expected 200, got {} {:?}",
        answer.status, answer.body
    );
    metric_names(&answer.body)
}

/// RULE: a request for a published metric type answers that type's catalog —
/// each entry naming a metric of that type with a description and a unit — and
/// answers it identically every time, because nothing about availability is
/// measured or stored.
#[tokio::test]
async fn each_published_metric_type_answers_its_catalog() {
    let emulator = emulator();

    for (metric_type, service_types, prefix) in CATALOGUED_TYPES {
        for service_type in *service_types {
            let case = format!("{service_type}/{metric_type}");
            let answer = call(
                &emulator,
                "ListAvailableResourceMetrics",
                list_input(service_type, IDENTIFIER, &[*metric_type], None, None),
            )
            .await;

            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );

            let entries = answer
                .member("Metrics")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !entries.is_empty(),
                "case {case}: expected a non-empty catalog, got {:?}",
                answer.body
            );

            for entry in &entries {
                let metric = entry.get("Metric").and_then(Value::as_str).unwrap_or("");
                assert!(
                    metric.starts_with(prefix),
                    "case {case}: expected every metric to be of the requested type \
                     ({prefix}*), got {entry:?}"
                );
                // `ResponseResourceMetric` is "an object that contains the full
                // name, description, and unit of a metric", and the modelled
                // `Description` has a length minimum of 1.
                let description = entry
                    .get("Description")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let unit = entry.get("Unit").and_then(Value::as_str).unwrap_or("");
                assert!(
                    !description.is_empty() && !unit.is_empty(),
                    "case {case}: expected a description and a unit for {metric:?}, \
                     got {entry:?}"
                );
                // The metric name carries no aggregate function: the caller
                // appends one ("to find the average for the metric db.load you
                // must use db.load.avg").
                for aggregate in [".avg", ".min", ".max", ".sum"] {
                    assert!(
                        !metric.ends_with(aggregate),
                        "case {case}: expected a metric name without an aggregate \
                         function, got {metric:?}"
                    );
                }
            }

            let names = metric_names(&answer.body);
            let mut unique = names.clone();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                names.len(),
                "case {case}: expected every metric listed once, got {names:?}"
            );

            // The whole catalog fits in one answer when the caller imposes no page
            // size, so there is nothing left to resume from.
            assert!(
                answer.member("NextToken").is_null(),
                "case {case}: expected no NextToken on a complete answer, got {:?}",
                answer.body
            );

            // Reading the catalog changed nothing a later caller sees.
            let again = call(
                &emulator,
                "ListAvailableResourceMetrics",
                list_input(service_type, IDENTIFIER, &[*metric_type], None, None),
            )
            .await;
            assert_eq!(
                again.body, answer.body,
                "case {case}: expected a repeated read to answer identically, got {:?}",
                again.body
            );
        }
    }

    // The DB load metrics are what the `db` type publishes, and this model
    // documents `db.load` as a Performance Insights metric by name
    // (`ListAvailableResourceDimensions`' `Metrics` member: "Valid values include
    // db.load").
    let db_catalog = whole_catalog(&emulator, "RDS", &["db"]).await;
    assert!(
        db_catalog.iter().any(|metric| metric == "db.load"),
        "case RDS/db: expected the DB load metric in the catalog, got {db_catalog:?}"
    );
}

/// RULE: naming several metric types answers their catalogs together, in the
/// order the types were named, with each metric listed once however often its
/// type was repeated.
#[tokio::test]
async fn several_metric_types_answer_their_catalogs_in_order() {
    let emulator = emulator();

    let os = whole_catalog(&emulator, "RDS", &["os"]).await;
    let db = whole_catalog(&emulator, "RDS", &["db"]).await;

    // Each row is a request naming more than one type, or one type more than
    // once, with the catalog it must answer.
    let cases: Vec<(Vec<&str>, Vec<String>)> = vec![
        (vec!["os", "db"], [os.clone(), db.clone()].concat()),
        (vec!["db", "os"], [db.clone(), os.clone()].concat()),
        (vec!["os", "os"], os.clone()),
        (vec!["db", "db", "db"], db.clone()),
        (vec!["db", "os", "db"], [db.clone(), os.clone()].concat()),
    ];

    for (metric_types, expected) in cases {
        let answer = call(
            &emulator,
            "ListAvailableResourceMetrics",
            list_input("RDS", IDENTIFIER, &metric_types, None, None),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {metric_types:?}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            metric_names(&answer.body),
            expected,
            "case {metric_types:?}: expected each named type's catalog once, in order, \
             got {:?}",
            answer.body
        );
    }
}

/// RULE: `MaxResults` bounds a page and `NextToken` resumes after it, so paging
/// visits every metric of the result set exactly once, in the unpaged order, and
/// a cursor is handed out only while metrics remain.
#[tokio::test]
async fn paging_visits_every_metric_exactly_once() {
    let emulator = emulator();

    // The rows differ along the page size — including both published
    // `MaxResults` bounds, 0 and 25 — and along the result set being paged.
    for metric_types in [vec!["os"], vec!["db"], vec!["db", "os"]] {
        let expected = whole_catalog(&emulator, "RDS", &metric_types).await;

        for page_size in [0_i64, 1, 2, 3, 7, 24, 25] {
            let case = format!("{metric_types:?} / MaxResults={page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            // Bounded: each page carries at least one metric when a page size is
            // imposed, and the whole catalog when it is not.
            for _ in 0..=expected.len() {
                let answer = call(
                    &emulator,
                    "ListAvailableResourceMetrics",
                    list_input(
                        "RDS",
                        IDENTIFIER,
                        &metric_types,
                        Some(page_size),
                        token.as_deref(),
                    ),
                )
                .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} {:?}",
                    answer.status, answer.body
                );

                let page = metric_names(&answer.body);
                if page_size > 0 {
                    assert!(
                        page.len() <= page_size as usize,
                        "case {case}: expected at most {page_size} metrics in a page, \
                         got {page:?}"
                    );
                }
                seen.extend(page);

                match answer.member("NextToken").as_str() {
                    Some(next) => {
                        assert!(
                            is_modelled_next_token(next),
                            "case {case}: expected a modelled NextToken, got {next:?}"
                        );
                        assert!(
                            seen.len() < expected.len(),
                            "case {case}: expected no cursor once every metric was \
                             answered, got {next:?}"
                        );
                        token = Some(next.to_string());
                    }
                    None => {
                        token = None;
                        break;
                    }
                }
            }

            assert!(
                token.is_none(),
                "case {case}: expected paging to finish, still holding {token:?}"
            );
            assert_eq!(
                seen, expected,
                "case {case}: expected paging to visit the whole catalog once, in order, \
                 got {seen:?}"
            );
        }
    }
}

/// RULE: a `MaxResults` outside the modelled range 0..25 is refused by the
/// validation stage, so the page sizes the handler ever sees are the published
/// ones.
#[tokio::test]
async fn a_page_size_past_the_published_bound_is_refused() {
    let emulator = emulator();

    for page_size in [-1_i64, 26, 100] {
        let answer = call(
            &emulator,
            "ListAvailableResourceMetrics",
            list_input("RDS", IDENTIFIER, &["os"], Some(page_size), None),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case MaxResults={page_size}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "ValidationException",
            "case MaxResults={page_size}: expected ValidationException, got {:?}",
            answer.body
        );
    }
}

/// RULE: a cursor is bound to the result set it was handed out for, so a token
/// replayed against another service type, data source or metric type set — or
/// one this service never issued — is refused with the modelled
/// `InvalidArgumentException`.
#[tokio::test]
async fn a_cursor_from_another_result_set_is_refused() {
    let emulator = emulator();

    let first = call(
        &emulator,
        "ListAvailableResourceMetrics",
        list_input("RDS", IDENTIFIER, &["os"], Some(2), None),
    )
    .await;
    let token = first
        .member("NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("a bounded first page carries a cursor, got {:?}", first.body))
        .to_string();

    // Each row is a request the cursor was NOT issued for, plus a token this
    // service never issued at all.
    let cases: Vec<(&str, Value)> = vec![
        (
            "another data source",
            list_input("RDS", OTHER_IDENTIFIER, &["os"], Some(2), Some(&token)),
        ),
        (
            "another service type",
            list_input("DOCDB", IDENTIFIER, &["os"], Some(2), Some(&token)),
        ),
        (
            "another metric type set",
            list_input("RDS", IDENTIFIER, &["os", "db"], Some(2), Some(&token)),
        ),
        (
            "a token this service never issued",
            list_input("RDS", IDENTIFIER, &["os"], Some(2), Some("ZZZZ")),
        ),
    ];

    for (label, input) in cases {
        let answer = call(&emulator, "ListAvailableResourceMetrics", input).await;
        assert_eq!(
            answer.status, 400,
            "case {label}: expected 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case {label}: expected InvalidArgumentException, got {:?}",
            answer.body
        );
    }

    // The cursor still resumes the result set it WAS issued for.
    let resumed = call(
        &emulator,
        "ListAvailableResourceMetrics",
        list_input("RDS", IDENTIFIER, &["os"], Some(2), Some(&token)),
    )
    .await;
    assert_eq!(
        resumed.status, 200,
        "case the issuing result set: expected 200, got {} {:?}",
        resumed.status, resumed.body
    );
    let first_page = metric_names(&first.body);
    let second_page = metric_names(&resumed.body);
    assert!(
        !second_page.is_empty() && second_page.iter().all(|m| !first_page.contains(m)),
        "case the issuing result set: expected the metrics after the first page, \
         got {second_page:?} after {first_page:?}"
    );
}

/// RULE: a metric type Performance Insights does not publish, and one the
/// requested `ServiceType` does not publish, are both refused with the modelled
/// `InvalidArgumentException` — never served as an empty catalog.
#[tokio::test]
async fn a_metric_type_that_is_not_published_is_refused() {
    let emulator = emulator();

    // Rows vary the form of the unpublished value: an unrelated word, the
    // documented types miscased, a metric name instead of a type, a prefix of a
    // published type, and a published type with a stray character — every one
    // inside the modelled `SanitizedString` constraints, so the handler is what
    // refuses it.
    let unpublished: &[&str] = &[
        "foo",
        "OS",
        "DB",
        "Db.Sql.Stats",
        "db.load",
        "os.cpuUtilization.total",
        "db.sql",
        "db.sql_tokenized",
        "db.stats",
        "os ",
    ];

    for service_type in SERVICE_TYPES {
        for metric_type in unpublished {
            for metric_types in [vec![*metric_type], vec!["os", *metric_type]] {
                let case = format!("{service_type} / {metric_types:?}");
                let answer = call(
                    &emulator,
                    "ListAvailableResourceMetrics",
                    list_input(service_type, IDENTIFIER, &metric_types, None, None),
                )
                .await;
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
                assert!(
                    answer.error_message().contains("MetricTypes"),
                    "case {case}: expected a message naming the member at fault, got {:?}",
                    answer.body
                );
                assert!(
                    answer.member("Metrics").is_null(),
                    "case {case}: expected no catalog in a refusal, got {:?}",
                    answer.body
                );
            }
        }
    }

    // The `db` types are documented for "All engines except for Amazon
    // DocumentDB", so a DocumentDB data source does not publish them.
    for metric_type in ["db", "db.sql.stats", "db.sql_tokenized.stats"] {
        let case = format!("DOCDB / {metric_type}");
        let answer = call(
            &emulator,
            "ListAvailableResourceMetrics",
            list_input("DOCDB", IDENTIFIER, &[metric_type], None, None),
        )
        .await;
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
        assert!(
            answer.error_message().contains("DOCDB"),
            "case {case}: expected a message naming the service type, got {:?}",
            answer.body
        );
    }
}

/// RULE: this emulator monitors no running DB engine, so it cannot know which
/// engine's per-SQL counter catalog a data source publishes; those metric types
/// answer the scaffold's 501 rather than another engine's names or an empty
/// catalog.
#[tokio::test]
async fn the_per_sql_metric_types_are_answered_as_not_implemented() {
    let emulator = emulator();

    for metric_type in PER_SQL_TYPES {
        for metric_types in [vec![*metric_type], vec!["os", *metric_type]] {
            let case = format!("RDS / {metric_types:?}");
            let answer = call(
                &emulator,
                "ListAvailableResourceMetrics",
                list_input("RDS", IDENTIFIER, &metric_types, None, None),
            )
            .await;
            assert_eq!(
                answer.status, 501,
                "case {case}: expected 501, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "NotImplemented",
                "case {case}: expected NotImplemented, got {:?}",
                answer.body
            );
            assert!(
                answer.error_message().contains(metric_type),
                "case {case}: expected a message naming the metric type, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: the catalog of a data source this account does not monitor is not
/// answered at all — the same uniform `NotAuthorizedException` every operation in
/// this service gives, disclosing nothing about the resource.
#[tokio::test]
async fn an_unmonitored_data_source_is_refused_as_not_authorized() {
    let emulator = emulator();

    // Rows span the forms an identifier that is no resource id can take: a DB
    // instance name, a lowercase token, a token with no kind prefix, an empty
    // token, and the identifier the wire corpus probes with.
    let unmonitored = [
        "my-production-database",
        "db-abcdefghijkl",
        "ABCDEFGHIJKLMNOPQRSTU1VW2X",
        "db-",
        "sim2real-probe-pi-dbinst8-e56fa096",
    ];

    for identifier in unmonitored {
        for service_type in SERVICE_TYPES {
            let case = format!("{service_type} / {identifier:?}");
            let answer = call(
                &emulator,
                "ListAvailableResourceMetrics",
                list_input(service_type, identifier, &["os"], None, None),
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
                answer.member("Metrics").is_null(),
                "case {case}: expected no catalog in a refusal, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: `MetricTypes` is what selects the catalogs, so naming none selects
/// nothing: the answer is an empty catalog with no cursor, not a refusal and not
/// every metric the service publishes.
#[tokio::test]
async fn naming_no_metric_type_answers_an_empty_catalog() {
    let emulator = emulator();

    for service_type in SERVICE_TYPES {
        let answer = call(
            &emulator,
            "ListAvailableResourceMetrics",
            list_input(service_type, IDENTIFIER, &[], None, None),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {service_type}: expected 200, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            metric_names(&answer.body),
            Vec::<String>::new(),
            "case {service_type}: expected an empty catalog, got {:?}",
            answer.body
        );
        assert!(
            answer.member("NextToken").is_null(),
            "case {service_type}: expected no NextToken, got {:?}",
            answer.body
        );
    }
}
