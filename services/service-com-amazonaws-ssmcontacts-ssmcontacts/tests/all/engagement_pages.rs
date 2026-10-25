// SPDX-License-Identifier: CC0-1.0

//! `ListPagesByEngagement`, end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1 request
//! (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the contact suite
//! builds, so what is asserted is what a signing AWS client would see.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one call:
//!
//! 1. an engagement engages a contact by walking that contact's engagement plan: every
//!    `ChannelTargetInfo` a stage names is ONE page to that contact channel, every
//!    `ContactTargetInfo` hands the remaining plan to that contact, and a stage's targets are
//!    engaged only once the stages before it have run out their `DurationInMinutes`. The
//!    listing reports exactly the pages already sent, in the order the plan engaged them,
//!    each carrying the engagement it belongs to and the contact whose channel it engaged.
//!    The rows span one and several targets in a stage, a target in a later stage, two stages
//!    that begin at the same instant, one channel targeted twice, a hand-off to another
//!    contact, a plan that names itself, a plan carrying `RotationIds` instead of stages, a
//!    stage with no targets, and both an activated and an unactivated channel; a target left
//!    dangling by deleting the channel it names has its own case, since `UpdateContact`
//!    refuses a plan that names a channel the store does not hold;
//! 2. the listing is paginated: no page holds more than the `MaxResults` the request named,
//!    an opaque `NextToken` resumes exactly after the last page's final entry, the last page
//!    carries none, and paging to exhaustion reports every page once in scheduling order. The
//!    rows span `MaxResults` absent, its published minimum (0, which names no page size at
//!    all), sizes below, equal to and above the number of pages, and its published maximum;
//! 3. a page the clock has not reached is not reported, and a page whose stage the engagement
//!    was stopped before reaching is not reported ever after — stopping an engagement stops
//!    the stages it had not reached. The rows span a send instant seconds and an hour either
//!    side of the stop, and the stop absent altogether;
//! 4. a page reports the acknowledgement instants `AcceptPage` recorded against it and
//!    nothing before that: a page nobody has answered carries neither `DeliveryTime` nor
//!    `ReadTime`;
//! 5. a listing reports the pages of the engagement it NAMES and no other's, over a store
//!    holding several engagements of several contacts;
//! 6. a request that cannot be served is refused with the modelled error: an engagement the
//!    store does not hold with `ResourceNotFoundException` (HTTP 404), a `NextToken` this
//!    listing did not issue with `ValidationException` (HTTP 400), and — before the
//!    `EngagementId` is resolved at all — an account with no ACTIVE Incident Manager
//!    replication set with `ValidationException` (HTTP 400).
//!
//! The listing is also a READ, which the first suite asserts for every row: repeating it
//! reports the same pages and leaves the store exactly as it was.
//!
//! The onboarded account every row needs comes from the contact suite's own fixture
//! (`contact_updates::onboard`, which `Emulator::new` calls): Incident Manager's
//! `CreateReplicationSet` is deprecated and refuses every call, so the store is the only
//! place an onboarded account can come from.

use std::collections::BTreeMap;
use std::time::SystemTime;

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    encode_next_token, service_state, AccountReplicationSet, TOKEN_SCOPE_CONTACTS,
};

use crate::contact_lifecycle::{error_code, parse, Emulator, ACCOUNT, REGION};
use crate::contact_updates::set_replication_set;
// A page's send instant is the plan's own schedule, which no modelled call moves, so the stop
// suite's store writer and its clock offset are what a row uses to place one.
use crate::engagement_stop_pages::{from_now, set_sent_time};

/// The `Sender` every engagement in this suite carries.
const SENDER: &str = "cli";

/// The `IncidentId` every engagement in this suite carries, so a page's own `IncidentId` is
/// asserted against a value that was not defaulted.
const INCIDENT: &str = "incident-7";

// ---------------------------------------------------------------------------
// The fixture: contacts, their channels, and the plan that targets them
// ---------------------------------------------------------------------------

/// A contact channel to create under a fixture contact.
struct ChannelFixture {
    name: &'static str,
    /// One of the model's `ChannelType` values.
    channel_type: &'static str,
    address: &'static str,
    /// Whether the channel is activated before the engagement starts. Only an ACTIVATED
    /// channel receives an engagement, which is what a reported `DeliveryTime` turns on.
    activated: bool,
}

/// A contact to create, its channels, and the plan it ends up carrying.
///
/// The plan is a TEMPLATE: `{contact:<alias>}` stands for that contact's `ContactArn` and
/// `{channel:<alias>/<name>}` for that channel's `ContactChannelArn`, so a row can name
/// resources that do not exist until the fixture has created them.
struct ContactFixture {
    alias: &'static str,
    /// One of the model's `ContactType` values.
    contact_type: &'static str,
    channels: &'static [ChannelFixture],
    plan: &'static str,
}

/// A row of the page-scheduling table.
struct PlanRow {
    case: &'static str,
    contacts: &'static [ContactFixture],
    /// The alias of the contact the engagement names.
    engage: &'static str,
    /// The channels expected to have been paged by now, as `(contact alias, channel name)`
    /// in the order the plan engaged them. One engagement engages a channel once, so a pair
    /// appears at most once however many stages target that channel.
    expected: &'static [(&'static str, &'static str)],
}

