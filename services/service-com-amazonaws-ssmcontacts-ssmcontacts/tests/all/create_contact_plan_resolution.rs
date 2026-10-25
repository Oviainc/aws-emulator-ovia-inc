// SPDX-License-Identifier: CC0-1.0

//! What `CreateContact` does with the `Plan` it is required to carry, end to end over the
//! wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see. Every contact, channel and rotation a plan names is created through the
//! service's own creates, and every plan is read back through `GetContact`, so what is
//! asserted is persisted state rather than anything this suite placed in the store.
//!
//! The rule: a `Plan` is a graph of REFERENCES — a `Target` names a contact channel
//! (`ChannelTargetInfo$ContactChannelId`, `@required` within that member) or a contact
//! (`ContactTargetInfo$ContactId`), and `Plan$RotationIds` names on-call rotations, every one
//! of them an `SsmContactsArn` — so a create whose plan names a resource the account does not
//! hold, or whose target names nothing at all, is refused with the modelled
//! `ValidationException` (HTTP 400; `CreateContact` models no not-found error) and persists
//! nothing at all.
//!
//! `tests/all/contact_plan_references.rs` is the suite about the resolver's full reference
//! table and its ceilings, shared by the two operations that STORE a plan. This one is about
//! `CreateContact` specifically, over the three things that suite does not assert:
//!
//! 1. the refusal is independent of `Type`, so the rows span the model's whole `ContactType`
//!    value set (`CONTACT_TYPE_VALUES`) against the plan shape each type carries in practice;
//! 2. the refusal MESSAGE is asserted whole — the service's recorded wording for an
//!    unresolvable ARN, with the ARN taken from the request, and the member-constraint
//!    wording where no ARN is at fault;
//! 3. a refused create leaves the alias FREE and its `IdempotencyToken` unrecorded: the same
//!    alias and token, with a plan that resolves, create a contact rather than answering the
//!    `ConflictException` a live alias answers or replaying an ARN.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::{
    CONTACT_TYPE_VALUES, VALIDATION_EXCEPTION_REASON_VALUES,
};
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::contact_arn;

use crate::contact_lifecycle::{error_code, parse, Emulator, ACCOUNT, REGION};

// ---------------------------------------------------------------------------
// The fixture: everything a plan here can name, and everything it can fail to
// name
// ---------------------------------------------------------------------------

/// The resources the table references, each created through the service's own operations.
struct Fixture {
    /// A stored contact, which a `ContactTargetInfo` can name.
    contact: String,
    /// A stored contact channel of [`Fixture::contact`], which a `ChannelTargetInfo` can
    /// name.
    channel: String,
    /// A stored on-call rotation, which a `RotationIds` entry can name.
    rotation: String,
}

