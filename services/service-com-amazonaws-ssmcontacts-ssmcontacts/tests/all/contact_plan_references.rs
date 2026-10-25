// SPDX-License-Identifier: CC0-1.0

//! What a stored `Plan` may NAME — `CreateContact` and `UpdateContact`, end to end over the
//! wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see. Every channel, contact and rotation a plan names is CREATED through the
//! service's own creates and the plan is read back through `GetContact`, so what is asserted
//! is persisted state rather than anything this suite placed in the store.
//!
//! A plan is a graph of REFERENCES: each `Stage`'s `Target` names a contact channel
//! (`ChannelTargetInfo$ContactChannelId`, `@required` within the target) or a contact
//! (`ContactTargetInfo$ContactId`), and `Plan$RotationIds` names on-call rotations — every
//! one of them an `SsmContactsArn`. The rules under test, each over a TABLE whose rows
//! differ along what the rule generalises over (which member carries the reference, which
//! kind of resource it names, how it fails to name one, its position in the plan and how
//! many references the plan carries) rather than over one call:
//!
//! 1. A plan every reference of which resolves is stored as given and read back by
//!    `GetContact`, whether it was written by the create or by the update.
//! 2. A plan naming a resource the account does not hold is refused HTTP 400
//!    `ValidationException` — the only refusal `CreateContact` models for it — and nothing is
//!    written: the contact's previous plan still reads back, and a refused create leaves no
//!    contact at all. A `Target` carrying neither target info, and a `ContactTargetInfo`
//!    carrying no `ContactId`, name nothing to engage and are refused the same way.
//! 3. The references are resolved WHEN THE PLAN IS WRITTEN. A reference that stops resolving
//!    afterwards — the channel it named is deleted — leaves the stored plan exactly as it
//!    was, which is what `DeleteContactChannel` documents ("the stage that includes the
//!    channel will be ignored").
//! 4. An `UpdateContact` naming neither `DisplayName` nor `Plan` describes no update: it is
//!    refused HTTP 400 `ValidationException` and the stored contact is untouched.
//! 5. The resolver's walk is bounded: a plan at each ceiling is stored, one entry past it is
//!    refused the same way, and the stored plan is left alone.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{
    MAX_PLAN_ROTATION_IDS, MAX_PLAN_STAGES, MAX_PLAN_TARGETS,
};

use crate::contact_lifecycle::{error_code, parse, Emulator, ACCOUNT, REGION};
use crate::contact_updates::onboard;

// ---------------------------------------------------------------------------
// The fixture: everything a plan in this suite can name, and everything it can
// fail to name
// ---------------------------------------------------------------------------

/// The resources the tables reference, each created (and, where the row is about a reference
/// that stopped resolving, deleted) through the service's own operations.
struct Fixture {
    /// The contact whose plan is written and read back.
    subject: String,
    /// A contact channel of [`Fixture::subject`].
    channel: String,
    /// A second contact, and one of ITS channels: a plan reaches a person either by naming
    /// them or by naming their channel.
    other: String,
    other_channel: String,
    /// Two on-call rotations, so a `RotationIds` list is exercised with more than one entry.
    rotation: String,
    second_rotation: String,
    /// A channel, a contact and a rotation that existed and were then deleted: a
    /// well-formed ARN this service really minted, naming nothing now.
    deleted_channel: String,
    deleted_contact: String,
    deleted_rotation: String,
}

