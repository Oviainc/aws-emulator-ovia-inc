// SPDX-License-Identifier: CC0-1.0

//! What a `Provider` member may CARRY, driven over the wire through the shared
//! `crate::emulator` harness.
//!
//! RULE: the value a union member carries in a REQUEST is held to that member's own
//! modelled constraints by every operation that reads the union, because the boundary
//! validator holds a top-level member to its target's constraints but not a member one
//! level down; a value the member cannot hold is a fault in the request — the modelled
//! `InterconnectValidationException` (400), naming the member and the length it came in
//! with — rather than a filter compared against stored records and answered with a page
//! the caller cannot tell from "nothing matched". A value INSIDE the member's
//! constraints is filtered on as before: one that matches nothing answers an empty page,
//! and a partner a stored record names answers that record.
//!
//! The constraint is the model's, read from
//! `smithy_query(query="shape",shape="Provider")`: `cloudServiceProvider` targets
//! `CloudServiceProvider` and `lastMileProvider` targets `LastMileProvider`, and both
//! targets are `@length(max: 32)` with no floor and no `@pattern`.
//!
//! The rows differ along what the rule generalises over: the member the value arrived
//! in, how far past the published ceiling the value's LENGTH is (one character, many),
//! the alphabet it is built from (ASCII, multi-byte characters whose byte count and
//! character count differ), the operation the union arrived on, and the Region the
//! request is signed for.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::emulator::{
    available_bandwidths, code, creatable_environments, message, text, Emulator,
};

/// The `@length` ceiling the model gives the target of both `Provider` members
/// (`smithy_query(query="shape",shape="Provider")` -> both targets `@length(max: 32)`).
const PROVIDER_NAME_MAX_LENGTH: usize = 32;

/// The envelope this service puts on the refusals it words itself, as every recorded
/// Interconnect rejection message spells it.
const REFUSAL_ENVELOPE: &str = "Unable to process request: ";

/// The error the service answers a request whose input it cannot accept with.
const VALIDATION_CODE: &str = "InterconnectValidationException";

/// The two Regions the rows sign their requests for, so the rule is checked across the
/// Region the request addresses as well as across the value.
const REGIONS: [&str; 2] = ["us-east-1", "eu-west-1"];

/// The two members `Provider` offers, as the model names them on the wire.
const MEMBERS: [&str; 2] = ["cloudServiceProvider", "lastMileProvider"];

/// A multi-byte character, so a row can send a name whose BYTE count and CHARACTER count
/// differ — `@length` counts characters.
const MULTIBYTE: char = 'é';

/// One row: what makes the value impossible for its member, and the name to send.
struct Row {
    name: String,
    value: String,
}

fn row(name: &str, value: String) -> Row {
    Row {
        name: name.to_string(),
        value,
    }
}

/// How the service words the refusal of a name of `length` characters in `member`: the
/// sentence this service uses for a value whose length its member cannot hold.
fn expected_message(member: &str, length: usize) -> String {
    format!(
        "{REFUSAL_ENVELOPE}provider.{member} must be at most {PROVIDER_NAME_MAX_LENGTH} \
         characters long, and one of length {length} is not"
    )
}

/// Names no `Provider` member may carry: each one is past the member's published length
/// ceiling, and they differ in how far past it they are and in what they are built from.
fn impossible_values() -> Vec<Row> {
    vec![
        row(
            "one character past the modelled ceiling",
            "A".repeat(PROVIDER_NAME_MAX_LENGTH + 1),
        ),
        row(
            "many characters past the modelled ceiling",
            "A".repeat(PROVIDER_NAME_MAX_LENGTH * 2),
        ),
        row(
            "past the ceiling in multi-byte characters",
            MULTIBYTE
                .to_string()
                .repeat(PROVIDER_NAME_MAX_LENGTH + 1),
        ),
        row(
            "past the ceiling in a mixed alphabet",
            format!(
                "partner-{}",
                "x.y_z ".repeat(PROVIDER_NAME_MAX_LENGTH / 2)
            ),
        ),
    ]
}