/// An email channel, activated before the engagement starts.
const EMAIL: ChannelFixture = ChannelFixture {
    name: "email",
    channel_type: "EMAIL",
    address: "ops@example.com",
    activated: true,
};

/// An SMS channel whose activation is still outstanding.
const SMS: ChannelFixture = ChannelFixture {
    name: "sms",
    channel_type: "SMS",
    address: "+15550100",
    activated: false,
};

/// The on-call rotation the rotation-plan row names, created by the fixture: a plan's
/// `RotationIds` entry has to name a rotation the store holds.
const ROTATION: &str = "weekend";

const PLAN_ROWS: &[PlanRow] = &[
    PlanRow {
        case: "one stage naming two channel targets pages both, in plan order",
        contacts: &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL, SMS],
            plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}},
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/sms}",
                                       "RetryIntervalInMinutes": 2}}
            ]}]}"#,
        }],
        engage: "oncall-primary",
        expected: &[("oncall-primary", "email"), ("oncall-primary", "sms")],
    },
    PlanRow {
        case: "a target in a later stage has not been engaged yet",
        contacts: &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL, SMS],
            plan: r#"{"Stages": [
                {"DurationInMinutes": 5, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
                ]},
                {"DurationInMinutes": 30, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/sms}"}}
                ]}
            ]}"#,
        }],
        engage: "oncall-primary",
        expected: &[("oncall-primary", "email")],
    },
    PlanRow {
        case: "a stage that waits for nothing begins at the same instant as the one before it",
        contacts: &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL, SMS],
            // `DurationInMinutes` 0 is the member's published minimum, and the model admits
            // it when the stage names a target.
            plan: r#"{"Stages": [
                {"DurationInMinutes": 0, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
                ]},
                {"DurationInMinutes": 0, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/sms}"}}
                ]}
            ]}"#,
        }],
        engage: "oncall-primary",
        expected: &[("oncall-primary", "email"), ("oncall-primary", "sms")],
    },
    PlanRow {
        case: "one channel targeted twice is engaged once by one engagement",
        contacts: &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL],
            plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}},
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
            ]}]}"#,
        }],
        engage: "oncall-primary",
        expected: &[("oncall-primary", "email")],
    },
    PlanRow {
        case: "an escalation plan hands the stage off to a contact, whose channels are paged",
        contacts: &[
            ContactFixture {
                alias: "escalation-tier-1",
                contact_type: "ESCALATION",
                channels: &[],
                plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                    {"ContactTargetInfo": {"ContactId": "{contact:oncall-primary}",
                                           "IsEssential": true}}
                ]}]}"#,
            },
            ContactFixture {
                alias: "oncall-primary",
                contact_type: "PERSONAL",
                channels: &[EMAIL, SMS],
                plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}},
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/sms}"}}
                ]}]}"#,
            },
        ],
        engage: "escalation-tier-1",
        expected: &[("oncall-primary", "email"), ("oncall-primary", "sms")],
    },
    PlanRow {
        case: "a plan that hands off to itself still pages each channel once",
        contacts: &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL],
            plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}},
                {"ContactTargetInfo": {"ContactId": "{contact:oncall-primary}",
                                       "IsEssential": false}}
            ]}]}"#,
        }],
        engage: "oncall-primary",
        expected: &[("oncall-primary", "email")],
    },
    PlanRow {
        case: "a plan carrying rotation ids instead of stages pages nothing",
        contacts: &[ContactFixture {
            alias: "weekend-rotation",
            contact_type: "ONCALL_SCHEDULE",
            channels: &[EMAIL],
            plan: r#"{"RotationIds": ["{rotation:weekend}"]}"#,
        }],
        engage: "weekend-rotation",
        expected: &[],
    },
    PlanRow {
        case: "a stage with no targets pages nothing",
        contacts: &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL],
            plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": []}]}"#,
        }],
        engage: "oncall-primary",
        expected: &[],
    },
];

// ---------------------------------------------------------------------------
// Seeding
// ---------------------------------------------------------------------------

/// The ARNs a fixture minted, so a row's plan template and its expectations can be resolved.
#[derive(Debug, Default)]
struct Seeded {
    /// Contact alias -> `ContactArn`.
    contacts: BTreeMap<String, String>,
    /// `<alias>/<channel name>` -> `ContactChannelArn`.
    channels: BTreeMap<String, String>,
    /// Rotation name -> `RotationArn`.
    rotations: BTreeMap<String, String>,
}

impl Seeded {
    fn contact(&self, alias: &str) -> &str {
        self.contacts
            .get(alias)
            .unwrap_or_else(|| panic!("the fixture created no contact {alias:?}"))
    }

    fn channel(&self, alias: &str, name: &str) -> &str {
        let key = format!("{alias}/{name}");
        self.channels
            .get(&key)
            .unwrap_or_else(|| panic!("the fixture created no channel {key:?}"))
    }

