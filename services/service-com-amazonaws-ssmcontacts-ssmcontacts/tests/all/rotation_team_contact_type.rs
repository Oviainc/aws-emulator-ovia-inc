// SPDX-License-Identifier: CC0-1.0

//! WHICH contacts a rotation's shift team may name, asserted as ONE rule over both operations
//! that write a team: `CreateRotation` and `UpdateRotation`.
//!
//! The rule: the member that carries a rotation's shift team admits only the `PERSONAL`
//! contact type — "the contact types `ESCALATION` and `ONCALL_SCHEDULE` are not supported for
//! this operation" (`smithy_query(query="doc", shape="com.amazonaws.ssmcontacts#\
//! CreateRotationRequest", member="ContactIds")`, and the same note on
//! `UpdateRotationRequest$ContactIds`) — so a team entry resolving to a stored contact of any
//! other type is refused with the modelled `ValidationException` (HTTP 400) and nothing is
//! written, while a team of personal contacts is stored and read back whole.
//!
//! The rows differ along what the rule generalises over: WHICH unsupported type the entry
//! resolves to (both of them, which with `PERSONAL` is the model's whole `ContactType` value
//! set), WHERE in the shift-order list it sits (only, first, middle, last), HOW LARGE the team
//! is (one entry up to the modelled maximum of 30) and HOW MANY unsupported entries it
//! carries.
//!
//! An entry that resolves to NO contact is a different failure — the operations' modelled
//! `ResourceNotFoundException` — and the last test pins that the two refusals stay apart.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with buffered awsJson1_1 requests, every contact and rotation is created
//! through the service's own operations, and every verdict is read back through
//! `GetRotation`/`ListRotations`, so what is asserted is persisted state rather than anything
//! this suite placed in the store.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::ops::types::CONTACT_TYPE_VALUES;
use service_com_amazonaws_ssmcontacts_ssmcontacts::state::contact_arn;

use crate::contact_lifecycle::{error_code, Emulator, REGION};

/// The one `ContactType` a rotation's shift team admits, as the member's own documentation
/// names it.
const ROTATION_MEMBER_TYPE: &str = "PERSONAL";

/// The types that documentation excludes from a shift team: "the contact types ESCALATION and
/// ONCALL_SCHEDULE are not supported for this operation".
const EXCLUDED_TYPES: [&str; 2] = ["ESCALATION", "ONCALL_SCHEDULE"];

/// The zone every rotation here is written on — one the IANA zone database resolves, so the
/// only thing under test is the team.
const ZONE: &str = "UTC";

/// The largest shift team the model admits: `ContactIds` is `length` 1..30.
const MAX_TEAM: usize = 30;

/// The simplest recurrence the service admits: one daily hand-off, one member on call.
fn recurrence() -> Value {
    json!({
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
        "NumberOfOnCalls": 1,
        "RecurrenceMultiplier": 1,
    })
}

/// The refusal message the rule answers with: the member path, the ARN the request carried and
/// the type the stored contact it names actually has.
fn expected_message(arn: &str, contact_type: &str) -> String {
    format!(
        "Value '{arn}' at 'contactIds' failed to satisfy constraint: Member must be a contact \
         of type {ROTATION_MEMBER_TYPE}, and names a contact of type {contact_type}"
    )
}

/// The refusal message an error body carries, whichever spelling the layer that refused uses:
/// a handler's modelled `ValidationException` spells it `Message`, the shared constraint
/// validator `message`.
fn error_message(body: &Value) -> &str {
    for member in ["Message", "message"] {
        if let Some(text) = body.get(member).and_then(Value::as_str) {
            return text;
        }
    }
    ""
}

/// One shift team under test: the contact `Type` of each entry, in shift order.
struct TeamRow {
    case: &'static str,
    types: Vec<&'static str>,
}

