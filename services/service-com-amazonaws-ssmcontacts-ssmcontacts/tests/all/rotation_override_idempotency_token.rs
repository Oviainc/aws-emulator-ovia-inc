// SPDX-License-Identifier: CC0-1.0

//! `CreateRotationOverride`'s `IdempotencyToken`, and the form of the id the create mints.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see — routing, deserialization, constraint validation, the handler and
//! serialization included.
//!
//! The rules under test, each as a TABLE rather than a single call:
//!
//! 1. A token identifies ONE create: the operation it was presented to and the resource
//!    that create produced. Presenting it again for the same create is the SAME call, so it
//!    answers the id the first call minted and stores no second override; presenting it for
//!    a create that names a DIFFERENT rotation, or to a different create operation, is a
//!    different call that mints its own resource. The rows differ along what the rule
//!    generalises over: the form and length of the token (both ends of its modelled length
//!    included, where the empty token is no token at all), the rotation it names, and the
//!    operation it is presented to.
//! 2. Every `RotationOverrideId` the create mints satisfies the modelled `Uuid` form, so the
//!    reader's own input member can carry it back.

use serde_json::json;

use crate::contact_lifecycle::Emulator;
use crate::rotation_overrides::{create_override, create_rotation, ensure_contacts, override_count};

/// The window every row overrides. The rule under test is about the token, not the window,
/// so one admitted window (an hour, end after start) serves every row;
/// `tests/all/rotation_overrides.rs` is the suite that varies the window.
const START_TIME: f64 = 1_735_689_600.0;
const END_TIME: f64 = 1_735_693_200.0;

/// One row of the token table.
struct TokenRow {
    case: &'static str,
    /// The token the two creates carry, built per row so a row can name a token as long as
    /// the model admits.
    token: fn() -> String,
    /// Whether presenting this token twice is the SAME call.
    replays: bool,
}

/// The rows. `CreateRotationOverride$IdempotencyToken` is modelled `length 0..2048`,
/// `pattern ^[\\\/a-zA-Z0-9_+=\-]*$`
/// (`smithy_query(query="shape",shape="IdempotencyToken")`), so the forms below cover both
/// published length bounds and every character class the pattern admits — and the empty
/// token, the minimum, carries no identity at all, so each call with it is its own create.
const TOKEN_ROWS: &[TokenRow] = &[
    TokenRow {
        case: "a short ascii token",
        token: || "retry-1".to_string(),
        replays: true,
    },
    TokenRow {
        case: "one character, the shortest token that identifies a call",
        token: || "x".to_string(),
        replays: true,
    },
    TokenRow {
        case: "every character class the modelled pattern admits",
        token: || "A/b_C+d=e-f\\g9".to_string(),
        replays: true,
    },
    TokenRow {
        case: "a token at the modelled maximum length of 2048",
        token: || "t".repeat(2048),
        replays: true,
    },
    TokenRow {
        case: "the empty token, the modelled minimum length",
        token: String::new,
        replays: false,
    },
];

/// The `CreateRotationOverride` body naming `rotation`, `team` and `token`.
fn override_request(rotation: &str, team: &[String], token: &str) -> serde_json::Value {
    json!({
        "RotationId": rotation,
        "NewContactIds": team,
        "StartTime": START_TIME,
        "EndTime": END_TIME,
        "IdempotencyToken": token,
    })
}

/// Assert `id` satisfies the modelled `Uuid` form: `length` 36..39 and the 8-4-4-4-12
/// hex-group pattern (`smithy_query(query="shape",shape="Uuid")`), the constraint
/// `GetRotationOverride$RotationOverrideId` carries too — an id outside it could not be
/// handed back to the reader.
fn assert_modelled_uuid(id: &str, case: &str) {
    assert!(
        (36..=39).contains(&id.chars().count()),
        "case {case:?}: expected a Uuid of 36..39 characters, got {id:?}"
    );
    let groups: Vec<&str> = id.split('-').collect();
    assert_eq!(
        groups.len(),
        5,
        "case {case:?}: expected five '-' separated groups, got {id:?}"
    );
    // The widths the modelled pattern admits, group by group.
    let widths = [(8usize, 11usize), (4, 4), (4, 4), (4, 4), (12, 12)];
    for (group, (low, high)) in groups.iter().zip(widths) {
        let width = group.chars().count();
        assert!(
            (low..=high).contains(&width) && group.chars().all(|c| c.is_ascii_hexdigit()),
            "case {case:?}: expected a hex group of {low}..={high} characters, got {group:?} \
             in {id:?}"
        );
    }
}