    /// A plan template with every placeholder replaced by the ARN it stands for.
    fn resolve(&self, template: &str) -> Value {
        let mut text = template.to_string();
        for (alias, arn) in &self.contacts {
            text = text.replace(&format!("{{contact:{alias}}}"), arn);
        }
        for (key, arn) in &self.channels {
            text = text.replace(&format!("{{channel:{key}}}"), arn);
        }
        for (name, arn) in &self.rotations {
            text = text.replace(&format!("{{rotation:{name}}}"), arn);
        }
        for marker in ["{contact:", "{channel:", "{rotation:"] {
            assert!(
                !text.contains(marker),
                "the plan template left {marker} unresolved: {text}"
            );
        }
        parse(&text)
    }
}

/// Create every fixture contact, its channels and then the plan that targets them.
///
/// The plan is set by `UpdateContact` rather than by the create, because a plan names
/// contact channels and a channel cannot exist before the contact that holds it.
async fn seed(emulator: &Emulator, contacts: &[ContactFixture], case: &str) -> Seeded {
    let mut seeded = Seeded::default();
    for fixture in contacts {
        let arn = emulator
            .create_contact(
                json!({
                    "Alias": fixture.alias,
                    "Type": fixture.contact_type,
                    "Plan": {"Stages": []},
                }),
                case,
            )
            .await;
        seeded.contacts.insert(fixture.alias.to_string(), arn);
    }
    for fixture in contacts {
        for channel in fixture.channels {
            let (status, body) = emulator
                .call(
                    "CreateContactChannel",
                    json!({
                        "ContactId": seeded.contact(fixture.alias),
                        "Name": channel.name,
                        "Type": channel.channel_type,
                        "DeliveryAddress": {"SimpleAddress": channel.address},
                        // An activated channel needs the code the create delivers; a channel
                        // that stays unactivated is created with the delivery deferred.
                        "DeferActivation": !channel.activated,
                    }),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected CreateContactChannel to answer 200, got {status} {body:?}"
            );
            let arn = body["ContactChannelArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a ContactChannelArn, got {body:?}")
                })
                .to_string();
            if channel.activated {
                activate(emulator, &arn, case).await;
            }
            seeded
                .channels
                .insert(format!("{}/{}", fixture.alias, channel.name), arn);
        }
    }
    // A plan's `RotationIds` entry has to name a rotation the store holds, so a row whose
    // plan carries one gets that rotation created first, with the row's own first contact as
    // its shift team.
    if contacts
        .iter()
        .any(|fixture| fixture.plan.contains("{rotation:"))
    {
        let team = seeded.contact(contacts[0].alias).to_string();
        let arn = emulator.create_rotation(&team, ROTATION, case).await;
        seeded.rotations.insert(ROTATION.to_string(), arn);
    }
    for fixture in contacts {
        let plan = seeded.resolve(fixture.plan);
        let (status, body) = emulator
            .call(
                "UpdateContact",
                json!({"ContactId": seeded.contact(fixture.alias), "Plan": plan}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected UpdateContact to answer 200, got {status} {body:?}"
        );
    }
    seeded
}

/// Activate a contact channel with the code its create delivered to it.
///
/// The code is never put on the wire — AWS delivers it out of band — so the store is where a
/// client's own activation step reads it from in this emulator.
async fn activate(emulator: &Emulator, channel_arn: &str, case: &str) {
    let code = {
        let store =
            service_state(&*emulator.engine).expect("the engine holds this service's state");
        let store = store.lock();
        store
            .contact_channels
            .get(channel_arn)
            .and_then(|channel| channel.activation_code.clone())
            .unwrap_or_else(|| {
                panic!("case {case:?}: no activation code was delivered to {channel_arn}")
            })
    };
    let (status, body) = emulator
        .call(
            "ActivateContactChannel",
            json!({"ContactChannelId": channel_arn, "ActivationCode": code}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ActivateContactChannel to answer 200, got {status} {body:?}"
    );
}

/// Start an engagement of `contact_arn` and answer its `EngagementArn`.
async fn start_engagement(emulator: &Emulator, contact_arn: &str, case: &str) -> String {
    let (status, body) = emulator
        .call(
            "StartEngagement",
            json!({
                "ContactId": contact_arn,
                "Sender": SENDER,
                "Subject": "disk pressure",
                "Content": "node-7 is at 95%",
                "IncidentId": INCIDENT,
            }),
        )
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

// ---------------------------------------------------------------------------
// Reading the listing
// ---------------------------------------------------------------------------

/// One `ListPagesByEngagement` call, asserted to have answered 200.
async fn list_pages(emulator: &Emulator, request: Value, case: &str) -> Value {
    let (status, body) = emulator
        .call("ListPagesByEngagement", request)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListPagesByEngagement to answer 200, got {status} {body:?}"
    );
    body
}

/// Every page `ListPagesByEngagement` reports for `request`, paging to exhaustion.
///
/// Also asserts no page holds more entries than the `MaxResults` the request names, and that
/// the paging terminates.
async fn all_pages(emulator: &Emulator, request: Value, case: &str) -> Vec<Value> {
    let mut request = request;
    let limit = request["MaxResults"].as_i64().filter(|limit| *limit > 0);
    let mut pages: Vec<Value> = Vec::new();
    loop {
        let body = list_pages(emulator, request.clone(), case).await;
        let reported = body["Pages"].as_array().cloned().unwrap_or_else(|| {
            panic!("case {case:?}: expected Pages to be a list, got {body:?}")
        });
        if let Some(limit) = limit {
            assert!(
                reported.len() as i64 <= limit,
                "case {case:?}: a page held {} entries for MaxResults={limit}",
                reported.len()
            );
        }
        pages.extend(reported);
        match body["NextToken"].as_str() {
            Some(token) => {
                assert!(
                    pages.len() <= 64,
                    "case {case:?}: paging did not terminate, collected {} entries",
                    pages.len()
                );
                request["NextToken"] = json!(token);
            }
            None => return pages,
        }
    }
}

/// Every stored page's `(PageArn, ContactChannelArn, sent instant)`, in scheduling order.
///
/// The channel a page engaged is not a modelled member of `Page`, so WHICH channel each
/// reported page went to is asserted against the store.
fn stored_pages(emulator: &Emulator) -> Vec<(String, String, SystemTime)> {
    let store = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    let mut pages: Vec<_> = store
        .pages
        .values()
        .map(|page| {
            (
                page.sequence,
                page.arn.clone(),
                page.contact_channel_arn.clone(),
                page.sent_time,
            )
        })
        .collect();
    pages.sort_by_key(|(sequence, ..)| *sequence);
    pages
        .into_iter()
        .map(|(_, arn, channel, sent_at)| (arn, channel, sent_at))
        .collect()
}

/// The string member `name` of a reported page.
fn member<'a>(page: &'a Value, name: &str, case: &str) -> &'a str {
    page[name]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} on the page, got {page:?}"))
}

