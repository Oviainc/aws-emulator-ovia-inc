// SPDX-License-Identifier: CC0-1.0

//! Which entities of a trace are ATTRIBUTED a fault or error root cause.
//!
//! One rule, checked over a table of submitted documents: a trace reports a
//! fault or error root cause only for an entity that BOTH recorded the outcome
//! and named, in its own `cause` block, the exception the outcome is attributed
//! to — so an entity that declared a flag and submitted no exception is reported
//! in no root cause list, and the lists stay empty.
//!
//! `RootCauseException` is "the exception associated with a root cause" and
//! `ErrorRootCauseEntity$Exceptions` "the types and messages of the exceptions"
//! (`smithy_query(query="shape",shape="ErrorRootCauseEntity,RootCauseException")`),
//! so there is nothing to report for an outcome no document named an exception
//! for. The rows differ along what the rule generalises over: which class the
//! outcome was, where in the submitted document the flagged entity sits, and
//! which of the forms a `cause` member takes it carried.

use serde_json::{json, Value};

use crate::xray_wire::{array, root_document, trace_id, Xray};

/// The window every seeded trace below falls inside.
const WINDOW: (f64, f64) = (1_000.0, 2_000.0);

/// A `cause` block naming each `(type, message)` as one exception.
fn cause_block(exceptions: &[(&str, &str)]) -> Value {
    let exceptions: Vec<Value> = exceptions
        .iter()
        .map(|(exception_type, message)| {
            json!({"id": "1234567890abcdef", "type": exception_type, "message": message})
        })
        .collect();
    json!({"exceptions": exceptions})
}

/// One subsegment of the seeded root document.
fn subsegment(id: &str, name: &str, extra: Value) -> Value {
    let mut entity = json!({
        "id": id,
        "name": name,
        "start_time": 1_000.1_f64,
        "end_time": 1_000.4_f64,
    });
    for (member, value) in extra.as_object().expect("the extra members are an object") {
        entity[member.clone()] = value.clone();
    }
    entity
}

/// One attribution case.
struct Case {
    case: &'static str,
    /// The members merged into the seeded root segment document.
    recorded: Value,
    /// The expected `(FaultRootCauses, ErrorRootCauses)` lengths.
    causes: (usize, usize),
    /// The `EntityPath` frame names of the LAST service of whichever list is
    /// populated, deepest last; empty when neither is.
    frames: Vec<&'static str>,
    /// The exception names the deepest of those frames carries.
    exceptions: Vec<&'static str>,
}