impl Fixture {
    async fn create(emulator: &Emulator) -> Self {
        let contact = emulator
            .create_contact(
                json!({
                    "Alias": "oncall-primary",
                    "DisplayName": "On Call Primary",
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
                "the contact a plan target names",
            )
            .await;
        let channel = emulator
            .create_channel(
                json!({
                    "ContactId": contact,
                    "Name": "email",
                    "Type": "EMAIL",
                    "DeliveryAddress": {"SimpleAddress": "primary@example.com"},
                }),
                "the channel a plan target names",
            )
            .await;
        let rotation = emulator
            .create_rotation(&contact, "weekdays", "the rotation a plan names")
            .await;
        Self {
            contact,
            channel,
            rotation,
        }
    }

    /// `text` with every placeholder replaced by the ARN it stands for, parsed as JSON.
    fn fill(&self, text: &str) -> Value {
        parse(&self.fill_text(text))
    }

    /// `text` with every placeholder replaced by the ARN it stands for.
    fn fill_text(&self, text: &str) -> String {
        text.replace("<contact>", &self.contact)
            .replace("<channel>", &self.channel)
            .replace("<rotation>", &self.rotation)
            .replace("<absent-contact>", &absent_arn("contact/ghost"))
            .replace(
                "<absent-channel>",
                &absent_arn("contact-channel/ghost/00000000-0000-0000-0000-000000000000"),
            )
            .replace("<absent-rotation>", &absent_arn("rotation/ghost"))
    }
}

/// A well-formed `SsmContactsArn` (`^arn:(aws|aws-cn|aws-us-gov):ssm-contacts:…$`) for a
/// resource nothing ever created, so the request passes the boundary validator and fails
/// only on resolution.
fn absent_arn(resource: &str) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:{resource}")
}

// ---------------------------------------------------------------------------
// The table
// ---------------------------------------------------------------------------

/// Why a row's plan cannot be stored, and therefore what the refusal says.
enum Fault {
    /// The plan NAMES a resource the account does not hold. The refusal names that ARN,
    /// taken from the request; the field is the placeholder the fixture fills.
    UnresolvableArn(&'static str),
    /// No ARN is at fault — the target names nothing at all — so the refusal names the
    /// member of the request whose constraint failed.
    Member(&'static str),
}

/// A `Target` carrying neither target info: the modelled `Target` makes both members
/// optional, so this is refused on the member rather than on an ARN.
const TARGET_WITHOUT_TARGET_INFO: &str =
    "Value at 'plan.stages.targets' failed to satisfy constraint: Member must carry one of \
     channelTargetInfo or contactTargetInfo";

/// A `ContactTargetInfo` carrying only its `@required` `IsEssential`: `ContactId` is
/// modelled optional, so this too is refused on the member.
const CONTACT_TARGET_WITHOUT_CONTACT_ID: &str =
    "Value at 'plan.stages.targets.contactTargetInfo' failed to satisfy constraint: Member \
     must carry contactId";

/// A create whose `Plan` cannot be stored.
struct Row {
    case: &'static str,
    /// The `Type` the create names: a value of the modelled `ContactType` set.
    contact_type: &'static str,
    /// The plan, as a JSON fragment carrying the fixture's placeholders.
    plan: &'static str,
    fault: Fault,
}

impl Row {
    /// The whole message this row must be refused with.
    fn expected_message(&self, fixture: &Fixture) -> String {
        match self.fault {
            // The service's recorded wording for an ARN it cannot resolve; the ARN is the
            // one the request carried.
            Fault::UnresolvableArn(placeholder) => format!(
                "Invalid value provided - Invalid resource Arn {}",
                fixture.fill_text(placeholder)
            ),
            Fault::Member(message) => message.to_string(),
        }
    }
}

/// The rows differ along the contact `Type` (the whole modelled value set), which member
/// carries the reference, how it fails to name a resource (never created, or an ARN of
/// another resource kind), where in the plan it sits (a lone entry, after one that resolves,
/// a later stage) and the two rows where no ARN is at fault at all.
const REFUSED_CREATES: &[Row] = &[
    Row {
        case: "an on-call schedule whose RotationIds names a rotation nothing created",
        contact_type: "ONCALL_SCHEDULE",
        plan: r#"{"RotationIds": ["<absent-rotation>"]}"#,
        fault: Fault::UnresolvableArn("<absent-rotation>"),
    },
    Row {
        case: "an on-call schedule whose SECOND rotation id does not resolve",
        contact_type: "ONCALL_SCHEDULE",
        plan: r#"{"RotationIds": ["<rotation>", "<absent-rotation>"]}"#,
        fault: Fault::UnresolvableArn("<absent-rotation>"),
    },
    Row {
        case: "an escalation plan whose contact target names a contact nothing created",
        contact_type: "ESCALATION",
        plan: r#"{"Stages": [{"DurationInMinutes": 20, "Targets": [
            {"ContactTargetInfo": {"ContactId": "<absent-contact>", "IsEssential": true}}
        ]}]}"#,
        fault: Fault::UnresolvableArn("<absent-contact>"),
    },
    Row {
        case: "an escalation plan whose contact target carries only IsEssential",
        contact_type: "ESCALATION",
        plan: r#"{"Stages": [{"DurationInMinutes": 20, "Targets": [
            {"ContactTargetInfo": {"IsEssential": true}}
        ]}]}"#,
        fault: Fault::Member(CONTACT_TARGET_WITHOUT_CONTACT_ID),
    },
    Row {
        case: "an escalation plan whose contact target carries a CONTACT CHANNEL's ARN",
        contact_type: "ESCALATION",
        plan: r#"{"Stages": [{"DurationInMinutes": 0, "Targets": [
            {"ContactTargetInfo": {"ContactId": "<channel>", "IsEssential": false}}
        ]}]}"#,
        fault: Fault::UnresolvableArn("<channel>"),
    },
    Row {
        case: "a personal contact whose channel target names a channel nothing created",
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [{"DurationInMinutes": 1, "Targets": [
            {"ChannelTargetInfo": {
                "ContactChannelId": "<absent-channel>",
                "RetryIntervalInMinutes": 1
            }}
        ]}]}"#,
        fault: Fault::UnresolvableArn("<absent-channel>"),
    },
    Row {
        case: "a personal contact whose SECOND stage names a channel nothing created",
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [
            {"DurationInMinutes": 5, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "<channel>"}}
            ]},
            {"DurationInMinutes": 5, "Targets": [
                {"ChannelTargetInfo": {"ContactChannelId": "<absent-channel>"}}
            ]}
        ]}"#,
        fault: Fault::UnresolvableArn("<absent-channel>"),
    },
    Row {
        case: "a personal contact whose target carries neither channel nor contact target info",
        contact_type: "PERSONAL",
        plan: r#"{"Stages": [{"DurationInMinutes": 5, "Targets": [{}]}]}"#,
        fault: Fault::Member(TARGET_WITHOUT_TARGET_INFO),
    },
];

/// A plan every reference of which resolves, carrying all three kinds: it is what each
/// refused alias is re-created with, so a successful create is observable as "the alias the
/// refusal did not take".
const RESOLVING_PLAN: &str = r#"{
    "Stages": [{"DurationInMinutes": 5, "Targets": [
        {"ChannelTargetInfo": {"ContactChannelId": "<channel>", "RetryIntervalInMinutes": 2}},
        {"ContactTargetInfo": {"ContactId": "<contact>", "IsEssential": true}}
    ]}],
    "RotationIds": ["<rotation>"]
}"#;