// ---------------------------------------------------------------------------
// Rule 1: the pages an engagement sends come from the engaged contact's plan
// ---------------------------------------------------------------------------

#[tokio::test]
async fn each_engagement_reports_only_its_own_pages() {
    let case = "two engagements in one store";
    let emulator = Emulator::new();
    let seeded = seed(
        &emulator,
        &[
            ContactFixture {
                alias: "oncall-primary",
                contact_type: "PERSONAL",
                channels: &[EMAIL],
                plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
                ]}]}"#,
            },
            ContactFixture {
                alias: "pager_backup_2",
                contact_type: "PERSONAL",
                channels: &[EMAIL, SMS],
                plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:pager_backup_2/email}"}},
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:pager_backup_2/sms}"}}
                ]}]}"#,
            },
        ],
        case,
    )
    .await;

    // Three pages in ONE store, from two engagements of two different contacts, plus a
    // second engagement of the first contact: a listing that ignored its `EngagementId`
    // would report all four.
    let first = start_engagement(&emulator, seeded.contact("oncall-primary"), case).await;
    let second = start_engagement(&emulator, seeded.contact("pager_backup_2"), case).await;
    let third = start_engagement(&emulator, seeded.contact("oncall-primary"), case).await;
    assert_eq!(
        stored_pages(&emulator).len(),
        4,
        "case {case:?}: expected the three starts to have paged four channels in all"
    );

    for (engagement, expected) in [
        (&first, vec![seeded.channel("oncall-primary", "email")]),
        (
            &second,
            vec![
                seeded.channel("pager_backup_2", "email"),
                seeded.channel("pager_backup_2", "sms"),
            ],
        ),
        (&third, vec![seeded.channel("oncall-primary", "email")]),
    ] {
        let reported = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
        for page in &reported {
            assert_eq!(
                member(page, "EngagementArn", case),
                engagement.as_str(),
                "case {case:?}: expected only this engagement's pages, got {page:?}"
            );
        }
        // The channels the reported pages engaged, read from the store since no modelled
        // member carries them.
        let scheduled = stored_pages(&emulator);
        let engaged: Vec<String> = reported
            .iter()
            .map(|page| {
                let arn = member(page, "PageArn", case);
                scheduled
                    .iter()
                    .find(|(stored, ..)| stored == arn)
                    .map(|(_, channel, _)| channel.clone())
                    .unwrap_or_else(|| panic!("case {case:?}: the store holds no page {arn:?}"))
            })
            .collect();
        assert_eq!(
            engaged, expected,
            "case {case:?}: expected engagement {engagement} to report exactly its own pages"
        );
    }
}

#[tokio::test]
async fn two_stages_handing_off_to_one_contact_engage_it_once() {
    let case = "two hand-offs to the same contact";
    let emulator = Emulator::new();
    let seeded = seed(
        &emulator,
        &[
            ContactFixture {
                alias: "escalation-tier-1",
                contact_type: "ESCALATION",
                channels: &[],
                // Both stages hand off to the SAME contact, the second only after the
                // first's five-minute wait.
                plan: r#"{"Stages": [
                    {"DurationInMinutes": 5, "Targets": [
                        {"ContactTargetInfo": {"ContactId": "{contact:oncall-primary}",
                                               "IsEssential": true}}
                    ]},
                    {"DurationInMinutes": 0, "Targets": [
                        {"ContactTargetInfo": {"ContactId": "{contact:oncall-primary}",
                                               "IsEssential": true}}
                    ]}
                ]}"#,
            },
            ContactFixture {
                alias: "oncall-primary",
                contact_type: "PERSONAL",
                channels: &[EMAIL],
                plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                    {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
                ]}]}"#,
            },
        ],
        case,
    )
    .await;
    let engagement =
        start_engagement(&emulator, seeded.contact("escalation-tier-1"), case).await;

    // One engagement engages a channel once: the second hand-off reaches a contact this
    // engagement has already engaged, so it adds no page — and the one page it does hold was
    // sent at the first hand-off's instant, which is the start.
    let scheduled = stored_pages(&emulator);
    assert_eq!(
        scheduled.len(),
        1,
        "case {case:?}: expected the engagement to engage the channel once, got {scheduled:?}"
    );
    let reported = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected that one page to have been sent already, got {reported:?}"
    );
    assert_eq!(
        member(&reported[0], "ContactArn", case),
        seeded.contact("oncall-primary"),
        "case {case:?}: expected the hand-off contact's own channel to be paged, \
         got {reported:?}"
    );
}

