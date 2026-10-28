// SPDX-License-Identifier: CC0-1.0

//! The two rules a rotation's schedule is admitted by, asserted as ONE rule each over both
//! operations that write a rotation: `CreateRotation` and `UpdateRotation`.
//!
//! 1. THE TIME ZONE. `TimeZoneId` is documented as a time zone "in Internet Assigned Numbers
//!    Authority (IANA) format", with the explicit note that designators for zones without
//!    Daylight Savings Time rules are not supported, so a name no zone database resolves is
//!    refused HTTP 400 `ValidationException` and nothing is written. The rows differ along
//!    what the rule generalises over: the FORM of the name (a region/city zone, a
//!    single-component zone, an `Etc/` zone, an alias, a zone with and one without DST, a
//!    half-hour offset) and, for a refusal, HOW the name fails to be a zone (an abbreviation
//!    designator, the zone's English name, the wrong case, a space for the underscore, a
//!    well-formed name of no real zone).
//! 2. THE CONCURRENT ON-CALLS. `Recurrence.NumberOfOnCalls` is "the number of contacts, or
//!    shift team members designated to be on call concurrently during a shift", so it cannot
//!    exceed the rotation's shift team; the model bounds it below (`range` min 1) and leaves
//!    the upper bound to the team. The rows differ along the team size, the count asked for
//!    and whether the update names a new team or keeps the stored one.
//!
//! Both rules are driven through the crate's ONE integration-test harness
//! (`contact_lifecycle`'s `Emulator`) with buffered awsJson1_1 requests, and every verdict is
//! read back through `GetRotation`/`ListRotations`, so what is asserted is what the service
//! persisted rather than anything the test placed in the store.

use serde_json::{json, Value};

use crate::contact_lifecycle::{error_code, Emulator};

/// The zone every fixture rotation is created with, so a refused row is visibly "the store
/// still holds this one".
const FIXTURE_ZONE: &str = "UTC";

/// A `TimeZoneId` under test and the verdict the rule gives it.
struct ZoneRow {
    case: &'static str,
    time_zone_id: &'static str,
    /// `true` when the name resolves to an IANA zone, `false` when it does not.
    admitted: bool,
}

/// The zone names the rule is asserted over.
///
/// The accepted rows are canonical names of the IANA Time Zone Database (the member's
/// documented format — `smithy_query(query="doc", shape="com.amazonaws.ssmcontacts#\
/// CreateRotationRequest", member="TimeZoneId")` names `America/Los_Angeles`, `UTC` and
/// `Asia/Seoul`), and include zones that document names: a southern-hemisphere DST zone, a
/// half-hour-offset zone, an `Etc/` zone and a backward-compatibility alias.
///
/// The refused rows are names the modelled `@pattern`/`@length` admit — so each one reaches
/// the handler — that name no zone of that database.
fn zone_rows() -> Vec<ZoneRow> {
    vec![
        ZoneRow {
            case: "a region/city zone that observes Daylight Savings Time",
            time_zone_id: "America/Los_Angeles",
            admitted: true,
        },
        ZoneRow {
            case: "the single-component zone the fixtures use",
            time_zone_id: "UTC",
            admitted: true,
        },
        ZoneRow {
            case: "a region/city zone that observes no Daylight Savings Time",
            time_zone_id: "Asia/Seoul",
            admitted: true,
        },
        ZoneRow {
            case: "a southern-hemisphere zone, whose DST runs over the turn of the year",
            time_zone_id: "Pacific/Auckland",
            admitted: true,
        },
        ZoneRow {
            case: "a zone whose offset is not a whole number of hours",
            time_zone_id: "Asia/Kolkata",
            admitted: true,
        },
        ZoneRow {
            case: "an Etc/ zone",
            time_zone_id: "Etc/UTC",
            admitted: true,
        },
        ZoneRow {
            case: "a three-component zone name",
            time_zone_id: "America/Argentina/Ushuaia",
            admitted: true,
        },
        ZoneRow {
            case: "an abbreviation designator for a zone without DST rules",
            time_zone_id: "PST",
            admitted: false,
        },
        ZoneRow {
            case: "an abbreviation designator for a zone's summer time",
            time_zone_id: "PDT",
            admitted: false,
        },
        ZoneRow {
            case: "a zone's English name rather than its zone name",
            time_zone_id: "Pacific Standard Time",
            admitted: false,
        },
        ZoneRow {
            case: "a canonical zone name in the wrong case",
            time_zone_id: "america/los_angeles",
            admitted: false,
        },
        ZoneRow {
            case: "a canonical zone name with a space for its underscore",
            time_zone_id: "America/Los Angeles",
            admitted: false,
        },
        ZoneRow {
            case: "a well-formed name of no zone at all",
            time_zone_id: "Mars/Phobos",
            admitted: false,
        },
        ZoneRow {
            case: "a canonical zone name with a component appended",
            time_zone_id: "America/Los_Angeles/Downtown",
            admitted: false,
        },
    ]
}