impl TeamRow {
    /// The position of the first entry of an unsupported type, or `None` when the whole team
    /// is of the type the member admits.
    fn offender(&self) -> Option<usize> {
        self.types
            .iter()
            .position(|contact_type| *contact_type != ROTATION_MEMBER_TYPE)
    }
}

/// The teams the rule is asserted over.
fn team_rows() -> Vec<TeamRow> {
    vec![
        TeamRow {
            case: "one personal contact",
            types: vec![ROTATION_MEMBER_TYPE],
        },
        TeamRow {
            case: "three personal contacts",
            types: vec![ROTATION_MEMBER_TYPE; 3],
        },
        TeamRow {
            case: "the largest modelled team, all personal",
            types: vec![ROTATION_MEMBER_TYPE; MAX_TEAM],
        },
        TeamRow {
            case: "an escalation plan as the only member",
            types: vec!["ESCALATION"],
        },
        TeamRow {
            case: "an on-call schedule as the only member",
            types: vec!["ONCALL_SCHEDULE"],
        },
        TeamRow {
            case: "an escalation plan first, before two personal contacts",
            types: vec!["ESCALATION", ROTATION_MEMBER_TYPE, ROTATION_MEMBER_TYPE],
        },
        TeamRow {
            case: "an on-call schedule in the middle of a team of three",
            types: vec![ROTATION_MEMBER_TYPE, "ONCALL_SCHEDULE", ROTATION_MEMBER_TYPE],
        },
        TeamRow {
            case: "an escalation plan last of a team of three",
            types: vec![ROTATION_MEMBER_TYPE, ROTATION_MEMBER_TYPE, "ESCALATION"],
        },
        TeamRow {
            case: "both unsupported types in one team, the on-call schedule first",
            types: vec!["ONCALL_SCHEDULE", "ESCALATION"],
        },
        TeamRow {
            case: "the largest modelled team with an escalation plan in the middle",
            types: {
                let mut types = vec![ROTATION_MEMBER_TYPE; MAX_TEAM];
                types[MAX_TEAM / 2] = "ESCALATION";
                types
            },
        },
    ]
}

/// The `Plan` a contact of `contact_type` is created with: a plan of the shape that type
/// carries in practice, every reference of which resolves.
///
/// `anchor` is a stored personal contact an escalation stage can target, and `rotation` a
/// stored rotation an on-call schedule's `RotationIds` can name.
fn plan_for(contact_type: &str, anchor: &str, rotation: &str) -> Value {
    match contact_type {
        "ESCALATION" => json!({"Stages": [{
            "DurationInMinutes": 5,
            "Targets": [{"ContactTargetInfo": {"ContactId": anchor, "IsEssential": true}}],
        }]}),
        "ONCALL_SCHEDULE" => json!({"RotationIds": [rotation]}),
        _ => json!({"Stages": []}),
    }
}

/// Create a rotation named `name` whose shift team is `team`, and answer the call's status and
/// body — a refused row's verdict is the point, so this asserts nothing itself.
async fn create_rotation(emulator: &Emulator, name: &str, team: &[String]) -> (u16, Value) {
    emulator
        .call(
            "CreateRotation",
            json!({
                "Name": name,
                "ContactIds": team,
                "TimeZoneId": ZONE,
                "Recurrence": recurrence(),
            }),
        )
        .await
}