#[tokio::test]
async fn a_create_whose_plan_names_what_the_store_does_not_hold_is_refused_and_writes_nothing() {
    let emulator = Emulator::new();
    let fixture = Fixture::create(&emulator).await;
    let resolving = fixture.fill(RESOLVING_PLAN);

    // The rule holds for every kind of contact, so the table spans the model's whole
    // `ContactType` value set rather than the one type an example would carry.
    let covered: Vec<&str> = REFUSED_CREATES.iter().map(|row| row.contact_type).collect();
    for value in CONTACT_TYPE_VALUES {
        assert!(
            covered.contains(value),
            "the table must carry a row of Type {value:?}, got {covered:?}"
        );
    }

    for (index, row) in REFUSED_CREATES.iter().enumerate() {
        let case = row.case;
        let alias = format!("plan-ref-{index:02}");
        let token = format!("plan-ref-token-{index:02}");
        let plan = fixture.fill(row.plan);

        let (status, body) = emulator
            .call(
                "CreateContact",
                json!({
                    "Alias": alias,
                    "Type": row.contact_type,
                    "Plan": plan,
                    "IdempotencyToken": token,
                }),
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
        let expected = row.expected_message(&fixture);
        let message = body["Message"].as_str().unwrap_or_default();
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
        // A member of the request is at fault, so `Reason` stays inside the modelled
        // `ValidationExceptionReason` set (the crate's own model-derived list).
        let reason = body["Reason"].as_str().unwrap_or_default();
        assert!(
            VALIDATION_EXCEPTION_REASON_VALUES.contains(&reason),
            "case {case:?}: expected a Reason inside the modelled ValidationExceptionReason \
             set {VALIDATION_EXCEPTION_REASON_VALUES:?}, got {reason:?}"
        );

        // Nothing was minted: the ARN this create would have returned names no contact. The
        // ARN is derived from the alias, so it is known without the refused response.
        let arn = contact_arn(REGION, &alias);
        let (status, body) = emulator
            .call("GetContact", json!({"ContactId": arn}))
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create must store no contact, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );

        // And the alias is still FREE, as is the idempotency token the refused call carried:
        // the same alias and token, with a plan that resolves, create the contact rather
        // than answering the ConflictException a live alias answers.
        let (status, body) = emulator
            .call(
                "CreateContact",
                json!({
                    "Alias": alias,
                    "Type": row.contact_type,
                    "Plan": resolving,
                    "IdempotencyToken": token,
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused create must leave the alias free, got {status} {body:?}"
        );
        assert_eq!(
            body["ContactArn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: expected the create to mint {arn:?}, got {body:?}"
        );

        let (status, stored) = emulator
            .call("GetContact", json!({"ContactId": arn}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetContact to resolve the created contact, got \
             {status} {stored:?}"
        );
        assert_eq!(
            stored["Plan"], resolving,
            "case {case:?}: expected the stored plan to be the one that resolves, got {:?}",
            stored["Plan"]
        );
        assert_eq!(
            stored["Type"].as_str(),
            Some(row.contact_type),
            "case {case:?}: expected the stored Type to be the one created, got {stored:?}"
        );
    }
}

#[tokio::test]
async fn the_same_create_is_served_once_the_contact_its_plan_names_exists() {
    let emulator = Emulator::new();
    let case = "a plan naming a contact created between the two identical calls";

    // A contact's ARN is derived from its caller-chosen alias, so a plan can name a contact
    // BEFORE that contact exists: the only difference between the two calls below is what
    // the store holds, which is what makes this a resolution against state rather than a
    // judgement about the request's shape.
    let backup_alias = "oncall-backup";
    let backup = contact_arn(REGION, backup_alias);
    let plan = json!({"Stages": [{"DurationInMinutes": 10, "Targets": [
        {"ContactTargetInfo": {"ContactId": backup, "IsEssential": true}}
    ]}]});
    let request = json!({
        "Alias": "escalation-tier-1",
        "Type": "ESCALATION",
        "Plan": plan,
    });

    let (status, body) = emulator.call("CreateContact", request.clone()).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 while the target does not exist, got \
         {status} {body:?}"
    );
    let expected = format!("Invalid value provided - Invalid resource Arn {backup}");
    let message = body["Message"].as_str().unwrap_or_default();
    assert_eq!(
        message, expected,
        "case {case:?}: expected the refusal {expected:?}, got {message:?}"
    );

    let created = emulator
        .create_contact(
            json!({"Alias": backup_alias, "Type": "PERSONAL", "Plan": {"Stages": []}}),
            case,
        )
        .await;
    assert_eq!(
        created, backup,
        "case {case:?}: expected the create to mint {backup:?}, got {created:?}"
    );

    let escalation = emulator.create_contact(request, case).await;
    let (status, stored) = emulator
        .call("GetContact", json!({"ContactId": escalation}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContact to resolve the created plan, got {status} {stored:?}"
    );
    assert_eq!(
        stored["Plan"], plan,
        "case {case:?}: expected the stored plan to be the one created, got {:?}",
        stored["Plan"]
    );
}
