// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the account's vended-metric enrichment configuration:
//! `StartOTelEnrichment`, `UpdateOTelEnrichment`, `StopOTelEnrichment` and the
//! `GetOTelEnrichment` that reports what they stored.
//!
//! Every table row is a distinct input along what the rules under test generalise over (which
//! filter lists a request carries, how many selectors each one holds, whether a selector names
//! metric names, and the order the start/update/stop calls arrive in), so a single recorded
//! start-then-get is one row among many rather than the specification.

use crate::harness::{build_router, build_router_with_engine, call, error_code, ok};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A string member of a body.
fn text(value: &Value, member: &str) -> Option<String> {
    value
        .get(member)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// A timestamp member of a body, which this protocol puts on the wire as epoch seconds.
fn stamp(value: &Value, member: &str) -> Option<f64> {
    value.get(member).and_then(Value::as_f64)
}

/// The filter list a response is expected to report for `member`, given the request that
/// stored it: an empty list constrains nothing, which is what no filter at all means, so it is
/// reported as absent rather than as an empty list.
fn expected_filters(request: &Value, member: &str) -> Option<Value> {
    request
        .get(member)
        .filter(|value| value.as_array().is_some_and(|list| !list.is_empty()))
        .cloned()
}

/// `count` selectors, each naming its own namespace, for the rows that exercise how MANY
/// filters a configuration holds.
fn selectors(prefix: &str, count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"Namespace": format!("Custom/{prefix}{index}")}))
            .collect(),
    )
}

/// Start enrichment with `request`, requiring the call to be accepted.
async fn start(router: &MultiServiceRouter, request: &Value) -> Value {
    ok(router, "StartOTelEnrichment", request).await
}

/// What `GetOTelEnrichment` reports for the account.
async fn get(router: &MultiServiceRouter) -> Value {
    ok(router, "GetOTelEnrichment", &json!({})).await
}

#[tokio::test]
async fn an_account_with_no_enrichment_configuration_is_reported_as_stopped() {
    let (router, engine) = build_router_with_engine();

    let got = get(&router).await;
    assert_eq!(
        text(&got, "Status"),
        Some("Stopped".to_string()),
        "an account enrichment was never started for must report Stopped, got {got}"
    );
    // Nothing has ever been stored for the account, so there is no configuration to carry a
    // start instant, an "last stored" instant or filters.
    for member in ["CreatedAt", "UpdatedAt", "IncludeFilters", "ExcludeFilters"] {
        assert!(
            got.get(member).is_none(),
            "an unconfigured account must omit {member}, got {got}"
        );
    }

    // The report is a read: it must not create the configuration it did not find, or a later
    // caller would see one nobody stored.
    let again = get(&router).await;
    assert_eq!(
        again, got,
        "a second GetOTelEnrichment must report the same status, got {again}"
    );
    let state =
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state::service_state(
            &*engine,
        )
        .expect("the service store is reachable from the shared engine");
    let stored = state.lock().otel_enrichment.clone();
    assert!(
        stored.is_none(),
        "reading the enrichment status must store nothing, got {stored:?}"
    );
}