/// Names every member constraint admits that still match no stored record: at the
/// published ceiling, below it, and with nothing the ceiling forbids — the member has no
/// floor and no `@pattern`, so an empty name and an unusual alphabet are both inside it.
fn well_formed_values_matching_nothing() -> Vec<Row> {
    vec![
        row("an empty name, the member having no floor", String::new()),
        row("a one-character name", "A".to_string()),
        row(
            "a name at the modelled ceiling",
            "A".repeat(PROVIDER_NAME_MAX_LENGTH),
        ),
        row(
            "a name at the ceiling in multi-byte characters, twice the ceiling in bytes",
            MULTIBYTE.to_string().repeat(PROVIDER_NAME_MAX_LENGTH),
        ),
        row(
            "a name of a partner nothing is placed with",
            "no-such-partner 42".to_string(),
        ),
    ]
}

/// The `provider` filter value for `member`, as a request carries the union.
fn provider(member: &str, value: &str) -> Value {
    json!({ member: value })
}

/// `ListConnections` and `ListEnvironments` filtered by `provider`, with the member of
/// the response each one pages, as `(operation, member)` — the two readers of this union.
const READERS: [(&str, &str); 2] = [
    ("ListConnections", "connections"),
    ("ListEnvironments", "environments"),
];

/// One reader, filtered by `provider` and signed for `region`, as a `(status, body)`
/// pair so a refusal is readable.
async fn list_filtered(
    emulator: &Emulator,
    region: &str,
    operation: &str,
    provider: &Value,
) -> (u16, Value) {
    emulator
        .ic_in(
            region,
            operation,
            json!({ "maxResults": 100, "provider": provider }),
        )
        .await
}

/// How many records a served page carries.
fn page_len(body: &Value, member: &str) -> usize {
    body[member].as_array().map(Vec::len).unwrap_or(0)
}

/// The ids of every connection this Region's endpoint holds.
async fn stored_ids(emulator: &Emulator, region: &str) -> BTreeSet<String> {
    let listed = emulator
        .ic_ok_in(region, "ListConnections", json!({ "maxResults": 100 }))
        .await;
    listed["connections"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|connection| text(connection, "id"))
        .collect()
}

// ---------------------------------------------------------------------------
// A value outside the member's own constraint
// ---------------------------------------------------------------------------