#[tokio::test]
async fn an_engagement_pages_the_channels_its_plan_targets() {
    for row in PLAN_ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let seeded = seed(&emulator, row.contacts, case).await;

        let before = SystemTime::now();
        let engagement = start_engagement(&emulator, seeded.contact(row.engage), case).await;
        let after = SystemTime::now();

        let scheduled = stored_pages(&emulator);
        let reported = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;

        assert_eq!(
            reported.len(),
            row.expected.len(),
            "case {case:?}: expected {} pages, got {reported:?}",
            row.expected.len()
        );

        for (index, (page, (alias, channel))) in reported.iter().zip(row.expected).enumerate() {
            let case = format!("{case} [page {index}]");
            let case = case.as_str();
            assert_eq!(
                member(page, "EngagementArn", case),
                engagement,
                "case {case:?}: expected the page to name the engagement it is part of, \
                 got {page:?}"
            );
            assert_eq!(
                member(page, "ContactArn", case),
                seeded.contact(alias),
                "case {case:?}: expected the paged channel's own contact, got {page:?}"
            );
            assert_eq!(
                member(page, "Sender", case),
                SENDER,
                "case {case:?}: expected the engagement's Sender, got {page:?}"
            );
            assert_eq!(
                member(page, "IncidentId", case),
                INCIDENT,
                "case {case:?}: expected the engagement's IncidentId, got {page:?}"
            );

            // The ARN is the service's own page ARN, scoped to the request's Region and
            // naming the paged contact.
            let prefix = format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:page/{alias}/");
            let page_arn = member(page, "PageArn", case);
            assert!(
                page_arn.starts_with(&prefix),
                "case {case:?}: expected a PageArn starting {prefix:?}, got {page_arn:?}"
            );

            // A page is sent when its stage begins, which for every expected page here is
            // the instant the engagement started.
            let sent = page["SentTime"].as_f64().unwrap_or_else(|| {
                panic!("case {case:?}: expected a SentTime on the page, got {page:?}")
            });
            let window = (
                before.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64() - 1.0,
                after.duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs_f64() + 1.0,
            );
            assert!(
                sent >= window.0 && sent <= window.1,
                "case {case:?}: expected a SentTime inside {window:?}, got {sent}"
            );

            // Nothing has answered the page, so it carries no acknowledgement instant:
            // `AcceptPage` is what records one, whether the channel is activated or not.
            assert_eq!(
                page["DeliveryTime"],
                Value::Null,
                "case {case:?}: expected no DeliveryTime on an unanswered page, got {page:?}"
            );
            assert_eq!(
                page["ReadTime"],
                Value::Null,
                "case {case:?}: expected no ReadTime on an unanswered page, got {page:?}"
            );

            // WHICH channel the page engaged is not on the wire, so it is read from the
            // store the handler read.
            let stored = scheduled
                .iter()
                .find(|(arn, ..)| arn == page_arn)
                .unwrap_or_else(|| panic!("case {case:?}: the store holds no page {page_arn:?}"));
            assert_eq!(
                stored.1,
                seeded.channel(alias, channel),
                "case {case:?}: expected the page to have engaged {alias}/{channel}, \
                 got {stored:?}"
            );
        }

        let arns: Vec<&str> = reported
            .iter()
            .map(|page| member(page, "PageArn", case))
            .collect();
        let mut unique = arns.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            arns.len(),
            "case {case:?}: expected every page to carry its own PageArn, got {arns:?}"
        );

        // The listing is a read: a second call reports the same pages and the store still
        // holds exactly what the engagement scheduled.
        let again = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
        assert_eq!(
            again, reported,
            "case {case:?}: expected a repeated listing to report the same pages"
        );
        assert_eq!(
            stored_pages(&emulator),
            scheduled,
            "case {case:?}: expected the listing to leave the stored pages untouched"
        );
    }
}