/// One team size and one `NumberOfOnCalls`, and the verdict the rule gives the pair.
struct OnCallRow {
    case: &'static str,
    /// How many contacts the rotation's shift team holds.
    team: usize,
    number_of_on_calls: i64,
    /// `true` when the team can meet the count, `false` when it cannot.
    admitted: bool,
}

/// The pairs the on-calls rule is asserted over: the published lower bound of the member, a
/// team that exactly meets the count, a team one short of it, and counts far past a team's
/// size — including the largest team the model admits (`ContactIds` is `length 1..30`).
fn on_call_rows() -> Vec<OnCallRow> {
    vec![
        OnCallRow {
            case: "one contact, one on call — the member's published minimum",
            team: 1,
            number_of_on_calls: 1,
            admitted: true,
        },
        OnCallRow {
            case: "three contacts, two on call",
            team: 3,
            number_of_on_calls: 2,
            admitted: true,
        },
        OnCallRow {
            case: "three contacts, all three on call",
            team: 3,
            number_of_on_calls: 3,
            admitted: true,
        },
        OnCallRow {
            case: "the largest modelled team, all of it on call",
            team: 30,
            number_of_on_calls: 30,
            admitted: true,
        },
        OnCallRow {
            case: "one contact, two on call — one past the team",
            team: 1,
            number_of_on_calls: 2,
            admitted: false,
        },
        OnCallRow {
            case: "three contacts, four on call — one past the team",
            team: 3,
            number_of_on_calls: 4,
            admitted: false,
        },
        OnCallRow {
            case: "one contact, thirty on call",
            team: 1,
            number_of_on_calls: 30,
            admitted: false,
        },
        OnCallRow {
            case: "the largest modelled team, one past it on call",
            team: 30,
            number_of_on_calls: 31,
            admitted: false,
        },
    ]
}

/// The simplest recurrence the service admits, carrying `number_of_on_calls`.
fn recurrence(number_of_on_calls: i64) -> Value {
    json!({
        "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
        "NumberOfOnCalls": number_of_on_calls,
        "RecurrenceMultiplier": 1,
    })
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

/// Create `count` contacts and answer their `ContactArn`s, in creation order.
async fn contact_pool(emulator: &Emulator, count: usize, case: &str) -> Vec<String> {
    let mut arns = Vec::with_capacity(count);
    for index in 0..count {
        arns.push(
            emulator
                .create_contact(
                    json!({
                        "Alias": format!("oncall-{index}"),
                        "Type": "PERSONAL",
                        "Plan": {"Stages": []},
                    }),
                    case,
                )
                .await,
        );
    }
    arns
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

/// How many rotations `ListRotations` reports, so a refused create can be shown to have
/// minted nothing.
async fn rotation_count(emulator: &Emulator, case: &str) -> usize {
    let (status, body) = emulator.call("ListRotations", json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListRotations to answer 200, got {status} {body:?}"
    );
    body["Rotations"].as_array().map_or(0, Vec::len)
}

/// Assert `body` is the member-level refusal the rule answers with: HTTP 400, a
/// `ValidationException`, and a message naming the member at fault.
fn assert_member_refusal(status: u16, body: &Value, member: &str, case: &str) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} {body:?}"
    );
    assert!(
        error_code(body).contains("ValidationException"),
        "case {case:?}: expected a ValidationException, got {body:?}"
    );
    assert!(
        error_message(body).contains(member),
        "case {case:?}: expected the refusal to name {member:?}, got {:?}",
        error_message(body)
    );
}

// ---------------------------------------------------------------------------
// Rule 1: a TimeZoneId names an IANA zone, in both operations that write one
// ---------------------------------------------------------------------------