/// `GetRotation` for `arn`, asserting it answered 200.
async fn get_rotation(emulator: &Emulator, arn: &str, case: &str) -> Value {
    let (status, body) = emulator
        .call("GetRotation", json!({"RotationId": arn}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRotation to answer 200, got {status} {body:?}"
    );
    body
}

/// How many rotations `ListRotations` reports, so a refused create can be shown to have minted
/// nothing.
async fn rotation_count(emulator: &Emulator, case: &str) -> usize {
    let (status, body) = emulator.call("ListRotations", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRotations to answer 200, got {status} {body:?}"
    );
    body["Rotations"].as_array().map_or(0, Vec::len)
}

/// The fixture every row starts from: a personal contact, and a rotation whose team is that
/// one contact — the rotation `UpdateRotation` is driven against, and the one an on-call
/// schedule's plan names.
struct Fixture {
    anchor: String,
    rotation: String,
}

impl Fixture {
    async fn create(emulator: &Emulator, case: &str) -> Self {
        let anchor = emulator
            .create_contact(
                json!({"Alias": "anchor", "Type": ROTATION_MEMBER_TYPE, "Plan": {"Stages": []}}),
                case,
            )
            .await;
        let (status, body) =
            create_rotation(emulator, "fixture", std::slice::from_ref(&anchor)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the fixture CreateRotation to answer 200, got {status} \
             {body:?}"
        );
        let rotation = body["RotationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
            .to_string();
        Self { anchor, rotation }
    }

    /// Create one contact per entry of `types` and answer their ARNs, in that order.
    async fn team(&self, emulator: &Emulator, types: &[&str], case: &str) -> Vec<String> {
        let mut team = Vec::with_capacity(types.len());
        for (index, contact_type) in types.iter().enumerate() {
            team.push(
                emulator
                    .create_contact(
                        json!({
                            "Alias": format!("member-{index}"),
                            "Type": contact_type,
                            "Plan": plan_for(contact_type, &self.anchor, &self.rotation),
                        }),
                        case,
                    )
                    .await,
            );
        }
        team
    }
}

/// Assert `body` is the member-level refusal the rule answers with: HTTP 400, a
/// `ValidationException`, and the message naming the offending ARN and its contact type.
fn assert_type_refusal(status: u16, body: &Value, arn: &str, contact_type: &str, case: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {body:?}"
    );
    assert_eq!(
        error_message(body),
        expected_message(arn, contact_type),
        "case {case:?}: expected the documented refusal, got {:?}",
        error_message(body)
    );
}

// ---------------------------------------------------------------------------
// The rule, over both operations that write a shift team
// ---------------------------------------------------------------------------

#[tokio::test]
async fn both_operations_admit_only_personal_contacts_on_a_shift_team() {
    // The rule is about the contact's TYPE, so the table spans the model's whole `ContactType`
    // value set rather than the one type an example would carry.
    let covered: Vec<&str> = team_rows()
        .iter()
        .flat_map(|row| row.types.clone())
        .collect();
    for value in CONTACT_TYPE_VALUES {
        assert!(
            covered.contains(value),
            "the table must carry a team member of Type {value:?}, got {covered:?}"
        );
    }

    for row in team_rows() {
        let case = format!("the shift-team rule applied to {}", row.case);
        let emulator = Emulator::new();
        let fixture = Fixture::create(&emulator, &case).await;
        let team = fixture.team(&emulator, &row.types, &case).await;

        let (create_status, create_body) = create_rotation(&emulator, "under-test", &team).await;
        let (update_status, update_body) = emulator
            .call(
                "UpdateRotation",
                json!({
                    "RotationId": fixture.rotation.clone(),
                    "ContactIds": team.clone(),
                    "Recurrence": recurrence(),
                }),
            )
            .await;

        // ONE rule, so the two operations answer the same status for the same team.
        assert_eq!(
            create_status, update_status,
            "case {case:?}: CreateRotation answered {create_status} {create_body:?} while \
             UpdateRotation answered {update_status} {update_body:?}"
        );

        let Some(offender) = row.offender() else {
            assert_eq!(
                create_status, 200,
                "case {case:?}: expected HTTP 200 for a team of personal contacts, got \
                 {create_status} {create_body:?}"
            );
            // Admitted means stored: each operation's rotation reads back on the whole team,
            // in the shift order the request listed.
            let created = create_body["RotationArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a RotationArn, got {create_body:?}")
                })
                .to_string();
            for (arn, operation) in [
                (&created, "CreateRotation"),
                (&fixture.rotation, "UpdateRotation"),
            ] {
                let stored = get_rotation(&emulator, arn, &case).await;
                assert_eq!(
                    stored["ContactIds"],
                    json!(team),
                    "case {case:?}: expected {operation} to store {team:?}, got {:?}",
                    stored["ContactIds"]
                );
            }
            continue;
        };

        let offending_arn = &team[offender];
        let offending_type = row.types[offender];
        assert_type_refusal(
            create_status,
            &create_body,
            offending_arn,
            offending_type,
            &case,
        );
        assert_type_refusal(
            update_status,
            &update_body,
            offending_arn,
            offending_type,
            &case,
        );

        // A refusal writes nothing: no second rotation was minted, and the fixture still names
        // the team it was created with.
        let rotations = rotation_count(&emulator, &case).await;
        assert_eq!(
            rotations, 1,
            "case {case:?}: expected the store to hold only the fixture rotation, got {rotations}"
        );
        let after = get_rotation(&emulator, &fixture.rotation, &case).await;
        assert_eq!(
            after["ContactIds"],
            json!([fixture.anchor.clone()]),
            "case {case:?}: expected the stored team untouched, got {:?}",
            after["ContactIds"]
        );
    }
}