#[tokio::test]
async fn a_started_configuration_round_trips_through_get() {
    // Rows differ in which filter lists the start carries, how many selectors each holds, and
    // whether a selector names metric names — the whole of what a stored configuration is.
    // The last row is the published bound: "A maximum of 100 filters is allowed across
    // IncludeFilters and ExcludeFilters combined." (`OTelEnrichmentMetricSelector`)
    let rows: Vec<(&str, Value)> = vec![
        ("no filters at all", json!({})),
        (
            "an empty include list",
            json!({"IncludeFilters": [], "ExcludeFilters": []}),
        ),
        (
            "one namespace, every metric in it",
            json!({"IncludeFilters": [{"Namespace": "AWS/EC2"}]}),
        ),
        (
            "one namespace, named metrics",
            json!({"IncludeFilters": [
                {"Namespace": "AWS/EC2", "MetricNames": ["CPUUtilization", "NetworkIn"]},
            ]}),
        ),
        (
            "exclude filters only",
            json!({"ExcludeFilters": [{"Namespace": "AWS/Lambda"}]}),
        ),
        (
            "both lists, several namespaces",
            json!({
                "IncludeFilters": [
                    {"Namespace": "AWS/EC2"},
                    {"Namespace": "AWS/RDS", "MetricNames": ["ReadIOPS"]},
                ],
                "ExcludeFilters": [
                    {"Namespace": "AWS/EC2", "MetricNames": ["NetworkOut"]},
                ],
            }),
        ),
        (
            "the 100 filters allowed across both lists",
            json!({
                "IncludeFilters": selectors("include", 60),
                "ExcludeFilters": selectors("exclude", 40),
            }),
        ),
    ];

    for (case, request) in rows {
        let router = build_router();
        let started = start(&router, &request).await;

        for member in ["IncludeFilters", "ExcludeFilters"] {
            assert_eq!(
                started.get(member).cloned(),
                expected_filters(&request, member),
                "case {case:?}: StartOTelEnrichment must report the filters it stored for \
                 {member}, got {started}"
            );
        }
        let created = stamp(&started, "CreatedAt");
        let updated = stamp(&started, "UpdatedAt");
        assert!(
            created.is_some() && updated == created,
            "case {case:?}: a start stores the configuration at the instant enrichment \
             started, got CreatedAt {created:?} and UpdatedAt {updated:?} in {started}"
        );

        let got = get(&router).await;
        assert_eq!(
            text(&got, "Status"),
            Some("Running".to_string()),
            "case {case:?}: enrichment must be reported Running after a start, got {got}"
        );
        for member in ["IncludeFilters", "ExcludeFilters"] {
            assert_eq!(
                got.get(member).cloned(),
                expected_filters(&request, member),
                "case {case:?}: GetOTelEnrichment must report the stored {member}, got {got}"
            );
        }
        assert_eq!(
            (stamp(&got, "CreatedAt"), stamp(&got, "UpdatedAt")),
            (created, updated),
            "case {case:?}: the reported instants must be the ones the start stored, got {got}"
        );

        // Reading twice changes nothing a later caller sees.
        let again = get(&router).await;
        assert_eq!(
            again, got,
            "case {case:?}: a second GetOTelEnrichment must report the same configuration, \
             got {again}"
        );
    }
}

#[tokio::test]
async fn a_repeat_start_leaves_the_stored_configuration_alone() {
    // "Calling StartOTelEnrichment for an account where enrichment is already running has no
    // effect and does not modify the filters that are applied." Rows differ in what the second
    // start asks for: other filters, no filters at all, and filters on the other list.
    let rows: Vec<(&str, Value)> = vec![
        ("other include filters", json!({"IncludeFilters": [{"Namespace": "Custom/Second"}]})),
        ("no filters at all", json!({})),
        ("exclude filters", json!({"ExcludeFilters": [{"Namespace": "Custom/Third"}]})),
    ];

    for (case, repeat) in rows {
        let router = build_router();
        let first = json!({
            "IncludeFilters": [{"Namespace": "Custom/First", "MetricNames": ["Latency"]}],
            "ExcludeFilters": [{"Namespace": "Custom/First", "MetricNames": ["Errors"]}],
        });
        let started = start(&router, &first).await;

        let repeated = start(&router, &repeat).await;
        assert_eq!(
            repeated, started,
            "case {case:?}: a repeat start must report the configuration the first one \
             stored, got {repeated}"
        );
        let got = get(&router).await;
        for member in ["IncludeFilters", "ExcludeFilters"] {
            assert_eq!(
                got.get(member).cloned(),
                expected_filters(&first, member),
                "case {case:?}: a repeat start must not modify {member}, got {got}"
            );
        }
        assert_eq!(
            stamp(&got, "CreatedAt"),
            stamp(&started, "CreatedAt"),
            "case {case:?}: a repeat start must not move CreatedAt, got {got}"
        );
    }
}

