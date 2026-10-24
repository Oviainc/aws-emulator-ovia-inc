// SPDX-License-Identifier: CC0-1.0

//! `ListPageResolutions` — the resolution path of one page — end to end over the wire.
//!
//! Every case drives the production `MultiServiceRouter` with a buffered awsJson1_1 request
//! (`POST /`, `X-Amz-Target: SSMContacts.<Operation>`) through the harness the contact suite
//! builds, so what is asserted is what a signing AWS client would see.
//!
//! The rules under test, each asserted over a TABLE whose rows differ along what the rule
//! generalises over rather than over one call:
//!
//! 1. the resolution path of a page is the chain of contacts the engagement resolved THROUGH
//!    to reach the channel that page engaged — the contact the engagement engaged first, the
//!    paged contact last — each step carrying the `Type` its contact was created with and the
//!    index of the plan stage that resolved to it, the engaged contact (resolved by no stage)
//!    carrying none. The rows span a path of one, two and three steps, every value of the
//!    model's `ContactType` set, a stage reached by a contact target and one reached by a
//!    channel target, the stage indexes 0, 1 and 2, a contact two stages of one plan target,
//!    and a plan closed into a cycle;
//! 2. the read REPORTS: repeating it answers the same path and writes nothing;
//! 3. a `PageId` that names no stored page is a 404 `ResourceNotFoundException`, whatever
//!    other kind of SSM Contacts resource the ARN names;
//! 4. the account-level Incident Manager precondition is evaluated BEFORE the `PageId` is
//!    resolved, so a non-onboarded account is refused with a 400 `ValidationException`
//!    whether or not the page exists;
//! 5. a `NextToken` this list did not issue is refused with a 400 `ValidationException`,
//!    while an empty token is no token at all — and the resolution path of a page fits one
//!    page of results, so a served path carries no `NextToken`.
//!
//! The pages themselves are read from the store (`ListPagesByEngagement` is a stub of its
//! own), exactly as `tests/all/page_acknowledgement.rs` reads them: this suite is about the
//! resolution of a page, not about listing the pages of an engagement.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::service_state;

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// The contacts this suite stores, by the alias the tables name them with: the alias, the
/// modelled `Type` the contact is created with, and the channel it owns (none for a plan
/// that only escalates).
///
/// The `Type` column spans the model's whole `ContactType` value set
/// (`CONTACT_TYPE_VALUES` in `src/ops/types/shared_types.rs`), so a resolution step is
/// asserted for every kind of contact a path can run through.
const CONTACTS: &[(&str, &str, Option<&str>)] = &[
    ("person-a", "PERSONAL", Some("email")),
    ("person-b", "PERSONAL", Some("sms")),
    ("schedule", "ONCALL_SCHEDULE", None),
    ("escalation", "ESCALATION", None),
    ("top", "ESCALATION", None),
    ("direct", "ESCALATION", None),
    ("diamond", "ESCALATION", None),
    ("cycle-a", "ESCALATION", None),
    ("cycle-b", "ESCALATION", None),
];

/// The channel a contact of [`CONTACTS`] owns: its `Name`, modelled `Type` and address.
const CHANNELS: &[(&str, &str, &str, &str)] = &[
    ("person-a", "email", "EMAIL", "a@example.com"),
    ("person-b", "sms", "SMS", "+15550100"),
];

/// What the suite stores: every fixture contact's `ContactArn` and every fixture channel's
/// `ContactChannelArn`, by the alias/name the tables name them with.
struct Fixture {
    contacts: BTreeMap<&'static str, String>,
    channels: BTreeMap<&'static str, String>,
}

impl Fixture {
    fn contact(&self, alias: &str) -> &str {
        self.contacts
            .get(alias)
            .unwrap_or_else(|| panic!("the fixture holds no contact {alias:?}"))
    }

    fn channel(&self, alias: &str) -> &str {
        self.channels
            .get(alias)
            .unwrap_or_else(|| panic!("the fixture holds no channel for {alias:?}"))
    }
}