/// A plan target can only name a channel the store held when the plan was set — `UpdateContact`
/// refuses one it does not — so the dangling target this asserts on comes from the one call
/// that produces it: deleting the channel the plan already targets.
#[tokio::test]
async fn a_target_whose_channel_was_deleted_pages_nothing() {
    let case = "a stage targeting a channel that was deleted after the plan was set";
    let emulator = Emulator::new();
    let seeded = seed(
        &emulator,
        &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL, SMS],
            plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}},
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/sms}"}}
            ]}]}"#,
        }],
        case,
    )
    .await;

    let (status, body) = emulator
        .call(
            "DeleteContactChannel",
            json!({"ContactChannelId": seeded.channel("oncall-primary", "email")}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DeleteContactChannel to answer 200, got {status} {body:?}"
    );

    let engagement = start_engagement(&emulator, seeded.contact("oncall-primary"), case).await;
    let reported = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected only the surviving channel's page, got {reported:?}"
    );

    // The one page that was sent engaged the channel the delete left standing.
    let page_arn = member(&reported[0], "PageArn", case);
    let stored = stored_pages(&emulator);
    let engaged = stored
        .iter()
        .find(|(arn, ..)| arn == page_arn)
        .unwrap_or_else(|| panic!("case {case:?}: the store holds no page {page_arn:?}"));
    assert_eq!(
        engaged.1,
        seeded.channel("oncall-primary", "sms"),
        "case {case:?}: expected the page to have engaged the channel that was not deleted, \
         got {engaged:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 2: pagination
// ---------------------------------------------------------------------------

/// How many channels the pagination fixture targets in one stage.
const PAGINATED_CHANNELS: usize = 5;

#[tokio::test]
async fn the_listing_pages_through_every_page_of_the_engagement() {
    let emulator = Emulator::new();
    let contact = emulator
        .create_contact(
            json!({
                "Alias": "oncall-primary",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
            }),
            "the paginated engagement",
        )
        .await;

    let mut channels: Vec<String> = Vec::new();
    for index in 0..PAGINATED_CHANNELS {
        let (status, body) = emulator
            .call(
                "CreateContactChannel",
                json!({
                    "ContactId": contact,
                    "Name": format!("email-{index}"),
                    "Type": "EMAIL",
                    "DeliveryAddress": {"SimpleAddress": format!("ops-{index}@example.com")},
                    "DeferActivation": true,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {index}: expected CreateContactChannel to answer 200, got {status} {body:?}"
        );
        channels.push(body["ContactChannelArn"].as_str().unwrap().to_string());
    }
    let targets: Vec<Value> = channels
        .iter()
        .map(|arn| json!({"ChannelTargetInfo": {"ContactChannelId": arn}}))
        .collect();
    let (status, body) = emulator
        .call(
            "UpdateContact",
            json!({
                "ContactId": contact,
                "Plan": {"Stages": [{"DurationInMinutes": 0, "Targets": targets}]},
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "expected UpdateContact to answer 200, got {status} {body:?}"
    );

    let engagement = start_engagement(&emulator, &contact, "the paginated engagement").await;
    let everything = all_pages(
        &emulator,
        json!({"EngagementId": engagement}),
        "no MaxResults",
    )
    .await;
    assert_eq!(
        everything.len(),
        PAGINATED_CHANNELS,
        "expected one page per channel target, got {everything:?}"
    );

    // `MaxResults` is modelled `range 0..1024`: the rows sit at its published minimum (0,
    // which names no page size at all), below, at and above the number of pages, and at its
    // published maximum.
    for max_results in [None, Some(0), Some(1), Some(2), Some(5), Some(6), Some(1024)] {
        let case = format!("MaxResults={max_results:?}");
        let case = case.as_str();
        let mut request = json!({"EngagementId": engagement});
        if let Some(max_results) = max_results {
            request["MaxResults"] = json!(max_results);
        }
        let reported = all_pages(&emulator, request, case).await;
        assert_eq!(
            reported, everything,
            "case {case:?}: expected paging to report every page once, in scheduling order"
        );
    }

    // The page a `NextToken` resumes from starts exactly after the previous page's last
    // entry, so a client that walks one entry at a time sees each page exactly once.
    let first = list_pages(
        &emulator,
        json!({"EngagementId": engagement, "MaxResults": 1}),
        "a one-entry page",
    )
    .await;
    assert_eq!(
        first["Pages"].as_array().map(Vec::len),
        Some(1),
        "expected a single-entry page, got {first:?}"
    );
    let token = first["NextToken"]
        .as_str()
        .unwrap_or_else(|| panic!("expected a NextToken on a partial page, got {first:?}"));
    let second = list_pages(
        &emulator,
        json!({"EngagementId": engagement, "MaxResults": 1, "NextToken": token}),
        "the resumed page",
    )
    .await;
    assert_eq!(
        second["Pages"][0], everything[1],
        "expected the resumed page to start after the first page's last entry, got {second:?}"
    );

    // The last page carries no token at all, which is what ends a client's loop.
    let last = list_pages(
        &emulator,
        json!({"EngagementId": engagement, "MaxResults": 1024}),
        "the only page",
    )
    .await;
    assert_eq!(
        last["NextToken"],
        Value::Null,
        "expected the final page to carry no NextToken, got {last:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 3: a page that has not been sent is not reported
// ---------------------------------------------------------------------------

/// A row of the clock-and-stop table: where the second stage's page lies relative to now,
/// and whether the engagement has been stopped before the listing.
struct StopRow {
    case: &'static str,
    /// Where the second stage's page was sent, in seconds from now: a negative value is a
    /// page the engagement had already sent, a positive one a stage still ahead of the clock.
    second_page_at_secs: i64,
    /// Whether `StopEngagement` is called before the listing.
    stopped: bool,
    expected_pages: usize,
}

const STOP_ROWS: &[StopRow] = &[
    StopRow {
        case: "a running engagement does not report a stage the clock has not reached",
        second_page_at_secs: 300,
        stopped: false,
        expected_pages: 1,
    },
    StopRow {
        case: "a running engagement reports both pages once both stages have begun",
        second_page_at_secs: -300,
        stopped: false,
        expected_pages: 2,
    },
    StopRow {
        case: "a stop with the second stage still ahead means its page was never sent",
        second_page_at_secs: 5,
        stopped: true,
        expected_pages: 1,
    },
    StopRow {
        case: "a stop an hour ahead of the second stage leaves that page unsent too",
        second_page_at_secs: 3600,
        stopped: true,
        expected_pages: 1,
    },
    StopRow {
        case: "a stop just after a page the engagement had sent keeps reporting it",
        second_page_at_secs: -1,
        stopped: true,
        expected_pages: 2,
    },
    StopRow {
        case: "a stop an hour after the second stage keeps every page",
        second_page_at_secs: -3600,
        stopped: true,
        expected_pages: 2,
    },
];

#[tokio::test]
async fn a_page_the_engagement_never_sent_is_not_reported() {
    for row in STOP_ROWS {
        let case = row.case;
        let emulator = Emulator::new();
        let seeded = seed(
            &emulator,
            &[ContactFixture {
                alias: "oncall-primary",
                contact_type: "PERSONAL",
                channels: &[EMAIL, SMS],
                plan: r#"{"Stages": [
                    {"DurationInMinutes": 5, "Targets": [
                        {"ChannelTargetInfo": {
                            "ContactChannelId": "{channel:oncall-primary/email}"}}
                    ]},
                    {"DurationInMinutes": 5, "Targets": [
                        {"ChannelTargetInfo": {
                            "ContactChannelId": "{channel:oncall-primary/sms}"}}
                    ]}
                ]}"#,
            }],
            case,
        )
        .await;
        let engagement = start_engagement(&emulator, seeded.contact("oncall-primary"), case).await;

        // Freshly started, the second stage has not come round: its page is scheduled five
        // minutes out and is not part of the engagement yet.
        let reported = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected only the first stage's page right after the start, \
             got {reported:?}"
        );

        // Move the second stage's page to where the row wants it, then — for a row that
        // stops — end the engagement. `StopEngagement` is what decides a stage the
        // engagement never reached, so the stop goes over the wire rather than into the
        // store.
        let second_page = stored_pages(&emulator)
            .into_iter()
            .map(|(arn, ..)| arn)
            .next_back()
            .unwrap_or_else(|| panic!("case {case:?}: the start must have minted two pages"));
        set_sent_time(&emulator, &second_page, from_now(row.second_page_at_secs));

        if row.stopped {
            let (status, body) = emulator
                .call(
                    "StopEngagement",
                    json!({"EngagementId": engagement, "Reason": case}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected StopEngagement to answer 200, got {status} {body:?}"
            );
        }

        let reported = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
        assert_eq!(
            reported.len(),
            row.expected_pages,
            "case {case:?}: expected {} pages, got {reported:?}",
            row.expected_pages
        );
        assert_eq!(
            member(&reported[0], "ContactArn", case),
            seeded.contact("oncall-primary"),
            "case {case:?}: expected the first stage's page first, got {reported:?}"
        );
        let holds_second = reported
            .iter()
            .any(|page| member(page, "PageArn", case) == second_page);
        assert_eq!(
            holds_second,
            row.expected_pages == 2,
            "case {case:?}: expected the second stage's page to be {}, got {reported:?}",
            if row.expected_pages == 2 {
                "reported"
            } else {
                "left out"
            }
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a page reports what `AcceptPage` recorded against it, and nothing else
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_page_reports_the_acknowledgement_that_was_recorded_against_it() {
    let case = "an acknowledged page";
    let emulator = Emulator::new();
    let seeded = seed(
        &emulator,
        &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL],
            plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
            ]}]}"#,
        }],
        case,
    )
    .await;
    let engagement = start_engagement(&emulator, seeded.contact("oncall-primary"), case).await;

    // Nothing has answered the page yet.
    let reported = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected the stage's single page, got {reported:?}"
    );
    let page_arn = member(&reported[0], "PageArn", case).to_string();
    for member_name in ["DeliveryTime", "ReadTime"] {
        assert_eq!(
            reported[0][member_name],
            Value::Null,
            "case {case:?}: expected no {member_name} before the page is answered, got {:?}",
            reported[0]
        );
    }

    // The accept code travels WITH the page, out of band, so the store is the only place a
    // client can read it from in this emulator.
    let accept_code = {
        let store =
            service_state(&*emulator.engine).expect("the engine holds this service's state");
        let store = store.lock();
        store
            .pages
            .get(&page_arn)
            .map(|page| page.accept_code.clone())
            .unwrap_or_else(|| panic!("case {case:?}: the store holds no page {page_arn:?}"))
    };
    // Both modelled `AcceptType` values, in the order a real acknowledgement takes them: a
    // page is delivered before it is read.
    for accept_type in ["DELIVERED", "READ"] {
        let (status, body) = emulator
            .call(
                "AcceptPage",
                json!({
                    "PageId": page_arn,
                    "AcceptType": accept_type,
                    "AcceptCode": accept_code,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?} [{accept_type}]: expected AcceptPage to answer 200, \
             got {status} {body:?}"
        );
    }

    // The listing now reports both instants, and they are the ones the acknowledgements
    // recorded: at or after the instant the page was sent, never before it.
    let answered = all_pages(&emulator, json!({"EngagementId": engagement}), case).await;
    let sent = answered[0]["SentTime"]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a SentTime, got {:?}", answered[0]));
    for member_name in ["DeliveryTime", "ReadTime"] {
        let at = answered[0][member_name].as_f64().unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected {member_name} once the page was answered, got {:?}",
                answered[0]
            )
        });
        assert!(
            at >= sent,
            "case {case:?}: expected {member_name} at or after the send instant {sent}, \
             got {at}"
        );
    }
    assert_eq!(
        answered[0]["PageArn"], reported[0]["PageArn"],
        "case {case:?}: expected the same page, not a second one, got {answered:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: the refusals
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_engagement_the_store_does_not_hold_is_refused_404() {
    let emulator = Emulator::new();
    let seeded = seed(
        &emulator,
        &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL],
            plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
            ]}]}"#,
        }],
        "a live engagement",
    )
    .await;
    let live = start_engagement(&emulator, seeded.contact("oncall-primary"), "a live engagement")
        .await;

    // The rows span the `SsmContactsArn` forms the model's own pattern admits: all three
    // partitions and a Region-less ARN.
    for arn in [
        "arn:aws:ssm-contacts:us-east-1:000000000000:engagement/oncall-primary/absent",
        "arn:aws-cn:ssm-contacts:cn-north-1:000000000000:engagement/oncall-primary/absent",
        "arn:aws-us-gov:ssm-contacts:us-gov-west-1:000000000000:engagement/absent/absent",
        "arn:aws:ssm-contacts::000000000000:engagement/oncall-primary/absent",
    ] {
        let (status, body) = emulator
            .call("ListPagesByEngagement", json!({"EngagementId": arn}))
            .await;
        assert_eq!(
            status, 404,
            "case {arn:?}: expected HTTP 404 for an engagement that does not exist, \
             got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {arn:?}: expected ResourceNotFoundException, got {body:?}"
        );
    }

    // The live engagement is still served, which is what makes the 404s above a statement
    // about the ARN rather than about the operation.
    let reported = all_pages(&emulator, json!({"EngagementId": live}), "a live engagement").await;
    assert_eq!(
        reported.len(),
        1,
        "expected the live engagement's own page, got {reported:?}"
    );
}