impl Fixture {
    async fn create(emulator: &Emulator) -> Self {
        onboard(emulator);

        let subject = emulator
            .create_contact(
                json!({
                    "Alias": "oncall-primary",
                    "DisplayName": "On Call Primary",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
                "the contact under test",
            )
            .await;
        let channel = emulator
            .create_channel(
                json!({
                    "ContactId": subject,
                    "Name": "email",
                    "Type": "EMAIL",
                    "DeliveryAddress": {"SimpleAddress": "primary@example.com"},
                }),
                "the subject's channel",
            )
            .await;

        let other = emulator
            .create_contact(
                json!({
                    "Alias": "oncall-secondary",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
                "the contact a plan target names",
            )
            .await;
        let other_channel = emulator
            .create_channel(
                json!({
                    "ContactId": other,
                    "Name": "mobile sms",
                    "Type": "SMS",
                    "DeliveryAddress": {"SimpleAddress": "+15550100"},
                }),
                "the other contact's channel",
            )
            .await;

        let rotation = emulator
            .create_rotation(&other, "weekdays", "the rotation a plan names")
            .await;
        let second_rotation = emulator
            .create_rotation(&other, "weekends", "a second rotation a plan names")
            .await;

        // The three resources that EXISTED and no longer do. Each is created exactly as the
        // live ones were, so the only thing that makes its ARN unresolvable is the delete.
        let deleted_channel = emulator
            .create_channel(
                json!({
                    "ContactId": subject,
                    "Name": "desk.phone",
                    "Type": "VOICE",
                    "DeliveryAddress": {"SimpleAddress": "+15550101"},
                }),
                "the channel that gets deleted",
            )
            .await;
        expect_ok(
            emulator,
            "DeleteContactChannel",
            json!({"ContactChannelId": deleted_channel}),
            "deleting the fixture's channel",
        )
        .await;

        let deleted_contact = emulator
            .create_contact(
                json!({
                    "Alias": "oncall-tertiary",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
                "the contact that gets deleted",
            )
            .await;
        expect_ok(
            emulator,
            "DeleteContact",
            json!({"ContactId": deleted_contact}),
            "deleting the fixture's contact",
        )
        .await;

        let deleted_rotation = emulator
            .create_rotation(&other, "retired", "the rotation that gets deleted")
            .await;
        expect_ok(
            emulator,
            "DeleteRotation",
            json!({"RotationId": deleted_rotation}),
            "deleting the fixture's rotation",
        )
        .await;

        Self {
            subject,
            channel,
            other,
            other_channel,
            rotation,
            second_rotation,
            deleted_channel,
            deleted_contact,
            deleted_rotation,
        }
    }

    /// `text` with every placeholder replaced by the ARN it stands for, parsed as JSON.
    fn fill(&self, text: &str) -> Value {
        parse(&self.fill_text(text))
    }

    /// `text` with every placeholder replaced by the ARN it stands for.
    fn fill_text(&self, text: &str) -> String {
        text.replace("<channel>", &self.channel)
            .replace("<other-channel>", &self.other_channel)
            .replace("<other>", &self.other)
            .replace("<subject>", &self.subject)
            .replace("<rotation-2>", &self.second_rotation)
            .replace("<rotation>", &self.rotation)
            .replace("<deleted-channel>", &self.deleted_channel)
            .replace("<deleted-contact>", &self.deleted_contact)
            .replace("<deleted-rotation>", &self.deleted_rotation)
            .replace("<absent-channel>", &absent_arn("contact-channel/ghost/1"))
            .replace("<absent-contact>", &absent_arn("contact/ghost"))
            .replace("<absent-rotation>", &absent_arn("rotation/ghost"))
    }
}

/// A well-formed `SsmContactsArn` (`^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:…$`) for a
/// resource nothing ever created.
fn absent_arn(resource: &str) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:{resource}")
}

/// Call `operation`, asserting it answered 200.
async fn expect_ok(emulator: &Emulator, operation: &str, request: Value, case: &str) -> Value {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected {operation} to answer 200, got {status} {body:?}"
    );
    body
}

/// `GetContact` for `arn`, asserting it answered 200.
async fn get_contact(emulator: &Emulator, arn: &str, case: &str) -> Value {
    expect_ok(emulator, "GetContact", json!({"ContactId": arn}), case).await
}

// ---------------------------------------------------------------------------
// Rule 1: a plan whose references resolve is stored as given
// ---------------------------------------------------------------------------

/// A plan every reference of which resolves.
///
/// The rows differ along which member carries the reference (a channel target, a contact
/// target, a `RotationIds` entry), how many references the plan carries, which stage carries
/// them, and the two shapes the modelled `Plan` admits — plus the published bounds of
/// `ChannelTargetInfo$RetryIntervalInMinutes` (range 0..60) and `Stage$DurationInMinutes`
/// (range 0..30).
const RESOLVING_PLANS: &[(&str, &str)] = &[
    ("no stages at all", r#"{"Stages": []}"#),
    ("the RotationIds shape with no entry", r#"{"RotationIds": []}"#),
    (
        "one stage naming the contact's own channel, retry at the published minimum",
        r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
            {"ChannelTargetInfo": {"ContactChannelId": "<channel>", "RetryIntervalInMinutes": 0}}
        ]}]}"#,
    ),
    (
        "one stage naming ANOTHER contact's channel, retry at the published maximum",
        r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
            {"ChannelTargetInfo": {
                "ContactChannelId": "<other-channel>",
                "RetryIntervalInMinutes": 60
            }}
        ]}]}"#,
    ),
    (
        "a contact target naming another contact",
        r#"{"Stages": [{"DurationInMinutes": 30, "Targets": [
            {"ContactTargetInfo": {"ContactId": "<other>", "IsEssential": true}}
        ]}]}"#,
    ),
    (
        "a contact target naming the contact whose plan it is",
        r#"{"Stages": [{"DurationInMinutes": 1, "Targets": [
            {"ContactTargetInfo": {"ContactId": "<subject>", "IsEssential": false}}
        ]}]}"#,
    ),
    ("one rotation id", r#"{"RotationIds": ["<rotation>"]}"#),
    (
        "two rotation ids",
        r#"{"RotationIds": ["<rotation>", "<rotation-2>"]}"#,
    ),
    (
        "stages and rotation ids in the same plan",
        r#"{
            "Stages": [{"DurationInMinutes": 2, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "<channel>"}}
            ]}],
            "RotationIds": ["<rotation>"]
        }"#,
    ),
    (
        "two stages, the second carrying both kinds of target and the same channel twice",
        r#"{"Stages": [
            {"DurationInMinutes": 10, "Targets": []},
            {"DurationInMinutes": 20, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "<channel>"}},
                {"ChannelTargetInfo": {"ContactChannelId": "<channel>"}},
                {"ContactTargetInfo": {"ContactId": "<other>", "IsEssential": false}}
            ]}
        ]}"#,
    ),
];