#[tokio::test]
async fn a_root_cause_is_reported_only_where_a_cause_block_named_the_exception() {
    let rows: Vec<Case> = vec![
        Case {
            case: "an error flag and no cause member",
            recorded: json!({"error": true, "http": {"response": {"status": 400}}}),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            case: "a fault flag and no cause member",
            recorded: json!({"fault": true, "http": {"response": {"status": 500}}}),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            case: "a throttle flag and no cause member",
            recorded: json!({"throttle": true}),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            case: "an error flag whose cause block names an exception",
            recorded: json!({
                "error": true,
                "cause": cause_block(&[("ValidationError", "bad input")]),
            }),
            causes: (0, 1),
            frames: vec!["svc"],
            exceptions: vec!["ValidationError"],
        },
        Case {
            case: "a fault flag whose cause block names an exception",
            recorded: json!({
                "fault": true,
                "cause": cause_block(&[("java.lang.RuntimeException", "boom")]),
            }),
            causes: (1, 0),
            frames: vec!["svc"],
            exceptions: vec!["java.lang.RuntimeException"],
        },
        Case {
            case: "a fault flag whose cause block names two exceptions",
            recorded: json!({
                "fault": true,
                "cause": cause_block(&[("OuterError", "outer"), ("InnerError", "inner")]),
            }),
            causes: (1, 0),
            frames: vec!["svc"],
            exceptions: vec!["OuterError", "InnerError"],
        },
        Case {
            // A throttle is neither a fault nor an error, so the exception it
            // named lands in neither list.
            case: "a throttle flag whose cause block names an exception",
            recorded: json!({
                "throttle": true,
                "cause": cause_block(&[("ThrottlingException", "slow down")]),
            }),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            case: "an error flag whose cause block names no exception at all",
            recorded: json!({"error": true, "cause": json!({"exceptions": []})}),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            case: "an error flag whose cause block carries no exceptions member",
            recorded: json!({"error": true, "cause": json!({"working_directory": "/app"})}),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            // `cause` is either an object or a bare cause id, and only the
            // former names exceptions.
            case: "an error flag whose cause is a bare cause id",
            recorded: json!({"error": true, "cause": "1234567890abcdef"}),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            // Both halves are required: an exception without a declared outcome
            // is no root cause either.
            case: "a cause block naming an exception, with no outcome flag",
            recorded: json!({"cause": cause_block(&[("ValidationError", "bad input")])}),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            case: "a subsegment's fault, named, inside a segment that recorded none",
            recorded: json!({
                "subsegments": [subsegment(
                    "2222222222222222",
                    "inner",
                    json!({"fault": true, "cause": cause_block(&[("InnerException", "deep")])}),
                )],
            }),
            causes: (1, 0),
            frames: vec!["svc", "inner"],
            exceptions: vec!["InnerException"],
        },
        Case {
            case: "a subsegment's fault with no cause member, inside a segment that recorded none",
            recorded: json!({
                "subsegments": [subsegment("2222222222222222", "inner", json!({"fault": true}))],
            }),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
        Case {
            // The attribution lands on the entity that NAMED the exception, not
            // on the deepest one that merely declared the flag.
            case: "a nameless subsegment fault inside a segment whose own cause names one",
            recorded: json!({
                "fault": true,
                "cause": cause_block(&[("OuterException", "outer")]),
                "subsegments": [subsegment("2222222222222222", "inner", json!({"fault": true}))],
            }),
            causes: (1, 0),
            frames: vec!["svc"],
            exceptions: vec!["OuterException"],
        },
        Case {
            case: "a remote subsegment's fault, named, which infers the service it called",
            recorded: json!({
                "subsegments": [subsegment(
                    "2222222222222222",
                    "dep",
                    json!({
                        "namespace": "remote",
                        "fault": true,
                        "cause": cause_block(&[("RemoteServiceException", "downstream failed")]),
                    }),
                )],
            }),
            causes: (1, 0),
            frames: vec!["dep"],
            exceptions: vec!["RemoteServiceException"],
        },
        Case {
            case: "a remote subsegment's fault with no cause member",
            recorded: json!({
                "subsegments": [subsegment(
                    "2222222222222222",
                    "dep",
                    json!({"namespace": "remote", "fault": true}),
                )],
            }),
            causes: (0, 0),
            frames: vec![],
            exceptions: vec![],
        },
    ];

    for row in rows {
        let Case {
            case,
            recorded,
            causes: (fault, error),
            frames,
            exceptions,
        } = row;
        let xray = Xray::new();
        let trace = trace_id(41);
        let mut document =
            root_document("1111111111111111", &trace, "svc", 1_000.0, Some(1_000.5));
        for (member, value) in recorded.as_object().expect("the row is an object") {
            document[member.clone()] = value.clone();
        }
        let put = xray.put_segments(&[document.to_string()]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: seeding must succeed, got {put:?}"
        );
        assert!(
            array(&put.body, "UnprocessedTraceSegments").is_empty(),
            "case {case:?}: the document must be stored, got {put:?}"
        );

        let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
        let summaries = array(&got.body, "TraceSummaries");
        assert_eq!(summaries.len(), 1, "case {case:?}: one trace, got {got:?}");
        let summary = &summaries[0];
        for (member, expected) in [("FaultRootCauses", fault), ("ErrorRootCauses", error)] {
            assert_eq!(
                array(summary, member).len(),
                expected,
                "case {case:?}: expected {expected} {member}, got {:?}",
                summary.get(member)
            );
        }

        let member = if fault > 0 {
            "FaultRootCauses"
        } else if error > 0 {
            "ErrorRootCauses"
        } else {
            assert!(
                frames.is_empty() && exceptions.is_empty(),
                "case {case:?}: a row that reports no root cause expects no frames, \
                 got {frames:?} and {exceptions:?}"
            );
            continue;
        };
        let services = array(&array(summary, member)[0], "Services");
        let attributed = services
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: a root cause names its services"));
        let path = array(attributed, "EntityPath");
        let names: Vec<&str> = path
            .iter()
            .filter_map(|entity| entity.get("Name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            names, frames,
            "case {case:?}: expected the frames {frames:?}, got {names:?}"
        );
        let deepest = path
            .last()
            .unwrap_or_else(|| panic!("case {case:?}: the service names its frames"));
        let named: Vec<&str> = array(deepest, "Exceptions")
            .iter()
            .filter_map(|exception| exception.get("Name").and_then(Value::as_str))
            .collect();
        assert_eq!(
            named, exceptions,
            "case {case:?}: expected the exceptions {exceptions:?}, got {named:?}"
        );
    }
}

#[tokio::test]
async fn only_the_entity_that_named_an_exception_is_attributed_the_trace_s_error() {
    // Three services of one trace all recorded an error; one named the
    // exception. The trace is attributed one root cause, that service's, and the
    // `rootcause.error.*` keywords select on exactly that cause.
    let xray = Xray::new();
    let trace = trace_id(42);
    let mut documents =
        vec![root_document("1111111111111111", &trace, "frontend", 1_000.0, Some(1_000.9))
            .to_string()];
    for (index, (name, names_exception)) in [
        ("attributed", true),
        ("silent-a", false),
        ("silent-b", false),
    ]
    .into_iter()
    .enumerate()
    {
        let mut child = root_document(
            &format!("{:016x}", index + 2),
            &trace,
            name,
            1_000.1,
            Some(1_000.8),
        );
        child["parent_id"] = json!("1111111111111111");
        child["error"] = json!(true);
        if names_exception {
            child["cause"] = cause_block(&[("ValidationError", "bad input")]);
        }
        documents.push(child.to_string());
    }
    let put = xray.put_segments(&documents).await;
    assert_eq!(put.status, 200, "seeding must succeed, got {put:?}");

    let got = xray.summaries(WINDOW.0, WINDOW.1, None).await;
    let summaries = array(&got.body, "TraceSummaries");
    assert_eq!(summaries.len(), 1, "one trace was stored, got {got:?}");
    let causes = array(&summaries[0], "ErrorRootCauses");
    assert_eq!(
        causes.len(),
        1,
        "only the service that named an exception is attributed, got {causes:?}"
    );
    let services = array(&causes[0], "Services");
    let names: Vec<&str> = services
        .iter()
        .filter_map(|service| service.get("Name").and_then(Value::as_str))
        .collect();
    assert_eq!(
        names,
        vec!["frontend", "attributed"],
        "the cause runs from the trace's root to the service it is attributed to, got {names:?}"
    );
    assert_eq!(
        causes[0].get("ClientImpacting").and_then(Value::as_bool),
        Some(false),
        "the trace's root recorded no error, so the client did not see it, got {:?}",
        causes[0]
    );

    // The filter language reads the SAME derivation, so it selects on the cause
    // that exists and on nothing else.
    for (expression, selects) in [
        ("rootcause.error.entity.name = \"attributed\"", 1),
        ("rootcause.error.exception.name = \"ValidationError\"", 1),
        ("rootcause.error.entity.name = \"silent-a\"", 0),
        ("rootcause.error.entity.name = \"silent-b\"", 0),
        ("rootcause.fault.entity.name = \"attributed\"", 0),
    ] {
        let filtered = xray.filtered(WINDOW.0, WINDOW.1, expression).await;
        assert_eq!(
            (filtered.status, array(&filtered.body, "TraceSummaries").len()),
            (200, selects),
            "case {expression:?}: expected {selects} selected traces, got {filtered:?}"
        );
    }
}
