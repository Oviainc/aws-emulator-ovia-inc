// SPDX-License-Identifier: CC0-1.0

//! The missing-resource rule for this service: when an operation resolves a resource by
//! an identifier the store does not hold, the `ResourceNotFoundException` it answers with
//! carries ONE fixed sentence — the same one whichever operation asked and whatever the
//! request asked for — and that sentence never contains the identifier from the request.
//!
//! The model pins only the shape of the error (`ResourceNotFoundException = structure
//! {Message: ErrorMessage}`, `errorKind client`, `httpError 404`, documented as "One of
//! the specified resources was not found."), so what is asserted here is the invariant
//! rather than one request's text: identical message across every operation and every
//! lookup key, and no echo of the key.
//!
//! Every assertion goes over the wire through the same `MultiServiceRouter` the emulator
//! boots, through the harness the lifecycle suite already defines.

use std::collections::BTreeSet;

use serde_json::json;

use crate::canary_lifecycle::{create_body, Answer, Synthetics};

/// The fixed sentence a canary lookup that misses answers with — constant, so it is
/// written here once and compared against every case.
const CANARY_NOT_FOUND: &str = "Canary not found.";

/// A canary name that exists in the store for the near-miss rows below.
const EXISTING: &str = "live-canary";

/// `ResourceNotFoundException$Message`, or a panic naming what came back instead.
fn message_of(case: &str, answer: &Answer) -> String {
    answer
        .at("Message")
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a Message on the error, got {:?}",
                answer.body
            )
        })
        .to_string()
}

/// Assert the 404/`ResourceNotFoundException` envelope and return the message.
fn not_found_message(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 404,
        "case {case}: expected 404, got {} ({:?})",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code.as_deref(),
        Some("ResourceNotFoundException"),
        "case {case}: expected ResourceNotFoundException, got {:?}",
        answer.error_code
    );
    message_of(case, answer)
}

/// Canary names no store holds, spanning what the rule generalises over: length (one
/// character up to the modelled maximum), charset (letters, digits, hyphen, underscore)
/// and resemblance to a name that does exist.
///
/// Every row is a legal `CanaryName` — `length 1..255`, `pattern ^[0-9a-z_\-]+$`, from
/// `smithy_query(query="shape", shape="CanaryName")` — so each request reaches the
/// handler's lookup instead of being refused by constraint validation.
fn missing_names() -> Vec<(&'static str, String)> {
    vec![
        ("single-character", "x".to_string()),
        ("digits-only", "404".to_string()),
        ("hyphenated", "no-such-canary".to_string()),
        ("underscored", "no_such_canary".to_string()),
        ("mixed-charset", "canary_9-probe-7".to_string()),
        // The modelled maximum length: the longest key a caller can ask about.
        ("max-length", "c".repeat(255)),
        // Resembles a canary that IS in the store, which must not change the answer.
        ("suffix-of-existing", format!("{EXISTING}-2")),
        ("prefix-of-existing", EXISTING[.."live".len()].to_string()),
    ]
}

/// Every operation that resolves a canary by name answers the same name-free sentence,
/// for every shape of missing name.
#[tokio::test]
async fn a_missing_canary_answers_one_fixed_name_free_message() {
    let aws = Synthetics::new("us-east-2");
    // A canary that DOES exist, so the near-miss rows are answered by a store that holds
    // a similar name rather than by an empty one.
    let created = aws.create(create_body(EXISTING)).await;
    assert_eq!(
        created.status, 200,
        "case create-existing: expected 200, got {} ({:?})",
        created.status, created.body
    );

    let mut messages: BTreeSet<String> = BTreeSet::new();
    for (row, name) in missing_names() {
        for (operation, answer) in [
            ("GetCanary", aws.get(&name).await),
            ("DeleteCanary", aws.delete(&name).await),
            ("StartCanary", aws.start(&name).await),
            ("StopCanary", aws.stop(&name).await),
            ("GetCanaryRuns", aws.runs(&name, json!({})).await),
        ] {
            let case = format!("{operation}/{row}");
            let message = not_found_message(&case, &answer);
            assert!(
                !message.is_empty(),
                "case {case}: expected a non-empty Message, got {message:?}"
            );
            // The lookup key must appear nowhere in the message, in any casing.
            assert!(
                !message.to_lowercase().contains(&name.to_lowercase()),
                "case {case}: expected a Message that does not name the requested canary \
                 {name:?}, got {message:?}"
            );
            assert_eq!(
                message, CANARY_NOT_FOUND,
                "case {case}: expected the fixed missing-canary sentence, got {message:?}"
            );
            messages.insert(message);
        }
    }

    // The invariant the rule really is: ONE message over every operation and every key.
    assert_eq!(
        messages.len(),
        1,
        "case request-independence: expected one fixed message across every operation and \
         name, got {messages:?}"
    );

    // The canary that does exist is untouched by all of the above: the near misses did
    // not resolve to it.
    let read = aws.get(EXISTING).await;
    assert_eq!(
        read.status, 200,
        "case existing-canary-survives: expected 200, got {} ({:?})",
        read.status, read.body
    );
    assert_eq!(
        read.string_at("Canary.Name"),
        EXISTING,
        "case existing-canary-survives: expected the stored canary, got {:?}",
        read.at("Canary")
    );
}

/// The same rule one level down: a dry run that does not exist is reported without the
/// `DryRunId` the request carried, and the sentence does not change with the id.
#[tokio::test]
async fn a_missing_dry_run_answers_one_fixed_id_free_message() {
    let aws = Synthetics::new("eu-central-1");
    let created = aws.create(create_body(EXISTING)).await;
    assert_eq!(
        created.status, 200,
        "case create-existing: expected 200, got {} ({:?})",
        created.status, created.body
    );

    // `DryRunId` is modelled as `UUID`
    // (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, from
    // `smithy_query(query="shape", shape="UUID")`), so every row is a legal id this
    // emulator never minted.
    let ids = [
        ("all-zero", "00000000-0000-0000-0000-000000000000"),
        ("all-f", "ffffffff-ffff-ffff-ffff-ffffffffffff"),
        ("mixed", "1a2b3c4d-5e6f-4a7b-8c9d-0e1f2a3b4c5d"),
    ];
    let mut messages: BTreeSet<String> = BTreeSet::new();
    for (row, id) in ids {
        let answer = aws
            .runs(
                EXISTING,
                json!({ "RunType": "DRY_RUN", "DryRunId": id }),
            )
            .await;
        let case = format!("GetCanaryRuns/{row}");
        let message = not_found_message(&case, &answer);
        assert!(
            !message.to_lowercase().contains(&id.to_lowercase()),
            "case {case}: expected a Message that does not name the requested dry run \
             {id:?}, got {message:?}"
        );
        assert!(
            !message.contains(EXISTING),
            "case {case}: expected a Message that does not name the canary, got {message:?}"
        );
        messages.insert(message);
    }
    assert_eq!(
        messages.len(),
        1,
        "case request-independence: expected one fixed message across every dry run id, \
         got {messages:?}"
    );
}