#[tokio::test]
async fn a_plan_whose_references_resolve_is_stored_and_read_back() {
    let emulator = Emulator::new();
    let fixture = Fixture::create(&emulator).await;

    for (index, (case, plan)) in RESOLVING_PLANS.iter().enumerate() {
        let plan = fixture.fill(plan);

        // Written by the UPDATE, which is the call that attaches a plan once the channels it
        // names exist.
        expect_ok(
            &emulator,
            "UpdateContact",
            json!({"ContactId": fixture.subject, "Plan": plan}),
            case,
        )
        .await;
        let stored = get_contact(&emulator, &fixture.subject, case).await;
        assert_eq!(
            stored["Plan"], plan,
            "case {case:?}: expected GetContact to report the plan that was written, got {:?}",
            stored["Plan"]
        );

        // And by the CREATE, whose `Plan` is required: the same references resolve there.
        let alias = format!("created-{index:02}");
        let created = emulator
            .create_contact(
                json!({"Alias": alias, "Type": "ESCALATION", "Plan": plan}),
                case,
            )
            .await;
        let stored = get_contact(&emulator, &created, case).await;
        assert_eq!(
            stored["Plan"], plan,
            "case {case:?}: expected GetContact to report the created plan, got {:?}",
            stored["Plan"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: a plan naming what the store does not hold is refused, and nothing
// is written
// ---------------------------------------------------------------------------

/// A plan that cannot be stored, and the ARN — when one is at fault — the refusal names.
///
/// The rows differ along which member carries the unresolvable reference, HOW it fails to
/// name a resource (never minted, deleted, or naming a resource of another type), its
/// position in the plan (first stage, later stage, beside a reference that does resolve),
/// and the two rows where no ARN is at fault at all: a `Target` carrying neither target
/// info, and a `ContactTargetInfo` carrying no `ContactId`.
struct RefusedPlan {
    case: &'static str,
    plan: &'static str,
    /// The placeholder whose ARN the refusal must name, or `""` when no ARN is at fault.
    offending: &'static str,
}

const REFUSED_PLANS: &[RefusedPlan] = &[
    RefusedPlan {
        case: "a channel target naming a channel nothing ever created",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
            {"ChannelTargetInfo": {"ContactChannelId": "<absent-channel>"}}
        ]}]}"#,
        offending: "<absent-channel>",
    },
    RefusedPlan {
        case: "a channel target naming a channel that was deleted",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
            {"ChannelTargetInfo": {"ContactChannelId": "<deleted-channel>"}}
        ]}]}"#,
        offending: "<deleted-channel>",
    },
    RefusedPlan {
        case: "a channel target carrying a CONTACT's ARN",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
            {"ChannelTargetInfo": {"ContactChannelId": "<other>"}}
        ]}]}"#,
        offending: "<other>",
    },
    RefusedPlan {
        case: "a contact target naming a contact nothing ever created",
        plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
            {"ContactTargetInfo": {"ContactId": "<absent-contact>", "IsEssential": true}}
        ]}]}"#,
        offending: "<absent-contact>",
    },
    RefusedPlan {
        case: "a contact target naming a contact that was deleted",
        plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
            {"ContactTargetInfo": {"ContactId": "<deleted-contact>", "IsEssential": false}}
        ]}]}"#,
        offending: "<deleted-contact>",
    },
    RefusedPlan {
        case: "a contact target carrying a CONTACT CHANNEL's ARN",
        plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
            {"ContactTargetInfo": {"ContactId": "<channel>", "IsEssential": false}}
        ]}]}"#,
        offending: "<channel>",
    },
    RefusedPlan {
        case: "a rotation id nothing ever created",
        plan: r#"{"RotationIds": ["<absent-rotation>"]}"#,
        offending: "<absent-rotation>",
    },
    RefusedPlan {
        case: "a rotation id that was deleted",
        plan: r#"{"RotationIds": ["<deleted-rotation>"]}"#,
        offending: "<deleted-rotation>",
    },
    RefusedPlan {
        case: "a rotation id carrying a CONTACT's ARN",
        plan: r#"{"RotationIds": ["<other>"]}"#,
        offending: "<other>",
    },
    RefusedPlan {
        case: "a second rotation id that does not resolve, after one that does",
        plan: r#"{"RotationIds": ["<rotation>", "<absent-rotation>"]}"#,
        offending: "<absent-rotation>",
    },
    RefusedPlan {
        case: "an unresolvable target beside a resolvable one, in the same stage",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
            {"ChannelTargetInfo": {"ContactChannelId": "<channel>"}},
            {"ChannelTargetInfo": {"ContactChannelId": "<absent-channel>"}}
        ]}]}"#,
        offending: "<absent-channel>",
    },
    RefusedPlan {
        case: "an unresolvable target in the SECOND stage",
        plan: r#"{"Stages": [
            {"DurationInMinutes": 5, "Targets": [
                {"ContactTargetInfo": {"ContactId": "<other>", "IsEssential": true}}
            ]},
            {"DurationInMinutes": 5, "Targets": [
                {"ContactTargetInfo": {"ContactId": "<absent-contact>", "IsEssential": true}}
            ]}
        ]}"#,
        offending: "<absent-contact>",
    },
    RefusedPlan {
        case: "a target carrying neither channel nor contact target info",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [{}]}]}"#,
        offending: "",
    },
    RefusedPlan {
        case: "a contact target carrying no ContactId at all",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
            {"ContactTargetInfo": {"IsEssential": true}}
        ]}]}"#,
        offending: "",
    },
];