/// The excluded types are exactly the modelled `ContactType` values other than the one a shift
/// team admits, so the rule leaves no type unaccounted for.
#[tokio::test]
async fn the_excluded_types_are_every_modelled_type_but_the_admitted_one() {
    let mut expected: Vec<&str> = CONTACT_TYPE_VALUES
        .iter()
        .copied()
        .filter(|value| *value != ROTATION_MEMBER_TYPE)
        .collect();
    expected.sort_unstable();
    let mut excluded = EXCLUDED_TYPES.to_vec();
    excluded.sort_unstable();
    assert_eq!(
        excluded, expected,
        "the documented exclusions must be every modelled ContactType but \
         {ROTATION_MEMBER_TYPE:?}, got {excluded:?} against {expected:?}"
    );

    // And each of them, created as a contact, is refused a place on a shift team — asserted
    // here against the FIRST entry of the team, so the refusal does not depend on anything
    // else being on it.
    for contact_type in EXCLUDED_TYPES {
        let case = format!("a lone {contact_type} team member");
        let emulator = Emulator::new();
        let fixture = Fixture::create(&emulator, &case).await;
        let team = fixture.team(&emulator, &[contact_type], &case).await;
        let (status, body) = create_rotation(&emulator, "under-test", &team).await;
        assert_type_refusal(status, &body, &team[0], contact_type, &case);
    }
}

/// A team entry that resolves to no contact AT ALL is the other refusal: the operations'
/// modelled `ResourceNotFoundException` (HTTP 404), which the type rule neither replaces nor
/// masks — an absent ARN beside an unsupported one still answers the 404.
#[tokio::test]
async fn an_unresolvable_team_entry_stays_the_not_found_refusal() {
    let case = "a team naming an ARN no contact was created for";
    let emulator = Emulator::new();
    let fixture = Fixture::create(&emulator, case).await;
    let escalation = fixture.team(&emulator, &["ESCALATION"], case).await;
    let absent = contact_arn(REGION, "never-created");

    for (label, team) in [
        ("the absent ARN alone", vec![absent.clone()]),
        (
            "the absent ARN before an escalation plan",
            vec![absent.clone(), escalation[0].clone()],
        ),
        (
            "the absent ARN after an escalation plan",
            vec![escalation[0].clone(), absent.clone()],
        ),
    ] {
        let case = format!("{case}: {label}");
        let (status, body) = create_rotation(&emulator, "under-test", &team).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for an unresolvable team entry, got {status} \
             {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(absent.as_str()),
            "case {case:?}: expected the refusal to name the absent ARN, got {body:?}"
        );
    }

    // Neither refusal minted a rotation.
    let rotations = rotation_count(&emulator, case).await;
    assert_eq!(
        rotations, 1,
        "case {case:?}: expected the store to hold only the fixture rotation, got {rotations}"
    );
}
