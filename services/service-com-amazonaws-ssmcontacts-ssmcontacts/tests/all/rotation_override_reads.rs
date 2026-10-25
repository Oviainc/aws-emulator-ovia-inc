// SPDX-License-Identifier: CC0-1.0

//! `GetRotationOverride` as a REPORTING operation addressed by a pair.
//!
//! Every case drives the crate's ONE integration-test harness (`contact_lifecycle`'s
//! `Emulator`) with a buffered awsJson1_1 request (`POST /`,
//! `X-Amz-Target: SSMContacts.<Operation>`), so what is asserted is what a signing AWS
//! client would see — routing, deserialization, constraint validation, the handler and
//! serialization included.
//!
//! `tests/all/rotation_overrides.rs` is the create/read/delete lifecycle suite; this one
//! pins the two rules of the READ that the lifecycle suite does not, each as a TABLE:
//!
//! 1. THE SELECTOR. The `(RotationId, RotationOverrideId)` pair is the override's whole
//!    identity, so a read answers the record THAT pair names whatever else the store holds —
//!    other overrides of the same rotation, and overrides of other rotations, created
//!    before and after it. The rows differ along what that generalises over: the size of the
//!    replacement team, the form of the window, and the override's position among its
//!    rotation's own overrides.
//! 2. THE REFUSAL. When the pair names no stored override the read answers HTTP 404
//!    `ResourceNotFoundException`, whose three required members
//!    (`smithy_query(query="shape",shape="ResourceNotFoundException")` — `Message`,
//!    `ResourceId` and `ResourceType`, all `required`) are populated: `ResourceId` is the
//!    `RotationOverrideId` the request named, and `ResourceType` is the one type an override
//!    is, the same one `DeleteRotationOverride` reports for the same pair. The rows are the
//!    ways a pair can name nothing — an id never minted, an id minted under another
//!    rotation, a rotation the store does not hold, an override that was deleted, and one
//!    whose rotation was deleted.
//!
//! And, across both: a read writes NOTHING, so the override store is byte-identical before
//! and after a batch of reads, the answered ones and the refused ones alike.

use serde_json::{json, Value};

use service_com_amazonaws_ssmcontacts_ssmcontacts::state;

use crate::contact_lifecycle::{error_code, Emulator};
use crate::rotation_override_listing::absent_rotation_arn;
use crate::rotation_overrides::{create_override, create_rotation, ensure_contacts};

/// A `RotationOverrideId` this service never minted. Inside the modelled `Uuid`
/// (`length` 36..39, the 8-4-4-4-12 pattern), so it reaches the handler rather than being
/// refused on the constraint.
const NEVER_MINTED_ID: &str = "ffffffff-ffff-ffff-ffff-ffffffffffff";

/// The override store, rendered so a read can be shown to have left it untouched.
fn override_store(emulator: &Emulator) -> String {
    let store =
        state::service_state(&*emulator.engine).expect("the engine holds this service's state");
    let store = store.lock();
    format!("{:?}", store.rotation_overrides)
}

/// Read one override and answer the HTTP status and the parsed body.
async fn read(emulator: &Emulator, rotation: &str, override_id: &str) -> (u16, Value) {
    emulator
        .call(
            "GetRotationOverride",
            json!({"RotationId": rotation, "RotationOverrideId": override_id}),
        )
        .await
}

/// One row of the selector table: the replacement team's size and the override's window.
struct Row {
    case: &'static str,
    /// How many contacts replace the rotation's own team for the window.
    team: usize,
    /// `StartTime`, in epoch seconds as the wire carries a `DateTime`.
    start: f64,
    /// `EndTime`, likewise.
    end: f64,
    /// Which of the two rotations this override belongs to, so the rows interleave the two
    /// and a read has to select by ARN as well as by id.
    second_rotation: bool,
}

