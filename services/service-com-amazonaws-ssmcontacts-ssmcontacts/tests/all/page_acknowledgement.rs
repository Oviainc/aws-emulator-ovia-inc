// SPDX-License-Identifier: CC0-1.0

//! Pages and their acknowledgement — `AcceptPage` and `DescribePage` — end to end over the
//! wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1 request
//! (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the contact suite
//! builds, so what is asserted is what a signing AWS client would see.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one call:
//!
//! 1. starting an engagement engages the contact's plan, and engaging a plan PAGES every
//!    contact channel its stages target — the channels of the contacts an escalation stage
//!    names included — once per channel, each page carrying the message the start carried and
//!    sent at its own stage's offset into the engagement. `DescribePage` reports exactly that
//!    from then on. The rows span a one-stage plan, a multi-stage plan, a plan that targets
//!    the same channel twice, a plan reached through an escalation stage, a plan whose target
//!    names a channel the store does not hold, and a plan with no channel target at all;
//! 2. `AcceptPage` records the acknowledgement against the page: the `AcceptType` the call
//!    named stamps that type's instant — `DELIVERED` the delivery, `READ` the read — leaves
//!    the other absent, and appends one receipt carrying the `Note`. The rows span the
//!    model's whole `AcceptType` value set, the whole `AcceptCodeValidation` value set and
//!    its absence, both the published length bounds of `Note`, and whether the request names
//!    the paged `ContactChannelId`;
//! 3. the stamped instant is the FIRST acknowledgement of its type: acknowledging again
//!    appends a further receipt and moves nothing. The rows span the sequences of accept
//!    types a caller can send;
//! 4. a refusal records nothing: an unknown page is a 404, and a code that is not the one
//!    sent with the page (while validation is enforced), a channel the page did not engage,
//!    an off-set enum value and a member outside its modelled constraint are each a 400 —
//!    while `IGNORE` accepts any well-formed code;
//! 5. the account-level Incident Manager precondition is evaluated BEFORE the `PageId` is
//!    resolved, for the acknowledgement and for the read alike;
//! 6. the plan walk is bounded: a chain of contact-to-contact plans longer than the walk's
//!    own ceiling, closed into a cycle, is walked to that ceiling and the start returns.
//!
//! The accept code AWS sends WITH a page is delivered to the contact channel out of band and
//! never appears on the control-plane wire, so — exactly as a channel's activation code — the
//! store is where this suite reads it: that read stands in for the contact reading their
//! pager.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::{
    ACCEPT_CODE_VALIDATION_VALUES, ACCEPT_TYPE_VALUES,
};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    service_state, PageRecord, MAX_PLANS_PER_ENGAGEMENT,
};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The channels the fixture contact holds: one per value of the model's own `ChannelType`
/// set (`CHANNEL_TYPE_VALUES` in `src/ops/types/shared_types.rs`), so a page is asserted over
/// every kind of channel a plan can target.
const CHANNELS: &[(&str, &str, &str)] = &[
    ("email", "EMAIL", "ops@example.com"),
    ("sms", "SMS", "+15550100"),
    ("voice", "VOICE", "+15550101"),
];

/// A well-formed `SsmContactsArn` for a contact channel nothing ever created, so a plan can
/// target a channel the store does not hold.
fn absent_channel_arn() -> String {
    format!(
        "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact-channel/oncall-primary/\
         00000000-0000-4000-8000-00000000dead"
    )
}

/// A well-formed `SsmContactsArn` for a page nothing ever sent.
fn absent_page_arn() -> String {
    format!(
        "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:page/oncall-primary/\
         00000000-0000-4000-8000-00000000beef"
    )
}

/// What the suite stores before any engagement is started.
struct Fixture {
    /// A contact whose three-stage plan targets each of its channels, the second stage
    /// targeting the first channel a SECOND time.
    primary: String,
    /// `primary`'s channel ARNs, in the order [`CHANNELS`] lists them.
    channels: Vec<String>,
    /// An escalation plan whose second stage targets `primary`, so its engagement reaches
    /// `primary`'s channels through a contact target, 7 minutes in.
    escalation: String,
    /// A contact whose plan targets one real channel and one channel the store does not
    /// hold.
    partial: String,
    /// A contact whose plan targets no channel at all.
    silent: String,
}

/// Which fixture contact an engagement names.
#[derive(Debug, Clone, Copy)]
enum Engaged {
    Primary,
    Escalation,
    Partial,
    Silent,
}

impl Fixture {
    fn contact(&self, engaged: Engaged) -> &str {
        match engaged {
            Engaged::Primary => &self.primary,
            Engaged::Escalation => &self.escalation,
            Engaged::Partial => &self.partial,
            Engaged::Silent => &self.silent,
        }
    }
}