#[tokio::test]
async fn both_operations_admit_only_an_iana_time_zone() {
    for row in zone_rows() {
        let case = format!("the time-zone rule applied to {}", row.case);
        let emulator = Emulator::new();
        let team = contact_pool(&emulator, 1, &case).await;

        // The fixture rotation, created on a zone the rule admits, is what a refused row has
        // to leave untouched.
        let (status, body) = emulator
            .call(
                "CreateRotation",
                json!({
                    "Name": "fixture",
                    "ContactIds": team.clone(),
                    "TimeZoneId": FIXTURE_ZONE,
                    "Recurrence": recurrence(1),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the fixture CreateRotation to answer 200, \
             got {status} {body:?}"
        );
        let fixture = body["RotationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
            .to_string();

        let (create_status, create_body) = emulator
            .call(
                "CreateRotation",
                json!({
                    "Name": "under-test",
                    "ContactIds": team.clone(),
                    "TimeZoneId": row.time_zone_id,
                    "Recurrence": recurrence(1),
                }),
            )
            .await;
        let (update_status, update_body) = emulator
            .call(
                "UpdateRotation",
                json!({
                    "RotationId": fixture.clone(),
                    "TimeZoneId": row.time_zone_id,
                    "Recurrence": recurrence(1),
                }),
            )
            .await;

        // One rule, so the two operations answer the same status for the same zone name.
        assert_eq!(
            create_status, update_status,
            "case {case:?}: CreateRotation answered {create_status} {create_body:?} while \
             UpdateRotation answered {update_status} {update_body:?}"
        );

        if row.admitted {
            assert_eq!(
                create_status, 200,
                "case {case:?}: expected HTTP 200, got {create_status} {create_body:?}"
            );
            // Admitted means stored: each operation's rotation reads back on the named zone.
            let created = create_body["RotationArn"]
                .as_str()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected a RotationArn, got {create_body:?}")
                })
                .to_string();
            let after_create = get_rotation(&emulator, &created, &case).await;
            assert_eq!(
                after_create["TimeZoneId"],
                json!(row.time_zone_id),
                "case {case:?}: expected CreateRotation to store {:?}, got {:?}",
                row.time_zone_id,
                after_create["TimeZoneId"]
            );
            let after_update = get_rotation(&emulator, &fixture, &case).await;
            assert_eq!(
                after_update["TimeZoneId"],
                json!(row.time_zone_id),
                "case {case:?}: expected UpdateRotation to store {:?}, got {:?}",
                row.time_zone_id,
                after_update["TimeZoneId"]
            );
            continue;
        }

        assert_member_refusal(create_status, &create_body, "timeZoneId", &case);
        assert_member_refusal(update_status, &update_body, "timeZoneId", &case);
        assert_eq!(
            error_message(&create_body),
            error_message(&update_body),
            "case {case:?}: expected one refusal message, got {:?} from CreateRotation and \
             {:?} from UpdateRotation",
            error_message(&create_body),
            error_message(&update_body)
        );

        // A refusal writes nothing: no second rotation was minted, and the fixture is still
        // on the zone it was created with.
        let rotations = rotation_count(&emulator, &case).await;
        assert_eq!(
            rotations, 1,
            "case {case:?}: expected the store to hold only the fixture rotation, \
             got {rotations}"
        );
        let after = get_rotation(&emulator, &fixture, &case).await;
        assert_eq!(
            after["TimeZoneId"],
            json!(FIXTURE_ZONE),
            "case {case:?}: expected the stored TimeZoneId untouched, got {:?}",
            after["TimeZoneId"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: NumberOfOnCalls is bounded by the rotation's shift team
// ---------------------------------------------------------------------------

/// The count is judged against the team the request leaves the rotation with, in both
/// operations: `CreateRotation`'s own `ContactIds`, and for `UpdateRotation` the team it
/// names — or the stored team when it names none.
#[tokio::test]
async fn both_operations_bound_the_on_call_count_by_the_shift_team() {
    for row in on_call_rows() {
        let case = format!("the on-calls rule applied to {}", row.case);
        let emulator = Emulator::new();
        let pool = contact_pool(&emulator, row.team, &case).await;

        // The fixture rotation carries the row's team and the smallest count every team can
        // meet, so the only thing under test is the count the calls below ask for.
        let (status, body) = emulator
            .call(
                "CreateRotation",
                json!({
                    "Name": "fixture",
                    "ContactIds": pool.clone(),
                    "TimeZoneId": FIXTURE_ZONE,
                    "Recurrence": recurrence(1),
                }),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the fixture CreateRotation to answer 200, \
             got {status} {body:?}"
        );
        let fixture = body["RotationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
            .to_string();

        let (create_status, create_body) = emulator
            .call(
                "CreateRotation",
                json!({
                    "Name": "under-test",
                    "ContactIds": pool.clone(),
                    "TimeZoneId": FIXTURE_ZONE,
                    "Recurrence": recurrence(row.number_of_on_calls),
                }),
            )
            .await;
        // The update names NO team, so the count is judged against the stored one.
        let (update_status, update_body) = emulator
            .call(
                "UpdateRotation",
                json!({
                    "RotationId": fixture.clone(),
                    "Recurrence": recurrence(row.number_of_on_calls),
                }),
            )
            .await;
        // And again naming exactly the team it already has, which must give the same verdict.
        let (named_status, named_body) = emulator
            .call(
                "UpdateRotation",
                json!({
                    "RotationId": fixture.clone(),
                    "ContactIds": pool.clone(),
                    "Recurrence": recurrence(row.number_of_on_calls),
                }),
            )
            .await;

        assert_eq!(
            create_status, update_status,
            "case {case:?}: CreateRotation answered {create_status} {create_body:?} while \
             UpdateRotation answered {update_status} {update_body:?}"
        );
        assert_eq!(
            update_status, named_status,
            "case {case:?}: an update naming the stored team answered {named_status} \
             {named_body:?} while one naming none answered {update_status} {update_body:?}"
        );

        if row.admitted {
            assert_eq!(
                create_status, 200,
                "case {case:?}: expected HTTP 200, got {create_status} {create_body:?}"
            );
            let after = get_rotation(&emulator, &fixture, &case).await;
            assert_eq!(
                after["Recurrence"],
                recurrence(row.number_of_on_calls),
                "case {case:?}: expected the installed recurrence, got {:?}",
                after["Recurrence"]
            );
            continue;
        }

        assert_member_refusal(
            create_status,
            &create_body,
            "recurrence.numberOfOnCalls",
            &case,
        );
        assert_member_refusal(
            update_status,
            &update_body,
            "recurrence.numberOfOnCalls",
            &case,
        );
        // A refusal writes nothing: the fixture still carries the recurrence it was created
        // with, and no second rotation was minted.
        let after = get_rotation(&emulator, &fixture, &case).await;
        assert_eq!(
            after["Recurrence"],
            recurrence(1),
            "case {case:?}: expected the stored recurrence untouched, got {:?}",
            after["Recurrence"]
        );
        let rotations = rotation_count(&emulator, &case).await;
        assert_eq!(
            rotations, 1,
            "case {case:?}: expected the store to hold only the fixture rotation, \
             got {rotations}"
        );
    }
}

/// A count a rotation's CURRENT team cannot meet is admitted by the same update when it
/// names a team that can, and a count the stored team meets is refused by an update that
/// cuts the team below it: the bound follows the team the update leaves behind, not the one
/// the rotation was created with.
#[tokio::test]
async fn the_bound_follows_the_team_the_update_leaves_behind() {
    let case = "an update that grows the team to meet the count";
    let emulator = Emulator::new();
    let pool = contact_pool(&emulator, 4, case).await;

    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": "growing",
                "ContactIds": [pool[0].clone()],
                "TimeZoneId": FIXTURE_ZONE,
                "Recurrence": recurrence(1),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation to answer 200, got {status} {body:?}"
    );
    let arn = body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string();

    // Three on call is past the one-contact team the rotation holds...
    let (status, body) = emulator
        .call(
            "UpdateRotation",
            json!({"RotationId": arn.clone(), "Recurrence": recurrence(3)}),
        )
        .await;
    assert_member_refusal(status, &body, "recurrence.numberOfOnCalls", case);

    // ... and admitted by the update that brings the team up to three in the same call.
    let grown = vec![pool[0].clone(), pool[1].clone(), pool[2].clone()];
    let (status, body) = emulator
        .call(
            "UpdateRotation",
            json!({
                "RotationId": arn.clone(),
                "ContactIds": grown.clone(),
                "Recurrence": recurrence(3),
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 once the team meets the count, got {status} {body:?}"
    );
    let after = get_rotation(&emulator, &arn, case).await;
    assert_eq!(
        after["ContactIds"],
        json!(grown),
        "case {case:?}: expected the grown team stored, got {:?}",
        after["ContactIds"]
    );
    assert_eq!(
        after["Recurrence"],
        recurrence(3),
        "case {case:?}: expected the installed recurrence, got {:?}",
        after["Recurrence"]
    );

    // The mirror: an update that cuts the team below the count it keeps is refused, and the
    // rotation it would have left unschedulable is untouched.
    let case = "an update that cuts the team below the count it keeps";
    let (status, body) = emulator
        .call(
            "UpdateRotation",
            json!({
                "RotationId": arn.clone(),
                "ContactIds": [pool[3].clone()],
                "Recurrence": recurrence(3),
            }),
        )
        .await;
    assert_member_refusal(status, &body, "recurrence.numberOfOnCalls", case);
    let after = get_rotation(&emulator, &arn, case).await;
    assert_eq!(
        after["ContactIds"],
        json!(grown),
        "case {case:?}: expected the stored team untouched, got {:?}",
        after["ContactIds"]
    );
    assert_eq!(
        after["Recurrence"],
        recurrence(3),
        "case {case:?}: expected the stored recurrence untouched, got {:?}",
        after["Recurrence"]
    );
}
