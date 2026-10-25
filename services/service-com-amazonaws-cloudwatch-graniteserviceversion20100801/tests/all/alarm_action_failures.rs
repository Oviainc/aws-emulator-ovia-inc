// SPDX-License-Identifier: CC0-1.0

//! What an alarm's history records when an alarm ACTION could not be delivered, end to end
//! over the wire.
//!
//! ONE rule, asserted over a table: the `Action` history item of a delivery that failed
//! carries, as the `error` of its JSON document ("Data about the alarm, in JSON format.",
//! `AlarmHistoryItem$HistoryData`, `@length 1..4095`), the message the DESTINATION service
//! answered the delivery with — the same text that service gives a client who calls it
//! directly — and nothing of how this emulator carried the call. A delivery the destination
//! ACCEPTED carries that member with no message in it, and the payload it was handed.
//!
//! The rows differ along what the rule generalises over: the refusal the destination makes (a
//! topic that was deleted, a topic that was never created, a destination that is no topic ARN
//! at all) and a delivery that SUCCEEDS, which records the same member with no message in it.
//! Each row's expectation is read from the destination itself, by making the same call against
//! it, so the rule is asserted rather than one recorded sentence.

use serde_json::{json, Value};

use crate::alarm_evaluation::{datum, patched, publish};
use crate::harness::{element, emulator, ok, sns, sns_call, wait_for_evaluation};

/// The modelled maximum length of one history document
/// (`smithy_query(query="shape",shape="HistoryData")`).
const MAX_HISTORY_DATA_LEN: usize = 4095;

/// Fragments of the SDK's own rendering of an error value: what a document that reports the
/// destination's message instead of its transport must not carry.
const INTERNALS: [&str; 5] = [
    "ServiceError",
    "SdkBody",
    "ErrorMetadata",
    "StatusCode",
    "ErrorResponse",
];

/// One row of the table.
struct Row {
    /// What this row is about, named by every assertion it makes.
    case: &'static str,
    /// The destination the alarm's `AlarmActions` names.
    action: String,
    /// Whether the delivery to it must fail.
    fails: bool,
}