/// Create the contacts, the channels and the plans that target them.
async fn fixture(emulator: &Emulator) -> Fixture {
    onboard(emulator);

    let primary = emulator
        .create_contact(
            json!({
                "Alias": "oncall-primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
            }),
            "the suite's paged contact",
        )
        .await;

    let mut channels: Vec<String> = Vec::new();
    for (name, channel_type, address) in CHANNELS {
        let (status, body) = emulator
            .call(
                "CreateContactChannel",
                json!({
                    "ContactId": primary,
                    "Name": name,
                    "Type": channel_type,
                    "DeliveryAddress": {"SimpleAddress": address},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "the fixture's {channel_type} channel must be created, got {status} {body:?}"
        );
        channels.push(
            body["ContactChannelArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a ContactChannelArn, got {body:?}"))
                .to_string(),
        );
    }

    // The plan can only be written once the channels it targets exist, so it is set by
    // `UpdateContact`: every `DurationInMinutes` below is inside the modelled
    // `StageDurationInMins` range (0..30), and the second stage names the first channel a
    // second time so "one page per channel" is asserted over a repeated target.
    set_plan(
        emulator,
        &primary,
        json!({
            "Stages": [
                {"DurationInMinutes": 5, "Targets": [channel_target(&channels[0])]},
                {
                    "DurationInMinutes": 10,
                    "Targets": [channel_target(&channels[1]), channel_target(&channels[0])],
                },
                {"DurationInMinutes": 0, "Targets": [channel_target(&channels[2])]},
            ]
        }),
        "the paged contact's plan",
    )
    .await;

    let escalation = emulator
        .create_contact(
            json!({
                "Alias": "escalation-tier-1",
                "Type": "ESCALATION",
                "Plan": {"Stages": [
                    {"DurationInMinutes": 7, "Targets": []},
                    {
                        "DurationInMinutes": 0,
                        "Targets": [{"ContactTargetInfo": {
                            "ContactId": primary,
                            "IsEssential": true,
                        }}],
                    },
                ]},
            }),
            "the suite's escalation plan",
        )
        .await;

    let partial = emulator
        .create_contact(
            json!({
                "Alias": "oncall-secondary",
                "Type": "PERSONAL",
                "Plan": {"Stages": [{
                    "DurationInMinutes": 1,
                    "Targets": [
                        channel_target(&channels[2]),
                        channel_target(&absent_channel_arn()),
                    ],
                }]},
            }),
            "the suite's partially resolvable plan",
        )
        .await;

    let silent = emulator
        .create_contact(
            json!({
                "Alias": "oncall-tertiary",
                "Type": "PERSONAL",
                "Plan": {"Stages": [{"DurationInMinutes": 3, "Targets": []}]},
            }),
            "the suite's contact with no channel target",
        )
        .await;

    Fixture {
        primary,
        channels,
        escalation,
        partial,
        silent,
    }
}

/// A plan target naming one contact channel.
fn channel_target(channel_arn: &str) -> Value {
    json!({"ChannelTargetInfo": {"ContactChannelId": channel_arn}})
}

/// Replace `contact`'s engagement plan, asserting the update succeeded.
async fn set_plan(emulator: &Emulator, contact: &str, plan: Value, case: &str) {
    let (status, body) = emulator
        .call("UpdateContact", json!({"ContactId": contact, "Plan": plan}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected UpdateContact to answer 200, got {status} {body:?}"
    );
}

/// The message every engagement this suite starts carries.
fn start_request(contact: &str) -> Value {
    json!({
        "ContactId": contact,
        "Sender": "incident-manager",
        "Subject": "Database latency",
        "Content": "p99 write latency is over budget.\nAcknowledge when you are on it.",
        "PublicSubject": "Latency",
        "PublicContent": "Ack the page",
        "IncidentId": "incident-4711",
    })
}

/// Start an engagement of `contact`, asserting it succeeded, and answer its
/// `EngagementArn`.
async fn start_engagement(emulator: &Emulator, contact: &str, case: &str) -> String {
    let (status, body) = emulator
        .call("StartEngagement", start_request(contact))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected StartEngagement to answer 200, got {status} {body:?}"
    );
    body["EngagementArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an EngagementArn, got {body:?}"))
        .to_string()
}

/// The pages the store holds for `engagement_arn`, in the order the plan engaged them.
///
/// The store is read directly because `ListPagesByEngagement` is a stub of its own: this
/// suite is about the pages themselves, not about listing them.
fn pages_of(emulator: &Emulator, engagement_arn: &str) -> Vec<PageRecord> {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    let mut pages: Vec<PageRecord> = state
        .pages
        .values()
        .filter(|page| page.engagement_arn == engagement_arn)
        .cloned()
        .collect();
    pages.sort_by_key(|page| page.sequence);
    pages
}

