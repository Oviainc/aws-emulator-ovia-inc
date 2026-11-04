// SPDX-License-Identifier: CC0-1.0

//! A time member whose value no instant can be read from.
//!
//! The rule asserted here: a value bound to a TIMESTAMP member that this
//! service reads and cannot convert to an instant never becomes a value at all,
//! so it is refused at the deserialization layer — HTTP 400, the protocol's own
//! error code in the `x-amzn-errortype` header every SDK resolves its error
//! type from, and the reason under the capital `Message` member the service
//! states a protocol-level refusal in — naming the CLASS of value that was sent
//! and the FORM an instant is read in, never the value itself. It is not a
//! constraint violation: the value was never read, so no rule the model declares
//! was reached.
//!
//! The rows differ along what the rule generalises over: which operation carries
//! the member, whether the member is top-level or nested inside a structure in a
//! list, and the class of value sent (a string, an object, an array, a boolean).
//! A number is the form the protocol carries an instant in, so the last case
//! holds the accepted side of the same members.

use serde_json::{json, Value};

use super::xray_wire::{Answer, Xray};

/// The code a value that never became a value is refused with — the PROTOCOL's,
/// because no operation ran to answer with one of its own.
const SERIALIZATION: &str = "SerializationException";

/// The fixed tail of the refusal: what a time member's value could not be read
/// into, stated as the unit an instant is counted in.
const UNCONVERTIBLE: &str = " can not be converted to milliseconds since epoch";

/// One request carrying a time member that cannot become an instant.
struct Case {
    label: &'static str,
    path: &'static str,
    body: Value,
    /// The class of value the request sent, which the refusal names.
    class: &'static str,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "an ISO-8601 instant where the trace query takes a StartTime",
            path: "/TraceSummaries",
            body: json!({"StartTime": "2024-05-01T00:00:00Z", "EndTime": 1_700_000_000.0}),
            class: "STRING_VALUE",
        },
        Case {
            label: "prose where the trace query takes an EndTime",
            path: "/TraceSummaries",
            body: json!({"StartTime": 1_700_000_000.0, "EndTime": "yesterday"}),
            class: "STRING_VALUE",
        },
        Case {
            label: "prose where the service map takes a StartTime",
            path: "/ServiceGraph",
            body: json!({"StartTime": "an hour ago", "EndTime": 1_700_000_000.0}),
            class: "STRING_VALUE",
        },
        Case {
            label: "a number written with separators where the statistics read takes an EndTime",
            path: "/TimeSeriesServiceStatistics",
            body: json!({"StartTime": 1_700_000_000.0, "EndTime": "1,700,000,060"}),
            class: "STRING_VALUE",
        },
        Case {
            label: "the empty string where a retrieval takes a StartTime",
            path: "/StartTraceRetrieval",
            body: json!({
                "TraceIds": ["1-00000001-000000000000000000000001"],
                "StartTime": "",
                "EndTime": 1_700_000_000.0,
            }),
            class: "STRING_VALUE",
        },
        Case {
            label: "prose nested inside a telemetry record",
            path: "/TelemetryRecords",
            body: json!({"TelemetryRecords": [{"Timestamp": "half past four"}]}),
            class: "STRING_VALUE",
        },
        Case {
            label: "an object where the trace query takes a StartTime",
            path: "/TraceSummaries",
            body: json!({"StartTime": {"seconds": 1_700_000_000}, "EndTime": 1_700_000_000.0}),
            class: "OBJECT_VALUE",
        },
        Case {
            label: "an array where the trace query takes a StartTime",
            path: "/TraceSummaries",
            body: json!({"StartTime": [1_700_000_000.0], "EndTime": 1_700_000_000.0}),
            class: "ARRAY_VALUE",
        },
        Case {
            label: "a boolean where the trace query takes a StartTime",
            path: "/TraceSummaries",
            body: json!({"StartTime": true, "EndTime": 1_700_000_000.0}),
            class: "BOOLEAN_VALUE",
        },
    ]
}

/// The refusal message of an answer, whichever casing rendered it.
fn said(answer: &Answer) -> &str {
    answer.message().unwrap_or_default()
}

#[tokio::test]
async fn a_value_no_instant_can_be_read_from_names_its_class_and_the_form_it_missed() {
    let xray = Xray::new();

    for case in cases() {
        let answered = xray.call(case.path, &case.body).await;
        let label = case.label;
        assert_eq!(
            (answered.status, answered.error_type.as_deref()),
            (400, Some(SERIALIZATION)),
            "case {label:?}: expected 400 {SERIALIZATION} named in the header every SDK reads \
             the error shape from, got {answered:?}"
        );
        // A value of a class the member admits no instant from is refused as the
        // epoch FORM it missed; one of a class no member can be read from at all
        // is refused as the modelled type. Both name the class and never the
        // value.
        let stated = said(&answered);
        assert!(
            stated.starts_with(case.class),
            "case {label:?}: the refusal must name the class of value sent ({}), got {stated:?}",
            case.class
        );
        assert!(
            stated.ends_with(UNCONVERTIBLE) || stated.ends_with(" can not be converted to a Timestamp"),
            "case {label:?}: the refusal must name what the value could not be read into, got \
             {stated:?}"
        );
        assert!(
            !stated.contains("2024-05-01")
                && !stated.contains("yesterday")
                && !stated.contains("half past four"),
            "case {label:?}: the refusal names the class and never the value, got {stated:?}"
        );
    }
}

#[tokio::test]
async fn text_that_is_not_an_instant_is_refused_as_the_epoch_form_it_missed() {
    let xray = Xray::new();

    // The class every unconvertible TEXT value carries, and the one form it
    // could not be read into — asserted exactly, since this is the sentence a
    // client (or a log) reads back.
    let answered = xray
        .call(
            "/TraceSummaries",
            &json!({"StartTime": "2024-05-01T00:00:00Z", "EndTime": 1_700_000_000.0}),
        )
        .await;
    assert_eq!(
        said(&answered),
        "STRING_VALUE can not be converted to milliseconds since epoch",
        "expected the recorded sentence for a time member no instant can be read from, got \
         {answered:?}"
    );
    // Under the member the service states a protocol-level refusal in, so a
    // client reading `Message` sees the explanation.
    assert!(
        answered.get("Message").as_str().is_some(),
        "the explanation must be stated under Message, got {answered:?}"
    );
    assert!(
        answered.get("message").is_null(),
        "the refusal renders no second, lowercase message, got {answered:?}"
    );
}

#[tokio::test]
async fn an_instant_the_service_can_convert_is_answered() {
    let xray = Xray::new();

    // The same members, carrying values that ARE instants: epoch seconds whole
    // and fractional, and the epoch itself. None is refused, so the rule above
    // is about the value and not about the member.
    let end = 1_700_000_000.0;
    for (label, start, end) in [
        ("whole epoch seconds", json!(end - 60.0), json!(end)),
        ("fractional epoch seconds", json!(end - 0.5), json!(end)),
        ("the epoch itself", json!(0.0), json!(60.0)),
    ] {
        let answered = xray
            .call("/TraceSummaries", &json!({"StartTime": start, "EndTime": end}))
            .await;
        assert_eq!(
            answered.status, 200,
            "case {label:?}: a value that is an instant must be answered, got {answered:?}"
        );
    }
}