#[tokio::test]
async fn an_update_replaces_the_filters_as_a_pair() {
    // "The filters in the request completely replace the stored filters; they are not merged
    // with them. IncludeFilters and ExcludeFilters are replaced as a pair, so a request that
    // specifies only IncludeFilters also clears the stored ExcludeFilters, and a request that
    // specifies neither clears both." Each row is an update of a different shape over the same
    // starting pair.
    let rows: Vec<(&str, Value)> = vec![
        (
            "both lists replaced",
            json!({
                "IncludeFilters": [{"Namespace": "Custom/NewIn"}],
                "ExcludeFilters": [{"Namespace": "Custom/NewOut", "MetricNames": ["Faults"]}],
            }),
        ),
        (
            "only the include list, which clears the exclude list",
            json!({"IncludeFilters": [{"Namespace": "Custom/NewIn"}]}),
        ),
        (
            "only the exclude list, which clears the include list",
            json!({"ExcludeFilters": [{"Namespace": "Custom/NewOut"}]}),
        ),
        ("neither list, which clears both", json!({})),
        (
            "empty lists, which clear both",
            json!({"IncludeFilters": [], "ExcludeFilters": []}),
        ),
        (
            "the 100 filters allowed across both lists",
            json!({
                "IncludeFilters": selectors("in", 1),
                "ExcludeFilters": selectors("out", 99),
            }),
        ),
    ];

    for (case, update) in rows {
        let router = build_router();
        let started = start(
            &router,
            &json!({
                "IncludeFilters": [{"Namespace": "Custom/Start"}],
                "ExcludeFilters": [{"Namespace": "Custom/Start", "MetricNames": ["Ignored"]}],
            }),
        )
        .await;

        let updated = ok(&router, "UpdateOTelEnrichment", &update).await;
        for member in ["IncludeFilters", "ExcludeFilters"] {
            assert_eq!(
                updated.get(member).cloned(),
                expected_filters(&update, member),
                "case {case:?}: an update must report the replacing {member}, got {updated}"
            );
        }
        // The enrichment itself is untouched by a filter replacement, so the instant it
        // started survives and only the "last stored" instant moves.
        assert_eq!(
            stamp(&updated, "CreatedAt"),
            stamp(&started, "CreatedAt"),
            "case {case:?}: an update must not move CreatedAt, got {updated}"
        );
        assert!(
            stamp(&updated, "UpdatedAt") >= stamp(&started, "UpdatedAt"),
            "case {case:?}: UpdatedAt must not move backwards, got {updated}"
        );

        let got = get(&router).await;
        assert_eq!(
            text(&got, "Status"),
            Some("Running".to_string()),
            "case {case:?}: an update must leave enrichment running, got {got}"
        );
        for member in ["IncludeFilters", "ExcludeFilters", "CreatedAt", "UpdatedAt"] {
            assert_eq!(
                got.get(member).cloned(),
                updated.get(member).cloned(),
                "case {case:?}: GetOTelEnrichment must report what the update stored for \
                 {member}, got {got}"
            );
        }
    }
}

#[tokio::test]
async fn an_update_without_running_enrichment_is_not_found() {
    // "Enrichment must already be running for the account. If it is not, this operation
    // returns a ResourceNotFoundException." Rows are the two ways enrichment is not running:
    // it was never started, and it was started and then stopped.
    let rows: Vec<(&str, bool)> = vec![("never started", false), ("started then stopped", true)];

    for (case, started_then_stopped) in rows {
        let router = build_router();
        if started_then_stopped {
            start(&router, &json!({"IncludeFilters": [{"Namespace": "Custom/Gone"}]})).await;
            ok(&router, "StopOTelEnrichment", &json!({})).await;
        }

        let (status, body) = call(
            &router,
            "UpdateOTelEnrichment",
            &json!({"IncludeFilters": [{"Namespace": "Custom/Late"}]}),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );

        // A refused update stores nothing, so the account is still not running.
        let got = get(&router).await;
        assert_eq!(
            text(&got, "Status"),
            Some("Stopped".to_string()),
            "case {case:?}: a refused update must leave enrichment stopped, got {got}"
        );
        assert!(
            got.get("IncludeFilters").is_none(),
            "case {case:?}: a refused update must store no filters, got {got}"
        );
    }
}