/// The page `page_arn` names, as the store holds it.
fn page_record(emulator: &Emulator, page_arn: &str) -> PageRecord {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    state
        .pages
        .get(page_arn)
        .cloned()
        .unwrap_or_else(|| panic!("the store holds no page {page_arn:?}"))
}

/// `DescribePage`, answering the status and the body.
async fn describe_page(emulator: &Emulator, page_arn: &str) -> (u16, Value) {
    emulator
        .call("DescribePage", json!({"PageId": page_arn}))
        .await
}

/// `DescribePage`, asserting it answered 200.
async fn described(emulator: &Emulator, page_arn: &str, case: &str) -> Value {
    let (status, body) = describe_page(emulator, page_arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribePage to answer 200, got {status} {body:?}"
    );
    body
}

/// Start a fresh engagement of the fixture's paged contact and answer its first page, with
/// the accept code Incident Manager sent with it and the channel it engaged.
async fn fresh_page(emulator: &Emulator, contact: &str, case: &str) -> PageRecord {
    let engagement = start_engagement(emulator, contact, case).await;
    let pages = pages_of(emulator, &engagement);
    pages
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: the start must have sent at least one page"))
}

/// Whole seconds since the epoch, as the wire reports a modelled timestamp.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs_f64()
}

/// `body[field]` as epoch seconds, when the response carries it.
fn wire_time(body: &Value, field: &str) -> Option<f64> {
    body.get(field).and_then(Value::as_f64)
}

// ---------------------------------------------------------------------------
// 1. A start pages every channel the plan targets
// ---------------------------------------------------------------------------

/// One row of the paging table: which contact the engagement names and the pages it must
/// send, as `(fixture channel index, the minute offset of the stage that engages it)`.
struct PagingCase {
    case: &'static str,
    engaged: Engaged,
    expected: &'static [(usize, u32)],
}

/// A stage's `DurationInMinutes` is "the time to wait until beginning the next stage", so a
/// stage's own targets are engaged as soon as that stage begins: the offsets below are the
/// fixture plans' durations SUMMED over the stages AHEAD of each target. `primary` waits 5
/// minutes after its first stage and 10 after its second, and the escalation plan waits 7
/// minutes before the stage that targets `primary` begins.
const PAGING_CASES: &[PagingCase] = &[
    PagingCase {
        case: "a three-stage plan pages each targeted channel once, at its stage's offset",
        engaged: Engaged::Primary,
        expected: &[(0, 0), (1, 5), (2, 15)],
    },
    PagingCase {
        case: "an escalation stage pages the channels of the contact it targets",
        engaged: Engaged::Escalation,
        expected: &[(0, 7), (1, 12), (2, 22)],
    },
    PagingCase {
        case: "a target naming a channel the store does not hold pages nothing",
        engaged: Engaged::Partial,
        expected: &[(2, 0)],
    },
    PagingCase {
        case: "a plan with no channel target pages nothing at all",
        engaged: Engaged::Silent,
        expected: &[],
    },
];