/// The plan the subject carries while the refusals are sent: one that resolves, so a refused
/// write is observable as "the previous plan is still there".
const PREVIOUS_PLAN: &str = r#"{"Stages": [{"DurationInMinutes": 7, "Targets": [
    {"ChannelTargetInfo": {"ContactChannelId": "<channel>"}}
]}]}"#;

#[tokio::test]
async fn a_plan_naming_what_the_store_does_not_hold_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let fixture = Fixture::create(&emulator).await;
    let previous = fixture.fill(PREVIOUS_PLAN);
    expect_ok(
        &emulator,
        "UpdateContact",
        json!({"ContactId": fixture.subject, "Plan": previous}),
        "the plan the refusals must leave alone",
    )
    .await;

    for (index, row) in REFUSED_PLANS.iter().enumerate() {
        let case = row.case;
        let plan = fixture.fill(row.plan);

        // The UPDATE is refused and the stored plan is untouched.
        let (status, body) = emulator
            .call(
                "UpdateContact",
                json!({"ContactId": fixture.subject, "Plan": plan}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected UpdateContact to answer HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        if !row.offending.is_empty() {
            // The refusal is built from the request, so it names the value at fault.
            let offending = fixture.fill_text(row.offending);
            let message = body["Message"].as_str().unwrap_or_default();
            assert!(
                message.contains(&offending),
                "case {case:?}: expected the refusal to name {offending:?}, got {message:?}"
            );
        }
        let stored = get_contact(&emulator, &fixture.subject, case).await;
        assert_eq!(
            stored["Plan"], previous,
            "case {case:?}: a refused update must leave the stored plan alone, got {:?}",
            stored["Plan"]
        );
        assert_eq!(
            stored["DisplayName"].as_str(),
            Some("On Call Primary"),
            "case {case:?}: a refused update must write nothing, got {stored:?}"
        );

        // The CREATE is refused the same way, and leaves no contact behind: the alias is
        // still free, which is what a `ConflictException`-free re-create proves.
        let alias = format!("refused-{index:02}");
        let (status, body) = emulator
            .call(
                "CreateContact",
                json!({"Alias": alias, "Type": "ESCALATION", "Plan": plan}),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected CreateContact to answer HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );
        let (status, body) = emulator
            .call(
                "GetContact",
                json!({"ContactId": absent_arn(&format!("contact/{alias}"))}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store no contact, got {status} {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the references are resolved when the plan is WRITTEN
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_reference_that_stops_resolving_after_the_write_leaves_the_plan_alone() {
    let emulator = Emulator::new();
    let fixture = Fixture::create(&emulator).await;

    // A plan naming two channels, both of which resolve when it is written.
    let plan = fixture.fill(
        r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [
            {"ChannelTargetInfo": {"ContactChannelId": "<channel>"}},
            {"ChannelTargetInfo": {"ContactChannelId": "<other-channel>"}}
        ]}]}"#,
    );
    let case = "a plan whose channel is deleted after it was written";
    expect_ok(
        &emulator,
        "UpdateContact",
        json!({"ContactId": fixture.subject, "Plan": plan}),
        case,
    )
    .await;

    expect_ok(
        &emulator,
        "DeleteContactChannel",
        json!({"ContactChannelId": fixture.other_channel}),
        case,
    )
    .await;

    let stored = get_contact(&emulator, &fixture.subject, case).await;
    assert_eq!(
        stored["Plan"], plan,
        "case {case:?}: deleting a channel must leave the stored plan as written, got {:?}",
        stored["Plan"]
    );

    // And a later update that names only `DisplayName` is serviced: the stored plan is not
    // re-resolved, only a plan the REQUEST carries is.
    expect_ok(
        &emulator,
        "UpdateContact",
        json!({"ContactId": fixture.subject, "DisplayName": "Renamed Primary"}),
        case,
    )
    .await;
    let stored = get_contact(&emulator, &fixture.subject, case).await;
    assert_eq!(
        stored["DisplayName"].as_str(),
        Some("Renamed Primary"),
        "case {case:?}: expected the rename to be stored, got {stored:?}"
    );
    assert_eq!(
        stored["Plan"], plan,
        "case {case:?}: a rename must not touch the stored plan, got {:?}",
        stored["Plan"]
    );
}

// ---------------------------------------------------------------------------
// The resolver's own ceilings
// ---------------------------------------------------------------------------

/// A plan at each ceiling the resolver walks is stored; one entry past it is refused with
/// the same HTTP 400 `ValidationException`, and nothing is written.
///
/// The modelled `StagesList` and `TargetsList` declare no length, so the ceilings are the
/// emulator's own (`MAX_PLAN_STAGES`, `MAX_PLAN_TARGETS` in `src/state.rs`); `RotationIds`
/// is the member the model DOES bound (`length` 0..25), so its published maximum and one
/// past it are both asserted.
#[tokio::test]
async fn a_plan_at_each_ceiling_is_stored_and_one_past_it_is_refused() {
    let emulator = Emulator::new();
    let fixture = Fixture::create(&emulator).await;
    let previous = fixture.fill(PREVIOUS_PLAN);
    expect_ok(
        &emulator,
        "UpdateContact",
        json!({"ContactId": fixture.subject, "Plan": previous}),
        "the plan a refused oversized plan must leave alone",
    )
    .await;
    // The plan the store holds right now, which every refusal below must leave alone.
    let mut last_stored = previous;

    let stage = |targets: usize| {
        json!({
            "DurationInMinutes": 5,
            "Targets": (0..targets)
                .map(|_| json!({"ChannelTargetInfo": {"ContactChannelId": fixture.channel}}))
                .collect::<Vec<Value>>(),
        })
    };
    let stages = |count: usize, targets_each: usize| {
        json!({"Stages": (0..count).map(|_| stage(targets_each)).collect::<Vec<Value>>()})
    };
    let rotation_ids = |count: usize| {
        json!({"RotationIds": (0..count).map(|_| json!(fixture.rotation)).collect::<Vec<Value>>()})
    };

    let rows: &[(String, Value, u16)] = &[
        (
            format!("{MAX_PLAN_STAGES} stages, the resolver's ceiling"),
            stages(MAX_PLAN_STAGES, 0),
            200,
        ),
        (
            format!("{} stages, one past the ceiling", MAX_PLAN_STAGES + 1),
            stages(MAX_PLAN_STAGES + 1, 0),
            400,
        ),
        (
            format!("{MAX_PLAN_TARGETS} targets in one stage, the resolver's ceiling"),
            stages(1, MAX_PLAN_TARGETS),
            200,
        ),
        (
            format!("{} targets in one stage, one past the ceiling", MAX_PLAN_TARGETS + 1),
            stages(1, MAX_PLAN_TARGETS + 1),
            400,
        ),
        (
            format!("{MAX_PLAN_ROTATION_IDS} rotation ids, the published maximum"),
            rotation_ids(MAX_PLAN_ROTATION_IDS),
            200,
        ),
        (
            format!(
                "{} rotation ids, one past the published maximum",
                MAX_PLAN_ROTATION_IDS + 1
            ),
            rotation_ids(MAX_PLAN_ROTATION_IDS + 1),
            400,
        ),
    ];

    for (case, plan, want) in rows {
        let (status, body) = emulator
            .call(
                "UpdateContact",
                json!({"ContactId": fixture.subject, "Plan": plan}),
            )
            .await;
        assert_eq!(
            status, *want,
            "case {case:?}: expected HTTP {want}, got {status} {body:?}"
        );
        let stored = get_contact(&emulator, &fixture.subject, case).await;
        if *want == 200 {
            assert_eq!(
                stored["Plan"], *plan,
                "case {case:?}: expected the plan at the ceiling to be stored"
            );
            // The next refusal must leave THIS plan — the last one stored — alone.
            last_stored = plan.clone();
        } else {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
            assert_eq!(
                stored["Plan"], last_stored,
                "case {case:?}: a refused update must leave the stored plan alone"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an update that names neither member describes no update
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_naming_neither_display_name_nor_plan_is_refused() {
    let emulator = Emulator::new();
    let fixture = Fixture::create(&emulator).await;
    let plan = fixture.fill(PREVIOUS_PLAN);
    expect_ok(
        &emulator,
        "UpdateContact",
        json!({"ContactId": fixture.subject, "Plan": plan}),
        "the plan the no-op update must leave alone",
    )
    .await;

    // Every contact the fixture holds, so the refusal is not a property of one contact: the
    // subject, the contact a plan targets, and an escalation plan created for this case.
    let escalation = emulator
        .create_contact(
            json!({
                "Alias": "escalation-tier-1",
                "Type": "ESCALATION",
                "Plan": {"RotationIds": []},
            }),
            "an escalation plan to update",
        )
        .await;

    for target in [&fixture.subject, &fixture.other, &escalation] {
        let case = format!("an update naming only ContactId {target}");
        let before = get_contact(&emulator, target, &case).await;

        let (status, body) = emulator
            .call("UpdateContact", json!({"ContactId": target}))
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ValidationException"),
            "case {case:?}: expected a ValidationException, got {body:?}"
        );

        let after = get_contact(&emulator, target, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must change nothing, got {after:?}"
        );
    }
}
