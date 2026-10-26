// SPDX-License-Identifier: CC0-1.0

//! What `DeleteContact` does about the resources that DEPEND on the contact, end to end over
//! the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS client
//! would see — routing, deserialization, constraint validation, the handler and serialization
//! included. Every contact, channel, rotation and override is CREATED through the service's
//! own operations and read back through `GetContact`/`GetRotation`/`GetRotationOverride`, so
//! what is asserted is persisted state rather than anything this suite placed in the store.
//!
//! The rules under test, each as a TABLE rather than a single call:
//!
//! 1. A DELETE IS REFUSED while another resource still names the contact in a member that
//!    resource is required to carry: a contact channel's `ContactArn`, a rotation's
//!    `ContactIds` (modelled `length` 1..30) or a rotation override's `NewContactIds` — the
//!    relation the model's own `DependentEntity` doc names ("if a contact is a member of a
//!    rotation, the rotation is a dependent entity of the contact"). The refusal is HTTP 409
//!    `ConflictException` naming the contact in `ResourceId`/`ResourceType` and every
//!    depending resource in `DependentEntities`, the contact stays readable, and the
//!    depending resources are left exactly as they were. The rows differ along what the rule
//!    generalises over: which member carries the reference, where in the team list the
//!    contact sits, how large that team is, how many rotations hold the reference, and
//!    whether a channel's reference is reported beside it.
//! 2. THE DELETE GOES THROUGH the moment the last of those references is gone — and not one
//!    reference earlier — however the reference ends: the override is deleted, the team is
//!    replaced by `UpdateRotation`, the rotation itself is deleted, or the channel is
//!    deleted.
//! 3. A REFERENCE FROM A `Plan` DOES NOT BLOCK, which is this operation's own documented
//!    contrary case ("deleting a contact does not remove it from escalation plans and related
//!    response plans"), and neither does a rotation that names OTHER contacts.
//! 4. ONE `DependentEntity` CARRIES AT MOST the 25 ARNs its `DependentResourceIds` list is
//!    modelled to (`SsmContactsArnList`, `length` 0..25), whatever the store holds.

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator};

/// The smallest recurrence `CreateRotation` and `UpdateRotation` admit: one daily hand-off.
fn recurrence() -> Value {
    json!({
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
    })
}

/// Create a PERSONAL contact with an empty plan and answer its `ContactArn`.
async fn create_contact(emulator: &Emulator, alias: &str, case: &str) -> String {
    emulator
        .create_contact(
            json!({"Alias": alias, "Type": "PERSONAL", "Plan": {"Stages": []}}),
            case,
        )
        .await
}

/// Create a rotation whose shift team is `team`, in that order, and answer its `RotationArn`.
async fn create_rotation(emulator: &Emulator, name: &str, team: &[String], case: &str) -> String {
    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": name,
                "ContactIds": team,
                "TimeZoneId": "UTC",
                "Recurrence": recurrence(),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation {name:?} to answer 200, got {status} {body:?}"
    );
    body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string()
}