#[tokio::test]
async fn a_next_token_this_listing_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let seeded = seed(
        &emulator,
        &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL],
            plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
            ]}]}"#,
        }],
        "a bad token",
    )
    .await;
    let engagement =
        start_engagement(&emulator, seeded.contact("oncall-primary"), "a bad token").await;

    // The rows span a value that is not hex at all, hex that decodes to no token, a token
    // another listing of this service issued, and the member's published minimum length (the
    // empty token, which names no token at all and is SERVED).
    let another_listings_token = encode_next_token(TOKEN_SCOPE_CONTACTS, "oncall-primary");
    for token in ["not-a-token", "deadbeef", "00", another_listings_token.as_str()] {
        let (status, body) = emulator
            .call(
                "ListPagesByEngagement",
                json!({"EngagementId": engagement, "NextToken": token}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400 for a token this listing did not issue, \
             got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {token:?}: expected ValidationException, got {body:?}"
        );
    }

    let served = list_pages(
        &emulator,
        json!({"EngagementId": engagement, "NextToken": ""}),
        "the empty token",
    )
    .await;
    assert_eq!(
        served["Pages"].as_array().map(Vec::len),
        Some(1),
        "expected the empty token to be read as no token at all, got {served:?}"
    );
}

#[tokio::test]
async fn the_listing_is_refused_until_the_account_has_an_active_replication_set() {
    let emulator = Emulator::new();
    let seeded = seed(
        &emulator,
        &[ContactFixture {
            alias: "oncall-primary",
            contact_type: "PERSONAL",
            channels: &[EMAIL],
            plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "{channel:oncall-primary/email}"}}
            ]}]}"#,
        }],
        "the account gate",
    )
    .await;
    let live =
        start_engagement(&emulator, seeded.contact("oncall-primary"), "the account gate").await;
    let absent = "arn:aws:ssm-contacts:us-east-1:000000000000:engagement/oncall-primary/absent";

    // The rows span the two states that do NOT open the data plane — no replication set at
    // all and one that has not finished replicating — crossed with an engagement that IS
    // stored and one that never existed, so the gate is shown to be evaluated BEFORE the
    // `EngagementId` is resolved.
    for (case, replication_set) in [
        ("no replication set at all", None),
        (
            "a replication set that is not active yet",
            Some(AccountReplicationSet {
                arn: format!("arn:aws:ssm-incidents::{ACCOUNT}:replication-set/emulator-set"),
                active: false,
            }),
        ),
    ] {
        set_replication_set(&emulator, replication_set);
        for arn in [live.as_str(), absent] {
            let (status, body) = emulator
                .call("ListPagesByEngagement", json!({"EngagementId": arn}))
                .await;
            assert_eq!(
                status, 400,
                "case {case:?} on {arn:?}: expected HTTP 400 while the account is not \
                 onboarded, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?} on {arn:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}
