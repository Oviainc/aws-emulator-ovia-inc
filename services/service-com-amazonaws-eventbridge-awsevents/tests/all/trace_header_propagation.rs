// SPDX-License-Identifier: CC0-1.0

//! The X-Ray TRACE HEADER a publication carries travels with the event to the
//! targets it reaches, end to end over the wire through the real
//! `MultiServiceRouter` with SQS mounted beside EventBridge — so the header is
//! read back where a consumer reads it: as a message system attribute on the
//! queue.
//!
//! The RULE asserted here: `PutEventsRequestEntry$TraceHeader` is "an X-Ray trace
//! header, which is an http header (`X-Amzn-Trace-Id`) that contains the trace-id
//! associated with the event", so an event published with one reaches an SQS
//! queue target with that text as the queue's `AWSTraceHeader` system attribute
//! ("currently, the only supported message system attribute is
//! `AWSTraceHeader`", `SendMessage$MessageSystemAttributes`). Text that names no
//! trace is not a trace header: the entry is published all the same — the header
//! says nothing about whether the event is publishable — and the delivery carries
//! no such attribute, rather than carrying text no consumer can read as a trace.
//! The header is a property of the PUBLICATION, not of the event, so it never
//! appears in the delivered envelope either.
//!
//! The rows differ along what the entry carries: no header, the documented
//! tracing-header forms, and every shape of text that is not one — a field with
//! no `=`, a `Root` holding something that is not a trace id, a trace id of the
//! wrong length, and one whose digits are not hexadecimal. A batch row asserts
//! that the attribute follows the ENTRY it was published with.

use serde_json::{json, Value};

use crate::target_dispatch::{emulator, events, given_queue, given_rule, DETAIL_TYPE, SOURCE};

/// The system attribute name Amazon SQS publishes for a trace header. Source:
/// `SendMessage$MessageSystemAttributes` ("the only supported message system
/// attribute is `AWSTraceHeader`").
const TRACE_ATTRIBUTE: &str = "AWSTraceHeader";

