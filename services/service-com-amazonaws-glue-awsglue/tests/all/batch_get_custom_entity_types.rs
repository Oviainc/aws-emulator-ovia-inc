// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `BatchGetCustomEntityTypes`.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: `BatchGetCustomEntityTypes` answers 200 and reports, for EACH
//! name in `Names` and in the order the request listed them, either the custom pattern
//! stored under that name — the same projection `GetCustomEntityType` reports — or that
//! name in `CustomEntityTypesNotFound`; a name no pattern is stored under is a miss and
//! not an error, and the call changes nothing a later caller sees. The tables below span
//! the batch along what the rule generalises over: how many names are asked for, how many
//! of them are stored, where the misses sit in the order, whether a name repeats, and the
//! member set the stored pattern carries.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateCustomEntityTypeInput;
use service_com_amazonaws_glue_awsglue::state::{ComAmazonawsGlueAwsglueState, CustomEntityTypeEntry};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, ok};

/// The largest `Names` list the model accepts, and the same bound on the response's
/// `CustomEntityTypesNotFound`.
///
/// Source: `smithy_query(query="operation",operation="BatchGetCustomEntityTypes")`, whose
/// `Names` member targets `CustomEntityTypeNames` with `length 1..50`.
const MAX_NAMES: usize = 50;

/// The longest `Name` the model accepts: `length 1..255` on `NameString`
/// (`smithy_query(query="operation",operation="CreateCustomEntityType")`).
const MAX_NAME_LENGTH: usize = 255;

/// A minimal `CreateCustomEntityType` body: the two required members, with an expression
/// that distinguishes each stored pattern from its siblings.
fn pattern_body(name: &str) -> Value {
    json!({
        "Name": name,
        "RegexString": format!("{name}-[0-9]+"),
    })
}

/// One `BatchGetCustomEntityTypes` call that must succeed, as
/// `(the reported patterns' names, CustomEntityTypesNotFound)`.
async fn batch_get(router: &MultiServiceRouter, names: &[&str]) -> (Vec<String>, Vec<String>) {
    let body = ok(router, "BatchGetCustomEntityTypes", &json!({ "Names": names })).await;
    (reported_names(&body), missing_names(&body))
}

/// The `Name` of every element of the response's `CustomEntityTypes` list, in order.
fn reported_names(body: &Value) -> Vec<String> {
    body.get("CustomEntityTypes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("BatchGetCustomEntityTypes must report a CustomEntityTypes list, got {body}")
        })
        .iter()
        .map(|entry| {
            entry
                .get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("every reported CustomEntityType carries a Name, got {entry}")
                })
                .to_string()
        })
        .collect()
}