/// RULE: a name longer than its `Provider` member's modelled ceiling is refused by BOTH
/// readers of the union as invalid input — HTTP 400 with the modelled
/// `InterconnectValidationException`, naming the member and the length — and the refusal
/// is about the value, so it reports nothing about what the account holds.
#[tokio::test]
async fn a_provider_value_past_its_members_length_is_invalid_input() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;
    let creatable = creatable_environments(&catalog);
    let placement = creatable
        .first()
        .expect("case \"catalog\": expected at least one environment that takes connections")
        .clone();
    let bandwidths = available_bandwidths(&placement);
    // One stored connection, so the refusals below are not answered by an account that
    // holds nothing at all.
    let held = emulator.attach_point().await;
    let stored = emulator
        .create_connection(
            &text(&placement, "environmentId"),
            &bandwidths[0],
            &held,
            "provider filter case",
        )
        .await;
    let before = stored_ids(&emulator, REGIONS[0]).await;
    assert!(
        before.contains(&text(&stored, "id")),
        "setup: expected the created connection to be stored, got {before:?}"
    );

    for region in REGIONS {
        for member in MEMBERS {
            for Row { name, value } in &impossible_values() {
                for (operation, paged) in READERS {
                    let case = format!("{region} / {member} / {name} / {operation}");
                    let (status, body) =
                        list_filtered(&emulator, region, operation, &provider(member, value)).await;
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400 for a name the member cannot hold, got {status} with {body:?}"
                    );
                    assert!(
                        code(&body).contains(VALIDATION_CODE),
                        "case {case:?}: expected {VALIDATION_CODE}, got {:?} with {body:?}",
                        code(&body)
                    );
                    assert_eq!(
                        message(&body),
                        expected_message(member, value.chars().count()),
                        "case {case:?}: expected the too-long wording, got {:?}",
                        message(&body)
                    );
                    assert_eq!(
                        page_len(&body, paged),
                        0,
                        "case {case:?}: expected the refusal to carry no page, got {:?}",
                        body[paged]
                    );
                }
            }
        }
    }

    // Not one of those refusals changed what the account holds.
    let after = stored_ids(&emulator, REGIONS[0]).await;
    assert_eq!(
        after, before,
        "case \"after the refusals\": expected the stored connections to be untouched, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// A value inside the member's constraint
// ---------------------------------------------------------------------------

/// RULE: a name the member's constraint admits is still FILTERED on, so the 400s above
/// are about the value's own length and not a blanket refusal: a name nothing is placed
/// with answers a served, empty page, and a partner the catalog names answers exactly
/// the records placed with it.
#[tokio::test]
async fn a_well_formed_provider_filter_is_still_filtered_on() {
    let emulator = Emulator::new();
    let catalog = emulator.environments().await;

    for region in REGIONS {
        for member in MEMBERS {
            for Row { name, value } in &well_formed_values_matching_nothing() {
                for (operation, paged) in READERS {
                    let case = format!("{region} / {member} / {name} / {operation}");
                    let (status, body) =
                        list_filtered(&emulator, region, operation, &provider(member, value)).await;
                    assert_eq!(
                        status, 200,
                        "case {case:?}: expected a name inside the member's constraint to be served, got {status} with {body:?}"
                    );
                    assert_eq!(
                        page_len(&body, paged),
                        0,
                        "case {case:?}: expected no record to match a partner nothing is placed with, got {:?}",
                        body[paged]
                    );
                }
            }
        }
    }

    // Every partner the LIVE catalog names — including the ones no row above cites —
    // selects exactly the environments placed with it, so the empty pages above are
    // about the value and not about the filter being ignored.
    let partners: Vec<Value> = {
        let mut seen: Vec<Value> = Vec::new();
        for environment in &catalog {
            let partner = environment["provider"].clone();
            if !seen.contains(&partner) {
                seen.push(partner);
            }
        }
        seen
    };
    assert!(
        !partners.is_empty(),
        "case \"catalog\": expected the catalog to name a partner, got {partners:?}"
    );
    for partner in &partners {
        let expected: BTreeSet<String> = catalog
            .iter()
            .filter(|environment| &environment["provider"] == partner)
            .map(|environment| text(environment, "environmentId"))
            .collect();
        let (status, body) =
            list_filtered(&emulator, REGIONS[0], "ListEnvironments", partner).await;
        assert_eq!(
            status, 200,
            "case {partner:?}: expected the catalog filter to be served, got {status} with {body:?}"
        );
        let listed: BTreeSet<String> = body["environments"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|environment| text(environment, "environmentId"))
            .collect();
        assert_eq!(
            listed, expected,
            "case {partner:?}: expected exactly the environments of that partner, got {listed:?}"
        );
    }

    // And the connection filter selects on the partner of the environment a connection
    // was placed on.
    let held = emulator.attach_point().await;
    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let bandwidths = available_bandwidths(&environment);
        let created = emulator
            .create_connection(
                &environment_id,
                &bandwidths[0],
                &held,
                "provider filter positive control",
            )
            .await;
        let partner = environment["provider"].clone();
        let (status, body) =
            list_filtered(&emulator, REGIONS[0], "ListConnections", &partner).await;
        assert_eq!(
            status, 200,
            "case {environment_id:?}: expected the connection filter to be served, got {status} with {body:?}"
        );
        let matched: BTreeSet<String> = body["connections"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|connection| text(connection, "id"))
            .collect();
        assert!(
            matched.contains(&text(&created, "id")),
            "case {environment_id:?}: expected the connection placed with {partner:?} to match, got {matched:?}"
        );
    }
}
