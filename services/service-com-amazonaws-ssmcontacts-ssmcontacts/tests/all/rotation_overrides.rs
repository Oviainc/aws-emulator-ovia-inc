// SPDX-License-Identifier: CC0-1.0

//! The rotation-override lifecycle — `CreateRotationOverride`, `GetRotationOverride` and
//! `DeleteRotationOverride` — end to end over the wire.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see — routing, deserialization, constraint validation, the handler and
//! serialization included. Nothing is placed in the store by hand except the account's
//! replication set, which no modelled operation of this service can mint.
//!
//! The rules under test, each as a TABLE rather than a single call:
//!
//! 1. THE ROUND TRIP. An override created for a rotation is reported by
//!    `GetRotationOverride` with exactly the members the create persisted, and the id the
//!    create minted is unique per override. The rows differ along what that generalises
//!    over: the size of the replacement team (including both ends of the list's modelled
//!    length), the length and position of the override window, and its precision.
//! 2. THE DELETE. `DeleteRotationOverride` removes the one override the `(RotationId,
//!    RotationOverrideId)` pair names — after it the override reads 404 and a second delete
//!    is refused — and leaves every other override of that rotation, and of every other
//!    rotation, exactly as it was. An id is unique only WITHIN its rotation, so the same id
//!    under a different rotation names nothing, and deleting the ROTATION takes its own
//!    overrides with it.
//! 3. THE ACCOUNT GATE. Incident Manager is fronted by a per-account replication set, and
//!    every one of these operations is admitted only while the account holds an ACTIVE one;
//!    otherwise it is refused HTTP 400 `ValidationException` BEFORE the override is looked
//!    up or removed.
//! 4. THE REFUSALS. A create naming a rotation or a replacement contact the store does not
//!    hold is refused 404, a window whose end does not follow its start is refused 400, and
//!    a replacement list past its modelled maximum is refused 400 before the handler runs.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state::{self, AccountReplicationSet};

use crate::contact_lifecycle::{error_code, Emulator, ACCOUNT, REGION};
use crate::contact_updates::{active_replication_set, onboard, set_replication_set};

/// A rotation ARN this service never minted, for the "no such rotation" rows.
const ABSENT_ROTATION_ARN: &str =
    "arn:aws:ssm-contacts:us-east-1:000000000000:rotation/00000000-0000-0000-0000-000000000000";

/// A `RotationOverrideId` this service never minted. Inside the modelled `Uuid`
/// (`length` 36..39, the 8-4-4-4-12 pattern), so it reaches the handler.
const ABSENT_OVERRIDE_ID: &str = "00000000-0000-0000-0000-000000000000";

/// A contact ARN of the form `CreateContact` mints, numbered.
fn contact_arn(index: usize) -> String {
    format!("arn:aws:ssm-contacts:{REGION}:{ACCOUNT}:contact/oncall-{index}")
}

/// Create the contacts `oncall-0..count` and answer their ARNs, which is what a rotation
/// team and an override's replacement team are named by.
///
/// A row that already created a contact gets the conflict the create answers for a
/// duplicate alias, which is as good as having created it.
pub(crate) async fn ensure_contacts(emulator: &Emulator, count: usize, case: &str) -> Vec<String> {
    for index in 0..count {
        let (status, body) = emulator
            .call(
                "CreateContact",
                json!({
                    "Alias": format!("oncall-{index}"),
                    "Type": "PERSONAL",
                    "Plan": {"Stages": []},
                }),
            )
            .await;
        assert!(
            status == 200 || status == 409,
            "case {case:?}: expected CreateContact for oncall-{index} to answer 200 (or 409 \
             if an earlier row created it), got {status} {body:?}"
        );
    }
    (0..count).map(contact_arn).collect()
}

/// Create a rotation whose team is one contact, and answer its `RotationArn`.
pub(crate) async fn create_rotation(emulator: &Emulator, name: &str, case: &str) -> String {
    let team = ensure_contacts(emulator, 1, case).await;
    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": name,
                "ContactIds": team,
                "TimeZoneId": "UTC",
                "StartTime": 1_735_689_600,
                "Recurrence": {
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                },
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation to answer 200, got {status} {body:?}"
    );
    body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string()
}