/// Create an override of `rotation` whose replacement team is `team`, and answer its
/// `RotationOverrideId`.
async fn create_override(
    emulator: &Emulator,
    rotation: &str,
    team: &[String],
    case: &str,
) -> String {
    let (status, body) = emulator
        .call(
            "CreateRotationOverride",
            json!({
                "RotationId": rotation,
                "NewContactIds": team,
                "StartTime": 1_735_689_600,
                "EndTime": 1_735_693_200,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotationOverride to answer 200, got {status} {body:?}"
    );
    body["RotationOverrideId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationOverrideId, got {body:?}"))
        .to_string()
}

/// Create a channel of `contact`, and answer its `ContactChannelArn`.
async fn create_channel(emulator: &Emulator, contact: &str, case: &str) -> String {
    emulator
        .create_channel(
            json!({
                "ContactId": contact,
                "Name": "email",
                "Type": "EMAIL",
                "DeliveryAddress": {"SimpleAddress": "member@example.com"},
            }),
            case,
        )
        .await
}

/// The `ContactIds` the stored rotation reports, read back through `GetRotation`.
async fn rotation_contact_ids(emulator: &Emulator, rotation: &str, case: &str) -> Vec<String> {
    let (status, body) = emulator
        .call("GetRotation", json!({"RotationId": rotation}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRotation to answer 200, got {status} {body:?}"
    );
    string_list(&body["ContactIds"])
}

/// The status `GetContact` answers for `contact`.
async fn get_contact_status(emulator: &Emulator, contact: &str) -> u16 {
    emulator
        .call("GetContact", json!({"ContactId": contact}))
        .await
        .0
}

/// A JSON array of strings as a `Vec<String>`; anything else is empty.
fn string_list(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.as_str().map(str::to_string))
        .collect()
}

/// The `DependentResourceIds` the refusal reports for `relation_type`, or `None` when the
/// error carries no entry for that relation.
fn dependent_ids(body: &Value, relation_type: &str) -> Option<Vec<String>> {
    body["DependentEntities"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|entity| entity["RelationType"].as_str() == Some(relation_type))
        .map(|entity| string_list(&entity["DependentResourceIds"]))
}

/// Assert that `DeleteContact` on `contact` is refused the modelled conflict, naming exactly
/// `rotations` as its dependent rotations and `channels` as its dependent channels.
async fn assert_refused(
    emulator: &Emulator,
    contact: &str,
    rotations: &[String],
    channels: &[String],
    case: &str,
) {
    let (status, body) = emulator
        .call("DeleteContact", json!({"ContactId": contact}))
        .await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected DeleteContact to answer 409 while {} rotation(s) and {} \
         channel(s) depend on the contact, got {status} {body:?}",
        rotations.len(),
        channels.len()
    );
    assert!(
        error_code(&body).contains("ConflictException"),
        "case {case:?}: expected a ConflictException, got {body:?}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(contact),
        "case {case:?}: expected the refusal to name the contact in ResourceId, got {body:?}"
    );
    assert_eq!(
        body["ResourceType"].as_str(),
        Some("contact"),
        "case {case:?}: expected ResourceType \"contact\", got {body:?}"
    );
    assert!(
        body["Message"].as_str().is_some_and(|text| !text.is_empty()),
        "case {case:?}: expected a populated Message, got {body:?}"
    );

    let reported_rotations = dependent_ids(&body, "rotation");
    let expected_rotations = (!rotations.is_empty()).then(|| rotations.to_vec());
    assert_eq!(
        reported_rotations, expected_rotations,
        "case {case:?}: expected the rotation relation to name {expected_rotations:?}, got \
         {reported_rotations:?} in {body:?}"
    );
    let reported_channels = dependent_ids(&body, "contact-channel");
    let expected_channels = (!channels.is_empty()).then(|| channels.to_vec());
    assert_eq!(
        reported_channels, expected_channels,
        "case {case:?}: expected the channel relation to name {expected_channels:?}, got \
         {reported_channels:?} in {body:?}"
    );

    // A refused delete is not a partial one: the contact is still readable.
    let status = get_contact_status(emulator, contact).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the refused delete to leave the contact readable, got {status}"
    );
}

// ---------------------------------------------------------------------------
// 1 + 2. The refusal, and the delete once the last reference is gone
// ---------------------------------------------------------------------------

/// One membership row: how the rotations of this case name the contact under test.
struct MembershipRow {
    case: &'static str,
    /// How many contacts each rotation's own `ContactIds` names (modelled `length` 1..30).
    team: usize,
    /// Where in that team list the contact under test sits, or `None` when the rotation's own
    /// team does not name it at all.
    position: Option<usize>,
    /// Whether an override of each rotation names the contact in its `NewContactIds`.
    overridden: bool,
    /// How many rotations hold the reference this row describes.
    rotations: usize,
    /// Whether the contact also owns a contact channel, so the channel relation is reported
    /// beside the rotation one.
    channel: bool,
}

