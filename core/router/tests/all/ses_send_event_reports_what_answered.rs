// SPDX-License-Identifier: CC0-1.0

//! What a published email event REPORTS about the reaction that produced it, over the whole
//! mounted service set — SESv2 publishes, SNS fans out, SQS holds what arrived.
//!
//! The RULE under test: each event a send publishes reports what the side that produced the
//! reaction said and what this service measured of it — a delivery report carries the reply
//! the receiving side gave for the message it took, the recipients it took it for, the mail
//! server that reported the delivery and the time between the message being accepted and the
//! delivery; a complaint report carries the agent that wrote the feedback report and the kind
//! of complaint it was; a bounce a mail server produced names that server, and one SES
//! produced without handing the message on names none, because no server was asked.
//!
//! The sibling suite
//! [`crate::ses_send_events_reach_a_subscribed_queue`] pins WHICH events a send produces and
//! WHERE they go; this one pins what is inside each one, and reuses that suite's wiring for
//! the sender, the configuration set and the send itself.
//!
//! The rows span the catalog of recipients the rule generalises over — every mailbox the SES
//! mailbox simulator defines, a mailbox of that domain it does not define, and an address
//! outside the simulator altogether — so the reaction under each row differs in which side
//! answered for it and therefore in what its report may carry.

use serde_json::Value;

use crate::cloudwatch_alarm_action_reaches_its_subscribed_queue::{
    drain, drain_until, given_topic_with_subscriber, Grant,
};
use crate::ses_send_events_reach_a_subscribed_queue::{
    event_types, given_configuration_set, given_verified_sender, one_event, router, send,
    EVERY_EVENT_TYPE, REGION,
};

/// The SMTP reply a receiving side gives for a message it took, as a delivery report quotes
/// it. Written out here so the suite pins the reply rather than reading it back from the code
/// under test.
const ACCEPTED_SMTP_RESPONSE: &str = "250 Ok";

/// The agent a feedback report names when the complaint came from the SES mailbox simulator.
const SIMULATOR_USER_AGENT: &str = "Amazon SES Mailbox Simulator";

/// The `complaintFeedbackType` a report carries for a recipient that reported the message as
/// unwanted, under the names RFC 5965 registers for a feedback report.
const COMPLAINT_FEEDBACK_TYPE: &str = "abuse";

/// A ceiling on the time SES may report having held one message in this emulator, so the row
/// pins a measured duration rather than an arbitrary number: the publication is made on the
/// send's own runtime, right after the message was accepted.
const MAX_PROCESSING_TIME_MILLIS: u64 = 60_000;

/// One row: the recipient the send addresses, the events the destination must then receive,
/// and what the report of the reaction may carry.
struct ReportCase {
    case: &'static str,
    recipient: &'static str,
    /// The event types the subscribed queue must receive, in any order.
    received: &'static [&'static str],
    /// Whether the bounce this row produces was reported BY a mail server.
    bounce_reported_by_mta: bool,
}

/// The rows: every mailbox the simulator defines, one of its domain it does not define, and
/// an address outside the simulator.
const REPORT_CASES: &[ReportCase] = &[
    ReportCase {
        case: "the simulator's accepting mailbox",
        recipient: "success@simulator.amazonses.com",
        received: &["Send", "Delivery"],
        bounce_reported_by_mta: false,
    },
    ReportCase {
        case: "the simulator's out-of-the-office mailbox, which takes the message too",
        recipient: "ooto@simulator.amazonses.com",
        received: &["Send", "Delivery"],
        bounce_reported_by_mta: false,
    },
    ReportCase {
        case: "an address outside the simulator",
        recipient: "reader@receivers.example.com",
        received: &["Send", "Delivery"],
        bounce_reported_by_mta: false,
    },
    ReportCase {
        case: "the simulator's complaining mailbox, which takes the message and reports it",
        recipient: "complaint@simulator.amazonses.com",
        received: &["Send", "Delivery", "Complaint"],
        bounce_reported_by_mta: false,
    },
    ReportCase {
        case: "the simulator's bouncing mailbox",
        recipient: "bounce@simulator.amazonses.com",
        received: &["Send", "Bounce"],
        bounce_reported_by_mta: true,
    },
    ReportCase {
        case: "a mailbox of the simulator's domain the simulator does not define",
        recipient: "1f8d-not-a-mailbox@simulator.amazonses.com",
        received: &["Send", "Bounce"],
        bounce_reported_by_mta: true,
    },
    ReportCase {
        case: "the simulator's suppression-list mailbox, which SES answers itself",
        recipient: "suppressionlist@simulator.amazonses.com",
        received: &["Send", "Bounce"],
        bounce_reported_by_mta: false,
    },
];

/// The mail server a report of this Region names: the SES SMTP interface the send executed
/// in, as a delivery report writes a host.
fn reporting_mta() -> String {
    format!("email-smtp.{REGION}.amazonaws.com")
}