/// Create an override, asserting the call succeeded, and answer its `RotationOverrideId`.
pub(crate) async fn create_override(emulator: &Emulator, request: Value, case: &str) -> String {
    let (status, body) = emulator.call("CreateRotationOverride", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotationOverride to answer 200, got {status} {body:?}"
    );
    body["RotationOverrideId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationOverrideId, got {body:?}"))
        .to_string()
}

/// How many overrides the store holds right now, over every rotation.
fn override_count(emulator: &Emulator) -> usize {
    let store =
        state::service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    store.rotation_overrides.len()
}

/// One round-trip row: the replacement team's size and the override's window.
struct OverrideRow {
    case: &'static str,
    /// How many contacts replace the rotation's own team for the window.
    team: usize,
    /// `StartTime`, in epoch seconds as the wire carries a `DateTime`.
    start: f64,
    /// `EndTime`, likewise.
    end: f64,
}

/// The rows. They differ along the size of the replacement team and the length, position
/// and precision of the window — what the round-trip rule generalises over.
///
/// `CreateRotationOverride$NewContactIds` is modelled `length 0..30`
/// (`smithy_query(query="operation",operation="CreateRotationOverride")`) and the list the
/// read reports, `RotationOverride$NewContactIds`, `length 0..25`, so 0 and 25 are the two
/// sizes both ends admit and both are rows here.
const OVERRIDE_ROWS: &[OverrideRow] = &[
    OverrideRow {
        case: "one replacement contact over a one-hour window",
        team: 1,
        start: 1_735_689_600.0,
        end: 1_735_693_200.0,
    },
    OverrideRow {
        case: "no replacement contacts at all — the list's modelled minimum",
        team: 0,
        start: 1_700_000_000.0,
        end: 1_700_604_800.0,
    },
    OverrideRow {
        case: "three replacements over a weekend",
        team: 3,
        start: 1_767_225_600.0,
        end: 1_767_398_400.0,
    },
    OverrideRow {
        case: "the largest replacement team both the create and the read admit",
        team: 25,
        start: 1_600_000_000.0,
        end: 1_600_000_060.0,
    },
    OverrideRow {
        case: "a window that predates the epoch",
        team: 2,
        start: -86_400.0,
        end: -3_600.0,
    },
    OverrideRow {
        case: "a sub-second window",
        team: 1,
        start: 1_700_000_000.5,
        end: 1_700_000_001.25,
    },
];

/// The `CreateRotationOverride` request body for a row.
async fn override_request(
    emulator: &Emulator,
    rotation_arn: &str,
    row: &OverrideRow,
) -> Value {
    let team = ensure_contacts(emulator, row.team, row.case).await;
    json!({
        "RotationId": rotation_arn,
        "NewContactIds": team,
        "StartTime": row.start,
        "EndTime": row.end,
    })
}

