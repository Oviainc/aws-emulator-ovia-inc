// SPDX-License-Identifier: CC0-1.0

//! What `DescribePage` reports for a stored page, end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see — routing, deserialization, constraint validation, the handler and
//! serialization included. Every page is SENT by `StartEngagement` engaging a contact's
//! plan, so the read is asserted against what that start persisted rather than against
//! anything the test placed in the store.
//!
//! The rule under test: the report is rebuilt from the page the `PageId` names, so it
//! carries the message the engagement that sent that page carried — each message member
//! VERBATIM, and an optional one present exactly when the start named it — and the report
//! is a pure read: a second call answers the identical body and the store it read is
//! unchanged, acknowledgement instants and receipts included.
//!
//! Both rules are TABLES rather than single calls. The message table's rows differ along
//! which optional members the start named and along each member's published length bounds
//! (`Sender` 0..255, `Subject` 1..2048, `Content` 1..8192, `PublicSubject` 1..2048,
//! `PublicContent` 1..8192, `IncidentId` 0..1024 — the constraints
//! `smithy_query(query="operation",operation="StartEngagement")` reports for
//! `StartEngagementRequest`, which `DescribePageResult` repeats), and the second table's
//! rows are two engagements of one contact carrying DIFFERENT messages, so a page reporting
//! its own message is told apart from one reporting the newest.
//!
//! `tests/all/page_acknowledgement.rs` is the suite about what a page's acknowledgement
//! moves and about this operation's two refusals; this one is about the projection.

use serde_json::{json, Value};

use crate::contact_lifecycle::Emulator;
use crate::page_acknowledgement::{channel_target, describe_page, pages_of, set_plan};

// ---------------------------------------------------------------------------
// The message members, with the bounds the model publishes for each
// ---------------------------------------------------------------------------

/// One message member of `StartEngagementRequest`, which `DescribePageResult` reports back.
struct Member {
    /// The member name on the wire, the same in the request and in the report.
    name: &'static str,
    /// The published minimum length of the member's target shape.
    min: usize,
    /// The published maximum length of the member's target shape.
    max: usize,
    /// A character the member's published pattern admits, repeated to reach a bound. The
    /// two `Sender`/`IncidentId` shapes admit a restricted character class, the message
    /// bodies admit any character, so an ASCII letter is inside every one of them.
    fill: char,
    /// A value of the size a client typically sends.
    typical: &'static str,
}

/// The required members: the user that started the engagement and the secure message.
const SENDER: Member = Member {
    name: "Sender",
    min: 0,
    max: 255,
    fill: 's',
    typical: "incident-manager",
};
const SUBJECT: Member = Member {
    name: "Subject",
    min: 1,
    max: 2048,
    fill: 'u',
    typical: "Database latency",
};
const CONTENT: Member = Member {
    name: "Content",
    min: 1,
    max: 8192,
    fill: 'c',
    typical: "p99 write latency is over budget.\nAcknowledge when you are on it.",
};

/// The optional members: the insecure message a page to an SMS channel carries, and the
/// incident the engagement is about.
const PUBLIC_SUBJECT: Member = Member {
    name: "PublicSubject",
    min: 1,
    max: 2048,
    fill: 'p',
    typical: "Latency",
};
const PUBLIC_CONTENT: Member = Member {
    name: "PublicContent",
    min: 1,
    max: 8192,
    fill: 'q',
    typical: "Ack the page",
};
const INCIDENT_ID: Member = Member {
    name: "IncidentId",
    min: 0,
    max: 1024,
    fill: 'i',
    typical: "incident-4711",
};

/// Every message member a page carries, in the order the report lists them.
const MESSAGE_MEMBERS: &[&Member] = &[
    &SENDER,
    &SUBJECT,
    &CONTENT,
    &PUBLIC_SUBJECT,
    &PUBLIC_CONTENT,
    &INCIDENT_ID,
];

/// Which length a row gives a member: the two published bounds, or a typical value between
/// them.
#[derive(Debug, Clone, Copy)]
enum Size {
    Min,
    Typical,
    Max,
}

impl Member {
    /// A value of this member at `size`. `Min` of a member whose published minimum is 0 is
    /// the EMPTY string, which the member admits and a client may therefore send.
    fn value(&self, size: Size) -> String {
        match size {
            Size::Min => self.fill.to_string().repeat(self.min),
            Size::Typical => self.typical.to_string(),
            Size::Max => self.fill.to_string().repeat(self.max),
        }
    }
}

// ---------------------------------------------------------------------------
// The fixture: a contact whose one-stage plan targets one channel
// ---------------------------------------------------------------------------

