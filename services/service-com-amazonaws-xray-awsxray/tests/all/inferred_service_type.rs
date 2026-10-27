// SPDX-License-Identifier: CC0-1.0

//! How a service X-Ray INFERRED from a call out of an instrumented service is
//! TYPED wherever its identity is reported.
//!
//! The rule: a service identity reports, as its type, the type of the node it
//! names — for a service that submitted segments, the `origin` those documents
//! carried; for a service this trace only CALLED, the type the calling entity's
//! `namespace` names (`Service$Type`'s "a downstream service of indeterminate
//! type" for a `remote` call), because a service that reported nothing carries
//! no `origin` of its own. That is what lets a reader tell an external
//! dependency from a service that reports for itself.
//!
//! The first test is a table over that class: the rows differ in whether the
//! calling segment names an `origin`, whether its subsegments left the process
//! at all, how many downstream calls there are and how deeply the call sits in
//! the segment's subsegment tree. The second pins the SAME derivation on the
//! other surfaces that report a service identity: a trace summary's root-cause
//! services, its `EntryPoint`, and a `service(name, type)` filter expression.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::xray_wire::{array, root_document, trace_id, Xray};

/// The window every seeded trace below falls inside.
const WINDOW: (f64, f64) = (1_000.0, 2_000.0);

/// The service that submits the segments, and so reports for itself.
const CALLER: &str = "frontend";

/// An `origin` a reporting service's document carries, which is the type such a
/// service is reported with.
const CALLER_ORIGIN: &str = "AWS::EC2::Instance";

/// The type of a node inferred from a `namespace: "remote"` call.
const REMOTE: &str = "remote";

/// One subsegment of the calling segment: `namespace` absent is work inside the
/// caller, `Some("remote")` is a call out to a downstream service.
fn subsegment(id: &str, name: &str, namespace: Option<&str>, children: Value) -> Value {
    let mut entity = json!({
        "id": id,
        "name": name,
        "start_time": 1_000.1_f64,
        "end_time": 1_019.1_f64,
    });
    if let Some(namespace) = namespace {
        entity["namespace"] = json!(namespace);
    }
    if !children.as_array().is_some_and(Vec::is_empty) {
        entity["subsegments"] = children;
    }
    entity
}

/// The calling service's root segment document, carrying `subsegments`.
fn caller_document(trace: &str, origin: Option<&str>, subsegments: Value) -> Value {
    let mut document = root_document("1111111111111111", trace, CALLER, 1_000.0, Some(1_021.0));
    if let Some(origin) = origin {
        document["origin"] = json!(origin);
    }
    document["subsegments"] = subsegments;
    document
}