/// THE ROUND TRIP: an override is reported with exactly what the create persisted, each
/// create mints its own id, and `CreateTime` is the instant the create stored it and is the
/// same on every read.
#[tokio::test]
async fn an_override_reads_back_as_created() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-round-trip", "round trip").await;
    let mut ids: Vec<String> = Vec::new();

    for row in OVERRIDE_ROWS {
        let case = row.case;
        let request = override_request(&emulator, &rotation, row).await;
        let before_create = epoch_seconds_now();
        let id = create_override(&emulator, request.clone(), case).await;
        let after_create = epoch_seconds_now();

        let (status, body) = emulator
            .call(
                "GetRotationOverride",
                json!({"RotationId": rotation, "RotationOverrideId": id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRotationOverride to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["RotationOverrideId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the id the create minted, got {body:?}"
        );
        assert_eq!(
            body["RotationArn"].as_str(),
            Some(rotation.as_str()),
            "case {case:?}: expected the overridden rotation's ARN, got {body:?}"
        );
        assert_eq!(
            body["NewContactIds"], request["NewContactIds"],
            "case {case:?}: expected the created NewContactIds in the created order, got \
             {body:?}"
        );
        assert_eq!(
            body["StartTime"].as_f64(),
            Some(row.start),
            "case {case:?}: expected the created StartTime {}, got {body:?}",
            row.start
        );
        assert_eq!(
            body["EndTime"].as_f64(),
            Some(row.end),
            "case {case:?}: expected the created EndTime {}, got {body:?}",
            row.end
        );
        let create_time = body["CreateTime"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a CreateTime, got {body:?}"));
        assert!(
            create_time >= before_create && create_time <= after_create,
            "case {case:?}: expected a CreateTime inside [{before_create}, {after_create}], \
             got {create_time}"
        );

        // A second read of an unchanged override answers identically: the read reports, it
        // does not recompute.
        let (_, again) = emulator
            .call(
                "GetRotationOverride",
                json!({"RotationId": rotation, "RotationOverrideId": id}),
            )
            .await;
        assert_eq!(
            again, body,
            "case {case:?}: expected a second read to answer the same, got {again:?}"
        );

        assert!(
            !ids.contains(&id),
            "case {case:?}: expected a fresh RotationOverrideId, got {id:?} again (ids so far \
             {ids:?})"
        );
        // The id is modelled `Uuid` (`length` 36..39): a value outside that could not be
        // carried by the read's own input member.
        assert!(
            (36..=39).contains(&id.chars().count()),
            "case {case:?}: expected a Uuid of 36..39 characters, got {id:?}"
        );
        ids.push(id);
    }

    assert_eq!(
        override_count(&emulator),
        OVERRIDE_ROWS.len(),
        "case \"round trip\": expected one stored override per row, got {}",
        override_count(&emulator)
    );
}

/// THE DELETE: it removes the one override the pair names, so that override then reads 404
/// and a second delete of it is refused, and every other override — of the same rotation
/// and of another one — is left exactly as it was.
#[tokio::test]
async fn a_delete_removes_only_the_override_it_names() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-delete", "delete").await;
    let other_rotation = create_rotation(&emulator, "override-delete-other", "delete").await;

    // Three overrides on the rotation under test, plus one on another rotation: deleting
    // from the middle is what shows the delete is keyed by the pair and not positional.
    let mut ids: Vec<String> = Vec::new();
    for row in &OVERRIDE_ROWS[..3] {
        let request = override_request(&emulator, &rotation, row).await;
        ids.push(create_override(&emulator, request, row.case).await);
    }
    let bystander_row = &OVERRIDE_ROWS[3];
    let bystander_request = override_request(&emulator, &other_rotation, bystander_row).await;
    let bystander = create_override(&emulator, bystander_request, bystander_row.case).await;

    for (index, id) in ids.clone().into_iter().enumerate() {
        let case = format!("deleting override {index} of {}", ids.len());
        let before = override_count(&emulator);

        let (status, body) = emulator
            .call(
                "DeleteRotationOverride",
                json!({"RotationId": rotation, "RotationOverrideId": id}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DeleteRotationOverride to answer 200, got {status} \
             {body:?}"
        );
        assert_eq!(
            override_count(&emulator),
            before - 1,
            "case {case:?}: expected exactly one override removed from {before}, got {}",
            override_count(&emulator)
        );

        // The override it named is gone...
        let (status, body) = emulator
            .call(
                "GetRotationOverride",
                json!({"RotationId": rotation, "RotationOverrideId": id}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected the deleted override to read 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );

        // ...and deleting it again is refused rather than answered 200, because there is no
        // longer any override for the pair to name.
        let (status, body) = emulator
            .call(
                "DeleteRotationOverride",
                json!({"RotationId": rotation, "RotationOverrideId": id}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected a second delete to answer 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );

        // Every override not yet deleted is still readable, this rotation's and the other
        // rotation's alike.
        for survivor in ids.iter().skip(index + 1) {
            let (status, body) = emulator
                .call(
                    "GetRotationOverride",
                    json!({"RotationId": rotation, "RotationOverrideId": survivor}),
                )
                .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected override {survivor:?} untouched, got {status} {body:?}"
            );
        }
        let (status, body) = emulator
            .call(
                "GetRotationOverride",
                json!({"RotationId": other_rotation, "RotationOverrideId": bystander}),
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the other rotation's override untouched, got {status} \
             {body:?}"
        );
    }

    assert_eq!(
        override_count(&emulator),
        1,
        "case \"delete\": expected only the other rotation's override left, got {}",
        override_count(&emulator)
    );
}

/// A `RotationOverrideId` is unique only WITHIN its rotation: the same id named under a
/// rotation that does not hold it names nothing, so neither the read nor the delete reaches
/// the override — and the delete that was refused removed nothing.
#[tokio::test]
async fn an_id_named_under_another_rotation_names_nothing() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-scope", "scope").await;
    let other_rotation = create_rotation(&emulator, "override-scope-other", "scope").await;
    let row = &OVERRIDE_ROWS[0];
    let request = override_request(&emulator, &rotation, row).await;
    let id = create_override(&emulator, request, "scope").await;

    for (case, named_rotation) in [
        ("another rotation the store holds", other_rotation.as_str()),
        ("a rotation the store does not hold", ABSENT_ROTATION_ARN),
    ] {
        for operation in ["GetRotationOverride", "DeleteRotationOverride"] {
            let (status, body) = emulator
                .call(
                    operation,
                    json!({"RotationId": named_rotation, "RotationOverrideId": id}),
                )
                .await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected {operation} to answer 404, got {status} {body:?}"
            );
            assert!(
                error_code(&body).contains("ResourceNotFoundException"),
                "case {case:?}: expected a ResourceNotFoundException from {operation}, got \
                 {body:?}"
            );
        }
    }

    // The override is still there, under the rotation that does hold it.
    let (status, body) = emulator
        .call(
            "GetRotationOverride",
            json!({"RotationId": rotation, "RotationOverrideId": id}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"scope\": expected the override still readable under its own rotation, got \
         {status} {body:?}"
    );
    assert_eq!(
        override_count(&emulator),
        1,
        "case \"scope\": expected the refused deletes to have removed nothing, got {}",
        override_count(&emulator)
    );
}

/// THE ACCOUNT GATE: each of the three operations is admitted only while the account holds
/// an ACTIVE replication set, and the gate is evaluated BEFORE the override is looked up or
/// removed — so a refused delete leaves the store as it was.
#[tokio::test]
async fn the_replication_set_gate_decides_before_the_override_is_touched() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-gate", "gate").await;
    let row = &OVERRIDE_ROWS[0];
    let request = override_request(&emulator, &rotation, row).await;
    let id = create_override(&emulator, request.clone(), "gate").await;

    // Every state the account's `replication_set` record can be in: absent, present but not
    // active (only an ACTIVE set opens the data plane), and present and active.
    let not_active = AccountReplicationSet {
        active: false,
        ..active_replication_set()
    };
    let gate_cases = [
        ("no replication set at all", None, 400),
        ("a replication set that is not active", Some(not_active), 400),
        ("an active replication set", Some(active_replication_set()), 200),
    ];

    for (case, replication_set, expected) in gate_cases {
        set_replication_set(&emulator, replication_set);

        for operation in [
            "CreateRotationOverride",
            "GetRotationOverride",
            "DeleteRotationOverride",
        ] {
            let body = match operation {
                "CreateRotationOverride" => request.clone(),
                _ => json!({"RotationId": rotation, "RotationOverrideId": id}),
            };
            let before = override_count(&emulator);
            let (status, response) = emulator.call(operation, body).await;
            assert_eq!(
                status, expected,
                "case {case:?}: expected {operation} to answer HTTP {expected}, got {status} \
                 {response:?}"
            );
            if expected == 400 {
                assert!(
                    error_code(&response).contains("ValidationException"),
                    "case {case:?}: expected a ValidationException from {operation}, got \
                     {response:?}"
                );
                // The refusal carries the shape this service's account refusals do: a
                // populated `Message` and `Reason`, and `Fields` present and empty — no one
                // input member is at fault.
                assert!(
                    response["Message"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty())
                        && response["Reason"]
                            .as_str()
                            .is_some_and(|text| !text.is_empty())
                        && response["Fields"] == json!([]),
                    "case {case:?}: expected a populated Message and Reason and an empty \
                     Fields list from {operation}, got {response:?}"
                );
                assert_eq!(
                    override_count(&emulator),
                    before,
                    "case {case:?}: expected the refused {operation} to change nothing, got \
                     {} override(s) where there were {before}",
                    override_count(&emulator)
                );
            }
        }
        // The admitted round deleted the override, so the next row needs it back.
        if expected == 200 {
            onboard(&emulator);
            create_override(&emulator, request.clone(), "gate").await;
        }
    }
}

/// A create naming a rotation the store does not hold, or a replacement contact it does not
/// hold, is refused 404 and persists nothing.
#[tokio::test]
async fn a_create_resolves_every_arn_it_names() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-refusals", "refusals").await;
    let known = ensure_contacts(&emulator, 1, "refusals").await;
    let unknown_contact = contact_arn(99);

    let cases = [
        (
            "a rotation the store does not hold",
            json!({
                "RotationId": ABSENT_ROTATION_ARN,
                "NewContactIds": known.clone(),
                "StartTime": 1_735_689_600,
                "EndTime": 1_735_693_200,
            }),
        ),
        (
            "a replacement contact the store does not hold",
            json!({
                "RotationId": rotation,
                "NewContactIds": [unknown_contact.clone()],
                "StartTime": 1_735_689_600,
                "EndTime": 1_735_693_200,
            }),
        ),
        (
            "one known and one unknown replacement contact",
            json!({
                "RotationId": rotation,
                "NewContactIds": [known[0].clone(), unknown_contact],
                "StartTime": 1_735_689_600,
                "EndTime": 1_735_693_200,
            }),
        ),
    ];

    for (case, request) in cases {
        let (status, body) = emulator.call("CreateRotationOverride", request).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected CreateRotationOverride to answer 404, got {status} \
             {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            override_count(&emulator),
            0,
            "case {case:?}: expected nothing persisted, got {} override(s)",
            override_count(&emulator)
        );
    }
}

/// A window whose `EndTime` does not follow its `StartTime` covers no shift, so it is
/// refused 400 and persists nothing; a window that does follow is created. The rows cover
/// the end before the start, the two equal, and — as the admitted counterpart — the
/// smallest window the wire can express.
#[tokio::test]
async fn a_create_requires_a_window_that_ends_after_it_starts() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-window", "window").await;
    let team = ensure_contacts(&emulator, 1, "window").await;

    let rows: [(&str, f64, f64, u16); 5] = [
        ("the end a day before the start", 1_735_689_600.0, 1_735_603_200.0, 400),
        ("the end one second before the start", 1_735_689_600.0, 1_735_689_599.0, 400),
        ("the end exactly at the start", 1_735_689_600.0, 1_735_689_600.0, 400),
        ("the end one second after the start", 1_735_689_600.0, 1_735_689_601.0, 200),
        (
            "the end a quarter-second after the start",
            1_735_689_600.0,
            1_735_689_600.25,
            200,
        ),
    ];

    let mut created = 0;
    for (case, start, end, expected) in rows {
        let (status, body) = emulator
            .call(
                "CreateRotationOverride",
                json!({
                    "RotationId": rotation,
                    "NewContactIds": team,
                    "StartTime": start,
                    "EndTime": end,
                }),
            )
            .await;
        assert_eq!(
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
        );
        if expected == 400 {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
        } else {
            created += 1;
        }
        assert_eq!(
            override_count(&emulator),
            created,
            "case {case:?}: expected {created} stored override(s), got {}",
            override_count(&emulator)
        );
    }
}