/// Create a contact with one contact channel and a plan whose only stage targets it, so
/// every start of that contact sends exactly ONE page and a row's message is read back from
/// a page of its own.
///
/// The stage's `DurationInMinutes` is 0 — inside the modelled `StageDurationInMins` range —
/// so the page is sent at the instant the engagement starts.
async fn paged_contact(emulator: &Emulator) -> String {
    let contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-described",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
            }),
            "the described page's contact",
        )
        .await;
    let channel = emulator
        .create_channel(
            json!({
                "ContactId": contact,
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "ops@example.com"},
            }),
            "the described page's channel",
        )
        .await;
    set_plan(
        emulator,
        &contact,
        json!({"Stages": [{"DurationInMinutes": 0, "Targets": [channel_target(&channel)]}]}),
        "the described page's plan",
    )
    .await;
    contact
}

/// Start an engagement with `message`, asserting it succeeded, and answer its
/// `EngagementArn`.
async fn start(emulator: &Emulator, message: &Value, case: &str) -> String {
    let (status, body) = emulator.call("StartEngagement", message.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartEngagement to answer 200, got {status} {body:?}"
    );
    body["EngagementArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
        .to_string()
}

/// `value[name]`, with an absent member and a member serialized as JSON null read alike:
/// both are "the member is not present", which is what an unnamed optional member is.
fn member<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|member| !member.is_null())
}

/// The ARN of the one page `engagement_arn` sent.
fn sole_page(emulator: &Emulator, engagement_arn: &str, case: &str) -> String {
    let pages = pages_of(emulator, engagement_arn);
    assert_eq!(
        pages.len(),
        1,
        "case {case:?}: expected the start to send exactly one page, got {:?}",
        pages.iter().map(|page| page.arn.clone()).collect::<Vec<_>>()
    );
    pages[0].arn.clone()
}

/// The store's own record of every page of `engagement_arn`, rendered, so a read can be
/// asserted to have changed nothing about them.
fn stored_pages(emulator: &Emulator, engagement_arn: &str) -> String {
    format!("{:?}", pages_of(emulator, engagement_arn))
}

// ---------------------------------------------------------------------------
// 1. The report carries the message the engagement sent
// ---------------------------------------------------------------------------

/// One row of the message table: the size of each required member, and the size of each
/// optional member the start names (`None` is a start that does not name it at all).
struct MessageCase {
    case: &'static str,
    sender: Size,
    subject: Size,
    content: Size,
    public_subject: Option<Size>,
    public_content: Option<Size>,
    incident_id: Option<Size>,
}

impl MessageCase {
    /// The `StartEngagement` request this row sends for `contact`.
    fn request(&self, contact: &str) -> Value {
        let mut request = json!({
            "ContactId": contact,
            "Sender": SENDER.value(self.sender),
            "Subject": SUBJECT.value(self.subject),
            "Content": CONTENT.value(self.content),
        });
        let optional = [
            (&PUBLIC_SUBJECT, self.public_subject),
            (&PUBLIC_CONTENT, self.public_content),
            (&INCIDENT_ID, self.incident_id),
        ];
        for (member, size) in optional {
            if let Some(size) = size {
                request[member.name] = Value::String(member.value(size));
            }
        }
        request
    }
}

const MESSAGE_CASES: &[MessageCase] = &[
    MessageCase {
        case: "every optional member named, at a typical size",
        sender: Size::Typical,
        subject: Size::Typical,
        content: Size::Typical,
        public_subject: Some(Size::Typical),
        public_content: Some(Size::Typical),
        incident_id: Some(Size::Typical),
    },
    MessageCase {
        case: "no optional member named at all",
        sender: Size::Typical,
        subject: Size::Typical,
        content: Size::Typical,
        public_subject: None,
        public_content: None,
        incident_id: None,
    },
    MessageCase {
        case: "every member at its published minimum length",
        sender: Size::Min,
        subject: Size::Min,
        content: Size::Min,
        public_subject: Some(Size::Min),
        public_content: Some(Size::Min),
        incident_id: Some(Size::Min),
    },
    MessageCase {
        case: "every member at its published maximum length",
        sender: Size::Max,
        subject: Size::Max,
        content: Size::Max,
        public_subject: Some(Size::Max),
        public_content: Some(Size::Max),
        incident_id: Some(Size::Max),
    },
    MessageCase {
        case: "the insecure subject named without the insecure content",
        sender: Size::Typical,
        subject: Size::Typical,
        content: Size::Typical,
        public_subject: Some(Size::Typical),
        public_content: None,
        incident_id: None,
    },
    MessageCase {
        case: "the incident named without any insecure message",
        sender: Size::Min,
        subject: Size::Typical,
        content: Size::Max,
        public_subject: None,
        public_content: None,
        incident_id: Some(Size::Typical),
    },
];