/// The rows. Each varies the member that carries the reference, the contact's position in the
/// shift team, the team's size, how many rotations hold the reference, and whether a channel
/// holds one too — what the dependency rule generalises over.
const MEMBERSHIP_ROWS: &[MembershipRow] = &[
    MembershipRow {
        case: "the only member of the only rotation",
        team: 1,
        position: Some(0),
        overridden: false,
        rotations: 1,
        channel: false,
    },
    MembershipRow {
        case: "the first of three members of one rotation",
        team: 3,
        position: Some(0),
        overridden: false,
        rotations: 1,
        channel: false,
    },
    MembershipRow {
        case: "the last of two members of each of two rotations",
        team: 2,
        position: Some(1),
        overridden: false,
        rotations: 2,
        channel: false,
    },
    MembershipRow {
        case: "named by an override only, never on the rotation's own team",
        team: 1,
        position: None,
        overridden: true,
        rotations: 1,
        channel: false,
    },
    MembershipRow {
        case: "on the team AND in an override of each of three rotations",
        team: 2,
        position: Some(1),
        overridden: true,
        rotations: 3,
        channel: false,
    },
    MembershipRow {
        // `ContactIds` is modelled `length` 1..30: this row sits on the maximum, with the
        // contact in the middle of it.
        case: "a middle member of the largest shift team, with a channel of its own too",
        team: 30,
        position: Some(14),
        overridden: false,
        rotations: 1,
        channel: true,
    },
];