/// `NewContactIds` is modelled `length 0..30`, and a list past that maximum is refused
/// before the handler runs — so the refusal needs no contact to exist at all.
#[tokio::test]
async fn a_replacement_list_past_its_modelled_maximum_is_refused() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-bounds", "bounds").await;

    // 30 is the published maximum and 31 is one past it; the entries need not resolve for
    // the 31-entry list, which is refused on the constraint before anything is looked up.
    for (case, size, expected) in [
        ("the published maximum, 30 replacements", 30, 200),
        ("one past the published maximum, 31 replacements", 31, 400),
    ] {
        let team: Vec<String> = if expected == 200 {
            ensure_contacts(&emulator, size, case).await
        } else {
            (0..size).map(contact_arn).collect()
        };
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
            status, expected,
            "case {case:?}: expected HTTP {expected}, got {status} {body:?}"
        );
        if expected == 400 {
            assert!(
                error_code(&body).contains("ValidationException"),
                "case {case:?}: expected a ValidationException, got {body:?}"
            );
        }
    }
}

/// `IdempotencyToken` makes a retried create the SAME call: it answers the id its first call
/// minted and stores no second override. Once the override is deleted the token names
/// nothing, so a create carrying it again mints a NEW override.
#[tokio::test]
async fn a_retried_create_replays_its_first_override() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-idempotency", "idempotency").await;
    let team = ensure_contacts(&emulator, 1, "idempotency").await;
    let request = json!({
        "RotationId": rotation,
        "NewContactIds": team,
        "StartTime": 1_735_689_600,
        "EndTime": 1_735_693_200,
        "IdempotencyToken": "retry-token-1",
    });

    let first = create_override(&emulator, request.clone(), "idempotency").await;
    let replayed = create_override(&emulator, request.clone(), "idempotency").await;
    assert_eq!(
        replayed, first,
        "case \"a retried create\": expected the first call's id {first:?}, got {replayed:?}"
    );
    assert_eq!(
        override_count(&emulator),
        1,
        "case \"a retried create\": expected one stored override, got {}",
        override_count(&emulator)
    );

    let (status, body) = emulator
        .call(
            "DeleteRotationOverride",
            json!({"RotationId": rotation, "RotationOverrideId": first}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"a retried create\": expected the delete to answer 200, got {status} {body:?}"
    );

    let after_delete = create_override(&emulator, request, "idempotency").await;
    assert_ne!(
        after_delete, first,
        "case \"the token of a deleted override\": expected a NEW id, got the deleted one \
         {first:?}"
    );
    assert_eq!(
        override_count(&emulator),
        1,
        "case \"the token of a deleted override\": expected one stored override, got {}",
        override_count(&emulator)
    );
}