/// `(Name, Type)` of every entry of a summary's `ServiceIds`, an absent `Type`
/// read as the empty string so a missing member is a visible difference.
fn reported_services(summary: &Value) -> BTreeSet<(String, String)> {
    array(summary, "ServiceIds")
        .iter()
        .map(|id| {
            (
                id.get("Name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                id.get("Type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
            )
        })
        .collect()
}

/// One row of the table below: the case, the `origin` the calling segment
/// reports, the subsegments it carries, and the `(Name, Type)` pairs the
/// summary's `ServiceIds` must list — `""` standing for a `Type` the response
/// omits.
type Row = (
    &'static str,
    Option<&'static str>,
    Value,
    Vec<(&'static str, &'static str)>,
);

#[tokio::test]
async fn trace_summary_service_ids_type_each_service_by_what_reported_it() {
    let rows: Vec<Row> = vec![
        (
            "in-process work only, caller reporting an origin",
            Some(CALLER_ORIGIN),
            json!([subsegment("2222222222222222", "InternalPipeline", None, json!([]))]),
            vec![(CALLER, CALLER_ORIGIN)],
        ),
        (
            "in-process work only, caller reporting no origin",
            None,
            json!([subsegment("2222222222222222", "InternalPipeline", None, json!([]))]),
            vec![(CALLER, "")],
        ),
        (
            "one downstream call beside in-process work, caller reporting an origin",
            Some(CALLER_ORIGIN),
            json!([
                subsegment("2222222222222222", "InternalPipeline", None, json!([])),
                subsegment("3333333333333333", "QuartzAPI", Some(REMOTE), json!([])),
            ]),
            vec![(CALLER, CALLER_ORIGIN), ("QuartzAPI", REMOTE)],
        ),
        (
            "one downstream call, caller reporting no origin",
            None,
            json!([subsegment("3333333333333333", "QuartzAPI", Some(REMOTE), json!([]))]),
            vec![(CALLER, ""), ("QuartzAPI", REMOTE)],
        ),
        (
            "two downstream calls, each its own service",
            Some(CALLER_ORIGIN),
            json!([
                subsegment("3333333333333333", "QuartzAPI", Some(REMOTE), json!([])),
                subsegment("4444444444444444", "basalt.example.com", Some(REMOTE), json!([])),
            ]),
            vec![
                (CALLER, CALLER_ORIGIN),
                ("QuartzAPI", REMOTE),
                ("basalt.example.com", REMOTE),
            ],
        ),
        (
            "a downstream call nested two levels inside the segment",
            Some(CALLER_ORIGIN),
            json!([subsegment(
                "2222222222222222",
                "InternalPipeline",
                None,
                json!([subsegment(
                    "5555555555555555",
                    "InnerStage",
                    None,
                    json!([subsegment("3333333333333333", "QuartzAPI", Some(REMOTE), json!([]))]),
                )]),
            )]),
            vec![(CALLER, CALLER_ORIGIN), ("QuartzAPI", REMOTE)],
        ),
    ];

    for (index, (case, origin, subsegments, expected)) in rows.into_iter().enumerate() {
        let xray = Xray::new();
        let nth = u32::try_from(index).expect("the row count fits a u32") + 1;
        let trace = trace_id(nth);
        let document = caller_document(&trace, origin, subsegments);

        let put = xray.put_segments(&[document.to_string()]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: seeding must succeed, got {put:?}"
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "case {case:?}: the document must be accepted, got {put:?}"
        );

        let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
        let summaries = array(&got.body, "TraceSummaries");
        assert_eq!(
            summaries.len(),
            1,
            "case {case:?}: one trace was stored, got {got:?}"
        );
        let actual = reported_services(&summaries[0]);
        let expected: BTreeSet<(String, String)> = expected
            .into_iter()
            .map(|(name, kind)| (name.to_string(), kind.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "case {case:?}: expected ServiceIds {expected:?}, got {actual:?}"
        );
    }
}

#[tokio::test]
async fn every_surface_reports_an_inferred_service_with_the_same_type() {
    let xray = Xray::new();
    let trace = trace_id(64);
    let mut downstream = subsegment("3333333333333333", "QuartzAPI", Some(REMOTE), json!([]));
    downstream["fault"] = json!(true);
    downstream["cause"] = json!({
        "exceptions": [{"type": "RemoteServiceException", "message": "downstream failed"}],
    });
    let document = caller_document(&trace, Some(CALLER_ORIGIN), json!([downstream]));

    let put = xray.put_segments(&[document.to_string()]).await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
    let summaries = array(&got.body, "TraceSummaries");
    assert_eq!(summaries.len(), 1, "one trace was stored, got {got:?}");
    let summary = &summaries[0];

    // The summary's own service list.
    let listed = reported_services(summary);
    let expected: BTreeSet<(String, String)> = [
        (CALLER.to_string(), CALLER_ORIGIN.to_string()),
        ("QuartzAPI".to_string(), REMOTE.to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        listed, expected,
        "case \"ServiceIds\": expected {expected:?}, got {listed:?}"
    );

    // The root cause derived from the same stored segments.
    let causes = array(summary, "FaultRootCauses");
    assert_eq!(
        causes.len(),
        1,
        "case \"FaultRootCauses\": the downstream fault is one root cause, got {causes:?}"
    );
    let identities: Vec<(Option<&str>, Option<&str>, Option<bool>)> = array(&causes[0], "Services")
        .iter()
        .map(|service| {
            (
                service.get("Name").and_then(Value::as_str),
                service.get("Type").and_then(Value::as_str),
                service.get("Inferred").and_then(Value::as_bool),
            )
        })
        .collect();
    assert_eq!(
        identities,
        vec![
            (Some(CALLER), Some(CALLER_ORIGIN), Some(false)),
            (Some("QuartzAPI"), Some(REMOTE), Some(true)),
        ],
        "case \"FaultRootCauses.Services\": the inferred service is typed by the call, \
         the reporting one by its origin, got {identities:?}"
    );

    // The entry point is the service that reported the root segment, so it
    // keeps that document's own origin.
    assert_eq!(
        summary
            .get("EntryPoint")
            .and_then(|entry| entry.get("Type"))
            .and_then(Value::as_str),
        Some(CALLER_ORIGIN),
        "case \"EntryPoint\": the root is a reporting service, got {:?}",
        summary.get("EntryPoint")
    );

    // And a filter expression selects a service by that same type.
    let selections = [
        (
            "the inferred service by the type the call gave it",
            format!("service(\"QuartzAPI\", \"{REMOTE}\")"),
            true,
        ),
        (
            "the inferred service under the caller's origin",
            format!("service(\"QuartzAPI\", \"{CALLER_ORIGIN}\")"),
            false,
        ),
        (
            "the reporting service by its origin",
            format!("service(\"{CALLER}\", \"{CALLER_ORIGIN}\")"),
            true,
        ),
        (
            "the reporting service as though the trace had inferred it",
            format!("service(\"{CALLER}\", \"{REMOTE}\")"),
            false,
        ),
    ];
    for (case, expression, selects) in selections {
        let answer = xray.filtered(WINDOW.0, WINDOW.1, &expression).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: the expression must be readable, got {answer:?}"
        );
        let found = array(&answer.body, "TraceSummaries").len();
        assert_eq!(
            found == 1,
            selects,
            "case {case:?}: expected {expression} to select {}, got {found} summaries",
            if selects { "the trace" } else { "nothing" }
        );
    }
}