/// The rows. `CreateRotationOverride$NewContactIds` is modelled `length 0..30` and the list
/// the read reports, `RotationOverride$NewContactIds`, `length 0..25`
/// (`smithy_query(query="operation",operation="CreateRotationOverride,GetRotationOverride")`),
/// so 0 and 25 are the sizes both ends admit and both are rows here.
const ROWS: &[Row] = &[
    Row {
        case: "an hour-long window for one replacement",
        team: 1,
        start: 1_735_689_600.0,
        end: 1_735_693_200.0,
        second_rotation: false,
    },
    Row {
        case: "an empty replacement team — the list's modelled minimum",
        team: 0,
        start: 1_700_000_000.0,
        end: 1_700_003_600.0,
        second_rotation: true,
    },
    Row {
        case: "a week-long window for four replacements",
        team: 4,
        start: 1_767_225_600.0,
        end: 1_767_830_400.0,
        second_rotation: false,
    },
    Row {
        case: "the largest replacement team the read's own list admits",
        team: 25,
        start: 1_600_000_000.0,
        end: 1_600_000_060.0,
        second_rotation: true,
    },
    Row {
        case: "a window that ends before the epoch",
        team: 2,
        start: -86_400.0,
        end: -3_600.0,
        second_rotation: false,
    },
    Row {
        case: "a sub-second window",
        team: 3,
        start: 1_700_000_000.5,
        end: 1_700_000_001.25,
        second_rotation: true,
    },
];