/// A contact another resource depends on is not deleted: the delete is refused HTTP 409
/// `ConflictException` naming every dependent resource, the rotations keep their membership
/// unchanged, and the delete goes through only once the LAST reference is gone.
#[tokio::test]
async fn a_contact_a_rotation_depends_on_refuses_deletion_until_the_membership_is_gone() {
    for row in MEMBERSHIP_ROWS {
        let case = row.case;
        let emulator = Emulator::new();

        let subject = create_contact(&emulator, "oncall-member", case).await;
        let channel = if row.channel {
            Some(create_channel(&emulator, &subject, case).await)
        } else {
            None
        };

        // The rotation teams: fillers, with the subject spliced in at its position when the
        // row puts it on the team.
        let filler_count = match row.position {
            Some(_) => row.team - 1,
            None => row.team,
        };
        let mut fillers = Vec::new();
        for index in 0..filler_count {
            fillers.push(create_contact(&emulator, &format!("filler-{index}"), case).await);
        }
        let mut team = fillers.clone();
        if let Some(position) = row.position {
            team.insert(position, subject.clone());
        }
        assert_eq!(
            team.len(),
            row.team,
            "case {case:?}: the fixture must build a team of {}, got {}",
            row.team,
            team.len()
        );

        let mut rotations = Vec::new();
        let mut overrides = Vec::new();
        for index in 0..row.rotations {
            let rotation =
                create_rotation(&emulator, &format!("rotation-{index}"), &team, case).await;
            if row.overridden {
                overrides.push(
                    create_override(&emulator, &rotation, std::slice::from_ref(&subject), case)
                        .await,
                );
            }
            rotations.push(rotation);
        }

        // The refusal names every rotation that depends on the contact, and the channel when
        // the row gave it one.
        let channels: Vec<String> = channel.iter().cloned().collect();
        assert_refused(&emulator, &subject, &rotations, &channels, case).await;

        // The refused delete left the memberships alone.
        for rotation in &rotations {
            let stored = rotation_contact_ids(&emulator, rotation, case).await;
            assert_eq!(
                stored, team,
                "case {case:?}: expected the refused delete to leave {rotation} naming \
                 {team:?}, got {stored:?}"
            );
        }

        // Now the references go, one kind at a time — and each time one kind is left the
        // delete is STILL refused.
        if let Some(channel) = &channel {
            let (status, body) = emulator
                .call("DeleteContactChannel", json!({"ContactChannelId": channel}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected DeleteContactChannel to answer 200, got {status} \
                 {body:?}"
            );
            assert_refused(&emulator, &subject, &rotations, &[], case).await;
        }

        if row.overridden {
            for (rotation, override_id) in rotations.iter().zip(&overrides) {
                let (status, body) = emulator
                    .call(
                        "DeleteRotationOverride",
                        json!({"RotationId": rotation, "RotationOverrideId": override_id}),
                    )
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DeleteRotationOverride to answer 200, got {status} \
                     {body:?}"
                );
            }
            if row.position.is_some() {
                // The rotations' own teams still name the contact, so dropping the overrides
                // is not enough.
                assert_refused(&emulator, &subject, &rotations, &[], case).await;
            }
        }

        // The direct membership goes last: `ContactIds` is modelled `length` 1..30, so a
        // rotation whose only member is the contact cannot be updated to drop it and is
        // deleted instead — which is the other way a membership ends.
        let mut survivors: Vec<String> = Vec::new();
        if row.position.is_some() {
            let remaining: Vec<String> = team
                .iter()
                .filter(|member| *member != &subject)
                .cloned()
                .collect();
            for (index, rotation) in rotations.iter().enumerate() {
                if remaining.is_empty() {
                    let (status, body) = emulator
                        .call("DeleteRotation", json!({"RotationId": rotation}))
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected DeleteRotation to answer 200, got {status} \
                         {body:?}"
                    );
                } else {
                    let (status, body) = emulator
                        .call(
                            "UpdateRotation",
                            json!({
                                "RotationId": rotation,
                                "ContactIds": remaining,
                                "Recurrence": recurrence(),
                            }),
                        )
                        .await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected UpdateRotation to answer 200, got {status} \
                         {body:?}"
                    );
                    survivors.push(rotation.clone());
                    // Every rotation but the one just updated still names the contact, so the
                    // delete is refused until the last of them has let it go.
                    let still_depending: Vec<String> = rotations[index + 1..].to_vec();
                    if !still_depending.is_empty() {
                        assert_refused(&emulator, &subject, &still_depending, &[], case).await;
                    }
                }
            }
        }

        // With the last reference gone the delete goes through, and the contact stops being
        // readable.
        let (status, body) = emulator
            .call("DeleteContact", json!({"ContactId": subject}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteContact to answer 200 once nothing depends on the \
             contact, got {status} {body:?}"
        );
        let status = get_contact_status(&emulator, &subject).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted contact to answer 404, got {status}"
        );

        // No rotation the delete left behind reports the contact it removed, so no reader can
        // see a member that does not exist.
        for rotation in &survivors {
            let stored = rotation_contact_ids(&emulator, rotation, case).await;
            assert!(
                !stored.contains(&subject),
                "case {case:?}: expected {rotation} not to report the deleted contact, got \
                 {stored:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. The references that do NOT block
// ---------------------------------------------------------------------------

/// Which reference to the contact the row installs, none of which blocks its delete.
#[derive(Clone, Copy)]
enum WeakReference {
    /// Another contact's `Plan` stage targets the contact (`ContactTargetInfo$ContactId`).
    EscalationPlanStage,
    /// A rotation exists, but its team names only OTHER contacts.
    RotationOfOtherContacts,
    /// A rotation override exists, but its replacement team names only OTHER contacts.
    OverrideOfOtherContacts,
    /// The contact was a rotation member and the rotation has since been deleted.
    DeletedRotation,
}

/// A reference that is not a dependency does not block the delete: this operation's own doc
/// says deleting a contact "does not remove it from escalation plans and related response
/// plans", and a rotation that names other contacts says nothing about this one.
#[tokio::test]
async fn a_reference_that_is_not_a_dependency_does_not_block_the_delete() {
    let rows = [
        (
            "an escalation plan stage targets the contact",
            WeakReference::EscalationPlanStage,
        ),
        (
            "a rotation exists whose team names other contacts",
            WeakReference::RotationOfOtherContacts,
        ),
        (
            "an override exists whose replacement team names other contacts",
            WeakReference::OverrideOfOtherContacts,
        ),
        (
            "the rotation the contact was a member of has been deleted",
            WeakReference::DeletedRotation,
        ),
    ];

    for (case, which) in rows {
        let emulator = Emulator::new();
        let subject = create_contact(&emulator, "oncall-member", case).await;
        let other = create_contact(&emulator, "oncall-other", case).await;

        let mut escalation_plan: Option<String> = None;
        match which {
            WeakReference::EscalationPlanStage => {
                escalation_plan = Some(
                    emulator
                        .create_contact(
                            json!({
                                "Alias": "escalation-tier-1",
                                "Type": "ESCALATION",
                                "Plan": {"Stages": [{
                                    "DurationInMinutes": 5,
                                    "Targets": [{"ContactTargetInfo": {
                                        "ContactId": subject,
                                        "IsEssential": true,
                                    }}],
                                }]},
                            }),
                            case,
                        )
                        .await,
                );
            }
            WeakReference::RotationOfOtherContacts => {
                create_rotation(&emulator, "others-only", std::slice::from_ref(&other), case)
                    .await;
            }
            WeakReference::OverrideOfOtherContacts => {
                let rotation = create_rotation(
                    &emulator,
                    "others-only",
                    std::slice::from_ref(&other),
                    case,
                )
                .await;
                create_override(&emulator, &rotation, std::slice::from_ref(&other), case).await;
            }
            WeakReference::DeletedRotation => {
                let rotation =
                    create_rotation(&emulator, "retired", std::slice::from_ref(&subject), case)
                        .await;
                let (status, body) = emulator
                    .call("DeleteRotation", json!({"RotationId": rotation}))
                    .await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected DeleteRotation to answer 200, got {status} {body:?}"
                );
            }
        }

        let (status, body) = emulator
            .call("DeleteContact", json!({"ContactId": subject}))
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteContact to answer 200, got {status} {body:?}"
        );
        let status = get_contact_status(&emulator, &subject).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted contact to answer 404, got {status}"
        );

        // The escalation plan keeps the stage that targeted it, exactly as the operation's own
        // doc says.
        if let Some(plan_owner) = &escalation_plan {
            let (status, body) = emulator
                .call("GetContact", json!({"ContactId": plan_owner}))
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected the escalation plan readable, got {status} {body:?}"
            );
            let targeted = body["Plan"]["Stages"][0]["Targets"][0]["ContactTargetInfo"]
                ["ContactId"]
                .as_str();
            assert_eq!(
                targeted,
                Some(subject.as_str()),
                "case {case:?}: expected the stage to keep naming the deleted contact, got \
                 {body:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 4. The modelled bound on one relation's ARN list
// ---------------------------------------------------------------------------

/// `DependentEntity$DependentResourceIds` targets `SsmContactsArnList`, modelled `length`
/// 0..25 (`smithy_query(query="shape",shape="SsmContactsArnList")`), so one relation reports
/// at most that many ARNs however many rotations the store holds.
const MAX_DEPENDENT_RESOURCE_IDS: usize = 25;

/// A contact depended on by more rotations than one `DependentEntity` can carry is still
/// refused, and the refusal reports the modelled maximum of real rotation ARNs rather than a
/// list the model cannot hold.
#[tokio::test]
async fn one_relation_reports_at_most_the_modelled_number_of_dependent_arns() {
    let case = "more depending rotations than the ARN list admits";
    let emulator = Emulator::new();
    let subject = create_contact(&emulator, "oncall-member", case).await;

    let mut rotations = Vec::new();
    for index in 0..MAX_DEPENDENT_RESOURCE_IDS + 3 {
        rotations.push(
            create_rotation(
                &emulator,
                &format!("rotation-{index}"),
                std::slice::from_ref(&subject),
                case,
            )
            .await,
        );
    }

    let (status, body) = emulator
        .call("DeleteContact", json!({"ContactId": subject}))
        .await;
    assert_eq!(
        status, 409,
        "case {case:?}: expected DeleteContact to answer 409, got {status} {body:?}"
    );
    let reported = dependent_ids(&body, "rotation")
        .unwrap_or_else(|| panic!("case {case:?}: expected a rotation relation, got {body:?}"));
    assert_eq!(
        reported.len(),
        MAX_DEPENDENT_RESOURCE_IDS,
        "case {case:?}: expected exactly {MAX_DEPENDENT_RESOURCE_IDS} reported ARNs, got {}",
        reported.len()
    );
    for arn in &reported {
        assert!(
            rotations.contains(arn),
            "case {case:?}: expected every reported ARN to name a created rotation, got {arn}"
        );
    }

    let status = get_contact_status(&emulator, &subject).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the refused delete to leave the contact readable, got {status}"
    );
}