#[tokio::test]
async fn a_failed_alarm_action_records_the_destination_services_own_error() {
    let emulator = emulator();

    // A topic that exists, and one that existed and was deleted — the two live destinations
    // the rows are built from. The account and Region come off the ARN SNS minted, so the
    // fabricated destinations below name the same account as the real one.
    let live = element(
        &sns(&emulator, "CreateTopic", &[("Name", "action-live")]).await,
        "TopicArn",
    );
    let deleted = element(
        &sns(&emulator, "CreateTopic", &[("Name", "action-deleted")]).await,
        "TopicArn",
    );
    sns(&emulator, "DeleteTopic", &[("TopicArn", &deleted)]).await;
    let prefix = live
        .rsplit_once(':')
        .map(|(prefix, _)| prefix.to_string())
        .expect("a topic ARN ends in the topic name");

    let rows = [
        Row {
            case: "a topic that was deleted",
            action: deleted,
            fails: true,
        },
        Row {
            case: "a topic that was never created",
            action: format!("{prefix}:action-absent"),
            fails: true,
        },
        Row {
            // An SNS ARN whose resource part is a subscription rather than a topic name: the
            // destination refuses it as bad input instead of as a missing resource, so the
            // message the item records is a different one.
            case: "a destination that is no topic ARN at all",
            action: format!("{prefix}:action-live:11111111-2222-3333-4444-555555555555"),
            fails: true,
        },
        Row {
            case: "a topic the delivery reaches",
            action: live,
            fails: false,
        },
    ];

    // Every row's alarm watches its own series, so all of them are configured on ONE emulator
    // and the evaluation delay is waited out once rather than per row.
    let mut names: Vec<String> = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        let name = format!("action-{index}");
        let alarm = patched(&name, &json!({"AlarmActions": [row.action.clone()]}));
        ok(&emulator.router, "PutMetricAlarm", &alarm).await;
        publish(&emulator.router, &name, &[datum(42.0, 0.0)]).await;
        names.push(name);
    }
    wait_for_evaluation();

    for (row, name) in rows.iter().zip(&names) {
        let case = row.case;
        // The read that materialises the transition also dispatches the action it owes.
        let described = ok(
            &emulator.router,
            "DescribeAlarms",
            &json!({"AlarmNames": [name]}),
        )
        .await;
        assert_eq!(
            described["MetricAlarms"][0]
                .get("StateValue")
                .and_then(Value::as_str),
            Some("ALARM"),
            "case {case:?}: the published datapoint must put the alarm in ALARM, got {described}"
        );

        let history = ok(
            &emulator.router,
            "DescribeAlarmHistory",
            &json!({"AlarmName": name, "HistoryItemType": "Action"}),
        )
        .await;
        let items = history["AlarmHistoryItems"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            items.len(),
            1,
            "case {case:?}: executing the one action must record one Action item, got {history}"
        );
        let raw = items[0]
            .get("HistoryData")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            (1..=MAX_HISTORY_DATA_LEN).contains(&raw.len()),
            "case {case:?}: HistoryData must stay inside its modelled length, got {} \
             characters: {raw:?}",
            raw.len()
        );
        let document: Value = serde_json::from_str(raw)
            .unwrap_or_else(|_| panic!("case {case:?}: HistoryData must be JSON, got {raw:?}"));
        assert_eq!(
            document["notificationResource"].as_str(),
            Some(row.action.as_str()),
            "case {case:?}: the item must name the destination it executed, got {document}"
        );
        assert_eq!(
            document["stateValue"].as_str(),
            Some("ALARM"),
            "case {case:?}: the item must name the state that fired the action, got {document}"
        );

        if !row.fails {
            assert_eq!(
                document["actionState"].as_str(),
                Some("Succeeded"),
                "case {case:?}: a delivery that was accepted must be recorded as Succeeded, \
                 got {document}"
            );
            assert!(
                document
                    .get("error")
                    .is_some_and(serde_json::Value::is_null),
                "case {case:?}: a delivery that was accepted must carry the error member with \
                 no message in it, got {document}"
            );
            // The payload the destination was handed is the notification this alarm's own
            // transition published, so it reads back as that alarm's notification.
            let published: Value = serde_json::from_str(
                document
                    .get("publishedMessage")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
            .unwrap_or_else(|_| {
                panic!("case {case:?}: the recorded payload must be the published JSON \
                        notification, got {document}")
            });
            assert_eq!(
                published["AlarmName"].as_str(),
                Some(name.as_str()),
                "case {case:?}: the recorded payload must be the one this alarm produced, got \
                 {published}"
            );
            assert_eq!(
                published["NewStateValue"].as_str(),
                Some("ALARM"),
                "case {case:?}: the recorded payload must carry the state that fired the \
                 action, got {published}"
            );
            continue;
        }

        assert_eq!(
            document["actionState"].as_str(),
            Some("Failed"),
            "case {case:?}: a delivery the destination refused must be recorded as Failed, \
             got {document}"
        );
        // What the destination says about the SAME call, asked of it directly: that message,
        // and nothing else, is what the item has to carry.
        let (status, refusal) = sns_call(
            &emulator,
            "Publish",
            &[("TopicArn", row.action.as_str()), ("Message", "probe")],
        )
        .await;
        assert!(
            (400..500).contains(&status),
            "case {case:?}: the destination must refuse this publish, got {status} / {refusal}"
        );
        let expected = element(&refusal, "Message");
        let recorded = document
            .get("error")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            recorded, expected,
            "case {case:?}: expected the destination's own message {expected:?}, got {recorded:?}"
        );
        for fragment in INTERNALS {
            assert!(
                !recorded.contains(fragment),
                "case {case:?}: the recorded error must carry nothing of this emulator's \
                 transport, found {fragment:?} in {recorded:?}"
            );
        }
        assert!(
            document.get("publishedMessage").is_none(),
            "case {case:?}: nothing was published to a destination that refused the call, so \
             the item must report no payload, got {document}"
        );
    }
}