/// The names in `CustomEntityTypesNotFound`.
///
/// That member targets the same `length 1..50` list as the request's `Names`, so an empty
/// list is not a value of its shape: "nothing missed" is the member being ABSENT, which
/// this reads as the empty vector.
fn missing_names(body: &Value) -> Vec<String> {
    let Some(missing) = body.get("CustomEntityTypesNotFound") else {
        return Vec::new();
    };
    let missing = missing.as_array().unwrap_or_else(|| {
        panic!("CustomEntityTypesNotFound is a list when present, got {body}")
    });
    assert!(
        !missing.is_empty(),
        "CustomEntityTypesNotFound is bounded length 1..50, so it is absent rather than \
         empty, got {body}"
    );
    missing
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("CustomEntityTypesNotFound holds strings, got {name}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The batch splits the requested names across the two lists
// ---------------------------------------------------------------------------

/// One row of the batch table: the case name, the names the request asks for, the names
/// the response must report in `CustomEntityTypes`, and the names it must report in
/// `CustomEntityTypesNotFound`.
type BatchCase<'a> = (&'a str, Vec<&'a str>, Vec<&'a str>, Vec<&'a str>);

/// Each requested name is reported in exactly one of the two lists, in the order asked.
///
/// The rows differ along the batch's size, how many of its names are stored, where the
/// misses sit (first, last, interleaved), and whether a name repeats. The recorded
/// reference case — one batch naming the pattern a lifecycle client just created — is the
/// "one stored name" row.
#[tokio::test]
async fn every_requested_name_is_reported_exactly_once() {
    let router = emulator();
    for stored in ["employee-id", "patient-record", "account-number"] {
        ok(&router, "CreateCustomEntityType", &pattern_body(stored)).await;
    }

    // (case, requested names, expected CustomEntityTypes names, expected misses)
    let cases: Vec<BatchCase> = vec![
        // The recorded case: a batch naming the single pattern under test.
        (
            "one stored name",
            vec!["employee-id"],
            vec!["employee-id"],
            vec![],
        ),
        (
            "every name stored",
            vec!["employee-id", "patient-record", "account-number"],
            vec!["employee-id", "patient-record", "account-number"],
            vec![],
        ),
        (
            "no name stored",
            vec!["absent", "missing"],
            vec![],
            vec!["absent", "missing"],
        ),
        (
            "the miss comes first",
            vec!["absent", "patient-record"],
            vec!["patient-record"],
            vec!["absent"],
        ),
        (
            "the miss comes last",
            vec!["patient-record", "absent"],
            vec!["patient-record"],
            vec!["absent"],
        ),
        (
            "misses interleaved with hits",
            vec![
                "absent",
                "employee-id",
                "missing",
                "account-number",
                "nope",
            ],
            vec!["employee-id", "account-number"],
            vec!["absent", "missing", "nope"],
        ),
        (
            // A pattern name carries no documented case fold, so a different casing is a
            // different pattern — and so a miss.
            "a stored name in another casing",
            vec!["EMPLOYEE-ID", "employee-id"],
            vec!["employee-id"],
            vec!["EMPLOYEE-ID"],
        ),
        (
            // One answer per requested name, not per distinct name.
            "a repeated stored name",
            vec!["patient-record", "patient-record"],
            vec!["patient-record", "patient-record"],
            vec![],
        ),
        (
            "a repeated unknown name",
            vec!["absent", "absent"],
            vec![],
            vec!["absent", "absent"],
        ),
    ];

    for (case, requested, expected_found, expected_missing) in cases {
        let body = ok(
            &router,
            "BatchGetCustomEntityTypes",
            &json!({ "Names": requested }),
        )
        .await;

        let reported = reported_names(&body);
        assert_eq!(
            reported, expected_found,
            "case {case:?}: expected CustomEntityTypes {expected_found:?}, got {reported:?} in \
             {body}"
        );
        let missing = missing_names(&body);
        assert_eq!(
            missing, expected_missing,
            "case {case:?}: expected CustomEntityTypesNotFound {expected_missing:?}, got \
             {missing:?} in {body}"
        );
        // The two lists together account for every requested name, once each.
        assert_eq!(
            reported.len() + missing.len(),
            requested.len(),
            "case {case:?}: every requested name must be reported exactly once, got {body}"
        );

        // A read does not change what the next caller sees: the same batch answers the
        // same thing, and each stored name is still readable through GetCustomEntityType.
        let again = ok(
            &router,
            "BatchGetCustomEntityTypes",
            &json!({ "Names": requested }),
        )
        .await;
        assert_eq!(
            again, body,
            "case {case:?}: BatchGetCustomEntityTypes must be repeatable, got {again}"
        );
        for stored in ["employee-id", "patient-record", "account-number"] {
            let read = ok(
                &router,
                "GetCustomEntityType",
                &json!({ "Name": stored }),
            )
            .await;
            assert_eq!(
                read.get("Name"),
                Some(&json!(stored)),
                "case {case:?}: the batch read must not have touched {stored:?}, got {read}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// What it reports is what the store holds
// ---------------------------------------------------------------------------

/// A pattern the batch reports is member for member the definition `GetCustomEntityType`
/// reports for the same name — the batch is a different addressing mode over one stored
/// resource, not a second projection of it.
///
/// The rows differ along the member set the create carried: a bare pattern, one context
/// word, the twenty context words the model allows, and a create that also sent tags.
#[tokio::test]
async fn a_batched_pattern_is_the_pattern_get_custom_entity_type_reports() {
    let router = emulator();

    // 20 is the modelled maximum length of `CreateCustomEntityTypeRequest$ContextWords`
    // (`smithy_query(query="operation",operation="CreateCustomEntityType")`).
    let context_words: Vec<String> = (0..20).map(|index| format!("word-{index}")).collect();

    let creates: Vec<(&str, Value)> = vec![
        ("required members only", pattern_body("bare")),
        (
            "one context word",
            json!({
                "Name": "one-word",
                "RegexString": "PAT-[0-9]+",
                "ContextWords": ["patient"],
            }),
        ),
        (
            "context words at the modelled maximum",
            json!({
                "Name": "twenty-words",
                "RegexString": "SECRET-[A-Z]+",
                "ContextWords": context_words,
            }),
        ),
        (
            "tags sent with the create",
            json!({
                "Name": "tagged",
                "RegexString": "TAG-[0-9]{4}",
                "ContextWords": ["tagged"],
                "Tags": { "owner": "data-governance" },
            }),
        ),
        (
            "an expression carrying escapes and a tab",
            json!({
                "Name": "escaped",
                "RegexString": "^\\s*(?:[A-Z]{2}\\d{6})\t+$",
            }),
        ),
    ];

    let mut names = Vec::new();
    for (case, request) in &creates {
        ok(&router, "CreateCustomEntityType", request).await;
        let name = request
            .get("Name")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: every row names its pattern"));
        names.push(name.to_string());
    }

    let requested: Vec<&str> = names.iter().map(String::as_str).collect();
    let body = ok(
        &router,
        "BatchGetCustomEntityTypes",
        &json!({ "Names": requested }),
    )
    .await;
    let reported = body
        .get("CustomEntityTypes")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("the batch must report a CustomEntityTypes list, got {body}"))
        .clone();

    for ((case, _), name) in creates.iter().zip(names.iter()) {
        let single = ok(&router, "GetCustomEntityType", &json!({ "Name": name })).await;
        let actual = reported
            .iter()
            .find(|entry| entry.get("Name") == Some(&json!(name)))
            .unwrap_or_else(|| panic!("case {case:?}: the batch must report {name:?}, got {body}"));
        for member in ["Name", "RegexString", "ContextWords"] {
            assert_eq!(
                actual.get(member),
                single.get(member),
                "case {case:?}: the batched {member} must be what GetCustomEntityType reports, \
                 got {actual} against {single}"
            );
        }
        // `Tags` is not a member of the `CustomEntityType` shape, so a batched pattern
        // carries none even when the create sent them.
        assert!(
            actual.get("Tags").is_none(),
            "case {case:?}: Tags is not a member of CustomEntityType, got {actual}"
        );
    }
}

// ---------------------------------------------------------------------------
// The modelled bounds are refused, not reported as a miss
// ---------------------------------------------------------------------------

/// A name outside the modelled `length 1..255`, a batch outside the modelled `Names`
/// length of `1..50`, and a request that omits the `@required` member are refused at the
/// request boundary rather than reported as misses — none of them addresses a pattern, so
/// each is a bad request.
///
/// Source for both bounds: `smithy_query(query="operation",operation=
/// "BatchGetCustomEntityTypes")` — `Names` targets `CustomEntityTypeNames`, `length
/// 1..50`, whose member targets `NameString`, `length 1..255`. Each published bound is
/// exercised, plus one past it.
#[tokio::test]
async fn names_outside_the_modeled_bounds_are_refused() {
    let router = emulator();
    ok(&router, "CreateCustomEntityType", &pattern_body("stored")).await;

    // At each published element bound the name is a legal address — so it is simply
    // unknown, and reported as a miss rather than refused.
    for length in [1usize, MAX_NAME_LENGTH] {
        let name = "n".repeat(length);
        let (found, missing) = batch_get(&router, &[name.as_str()]).await;
        assert_eq!(
            missing,
            vec![name.clone()],
            "case name of {length} characters: a legal but unknown name is a miss, got \
             {missing:?}"
        );
        assert!(
            found.is_empty(),
            "case name of {length} characters: no pattern is stored under it, got {found:?}"
        );
    }

    // One past each bound is refused: an element outside its length, a batch of one name
    // too many, and a batch with nothing in it (the modelled minimum is 1).
    let over: Vec<String> = (0..=MAX_NAMES)
        .map(|index| format!("absent-{index}"))
        .collect();
    for (case, names) in [
        ("an empty name", json!([""])),
        (
            "a name one character past the bound",
            json!(["n".repeat(MAX_NAME_LENGTH + 1)]),
        ),
        (
            "a legal name beside an over-long one",
            json!(["stored", "n".repeat(MAX_NAME_LENGTH + 1)]),
        ),
        ("one name too many", json!(over)),
        ("an empty batch", json!([])),
    ] {
        let (status, answer) =
            call(&router, "BatchGetCustomEntityTypes", &json!({ "Names": names })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} with {answer}"
        );
        assert!(
            !error_code(&answer).is_empty(),
            "case {case:?}: the refusal must name an error, got {answer}"
        );
    }

    // `Names` is `@required`, so a batch that names it not at all is refused too.
    let (status, answer) = call(&router, "BatchGetCustomEntityTypes", &json!({})).await;
    assert_eq!(
        status, 400,
        "case \"no Names member\": expected 400, got {status} with {answer}"
    );
    assert!(
        !error_code(&answer).is_empty(),
        "case \"no Names member\": the refusal must name an error, got {answer}"
    );
}

/// A batch AT the modelled ceiling is served in full, whatever its names are: the bound is
/// a bound, not a smaller page.
///
/// The rows differ along the batch's size (one, a middling batch, the ceiling exactly) and
/// along what the names are: distinct stored patterns, one stored name repeated, and
/// unknown names.
#[tokio::test]
async fn a_batch_at_the_modeled_ceiling_is_served_in_full() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Enough distinct patterns to fill a batch at the ceiling. Seeded directly: filling
    // the store over the wire would exercise the create path, which has its own suite.
    let stored: Vec<String> = (0..MAX_NAMES)
        .map(|index| format!("batched-{index:03}"))
        .collect();
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for name in &stored {
            let input = CreateCustomEntityTypeInput {
                name: name.clone(),
                regex_string: "[0-9]+".to_string(),
                ..CreateCustomEntityTypeInput::default()
            };
            store
                .custom_entity_types
                .insert(name.clone(), CustomEntityTypeEntry::from_create_input(&input));
        }
    }

    for size in [1usize, 17, MAX_NAMES] {
        let distinct: Vec<&str> = stored.iter().take(size).map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &distinct).await;
        assert_eq!(
            found, distinct,
            "case {size} distinct stored names: expected them all reported, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} distinct stored names: none may miss, got {missing:?}"
        );

        let repeated: Vec<&str> = vec!["batched-000"; size];
        let (found, missing) = batch_get(&router, &repeated).await;
        assert_eq!(
            found.len(),
            size,
            "case {size} copies of one stored name: one entry per requested name, got {found:?}"
        );
        assert!(
            missing.is_empty(),
            "case {size} copies of one stored name: none may miss, got {missing:?}"
        );

        let unknown: Vec<String> = (0..size).map(|index| format!("absent-{index}")).collect();
        let unknown: Vec<&str> = unknown.iter().map(String::as_str).collect();
        let (found, missing) = batch_get(&router, &unknown).await;
        assert!(
            found.is_empty(),
            "case {size} unknown names: nothing may be reported, got {found:?}"
        );
        assert_eq!(
            missing, unknown,
            "case {size} unknown names: all of them miss, got {missing:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A pattern that leaves the store becomes a miss
// ---------------------------------------------------------------------------

/// The batch reports what the store holds AT THE TIME OF THE CALL: a name moves from
/// `CustomEntityTypes` to `CustomEntityTypesNotFound` when its pattern leaves the store,
/// and back when it is created again.
///
/// The pattern leaves the store directly rather than through `DeleteCustomEntityType`,
/// which is still a scaffold in this crate; what is under test is that the batch reads the
/// store on every call rather than caching its first answer.
#[tokio::test]
async fn the_batch_follows_the_store() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    ok(
        &router,
        "CreateCustomEntityType",
        &pattern_body("transient"),
    )
    .await;

    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert_eq!(
        found,
        vec!["transient".to_string()],
        "case \"created\": a stored pattern is reported, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "case \"created\": a stored pattern is not missing, got {missing:?}"
    );

    engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .custom_entity_types
        .remove("transient");
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert!(
        found.is_empty(),
        "case \"removed\": a pattern no longer stored is not reported, got {found:?}"
    );
    assert_eq!(
        missing,
        vec!["transient".to_string()],
        "case \"removed\": a pattern no longer stored is reported missing, got {missing:?}"
    );

    ok(
        &router,
        "CreateCustomEntityType",
        &pattern_body("transient"),
    )
    .await;
    let (found, missing) = batch_get(&router, &["transient"]).await;
    assert_eq!(
        found,
        vec!["transient".to_string()],
        "case \"re-created\": a re-created pattern is reported, got {found:?}"
    );
    assert!(
        missing.is_empty(),
        "case \"re-created\": a re-created pattern is not missing, got {missing:?}"
    );
}