#[tokio::test]
async fn a_start_pages_every_contact_channel_its_plan_targets() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for row in PAGING_CASES {
        let case = row.case;
        let contact = fixture.contact(row.engaged).to_string();
        let before = now_secs();
        let engagement = start_engagement(&emulator, &contact, case).await;
        let after = now_secs();

        let pages = pages_of(&emulator, &engagement);
        assert_eq!(
            pages.len(),
            row.expected.len(),
            "case {case:?}: expected {} page(s), got {:?}",
            row.expected.len(),
            pages.iter().map(|page| page.arn.clone()).collect::<Vec<_>>()
        );

        for (page, (channel_index, offset_minutes)) in pages.iter().zip(row.expected) {
            let channel = &fixture.channels[*channel_index];
            assert_eq!(
                &page.contact_channel_arn, channel,
                "case {case:?}: expected the page to engage channel {channel:?}, got {:?}",
                page.contact_channel_arn
            );
            assert_eq!(
                page.contact_arn, fixture.primary,
                "case {case:?}: expected the page to name the contact that owns the channel, \
                 got {:?}",
                page.contact_arn
            );
            // Every page is readable through `DescribePage`, which is where a client sees it.
            let body = described(&emulator, &page.arn, case).await;
            let prefix = format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:page/oncall-primary/");
            assert!(
                page.arn.starts_with(&prefix),
                "case {case:?}: expected a PageArn starting {prefix:?}, got {:?}",
                page.arn
            );
            assert_eq!(
                body["PageArn"].as_str(),
                Some(page.arn.as_str()),
                "case {case:?}: expected the page to report its own ARN, got {body:?}"
            );
            assert_eq!(
                body["EngagementArn"].as_str(),
                Some(engagement.as_str()),
                "case {case:?}: expected the page to name its engagement, got {body:?}"
            );
            assert_eq!(
                body["ContactArn"].as_str(),
                Some(fixture.primary.as_str()),
                "case {case:?}: expected the page to name the paged contact, got {body:?}"
            );
            let sent = start_request(&contact);
            for member in [
                "Sender",
                "Subject",
                "Content",
                "PublicSubject",
                "PublicContent",
                "IncidentId",
            ] {
                assert_eq!(
                    body[member], sent[member],
                    "case {case:?}: expected the page to carry the {member} the start \
                     carried, got {body:?}"
                );
            }

            // The page is sent when the plan's own schedule engages its stage: the
            // engagement's start plus the durations of the stages ahead of it.
            let sent_time = wire_time(&body, "SentTime").unwrap_or_else(|| {
                panic!("case {case:?}: expected a SentTime, got {body:?}")
            });
            let offset = f64::from(*offset_minutes) * 60.0;
            assert!(
                sent_time >= before + offset - 1.0 && sent_time <= after + offset + 1.0,
                "case {case:?}: expected a SentTime inside \
                 [{}, {}], got {sent_time}",
                before + offset,
                after + offset
            );
            assert!(
                wire_time(&body, "DeliveryTime").is_none()
                    && wire_time(&body, "ReadTime").is_none(),
                "case {case:?}: an unacknowledged page carries neither time, got {body:?}"
            );
            assert!(
                page.receipts.is_empty(),
                "case {case:?}: an unacknowledged page records no receipt, got {:?}",
                page.receipts
            );
        }
    }
}