#[tokio::test]
async fn a_page_reports_the_message_the_engagement_sent_to_it() {
    let emulator = Emulator::new();
    let contact = paged_contact(&emulator).await;

    for row in MESSAGE_CASES {
        let case = row.case;
        let request = row.request(&contact);
        let engagement = start(&emulator, &request, case).await;
        let page_arn = sole_page(&emulator, &engagement, case);

        let (status, body) = describe_page(&emulator, &page_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribePage to answer 200, got {status} {body:?}"
        );

        // The three ARNs place the page: itself, the engagement that sent it and the
        // contact whose channel it engaged.
        assert_eq!(
            body["PageArn"].as_str(),
            Some(page_arn.as_str()),
            "case {case:?}: expected the page to report its own ARN, got {body:?}"
        );
        assert_eq!(
            body["EngagementArn"].as_str(),
            Some(engagement.as_str()),
            "case {case:?}: expected the page to name its engagement, got {body:?}"
        );
        assert_eq!(
            body["ContactArn"].as_str(),
            Some(contact.as_str()),
            "case {case:?}: expected the page to name the paged contact, got {body:?}"
        );

        // Each message member, VERBATIM and present exactly when the start named it.
        for message_member in MESSAGE_MEMBERS {
            let sent = member(&request, message_member.name);
            let reported = member(&body, message_member.name);
            assert_eq!(
                reported,
                sent,
                "case {case:?}: expected the page to report the {} the start carried \
                 (len {:?}), got len {:?}",
                message_member.name,
                sent.and_then(Value::as_str).map(str::len),
                reported.and_then(Value::as_str).map(str::len)
            );
        }

        // The page has been sent and nothing has acknowledged it, so it carries the one
        // instant the store holds for it.
        assert!(
            member(&body, "SentTime").is_some(),
            "case {case:?}: expected a sent page to report a SentTime, got {body:?}"
        );
        assert!(
            member(&body, "DeliveryTime").is_none() && member(&body, "ReadTime").is_none(),
            "case {case:?}: an unacknowledged page carries neither instant, got {body:?}"
        );

        // The report is a pure read: asking again answers the same page, and the store it
        // read is exactly as it found it.
        let before = stored_pages(&emulator, &engagement);
        let (status_again, body_again) = describe_page(&emulator, &page_arn).await;
        assert_eq!(
            status_again, 200,
            "case {case:?}: expected a repeated DescribePage to answer 200, got \
             {status_again} {body_again:?}"
        );
        assert_eq!(
            body_again, body,
            "case {case:?}: expected a repeated read to report the identical page, got \
             {body_again:?}"
        );
        let after = stored_pages(&emulator, &engagement);
        assert_eq!(
            before, after,
            "case {case:?}: expected the read to change nothing a later caller sees, got \
             {after:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Each page reports its OWN engagement's message
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_page_reports_its_own_engagements_message() {
    let emulator = Emulator::new();
    let contact = paged_contact(&emulator).await;

    // Two starts of ONE contact, differing only in the message they carry: the second start
    // writes the newest message, so a report keyed to the page tells the two apart while a
    // report of "the latest message" cannot.
    let rows = [
        MessageCase {
            case: "the earlier engagement's message",
            sender: Size::Typical,
            subject: Size::Typical,
            content: Size::Typical,
            public_subject: Some(Size::Typical),
            public_content: Some(Size::Typical),
            incident_id: Some(Size::Typical),
        },
        MessageCase {
            case: "the later engagement's message",
            sender: Size::Min,
            subject: Size::Min,
            content: Size::Min,
            public_subject: None,
            public_content: None,
            incident_id: None,
        },
    ];

    let mut sent: Vec<(&str, Value, String)> = Vec::new();
    for row in &rows {
        let request = row.request(&contact);
        let engagement = start(&emulator, &request, row.case).await;
        let page_arn = sole_page(&emulator, &engagement, row.case);
        sent.push((row.case, request, page_arn));
    }

    // Read in the order the pages were sent and then in the reverse order, so neither
    // report can be the one the previous read left behind.
    for (case, request, page_arn) in sent.iter().chain(sent.iter().rev()) {
        let (status, body) = describe_page(&emulator, page_arn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribePage to answer 200, got {status} {body:?}"
        );
        for message_member in MESSAGE_MEMBERS {
            let expected = member(request, message_member.name);
            let reported = member(&body, message_member.name);
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the page to report its own {}, got {reported:?}",
                message_member.name
            );
        }
    }
}