#[tokio::test]
async fn a_stop_leaves_the_account_stopped_whatever_was_running() {
    // Rows differ in the configuration the stop ends: the filters a stopped account reports
    // are omitted whichever lists were stored.
    let rows: Vec<(&str, Value)> = vec![
        ("no filters", json!({})),
        (
            "include filters",
            json!({"IncludeFilters": [{"Namespace": "Custom/Stopped"}]}),
        ),
        (
            "both lists",
            json!({
                "IncludeFilters": [{"Namespace": "Custom/A"}],
                "ExcludeFilters": [{"Namespace": "Custom/B", "MetricNames": ["Drop"]}],
            }),
        ),
    ];

    for (case, request) in rows {
        let router = build_router();
        let started = start(&router, &request).await;
        ok(&router, "StopOTelEnrichment", &json!({})).await;

        let got = get(&router).await;
        assert_eq!(
            text(&got, "Status"),
            Some("Stopped".to_string()),
            "case {case:?}: enrichment must be reported Stopped after a stop, got {got}"
        );
        // While enrichment is stopped the filters and the start instant are omitted; the stop
        // itself is the configuration last stored for the account, so UpdatedAt is not.
        for member in ["CreatedAt", "IncludeFilters", "ExcludeFilters"] {
            assert!(
                got.get(member).is_none(),
                "case {case:?}: a stopped account must omit {member}, got {got}"
            );
        }
        assert!(
            stamp(&got, "UpdatedAt") >= stamp(&started, "UpdatedAt"),
            "case {case:?}: a stop is stored, so UpdatedAt must not move backwards, got {got}"
        );

        // A second stop is the same answer, and a start afterwards is a fresh configuration:
        // the filters the stopped one carried are gone, so the new request's filters are the
        // ones in force.
        ok(&router, "StopOTelEnrichment", &json!({})).await;
        let restarted = start(&router, &json!({"ExcludeFilters": [{"Namespace": "Custom/New"}]}))
            .await;
        assert_eq!(
            restarted.get("ExcludeFilters").cloned(),
            Some(json!([{"Namespace": "Custom/New"}])),
            "case {case:?}: a start after a stop must store the new exclude filters, got \
             {restarted}"
        );
        assert!(
            restarted.get("IncludeFilters").is_none(),
            "case {case:?}: a start after a stop must not revive the old include filters, got \
             {restarted}"
        );
        assert!(
            stamp(&restarted, "CreatedAt") >= stamp(&started, "CreatedAt"),
            "case {case:?}: a start after a stop begins a new enrichment, so CreatedAt must \
             not move backwards, got {restarted}"
        );
    }
}

#[tokio::test]
async fn more_than_the_allowed_filters_across_both_lists_is_refused() {
    // The bound is published on `OTelEnrichmentMetricSelector`: "A maximum of 100 filters is
    // allowed across IncludeFilters and ExcludeFilters combined." Each list is modelled at
    // 0..100 on its own, so every row below satisfies the modelled constraints and only the
    // combined bound decides. The bound itself (100) is accepted, one past it is refused.
    let rows: Vec<(&str, usize, usize, u16)> = vec![
        ("the bound, split across both lists", 60, 40, 200),
        ("the bound, on one list", 100, 0, 200),
        ("one past the bound", 100, 1, 400),
        ("one past the bound, evenly split", 51, 50, 400),
        ("both lists at their own maximum", 100, 100, 400),
    ];

    for (case, include, exclude, expected) in rows {
        for operation in ["StartOTelEnrichment", "UpdateOTelEnrichment"] {
            let case = format!("{operation}: {case}");
            let router = build_router();
            // `UpdateOTelEnrichment` needs enrichment running before the filters it carries
            // are judged at all.
            if operation == "UpdateOTelEnrichment" {
                start(&router, &json!({})).await;
            }
            let request = json!({
                "IncludeFilters": selectors("in", include),
                "ExcludeFilters": selectors("out", exclude),
            });

            let (status, body) = call(&router, operation, &request).await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected HTTP {expected} for {include} include and {exclude} \
                 exclude filters, got {status} with body {body}"
            );
            if expected == 400 {
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected ValidationException, got {body}"
                );
                // A refused request stores nothing.
                let got = get(&router).await;
                assert!(
                    got.get("IncludeFilters").is_none()
                        && got.get("ExcludeFilters").is_none(),
                    "case {case:?}: a refused request must store no filters, got {got}"
                );
            }
        }
    }
}