/// A create with an id the caller invented is not reachable: `RotationOverrideId` is minted
/// by the service, so an id the store never issued reads and deletes as 404.
#[tokio::test]
async fn an_id_the_store_never_issued_names_nothing() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-unknown-id", "unknown id").await;

    for operation in ["GetRotationOverride", "DeleteRotationOverride"] {
        let (status, body) = emulator
            .call(
                operation,
                json!({"RotationId": rotation, "RotationOverrideId": ABSENT_OVERRIDE_ID}),
            )
            .await;
        assert_eq!(
            status, 404,
            "case \"an id the store never issued\": expected {operation} to answer 404, got \
             {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case \"an id the store never issued\": expected a ResourceNotFoundException from \
             {operation}, got {body:?}"
        );
    }
}

/// An override is an override OF a rotation: deleting the rotation takes its overrides with
/// it — they are then unreachable through every operation — and leaves another rotation's
/// overrides alone.
#[tokio::test]
async fn deleting_the_rotation_takes_its_overrides_with_it() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-rotation-delete", "rotation delete").await;
    let other_rotation =
        create_rotation(&emulator, "override-rotation-delete-other", "rotation delete").await;

    let mut ids: Vec<String> = Vec::new();
    for row in &OVERRIDE_ROWS[..3] {
        let request = override_request(&emulator, &rotation, row).await;
        ids.push(create_override(&emulator, request, row.case).await);
    }
    let bystander_row = &OVERRIDE_ROWS[3];
    let bystander_request = override_request(&emulator, &other_rotation, bystander_row).await;
    let bystander = create_override(&emulator, bystander_request, bystander_row.case).await;

    let (status, body) = emulator
        .call("DeleteRotation", json!({"RotationId": rotation}))
        .await;
    assert_eq!(
        status, 200,
        "case \"deleting the rotation\": expected DeleteRotation to answer 200, got {status} \
         {body:?}"
    );

    for id in &ids {
        for operation in ["GetRotationOverride", "DeleteRotationOverride"] {
            let (status, body) = emulator
                .call(
                    operation,
                    json!({"RotationId": rotation, "RotationOverrideId": id}),
                )
                .await;
            assert_eq!(
                status, 404,
                "case \"deleting the rotation\": expected {operation} for {id:?} to answer \
                 404, got {status} {body:?}"
            );
        }
    }
    assert_eq!(
        override_count(&emulator),
        1,
        "case \"deleting the rotation\": expected only the other rotation's override left, \
         got {}",
        override_count(&emulator)
    );
    let (status, body) = emulator
        .call(
            "GetRotationOverride",
            json!({"RotationId": other_rotation, "RotationOverrideId": bystander}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"deleting the rotation\": expected the other rotation's override untouched, \
         got {status} {body:?}"
    );
}

/// Epoch seconds now, as the bound a server-chosen `CreateTime` must lie inside.
fn epoch_seconds_now() -> f64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}