/// Every message on the queue at `url`, with its system attributes asked for by
/// name — a system attribute is reported only when the receive requests it.
async fn received(emulator: &crate::target_dispatch::Emulator, url: &str) -> Vec<Value> {
    let (status, body) = crate::target_dispatch::sqs(
        emulator,
        "ReceiveMessage",
        &json!({
            "QueueUrl": url,
            "MaxNumberOfMessages": 10,
            "MessageSystemAttributeNames": ["All"],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "receive {url}: ReceiveMessage answered {status} {body}"
    );
    body["Messages"].as_array().cloned().unwrap_or_default()
}

/// Publish one entry per `(detail, trace header)` pair, and answer each
/// `EventId` in the request's order.
async fn publish(
    emulator: &crate::target_dispatch::Emulator,
    entries: &[(&str, Option<&str>)],
) -> Vec<String> {
    let written: Vec<Value> = entries
        .iter()
        .map(|(detail, trace)| {
            let mut entry = json!({
                "Source": SOURCE,
                "DetailType": DETAIL_TYPE,
                "Detail": detail,
            });
            if let Some(trace) = trace {
                entry["TraceHeader"] = json!(trace);
            }
            entry
        })
        .collect();
    let (status, body) = events(emulator, "PutEvents", &json!({ "Entries": written })).await;
    assert_eq!(status, 200, "PutEvents answered {status} {body}");
    assert_eq!(
        body["FailedEntryCount"],
        json!(0),
        "PutEvents reported a failed entry: {body}"
    );
    body["Entries"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|entry| {
            entry["EventId"]
                .as_str()
                .unwrap_or_else(|| panic!("no EventId in {entry}"))
                .to_string()
        })
        .collect()
}

/// An event published WITH a trace header reaches its queue target carrying that
/// header; one published with text that is not a trace header, and one published
/// without any, reach it carrying none.
#[tokio::test]
async fn a_published_trace_header_reaches_the_queue_target() {
    // (case, the entry's TraceHeader, the attribute the message must carry)
    let rows: [(&str, Option<&str>, Option<&str>); 10] = [
        ("no trace header", None, None),
        (
            // The form X-Ray documents for a tracing header: a `Root` field
            // holding `<version>-<8 hex>-<24 hex>`, and optional fields beside
            // it.
            "a trace id alone",
            Some("Root=1-5759e988-bd862e3fe1be46a994272793"),
            Some("Root=1-5759e988-bd862e3fe1be46a994272793"),
        ),
        (
            "a sampled trace header",
            Some("Root=1-5759e988-bd862e3fe1be46a994272793;Sampled=1"),
            Some("Root=1-5759e988-bd862e3fe1be46a994272793;Sampled=1"),
        ),
        (
            "a trace header naming a parent segment",
            Some("Root=1-67891233-abcdef012345678912345678;Parent=53995c3f42cd8ad8"),
            Some("Root=1-67891233-abcdef012345678912345678;Parent=53995c3f42cd8ad8"),
        ),
        (
            // Whatever a well-formed header's fields say is what the target
            // receives: the header is carried, not interpreted.
            "a trace header carrying a field of its own",
            Some("Root=1-00000000-000000000000000000000000;Tenant=alpha"),
            Some("Root=1-00000000-000000000000000000000000;Tenant=alpha"),
        ),
        // Text that names no trace is not a trace header, so nothing is carried.
        ("text carrying no field at all", Some("not-a-trace-header"), None),
        (
            "a header naming no trace id",
            Some("Sampled=1"),
            None,
        ),
        (
            "a Root that is not a trace id",
            Some("Root=not-a-trace-id"),
            None,
        ),
        (
            "a trace id of the wrong length",
            Some("Root=1-5759e988-bd862e3fe1be46a9942727"),
            None,
        ),
        (
            "a trace id whose digits are not hexadecimal",
            Some("Root=1-5759e988-zzzzzzzzzzzzzzzzzzzzzzzz"),
            None,
        ),
    ];

    for (case, published, expected) in rows {
        let emulator = emulator();
        let (url, arn) = given_queue(&emulator, "trace-probe").await;
        given_rule(&emulator, "trace", json!([{ "Id": "t1", "Arn": arn }])).await;

        let ids = publish(&emulator, &[(r#"{"orderId":"A-1"}"#, published)]).await;
        let messages = received(&emulator, &url).await;
        assert_eq!(
            messages.len(),
            1,
            "case {case:?}: expected exactly one delivery, got {messages:?}"
        );
        let carried = messages[0]["Attributes"][TRACE_ATTRIBUTE].as_str();
        assert_eq!(
            carried, expected,
            "case {case:?}: expected the {TRACE_ATTRIBUTE} attribute to be {expected:?}, \
             got {carried:?} in {:?}",
            messages[0]["Attributes"]
        );
        // The header is a property of the publication, so the delivered envelope
        // is the event itself and carries no trace member.
        let body: Value = serde_json::from_str(messages[0]["Body"].as_str().unwrap_or_default())
            .unwrap_or_else(|error| panic!("case {case:?}: the body is not JSON ({error})"));
        assert_eq!(
            body["id"].as_str(),
            ids.first().map(String::as_str),
            "case {case:?}: expected the delivered envelope to be the published event, \
             got {body}"
        );
        assert!(
            body.get("TraceHeader").is_none() && body.get("trace-header").is_none(),
            "case {case:?}: the trace header is not a member of the event, got {body}"
        );
    }
}

/// In a BATCH, the attribute follows the entry it was published with: an entry
/// carrying a header delivers one, and the entries beside it do not.
#[tokio::test]
async fn a_trace_header_follows_the_entry_it_was_published_with() {
    let emulator = emulator();
    let (url, arn) = given_queue(&emulator, "trace-batch").await;
    given_rule(&emulator, "trace-batch", json!([{ "Id": "t1", "Arn": arn }])).await;

    let traced = "Root=1-5759e988-bd862e3fe1be46a994272793;Sampled=1";
    let entries: [(&str, Option<&str>); 3] = [
        (r#"{"orderId":"first"}"#, None),
        (r#"{"orderId":"second"}"#, Some(traced)),
        (r#"{"orderId":"third"}"#, None),
    ];
    publish(&emulator, &entries).await;

    let messages = received(&emulator, &url).await;
    assert_eq!(
        messages.len(),
        entries.len(),
        "expected one delivery per entry, got {messages:?}"
    );
    for message in &messages {
        let body: Value = serde_json::from_str(message["Body"].as_str().unwrap_or_default())
            .unwrap_or_else(|error| panic!("the delivered body is not JSON ({error})"));
        let order = body["detail"]["orderId"].as_str().unwrap_or_default().to_string();
        let expected = entries
            .iter()
            .find(|(detail, _)| detail.contains(&order))
            .and_then(|(_, trace)| *trace);
        let carried = message["Attributes"][TRACE_ATTRIBUTE].as_str();
        assert_eq!(
            carried, expected,
            "case {order:?}: expected the {TRACE_ATTRIBUTE} attribute to be {expected:?}, \
             got {carried:?}"
        );
    }
}