#[tokio::test]
async fn a_page_is_only_reported_for_the_engagement_that_sent_it() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    // The SAME channel paged by two engagements is two pages, each naming its own
    // engagement: nothing about the request distinguishes the two starts.
    let first = start_engagement(&emulator, &fixture.primary, "the first start").await;
    let second = start_engagement(&emulator, &fixture.primary, "the second start").await;
    let first_pages = pages_of(&emulator, &first);
    let second_pages = pages_of(&emulator, &second);

    assert_eq!(
        first_pages.len(),
        second_pages.len(),
        "two starts of one contact page the same channels, got {:?} and {:?}",
        first_pages.len(),
        second_pages.len()
    );
    for (left, right) in first_pages.iter().zip(&second_pages) {
        assert_eq!(
            left.contact_channel_arn, right.contact_channel_arn,
            "the two starts must page the channels in the same order, got {:?} and {:?}",
            left.contact_channel_arn, right.contact_channel_arn
        );
        assert_ne!(
            left.arn, right.arn,
            "case {:?}: two engagements of one channel are two pages, got one ARN {:?}",
            "two starts", left.arn
        );
        assert_ne!(
            left.accept_code, right.accept_code,
            "case {:?}: each page carries its own accept code, got {:?} twice",
            "two starts", left.accept_code
        );
        let body = described(&emulator, &left.arn, "the first start's page").await;
        assert_eq!(
            body["EngagementArn"].as_str(),
            Some(first.as_str()),
            "expected the first start's page to name the first engagement, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. AcceptPage records the acknowledgement
// ---------------------------------------------------------------------------

/// One row of the acknowledgement table.
struct AcceptCase {
    case: &'static str,
    /// The `AcceptType` the call carries, which decides WHICH instant is stamped.
    accept_type: &'static str,
    /// The `AcceptCodeValidation` the call carries, or `None` for a request that omits it.
    validation: Option<&'static str>,
    /// `true` when the call presents the code Incident Manager sent with the page; `false`
    /// presents another well-formed code, which only `IGNORE` accepts.
    present_sent_code: bool,
    /// The `Note` the call carries, sized along `ReceiptInfo`'s published bounds.
    note: Option<NoteSize>,
    /// Whether the call names the `ContactChannelId` of the channel the page engaged.
    name_channel: bool,
}

/// How long a row's `Note` is: the modelled `ReceiptInfo` admits 1..2048 characters.
#[derive(Debug, Clone, Copy)]
enum NoteSize {
    /// The published minimum length.
    Min,
    Typical,
    /// The published maximum length.
    Max,
}

impl NoteSize {
    fn text(self) -> String {
        match self {
            NoteSize::Min => "k".to_string(),
            NoteSize::Typical => "On it — scaling the write path".to_string(),
            NoteSize::Max => "n".repeat(2048),
        }
    }
}

const ACCEPT_CASES: &[AcceptCase] = &[
    AcceptCase {
        case: "DELIVERED with the sent code and no optional member",
        accept_type: "DELIVERED",
        validation: None,
        present_sent_code: true,
        note: None,
        name_channel: false,
    },
    AcceptCase {
        case: "READ with the sent code, a note and the paged channel named",
        accept_type: "READ",
        validation: None,
        present_sent_code: true,
        note: Some(NoteSize::Typical),
        name_channel: true,
    },
    AcceptCase {
        case: "DELIVERED with validation ENFORCEd and a one-character note",
        accept_type: "DELIVERED",
        validation: Some("ENFORCE"),
        present_sent_code: true,
        note: Some(NoteSize::Min),
        name_channel: true,
    },
    AcceptCase {
        case: "READ with validation ENFORCEd and a note at its published maximum length",
        accept_type: "READ",
        validation: Some("ENFORCE"),
        present_sent_code: true,
        note: Some(NoteSize::Max),
        name_channel: false,
    },
    AcceptCase {
        case: "DELIVERED with validation IGNOREd and a code that is not the sent one",
        accept_type: "DELIVERED",
        validation: Some("IGNORE"),
        present_sent_code: false,
        note: Some(NoteSize::Typical),
        name_channel: false,
    },
    AcceptCase {
        case: "READ with validation IGNOREd and a code that is not the sent one",
        accept_type: "READ",
        validation: Some("IGNORE"),
        present_sent_code: false,
        note: None,
        name_channel: true,
    },
];

/// A well-formed `AcceptCode` that is not the one sent with any page: ten digits, the
/// member's published maximum length.
const ANOTHER_CODE: &str = "1234567890";

#[tokio::test]
async fn an_acknowledgement_stamps_its_own_instant_and_records_a_receipt() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    // The rows claim to span the model's own value sets, so a value added to the model shows
    // up here rather than silently going untested.
    for value in ACCEPT_TYPE_VALUES {
        assert!(
            ACCEPT_CASES.iter().any(|row| row.accept_type == *value),
            "case {value:?}: the table must cover every modelled AcceptType, \
             ACCEPT_TYPE_VALUES is {ACCEPT_TYPE_VALUES:?}"
        );
    }
    for value in ACCEPT_CODE_VALIDATION_VALUES {
        assert!(
            ACCEPT_CASES.iter().any(|row| row.validation == Some(*value)),
            "case {value:?}: the table must cover every modelled AcceptCodeValidation, \
             ACCEPT_CODE_VALIDATION_VALUES is {ACCEPT_CODE_VALIDATION_VALUES:?}"
        );
    }

    for row in ACCEPT_CASES {
        let case = row.case;
        let page = fresh_page(&emulator, &fixture.primary, case).await;

        let mut request = json!({
            "PageId": page.arn,
            "AcceptType": row.accept_type,
            "AcceptCode": if row.present_sent_code {
                page.accept_code.clone()
            } else {
                ANOTHER_CODE.to_string()
            },
        });
        if let Some(validation) = row.validation {
            request["AcceptCodeValidation"] = json!(validation);
        }
        let note = row.note.map(NoteSize::text);
        if let Some(note) = &note {
            request["Note"] = json!(note);
        }
        if row.name_channel {
            request["ContactChannelId"] = json!(page.contact_channel_arn);
        }

        let before = now_secs();
        let (status, body) = emulator.call("AcceptPage", request).await;
        let after = now_secs();
        assert_eq!(
            status, 200,
            "case {case:?}: expected AcceptPage to answer 200, got {status} {body:?}"
        );

        // The acknowledged type's instant is stamped; the other stays absent.
        let described = described(&emulator, &page.arn, case).await;
        let (stamped, absent) = if row.accept_type == "READ" {
            ("ReadTime", "DeliveryTime")
        } else {
            ("DeliveryTime", "ReadTime")
        };
        let at = wire_time(&described, stamped).unwrap_or_else(|| {
            panic!("case {case:?}: expected a {stamped} after the acknowledgement, got {described:?}")
        });
        assert!(
            at >= before - 1.0 && at <= after + 1.0,
            "case {case:?}: expected a {stamped} inside [{before}, {after}], got {at}"
        );
        assert!(
            wire_time(&described, absent).is_none(),
            "case {case:?}: acknowledging {} must leave {absent} absent, got {described:?}",
            row.accept_type
        );

        // One receipt per acknowledgement, carrying the type and the note it named.
        let stored = page_record(&emulator, &page.arn);
        assert_eq!(
            stored.receipts.len(),
            1,
            "case {case:?}: expected exactly one receipt, got {:?}",
            stored.receipts
        );
        let receipt = &stored.receipts[0];
        assert_eq!(
            receipt.receipt_type, row.accept_type,
            "case {case:?}: expected a {} receipt, got {:?}",
            row.accept_type, receipt.receipt_type
        );
        assert_eq!(
            receipt.receipt_info, note,
            "case {case:?}: expected the receipt to carry the Note the call named, got {:?}",
            receipt.receipt_info
        );
        assert_eq!(
            receipt.contact_channel_arn, page.contact_channel_arn,
            "case {case:?}: expected the receipt to name the paged channel, got {:?}",
            receipt.contact_channel_arn
        );
    }
}

// ---------------------------------------------------------------------------
// 3. The stamped instant is the first acknowledgement of its type
// ---------------------------------------------------------------------------

/// The sequences of `AcceptType`s one caller can send against one page.
const ACKNOWLEDGEMENT_SEQUENCES: &[(&str, &[&str])] = &[
    ("the same type twice", &["DELIVERED", "DELIVERED"]),
    ("read twice", &["READ", "READ"]),
    ("delivered then read", &["DELIVERED", "READ"]),
    ("read then delivered", &["READ", "DELIVERED"]),
    (
        "the whole cycle, with a repeat of each type",
        &["DELIVERED", "READ", "DELIVERED", "READ"],
    ),
];

#[tokio::test]
async fn acknowledging_a_page_again_moves_no_stamped_instant() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for (case, sequence) in ACKNOWLEDGEMENT_SEQUENCES {
        let page = fresh_page(&emulator, &fixture.primary, case).await;
        // What the page reported the first time each type was acknowledged.
        let mut first_delivery: Option<f64> = None;
        let mut first_read: Option<f64> = None;

        for (index, accept_type) in sequence.iter().enumerate() {
            let (status, body) = emulator
                .call(
                    "AcceptPage",
                    json!({
                        "PageId": page.arn,
                        "AcceptType": accept_type,
                        "AcceptCode": page.accept_code,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected acknowledgement {index} to answer 200, \
                 got {status} {body:?}"
            );

            let described = described(&emulator, &page.arn, case).await;
            let delivery = wire_time(&described, "DeliveryTime");
            let read = wire_time(&described, "ReadTime");
            let expect_delivery = sequence[..=index].contains(&"DELIVERED");
            let expect_read = sequence[..=index].contains(&"READ");
            assert_eq!(
                delivery.is_some(),
                expect_delivery,
                "case {case:?}: after acknowledgement {index} a DeliveryTime must be \
                 {expect_delivery}, got {described:?}"
            );
            assert_eq!(
                read.is_some(),
                expect_read,
                "case {case:?}: after acknowledgement {index} a ReadTime must be \
                 {expect_read}, got {described:?}"
            );
            match first_delivery {
                None => first_delivery = delivery,
                Some(first) => assert_eq!(
                    delivery,
                    Some(first),
                    "case {case:?}: the DeliveryTime must stay the first one, got {delivery:?}"
                ),
            }
            match first_read {
                None => first_read = read,
                Some(first) => assert_eq!(
                    read,
                    Some(first),
                    "case {case:?}: the ReadTime must stay the first one, got {read:?}"
                ),
            }

            // Every acknowledgement is an event, so each appends its own receipt.
            let stored = page_record(&emulator, &page.arn);
            assert_eq!(
                stored.receipts.len(),
                index + 1,
                "case {case:?}: expected {} receipt(s) after acknowledgement {index}, got {:?}",
                index + 1,
                stored.receipts
            );
            let types: Vec<&str> = stored
                .receipts
                .iter()
                .map(|receipt| receipt.receipt_type.as_str())
                .collect();
            assert_eq!(
                types,
                sequence[..=index].to_vec(),
                "case {case:?}: expected the receipts in the order they were recorded, \
                 got {types:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. A refusal records nothing
// ---------------------------------------------------------------------------

/// One row of the refusal table: how the request differs from an acknowledgement that works,
/// and what the service must answer.
struct RefusalCase {
    case: &'static str,
    /// The member to overwrite, and its value — `""` names no overwrite.
    member: &'static str,
    value: fn(&PageRecord, &Fixture) -> Value,
    /// The status the call must answer, and the error code its body must name.
    status: u16,
    code: &'static str,
}

const REFUSAL_CASES: &[RefusalCase] = &[
    RefusalCase {
        case: "a PageId the store holds no page for",
        member: "PageId",
        value: |_, _| json!(absent_page_arn()),
        status: 404,
        code: "ResourceNotFoundException",
    },
    RefusalCase {
        case: "a code that is not the one sent with the page, with validation omitted",
        member: "AcceptCode",
        value: |_, _| json!(ANOTHER_CODE),
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "an AcceptCode one character below its published minimum length",
        member: "AcceptCode",
        value: |_, _| json!("12345"),
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "an AcceptCode one character past its published maximum length",
        member: "AcceptCode",
        value: |_, _| json!("12345678901"),
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "an AcceptCode outside its published pattern",
        member: "AcceptCode",
        value: |_, _| json!("12345a"),
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "an AcceptType that is not one of the modelled values",
        member: "AcceptType",
        value: |_, _| json!("SENT"),
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "an AcceptCodeValidation that is not one of the modelled values",
        member: "AcceptCodeValidation",
        value: |_, _| json!("ENFORCED"),
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "a ContactChannelId naming a channel this page did not engage",
        member: "ContactChannelId",
        value: |page, fixture| {
            let other = fixture
                .channels
                .iter()
                .find(|channel| **channel != page.contact_channel_arn)
                .expect("the fixture holds more than one channel");
            json!(other)
        },
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "a ContactChannelId naming a channel the store does not hold",
        member: "ContactChannelId",
        value: |_, _| json!(absent_channel_arn()),
        status: 400,
        code: "ValidationException",
    },
    RefusalCase {
        case: "a PageId that is not an ARN at all",
        member: "PageId",
        value: |_, _| json!("not-an-arn"),
        status: 400,
        code: "ValidationException",
    },
];

#[tokio::test]
async fn a_refused_acknowledgement_records_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for row in REFUSAL_CASES {
        let case = row.case;
        let page = fresh_page(&emulator, &fixture.primary, case).await;
        let mut request = json!({
            "PageId": page.arn,
            "AcceptType": "DELIVERED",
            "AcceptCode": page.accept_code,
        });
        request[row.member] = (row.value)(&page, &fixture);

        let (status, body) = emulator.call("AcceptPage", request).await;
        assert_eq!(
            status, row.status,
            "case {case:?}: expected HTTP {}, got {status} {body:?}",
            row.status
        );
        assert!(
            error_code(&body).contains(row.code),
            "case {case:?}: expected a {}, got {body:?}",
            row.code
        );

        let stored = page_record(&emulator, &page.arn);
        assert!(
            stored.delivery_time.is_none()
                && stored.read_time.is_none()
                && stored.receipts.is_empty(),
            "case {case:?}: a refused acknowledgement must record nothing, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn an_ignored_accept_code_accepts_any_well_formed_value() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    // The control for the refusal table: the same codes that are refused while validation is
    // enforced are accepted once the caller asks for `IGNORE`, which is the only difference
    // between the two calls. The values span the member's published length bounds.
    for code in ["000000", ANOTHER_CODE, "654321"] {
        let case = format!("IGNORE accepts the code {code:?}");
        let page = fresh_page(&emulator, &fixture.primary, &case).await;
        assert_ne!(
            page.accept_code, code,
            "case {case:?}: the row must present a code that is NOT the one sent"
        );

        let (status, body) = emulator
            .call(
                "AcceptPage",
                json!({
                    "PageId": page.arn,
                    "AcceptType": "DELIVERED",
                    "AcceptCode": code,
                    "AcceptCodeValidation": "IGNORE",
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected AcceptPage to answer 200, got {status} {body:?}"
        );

        let described = described(&emulator, &page.arn, &case).await;
        assert!(
            wire_time(&described, "DeliveryTime").is_some(),
            "case {case:?}: expected the acknowledgement to be recorded, got {described:?}"
        );
    }
}

#[tokio::test]
async fn a_describe_of_a_page_the_store_does_not_hold_answers_404() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    // A page ARN nothing sent, and the ARN of a resource of another kind: neither names a
    // page, so both are a not-found rather than a report about something else.
    for page_id in [absent_page_arn(), fixture.primary.clone(), fixture.channels[0].clone()] {
        let case = format!("DescribePage {page_id:?}");
        let (status, body) = describe_page(&emulator, &page_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 5. The account precondition, before the PageId is resolved
// ---------------------------------------------------------------------------

#[tokio::test]
async fn both_page_operations_are_gated_on_the_account_before_the_page_is_resolved() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let page = fresh_page(&emulator, &fixture.primary, "the gated page").await;
    let absent = absent_page_arn();

    // The two states that do not open the data plane: no replication set at all, and one
    // that exists but has not finished replicating.
    let inactive = service_com_amazonaws_ssmcontacts_ssmcontacts::state::AccountReplicationSet {
        active: false,
        ..active_replication_set()
    };
    for (account, replication_set) in [
        ("an account that was never onboarded", None),
        ("an account whose set is not ACTIVE", Some(inactive.clone())),
    ] {
        set_replication_set(&emulator, replication_set);
        for page_id in [page.arn.clone(), absent.clone()] {
            for operation in ["AcceptPage", "DescribePage"] {
                let case = format!("{operation} on {account}, page {page_id:?}");
                let request = if operation == "AcceptPage" {
                    json!({
                        "PageId": page_id,
                        "AcceptType": "DELIVERED",
                        "AcceptCode": page.accept_code,
                    })
                } else {
                    json!({"PageId": page_id})
                };
                let (status, body) = emulator.call(operation, request).await;
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected the account gate to answer 400, got {status} {body:?}"
                );
                assert!(
                    error_code(&body).contains("ValidationException"),
                    "case {case:?}: expected a ValidationException, got {body:?}"
                );
            }
        }
        let stored = page_record(&emulator, &page.arn);
        assert!(
            stored.receipts.is_empty() && stored.delivery_time.is_none(),
            "case {account:?}: a gated acknowledgement must record nothing, got {stored:?}"
        );
    }

    // Once the account is onboarded again the same calls answer about the page itself.
    onboard(&emulator);
    let (status, body) = emulator
        .call(
            "AcceptPage",
            json!({
                "PageId": page.arn,
                "AcceptType": "DELIVERED",
                "AcceptCode": page.accept_code,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "an onboarded account acknowledges its page, got {status} {body:?}"
    );
    let described = described(&emulator, &page.arn, "the onboarded account's page").await;
    assert!(
        wire_time(&described, "DeliveryTime").is_some(),
        "expected the acknowledgement to be recorded, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// 6. The plan walk is bounded, whatever chain the store holds
// ---------------------------------------------------------------------------

/// How long a chain of contact-to-contact plans the case below stores: past the walk's own
/// ceiling, so what the start follows is bounded by that ceiling rather than by the chain
/// ending.
const CHAIN_LENGTH: usize = MAX_PLANS_PER_ENGAGEMENT + 20;

/// The link whose channel is inside the ceiling, and the one past it.
const NEAR_LINK: usize = 1;
const FAR_LINK: usize = CHAIN_LENGTH - 1;

#[tokio::test]
async fn a_chain_of_plans_longer_than_the_ceiling_is_walked_to_it_and_the_start_returns() {
    let emulator = Emulator::new();
    onboard(&emulator);

    // Every link's plan targets the NEXT link, and the last link targets the first, so the
    // chain is also a cycle: a walk that followed it without a ceiling would not stop.
    let mut links: Vec<String> = Vec::new();
    for index in 0..CHAIN_LENGTH {
        links.push(
            emulator
                .create_contact(
                    json!({
                        "Alias": format!("chain-{index:04}"),
                        "Type": "ESCALATION",
                        "Plan": {"Stages": []},
                    }),
                    "a link of the plan chain",
                )
                .await,
        );
    }

    // One channel inside the ceiling and one past it, so what the walk reached is observable.
    let mut channels: Vec<(usize, String)> = Vec::new();
    for index in [NEAR_LINK, FAR_LINK] {
        let (status, body) = emulator
            .call(
                "CreateContactChannel",
                json!({
                    "ContactId": links[index],
                    "Name": "email",
                    "Type": "EMAIL",
                    "DeliveryAddress": {"SimpleAddress": format!("link-{index}@example.com")},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "the chain's channel at link {index} must be created, got {status} {body:?}"
        );
        channels.push((
            index,
            body["ContactChannelArn"]
                .as_str()
                .unwrap_or_else(|| panic!("expected a ContactChannelArn, got {body:?}"))
                .to_string(),
        ));
    }

    for index in 0..CHAIN_LENGTH {
        let next = links[(index + 1) % CHAIN_LENGTH].clone();
        let mut targets = vec![json!({"ContactTargetInfo": {
            "ContactId": next,
            "IsEssential": false,
        }})];
        if let Some((_, channel)) = channels.iter().find(|(link, _)| *link == index) {
            targets.push(channel_target(channel));
        }
        set_plan(
            &emulator,
            &links[index],
            json!({"Stages": [{"DurationInMinutes": 0, "Targets": targets}]}),
            "a link of the plan chain",
        )
        .await;
    }

    // The start RETURNS — the walk is a bounded queue, not a descent whose depth the chain
    // chooses — and it paged only the channel inside the ceiling.
    let engagement = start_engagement(&emulator, &links[0], "the chained plan").await;
    let pages = pages_of(&emulator, &engagement);
    let near = &channels[0].1;
    let far = &channels[1].1;
    let paged: Vec<String> = pages
        .iter()
        .map(|page| page.contact_channel_arn.clone())
        .collect();
    assert_eq!(
        paged,
        vec![near.clone()],
        "expected the start to page only the channel inside the walk's ceiling, got {paged:?}"
    );
    assert!(
        !paged.contains(far),
        "expected the channel past the ceiling to stay unpaged, got {paged:?}"
    );
}