/// THE SELECTOR: with every row's override in the store at once, each pair reads back its
/// OWN record — and a second read of it, taken after every other row has been read, answers
/// the same thing.
#[tokio::test]
async fn a_read_answers_the_override_its_pair_names() {
    let emulator = Emulator::new();
    let first_rotation = create_rotation(&emulator, "override-read-first", "the selector").await;
    let second_rotation = create_rotation(&emulator, "override-read-second", "the selector").await;

    // Create every row's override first, so no read runs against a store holding only its
    // own subject: selecting the right one out of many is the rule.
    let mut created: Vec<(&Row, String, String, Value)> = Vec::new();
    for row in ROWS {
        let rotation = if row.second_rotation {
            second_rotation.clone()
        } else {
            first_rotation.clone()
        };
        let team = ensure_contacts(&emulator, row.team, row.case).await;
        let request = json!({
            "RotationId": rotation,
            "NewContactIds": team,
            "StartTime": row.start,
            "EndTime": row.end,
        });
        let id = create_override(&emulator, request.clone(), row.case).await;
        created.push((row, rotation, id, request));
    }

    let before = override_store(&emulator);
    let mut first_answers: Vec<Value> = Vec::new();

    for (row, rotation, id, request) in &created {
        let case = row.case;
        let (status, body) = read(&emulator, rotation, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetRotationOverride to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            body["RotationOverrideId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the id the pair named, got {body:?}"
        );
        assert_eq!(
            body["RotationArn"].as_str(),
            Some(rotation.as_str()),
            "case {case:?}: expected the overridden rotation {rotation:?}, got {body:?}"
        );
        assert_eq!(
            body["NewContactIds"], request["NewContactIds"],
            "case {case:?}: expected this override's own replacement team in the created \
             order, got {body:?}"
        );
        assert_eq!(
            body["StartTime"].as_f64(),
            Some(row.start),
            "case {case:?}: expected StartTime {}, got {body:?}",
            row.start
        );
        assert_eq!(
            body["EndTime"].as_f64(),
            Some(row.end),
            "case {case:?}: expected EndTime {}, got {body:?}",
            row.end
        );
        first_answers.push(body);
    }

    // Every pair again, now that each one has been read once: a reporting operation has no
    // first-read-only answer, so the second pass is identical to the first.
    for ((row, rotation, id, _), expected) in created.iter().zip(&first_answers) {
        let case = row.case;
        let (status, body) = read(&emulator, rotation, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected a second read to answer 200, got {status} {body:?}"
        );
        assert_eq!(
            &body, expected,
            "case {case:?}: expected a second read to answer {expected:?}, got {body:?}"
        );
    }

    assert_eq!(
        override_store(&emulator),
        before,
        "case \"the selector\": expected the reads to have left the override store \
         untouched, got {}",
        override_store(&emulator)
    );
}

/// THE REFUSAL: every way a pair can name nothing is answered 404
/// `ResourceNotFoundException` with its three required members populated, `ResourceId`
/// carrying the `RotationOverrideId` the request named — and the refused reads change
/// nothing.
#[tokio::test]
async fn a_pair_that_names_nothing_is_refused_404_naming_the_override() {
    let emulator = Emulator::new();
    let rotation = create_rotation(&emulator, "override-miss", "the refusal").await;
    let other_rotation = create_rotation(&emulator, "override-miss-other", "the refusal").await;
    let doomed_rotation = create_rotation(&emulator, "override-miss-doomed", "the refusal").await;
    let team = ensure_contacts(&emulator, 1, "the refusal").await;

    let window = |rotation: &str| {
        json!({
            "RotationId": rotation,
            "NewContactIds": team,
            "StartTime": 1_735_689_600,
            "EndTime": 1_735_693_200,
        })
    };

    // One override that stays put, one that is deleted, and one whose rotation is deleted:
    // the three ways a once-valid pair stops naming anything.
    let live = create_override(&emulator, window(rotation.as_str()), "the refusal").await;
    let deleted = create_override(&emulator, window(rotation.as_str()), "the refusal").await;
    let orphaned =
        create_override(&emulator, window(doomed_rotation.as_str()), "the refusal").await;

    let (status, body) = emulator
        .call(
            "DeleteRotationOverride",
            json!({"RotationId": rotation, "RotationOverrideId": deleted}),
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"the refusal\": expected the setup delete to answer 200, got {status} {body:?}"
    );
    let (status, body) = emulator
        .call("DeleteRotation", json!({"RotationId": doomed_rotation}))
        .await;
    assert_eq!(
        status, 200,
        "case \"the refusal\": expected the setup DeleteRotation to answer 200, got {status} \
         {body:?}"
    );

    let cases: Vec<(&str, String, String)> = vec![
        (
            "an id this service never minted",
            rotation.clone(),
            NEVER_MINTED_ID.to_string(),
        ),
        (
            "an id minted under another rotation the store holds",
            other_rotation.clone(),
            live.clone(),
        ),
        (
            "a live id named under a rotation the store does not hold",
            absent_rotation_arn(),
            live.clone(),
        ),
        (
            "an id this service never minted, under a rotation it does not hold either",
            absent_rotation_arn(),
            NEVER_MINTED_ID.to_string(),
        ),
        (
            "the id of an override that was deleted",
            rotation.clone(),
            deleted,
        ),
        (
            "the id of an override whose rotation was deleted",
            doomed_rotation.clone(),
            orphaned,
        ),
    ];

    let before = override_store(&emulator);
    // `ResourceType` is the type an override IS, so it is one value across every row; the
    // delete of the same pair reports that same value, which is what fixes it rather than
    // each handler inventing its own.
    let mut resource_types: Vec<String> = Vec::new();

    for (case, named_rotation, named_id) in &cases {
        let (status, body) = read(&emulator, named_rotation, named_id).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected GetRotationOverride to answer 404, got {status} {body:?}"
        );
        assert!(
            error_code(&body).contains("ResourceNotFoundException"),
            "case {case:?}: expected a ResourceNotFoundException, got {body:?}"
        );
        assert!(
            body["Message"].as_str().is_some_and(|text| !text.is_empty()),
            "case {case:?}: expected a populated Message, got {body:?}"
        );
        assert_eq!(
            body["ResourceId"].as_str(),
            Some(named_id.as_str()),
            "case {case:?}: expected ResourceId to carry the RotationOverrideId the request \
             named, got {body:?}"
        );
        let resource_type = body["ResourceType"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected a ResourceType, got {body:?}"));
        assert!(
            !resource_type.is_empty(),
            "case {case:?}: expected a populated ResourceType, got {body:?}"
        );

        // The delete of the same pair is refused the same way, so the two handlers agree on
        // what kind of resource the pair failed to name.
        let (delete_status, delete_body) = emulator
            .call(
                "DeleteRotationOverride",
                json!({"RotationId": named_rotation, "RotationOverrideId": named_id}),
            )
            .await;
        assert_eq!(
            delete_status, 404,
            "case {case:?}: expected DeleteRotationOverride to answer 404 too, got \
             {delete_status} {delete_body:?}"
        );
        assert_eq!(
            delete_body["ResourceType"].as_str(),
            Some(resource_type),
            "case {case:?}: expected the delete to report the read's ResourceType \
             {resource_type:?}, got {delete_body:?}"
        );
        resource_types.push(resource_type.to_string());
    }

    assert!(
        resource_types.windows(2).all(|pair| pair[0] == pair[1]),
        "case \"the refusal\": expected one ResourceType across every row, got \
         {resource_types:?}"
    );
    assert_eq!(
        override_store(&emulator),
        before,
        "case \"the refusal\": expected the refused calls to change nothing, got {}",
        override_store(&emulator)
    );

    // The override the rows kept naming is still readable under its own rotation.
    let (status, body) = read(&emulator, &rotation, &live).await;
    assert_eq!(
        status, 200,
        "case \"the refusal\": expected the live override still readable, got {status} {body:?}"
    );
}