/// The same host as a delivery status report's Reporting-MTA field writes it, with the
/// address type RFC 3464 prefixes the field with.
fn delivery_status_reporting_mta() -> String {
    format!("dsn; {}", reporting_mta())
}

/// A string at `pointer` inside `event`, or `""` when the event carries nothing there.
fn text(event: &Value, pointer: &str) -> String {
    event
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn every_published_report_carries_what_the_answering_side_said() {
    for row in REPORT_CASES {
        let case = row.case;
        let (router, engine) = router();
        let (topic, queue) =
            given_topic_with_subscriber(&router, case, REGION, "ses-send-reports", Grant::Unscoped)
                .await;
        given_verified_sender(&router, &engine, case).await;
        given_configuration_set(&router, case, &topic, true, EVERY_EVENT_TYPE).await;

        let message_id = send(&router, case, row.recipient).await;
        let mut events = drain_until(&router, case, REGION, &queue, row.received.len()).await;
        // One more read, so a row that expects two events would also see a third it must not
        // have received.
        events.extend(drain(&router, case, REGION, &queue).await);
        let mut arrived = event_types(&events);
        arrived.sort();
        let mut wanted: Vec<String> = row.received.iter().map(|at| at.to_string()).collect();
        wanted.sort();
        assert_eq!(
            arrived, wanted,
            "case {case:?}: expected exactly {wanted:?} on the queue, got {events:?}"
        );

        let accepted_at = text(one_event(case, &events, "Send"), "/mail/timestamp");
        assert!(
            !accepted_at.is_empty(),
            "case {case:?}: every event must stamp when the message was accepted, got {events:?}"
        );

        if row.received.contains(&"Delivery") {
            let delivered = one_event(case, &events, "Delivery");
            assert_eq!(
                text(delivered, "/delivery/smtpResponse"),
                ACCEPTED_SMTP_RESPONSE,
                "case {case:?}: the delivery must quote the reply the receiving side gave, got \
                 {delivered}"
            );
            assert_eq!(
                delivered["delivery"]["recipients"]
                    .as_array()
                    .map(|recipients| recipients
                        .iter()
                        .map(|at| at.as_str().unwrap_or_default().to_string())
                        .collect::<Vec<String>>()),
                Some(vec![row.recipient.to_string()]),
                "case {case:?}: the delivery must name the recipients it reached, got {delivered}"
            );
            assert_eq!(
                text(delivered, "/delivery/reportingMTA"),
                reporting_mta(),
                "case {case:?}: the delivery must name the server that reported it, got \
                 {delivered}"
            );
            let held = delivered["delivery"]["processingTimeMillis"].as_u64();
            assert!(
                held.is_some_and(|millis| millis <= MAX_PROCESSING_TIME_MILLIS),
                "case {case:?}: the delivery must report the measured time SES held the \
                 message, at most {MAX_PROCESSING_TIME_MILLIS} ms here, got {delivered}"
            );
            assert!(
                !text(delivered, "/delivery/timestamp").is_empty(),
                "case {case:?}: the delivery must stamp when it happened, got {delivered}"
            );
        }

        if row.received.contains(&"Complaint") {
            let complained = one_event(case, &events, "Complaint");
            assert_eq!(
                text(complained, "/complaint/userAgent"),
                SIMULATOR_USER_AGENT,
                "case {case:?}: the complaint must name the agent that wrote the report, got \
                 {complained}"
            );
            assert_eq!(
                text(complained, "/complaint/complaintFeedbackType"),
                COMPLAINT_FEEDBACK_TYPE,
                "case {case:?}: the complaint must name the kind of complaint it was, got \
                 {complained}"
            );
            assert_eq!(
                text(complained, "/complaint/complainedRecipients/0/emailAddress"),
                row.recipient,
                "case {case:?}: the complaint must name the recipient that reported it, got \
                 {complained}"
            );
            assert_eq!(
                text(complained, "/complaint/arrivalDate"),
                accepted_at,
                "case {case:?}: the complaint must date the arrival of the message it is \
                 about, got {complained}"
            );
        }

        if row.received.contains(&"Bounce") {
            let bounced = one_event(case, &events, "Bounce");
            let named = text(bounced, "/bounce/reportingMTA");
            if row.bounce_reported_by_mta {
                assert_eq!(
                    named,
                    delivery_status_reporting_mta(),
                    "case {case:?}: a failure a mail server reported must name that server, \
                     got {bounced}"
                );
                assert!(
                    !text(bounced, "/bounce/bouncedRecipients/0/diagnosticCode").is_empty(),
                    "case {case:?}: it must carry what the failing side said, got {bounced}"
                );
            } else {
                assert!(
                    named.is_empty(),
                    "case {case:?}: a failure no mail server was asked for must name no \
                     server, got {named:?} in {bounced}"
                );
            }
        }

        for event in &events {
            assert_eq!(
                text(event, "/mail/messageId"),
                message_id,
                "case {case:?}: every event must name the message it is about, got {event}"
            );
        }
    }
}