/// Read an override back, answering the HTTP status and the `RotationArn` the read reports.
async fn read_override(
    emulator: &Emulator,
    rotation: &str,
    id: &str,
) -> (u16, Option<String>) {
    let (status, body) = emulator
        .call(
            "GetRotationOverride",
            json!({"RotationId": rotation, "RotationOverrideId": id}),
        )
        .await;
    let reported = body["RotationArn"].as_str().map(str::to_string);
    (status, reported)
}

/// RULE 1, over the form of the token: a repeated create carrying the token its first call
/// carried is the SAME call — it answers that call's id and stores no second override —
/// while the empty token identifies nothing, so each call with it mints its own override.
/// Every minted id satisfies the modelled `Uuid` form (RULE 2).
#[tokio::test]
async fn a_repeated_create_with_the_same_token_replays_its_first_override() {
    let emulator = Emulator::new();
    let team = ensure_contacts(&emulator, 1, "token forms").await;

    for (index, row) in TOKEN_ROWS.iter().enumerate() {
        let case = row.case;
        // A row gets its own rotation, so what a row observes is its own token's doing.
        let rotation = create_rotation(&emulator, &format!("override-token-{index}"), case).await;
        let token = (row.token)();
        let request = override_request(&rotation, &team, &token);

        let before = override_count(&emulator);
        let first = create_override(&emulator, request.clone(), case).await;
        assert_modelled_uuid(&first, case);
        let after_first = override_count(&emulator);
        assert_eq!(
            after_first,
            before + 1,
            "case {case:?}: expected the first create to store one override, got {after_first} \
             held where {} were held before",
            before
        );

        let second = create_override(&emulator, request, case).await;
        assert_modelled_uuid(&second, case);
        let after_second = override_count(&emulator);
        if row.replays {
            assert_eq!(
                second, first,
                "case {case:?}: expected the retry to answer the first call's id {first:?}, got \
                 {second:?}"
            );
            assert_eq!(
                after_second, after_first,
                "case {case:?}: expected the retry to store nothing new, got {after_second} \
                 overrides held where {after_first} were"
            );
        } else {
            assert_ne!(
                second, first,
                "case {case:?}: expected a second create to mint a NEW id, got {second:?} again"
            );
            assert_eq!(
                after_second,
                after_first + 1,
                "case {case:?}: expected a second stored override, got {after_second} held \
                 where {after_first} were"
            );
        }

        // Whatever the token did, every id the create answered names a stored override of
        // the rotation the create named.
        for id in [&first, &second] {
            let (status, reported) = read_override(&emulator, &rotation, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetRotationOverride for {id:?} to answer 200, got \
                 {status} reporting {reported:?}"
            );
            assert_eq!(
                reported.as_deref(),
                Some(rotation.as_str()),
                "case {case:?}: expected the override to report rotation {rotation:?}, got \
                 {reported:?}"
            );
        }
    }
}