/// Create the contacts, their channels and the plans that chain them together.
///
/// A plan can only name a channel or a contact that already exists, so every contact is
/// created with an empty plan and the plans are written afterwards by `UpdateContact` —
/// which is also what lets `cycle-a` and `cycle-b` target each other.
async fn fixture(emulator: &Emulator) -> Fixture {
    onboard(emulator);

    let mut contacts: BTreeMap<&'static str, String> = BTreeMap::new();
    for (alias, contact_type, _) in CONTACTS {
        let arn = emulator
            .create_contact(
                json!({
                    "Alias": alias,
                    "Type": contact_type,
                    "Plan": {"Stages": []},
                }),
                alias,
            )
            .await;
        contacts.insert(alias, arn);
    }

    let mut channels: BTreeMap<&'static str, String> = BTreeMap::new();
    for (alias, name, channel_type, address) in CHANNELS {
        let (status, body) = emulator
            .call(
                "CreateContactChannel",
                json!({
                    "ContactId": contacts[alias],
                    "Name": name,
                    "Type": channel_type,
                    "DeliveryAddress": {"SimpleAddress": address},
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {alias:?}: expected CreateContactChannel to answer 200, got {status} {body:?}"
        );
        channels.insert(
            alias,
            body["ContactChannelArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {alias:?}: expected a ContactChannelArn, got {body:?}"))
                .to_string(),
        );
    }

    let fixture = Fixture { contacts, channels };

    // Every `DurationInMinutes` below is 0 — inside the modelled `StageDurationInMins` range
    // (0..30) — because this suite is about WHICH contacts a page resolves through, not about
    // when each stage is engaged (`tests/all/page_acknowledgement.rs` is the suite about the
    // stage offsets).
    let plans: &[(&str, Value)] = &[
        // A person pages their own channel.
        (
            "person-a",
            json!({"Stages": [stage(&[channel_target(fixture.channel("person-a"))])]}),
        ),
        (
            "person-b",
            json!({"Stages": [stage(&[channel_target(fixture.channel("person-b"))])]}),
        ),
        // An on-call schedule that resolves onward to a person.
        (
            "schedule",
            json!({"Stages": [stage(&[contact_target(fixture.contact("person-a"))])]}),
        ),
        // An escalation plan whose SECOND stage targets one person and whose THIRD targets
        // another, so two pages of one engagement resolve through two different stages.
        (
            "escalation",
            json!({"Stages": [
                stage(&[]),
                stage(&[contact_target(fixture.contact("person-a"))]),
                stage(&[contact_target(fixture.contact("person-b"))]),
            ]}),
        ),
        // escalation plan > on-call schedule > contact: the hierarchy the operation's own
        // documentation describes.
        (
            "top",
            json!({"Stages": [stage(&[contact_target(fixture.contact("schedule"))])]}),
        ),
        // A plan that reaches a person by naming their CHANNEL rather than the person.
        (
            "direct",
            json!({"Stages": [
                stage(&[]),
                stage(&[channel_target(fixture.channel("person-a"))]),
            ]}),
        ),
        // A plan that targets the same person from two stages.
        (
            "diamond",
            json!({"Stages": [
                stage(&[contact_target(fixture.contact("person-a"))]),
                stage(&[contact_target(fixture.contact("person-a"))]),
            ]}),
        ),
        // Two escalation plans that target each other, the second also paging a channel.
        (
            "cycle-a",
            json!({"Stages": [stage(&[contact_target(fixture.contact("cycle-b"))])]}),
        ),
        (
            "cycle-b",
            json!({"Stages": [
                stage(&[contact_target(fixture.contact("cycle-a"))]),
                stage(&[channel_target(fixture.channel("person-b"))]),
            ]}),
        ),
    ];

    for (alias, plan) in plans {
        let (status, body) = emulator
            .call(
                "UpdateContact",
                json!({"ContactId": fixture.contact(alias), "Plan": plan}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {alias:?}: expected UpdateContact to answer 200, got {status} {body:?}"
        );
    }

    fixture
}

/// One plan stage carrying `targets`, engaged as soon as the stage begins.
fn stage(targets: &[Value]) -> Value {
    json!({"DurationInMinutes": 0, "Targets": targets})
}

/// A plan target naming one contact channel.
fn channel_target(channel_arn: &str) -> Value {
    json!({"ChannelTargetInfo": {"ContactChannelId": channel_arn}})
}

/// A plan target naming one contact, whose own plan is engaged in turn.
fn contact_target(contact_arn: &str) -> Value {
    json!({"ContactTargetInfo": {"ContactId": contact_arn, "IsEssential": false}})
}

/// Start an engagement of `contact_arn`, asserting it succeeded, and answer its
/// `EngagementArn`.
async fn start_engagement(emulator: &Emulator, contact_arn: &str, case: &str) -> String {
    let (status, body) = emulator
        .call(
            "StartEngagement",
            json!({
                "ContactId": contact_arn,
                "Sender": "incident-manager",
                "Subject": "Database latency",
                "Content": "p99 write latency is over budget.",
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

/// The `PageArn`s the store holds for `engagement_arn`, in the order the plan engaged them.
fn page_arns_of(emulator: &Emulator, engagement_arn: &str) -> Vec<String> {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    let mut pages: Vec<(u64, String)> = state
        .pages
        .values()
        .filter(|page| page.engagement_arn == engagement_arn)
        .map(|page| (page.sequence, page.arn.clone()))
        .collect();
    pages.sort();
    pages.into_iter().map(|(_, arn)| arn).collect()
}

/// `ListPageResolutions`, answering the status and the body.
async fn list_page_resolutions(emulator: &Emulator, request: Value) -> (u16, Value) {
    emulator.call("ListPageResolutions", request).await
}

/// The path a successful `ListPageResolutions` reported, as
/// `(ContactArn, Type, StageIndex)` in the order it was served.
fn reported_path(body: &Value) -> Vec<(String, String, Option<i64>)> {
    body["PageResolutions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a PageResolutions list, got {body:?}"))
        .iter()
        .map(|step| {
            (
                step["ContactArn"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected a ContactArn, got {step:?}"))
                    .to_string(),
                step["Type"]
                    .as_str()
                    .unwrap_or_else(|| panic!("expected a Type, got {step:?}"))
                    .to_string(),
                step.get("StageIndex").and_then(Value::as_i64),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// 1. The resolution path of a page
// ---------------------------------------------------------------------------

/// One expected step of a resolution path: the fixture contact it names, the `Type` that
/// contact was created with, and the stage of the resolving plan that reached it.
struct Step {
    alias: &'static str,
    contact_type: &'static str,
    stage_index: Option<i64>,
}

/// One row of the resolution table: which contact the engagement names, and the path each of
/// its pages must resolve to, in the order the plan engaged the pages.
struct ResolutionCase {
    case: &'static str,
    engaged: &'static str,
    expected: &'static [&'static [Step]],
}

const RESOLUTION_CASES: &[ResolutionCase] = &[
    ResolutionCase {
        case: "engaging a person resolves to that person alone",
        engaged: "person-a",
        expected: &[&[Step {
            alias: "person-a",
            contact_type: "PERSONAL",
            stage_index: None,
        }]],
    },
    ResolutionCase {
        case: "an escalation plan resolves each page through the stage that reached it",
        engaged: "escalation",
        expected: &[
            &[
                Step {
                    alias: "escalation",
                    contact_type: "ESCALATION",
                    stage_index: None,
                },
                Step {
                    alias: "person-a",
                    contact_type: "PERSONAL",
                    stage_index: Some(1),
                },
            ],
            &[
                Step {
                    alias: "escalation",
                    contact_type: "ESCALATION",
                    stage_index: None,
                },
                Step {
                    alias: "person-b",
                    contact_type: "PERSONAL",
                    stage_index: Some(2),
                },
            ],
        ],
    },
    ResolutionCase {
        case: "escalation plan > on-call schedule > contact",
        engaged: "top",
        expected: &[&[
            Step {
                alias: "top",
                contact_type: "ESCALATION",
                stage_index: None,
            },
            Step {
                alias: "schedule",
                contact_type: "ONCALL_SCHEDULE",
                stage_index: Some(0),
            },
            Step {
                alias: "person-a",
                contact_type: "PERSONAL",
                stage_index: Some(0),
            },
        ]],
    },
    ResolutionCase {
        case: "a stage that targets a channel resolves to the channel's owner",
        engaged: "direct",
        expected: &[&[
            Step {
                alias: "direct",
                contact_type: "ESCALATION",
                stage_index: None,
            },
            Step {
                alias: "person-a",
                contact_type: "PERSONAL",
                stage_index: Some(1),
            },
        ]],
    },
    ResolutionCase {
        case: "a contact two stages target is resolved by the first of them",
        engaged: "diamond",
        expected: &[&[
            Step {
                alias: "diamond",
                contact_type: "ESCALATION",
                stage_index: None,
            },
            Step {
                alias: "person-a",
                contact_type: "PERSONAL",
                stage_index: Some(0),
            },
        ]],
    },
    ResolutionCase {
        case: "a plan closed into a cycle resolves each contact once",
        engaged: "cycle-a",
        expected: &[&[
            Step {
                alias: "cycle-a",
                contact_type: "ESCALATION",
                stage_index: None,
            },
            Step {
                alias: "cycle-b",
                contact_type: "ESCALATION",
                stage_index: Some(0),
            },
            Step {
                alias: "person-b",
                contact_type: "PERSONAL",
                stage_index: Some(1),
            },
        ]],
    },
];

#[tokio::test]
async fn a_page_resolves_through_the_contacts_its_engagement_ran_through() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for row in RESOLUTION_CASES {
        let case = row.case;
        let engagement = start_engagement(&emulator, fixture.contact(row.engaged), case).await;
        let pages = page_arns_of(&emulator, &engagement);
        assert_eq!(
            pages.len(),
            row.expected.len(),
            "case {case:?}: expected {} page(s) for this engagement, got {pages:?}",
            row.expected.len()
        );

        for (page_arn, expected) in pages.iter().zip(row.expected) {
            let (status, body) =
                list_page_resolutions(&emulator, json!({"PageId": page_arn})).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected ListPageResolutions to answer 200, got {status} {body:?}"
            );

            let wanted: Vec<(String, String, Option<i64>)> = expected
                .iter()
                .map(|step| {
                    (
                        fixture.contact(step.alias).to_string(),
                        step.contact_type.to_string(),
                        step.stage_index,
                    )
                })
                .collect();
            let actual = reported_path(&body);
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected the resolution path {wanted:?}, got {actual:?}"
            );
            assert!(
                body.get("NextToken").is_none_or(Value::is_null),
                "case {case:?}: a resolution path fits one page, so it must carry no \
                 NextToken, got {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. The read reports, and writes nothing
// ---------------------------------------------------------------------------

/// Everything a reporting call must leave alone, read straight from the store.
#[derive(Debug, PartialEq)]
struct Snapshot {
    contacts: Vec<String>,
    channels: Vec<String>,
    engagements: Vec<String>,
    pages: Vec<PageSnapshot>,
}

/// One stored page, with the acknowledgement state a read must not move.
#[derive(Debug, PartialEq)]
struct PageSnapshot {
    arn: String,
    contact_channel_arn: String,
    delivered_at: Option<u64>,
    read_at: Option<u64>,
    receipts: usize,
}

fn snapshot(emulator: &Emulator) -> Snapshot {
    let state = service_state(&*emulator.engine).expect("the engine holds this service's state");
    let state = state.lock();
    Snapshot {
        contacts: state.contacts.keys().cloned().collect(),
        channels: state.contact_channels.keys().cloned().collect(),
        engagements: state.engagements.keys().cloned().collect(),
        pages: state
            .pages
            .values()
            .map(|page| PageSnapshot {
                arn: page.arn.clone(),
                contact_channel_arn: page.contact_channel_arn.clone(),
                delivered_at: page.delivery_time.map(epoch_secs),
                read_at: page.read_time.map(epoch_secs),
                receipts: page.receipts.len(),
            })
            .collect(),
    }
}

/// `at` as whole seconds since the epoch, so two instants compare without cloning a clock.
fn epoch_secs(at: std::time::SystemTime) -> u64 {
    at.duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default()
}

#[tokio::test]
async fn resolving_a_page_repeatedly_answers_the_same_path_and_writes_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;

    for row in RESOLUTION_CASES {
        let case = row.case;
        let engagement = start_engagement(&emulator, fixture.contact(row.engaged), case).await;

        for page_arn in page_arns_of(&emulator, &engagement) {
            let before = snapshot(&emulator);
            let (first_status, first) =
                list_page_resolutions(&emulator, json!({"PageId": &page_arn})).await;
            let (second_status, second) =
                list_page_resolutions(&emulator, json!({"PageId": &page_arn})).await;
            let after = snapshot(&emulator);

            assert_eq!(
                (first_status, second_status),
                (200, 200),
                "case {case:?}: both reads must answer 200, got {first_status} and \
                 {second_status}"
            );
            assert_eq!(
                reported_path(&second),
                reported_path(&first),
                "case {case:?}: a repeated read must answer the same path, got {second:?}"
            );
            assert_eq!(
                after, before,
                "case {case:?}: a reporting call must write nothing, got {after:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. A PageId that names no page
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_page_id_that_names_no_stored_page_answers_404() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    // One live engagement, so "no such page" cannot be confused with "the store is empty".
    let engagement = start_engagement(&emulator, fixture.contact("person-a"), "the live page").await;
    let live = page_arns_of(&emulator, &engagement);
    assert_eq!(
        live.len(),
        1,
        "the fixture engagement must have sent one page, got {live:?}"
    );

    // Well-formed `SsmContactsArn`s that name no page: a page nothing sent, and each other
    // kind of resource this service does hold.
    let absent: &[(&str, String)] = &[
        (
            "a page nothing ever sent",
            format!(
                "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:page/person-a/\
                 00000000-0000-4000-8000-00000000beef"
            ),
        ),
        ("the engagement that sent the live page", engagement.clone()),
        ("a stored contact", fixture.contact("person-a").to_string()),
        (
            "a stored contact channel",
            fixture.channel("person-a").to_string(),
        ),
    ];

    for (case, page_id) in absent {
        let (status, body) = list_page_resolutions(&emulator, json!({"PageId": page_id})).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
    }

    // And the live page still resolves, so the 404s above are about the ARN they named.
    let (status, body) = list_page_resolutions(&emulator, json!({"PageId": &live[0]})).await;
    assert_eq!(
        status, 200,
        "the live page must still resolve, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. The account-level precondition
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_account_precondition_is_evaluated_before_the_page_is_resolved() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let engagement =
        start_engagement(&emulator, fixture.contact("person-a"), "the gated page").await;
    let live = page_arns_of(&emulator, &engagement);
    let absent = format!(
        "arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:page/person-a/\
         00000000-0000-4000-8000-00000000dead"
    );

    let mut inactive = active_replication_set();
    inactive.active = false;
    let accounts = [
        ("no replication set at all", None),
        (
            "a replication set that is not active yet",
            Some(inactive.clone()),
        ),
    ];

    for (account_case, replication_set) in accounts {
        for (target_case, page_id) in [("a stored page", &live[0]), ("an absent page", &absent)] {
            set_replication_set(&emulator, replication_set.clone());
            let case = format!("{target_case}, {account_case}");

            let (status, body) = list_page_resolutions(&emulator, json!({"PageId": page_id})).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected the account precondition to answer 400, got \
                 {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException — not a \
                 ResourceNotFoundException and not a NotImplemented, even for an absent \
                 page — got {body:?}"
            );
        }
    }

    onboard(&emulator);
    let (status, body) = list_page_resolutions(&emulator, json!({"PageId": &live[0]})).await;
    assert_eq!(
        status, 200,
        "an onboarded account must resolve the page again, got {status} {body:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. Pagination tokens
// ---------------------------------------------------------------------------

/// `text` as the hex this service's `NextToken` codec encodes a token's payload as.
fn hex_of(text: &str) -> String {
    text.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[tokio::test]
async fn a_next_token_this_list_did_not_issue_is_refused() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator).await;
    let engagement = start_engagement(&emulator, fixture.contact("top"), "the paged chain").await;
    let pages = page_arns_of(&emulator, &engagement);
    let page_arn = pages
        .first()
        .unwrap_or_else(|| panic!("the fixture engagement must have sent a page"));

    // The separator the codec puts between a token's scope and its key.
    let separator = '\u{1f}';
    let refused: &[(&str, String)] = &[
        ("a token that is not this codec's hex", "not-a-token".to_string()),
        (
            "a token another list issued",
            hex_of(&format!("contacts{separator}{}", fixture.contact("person-a"))),
        ),
        (
            "a token naming a contact this path does not run through",
            hex_of(&format!(
                "page-resolutions{separator}arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/never-created"
            )),
        ),
        (
            "a token naming nothing at all",
            hex_of(&format!("page-resolutions{separator}")),
        ),
    ];

    for (case, token) in refused {
        let (status, body) =
            list_page_resolutions(&emulator, json!({"PageId": page_arn, "NextToken": token})).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected an unissued NextToken to answer 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
    }

    // An EMPTY token is no token: the modelled `PaginationToken` admits it (length 0..2048),
    // and it must serve the first page rather than be refused.
    let (status, body) =
        list_page_resolutions(&emulator, json!({"PageId": page_arn, "NextToken": ""})).await;
    assert_eq!(
        status, 200,
        "case \"an empty NextToken\": expected 200, got {status} {body:?}"
    );
    let unpaged = list_page_resolutions(&emulator, json!({"PageId": page_arn})).await.1;
    assert_eq!(
        reported_path(&body),
        reported_path(&unpaged),
        "case \"an empty NextToken\": expected the first page, got {body:?}"
    );
}