/// RULE 1, over the rotation the create names: a token is scoped to the resource its create
/// named, so the same token presented for a create naming another rotation is a DIFFERENT
/// call — it mints its own override, under its own rotation, and neither id is reachable
/// under the other rotation.
#[tokio::test]
async fn the_same_token_naming_another_rotation_mints_its_own_override() {
    let emulator = Emulator::new();
    let case = "one token, two rotations";
    let team = ensure_contacts(&emulator, 1, case).await;
    let rotations = [
        create_rotation(&emulator, "override-token-scope-a", case).await,
        create_rotation(&emulator, "override-token-scope-b", case).await,
    ];
    // Admitted by the token's modelled pattern.
    let token = "one-token-two-rotations";

    let mut ids: Vec<String> = Vec::new();
    for rotation in &rotations {
        let id = create_override(
            &emulator,
            override_request(rotation, &team, token),
            case,
        )
        .await;
        assert_modelled_uuid(&id, case);
        ids.push(id);
    }

    assert_ne!(
        ids[0], ids[1],
        "case {case:?}: expected each rotation's create to mint its own id, got {:?} twice",
        ids[0]
    );
    assert_eq!(
        override_count(&emulator),
        2,
        "case {case:?}: expected one stored override per rotation, got {}",
        override_count(&emulator)
    );

    for (owner, id) in ids.iter().enumerate() {
        let (status, reported) = read_override(&emulator, &rotations[owner], id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected {id:?} to read 200 under the rotation it was created \
             for, got {status} reporting {reported:?}"
        );
        assert_eq!(
            reported.as_deref(),
            Some(rotations[owner].as_str()),
            "case {case:?}: expected the override to report rotation {:?}, got {reported:?}",
            rotations[owner]
        );

        // An id is unique only WITHIN its rotation, so under the other one it names nothing.
        let other = &rotations[1 - owner];
        let (status, reported) = read_override(&emulator, other, id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected {id:?} to read 404 under rotation {other:?}, got {status} \
             reporting {reported:?}"
        );
    }
}

/// RULE 1, over the operation the token is presented to: a token identifies one create of
/// one operation, so the same token handed to `CreateContact`, `CreateRotation` and
/// `CreateRotationOverride` mints a contact, a rotation and an override — each readable
/// through its own reader, and none of them a replay of another operation's resource.
#[tokio::test]
async fn the_same_token_presented_to_another_create_mints_its_own_resource() {
    let emulator = Emulator::new();
    let case = "one token, three creates";
    // Admitted by the token's modelled pattern.
    let token = "one_token_three_creates";

    let contact = emulator
        .create_contact(
            json!({
                "Alias": "token-scope",
                "Type": "PERSONAL",
                "Plan": {"Stages": []},
                "IdempotencyToken": token,
            }),
            case,
        )
        .await;

    let (status, body) = emulator
        .call(
            "CreateRotation",
            json!({
                "Name": "override-token-operations",
                "ContactIds": [contact],
                "TimeZoneId": "UTC",
                "StartTime": START_TIME,
                "Recurrence": {
                    "DailySettings": [{"HourOfDay": 9, "MinuteOfHour": 0}],
                    "NumberOfOnCalls": 1,
                    "RecurrenceMultiplier": 1,
                },
                "IdempotencyToken": token,
            }),
        )
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateRotation carrying the contact's token to answer 200, \
         got {status} {body:?}"
    );
    let rotation = body["RotationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a RotationArn, got {body:?}"))
        .to_string();
    assert_ne!(
        rotation, contact,
        "case {case:?}: expected a rotation of its own, got the contact's ARN {contact:?}"
    );

    let team = vec![contact.clone()];
    let id = create_override(
        &emulator,
        override_request(&rotation, &team, token),
        case,
    )
    .await;
    assert_modelled_uuid(&id, case);
    assert_eq!(
        override_count(&emulator),
        1,
        "case {case:?}: expected the override create to store exactly one override, got {}",
        override_count(&emulator)
    );

    // Each resource reads back through its OWN reader, so no create replayed another's.
    let (status, body) = emulator
        .call("GetContact", json!({"ContactId": contact}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetContact for {contact:?} to answer 200, got {status} {body:?}"
    );
    assert_eq!(
        body["Alias"].as_str(),
        Some("token-scope"),
        "case {case:?}: expected the created alias, got {body:?}"
    );

    let (status, body) = emulator
        .call("GetRotation", json!({"RotationId": rotation}))
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRotation for {rotation:?} to answer 200, got {status} \
         {body:?}"
    );
    assert_eq!(
        body["Name"].as_str(),
        Some("override-token-operations"),
        "case {case:?}: expected the created rotation name, got {body:?}"
    );

    let (status, reported) = read_override(&emulator, &rotation, &id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetRotationOverride for {id:?} to answer 200, got {status} \
         reporting {reported:?}"
    );
    assert_eq!(
        reported.as_deref(),
        Some(rotation.as_str()),
        "case {case:?}: expected the override to report rotation {rotation:?}, got {reported:?}"
    );
}
