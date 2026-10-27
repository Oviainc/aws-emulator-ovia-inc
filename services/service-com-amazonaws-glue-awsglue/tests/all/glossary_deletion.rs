// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteGlossary`, the delete of the Glue business glossary slice
//! `CreateGlossary` writes and `GetGlossary` reads.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a delete removes the ONE glossary record its `Identifier` resolves
//! to — the opaque `GlossaryId` the create minted — so a later read of that glossary misses
//! while every other glossary is untouched and the name it held is free again; it is served
//! whether or not the glossary still holds glossary TERMS, and leaves those terms
//! addressable by their own identifiers, still naming the glossary they were created in;
//! and it reports no miss, because the operation models no not-found error
//! (`smithy_query operation=DeleteGlossary` lists `AccessDeniedException`,
//! `ConcurrentModificationException`, `ConflictException`, `InternalServiceException`,
//! `InvalidInputException` and `ThrottlingException` and no `EntityNotFoundException`,
//! unlike `GetGlossary`).
//!
//! The suites are tables whose rows differ along what the rule generalises over: the form
//! and length of the glossary's name, whether it carries a description, how many terms it
//! holds and what those terms carry, and how an identifier misses.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: "len 1..256" on `GlossaryName`
/// (`smithy_query operation=CreateGlossary`). A row at this bound proves a glossary is
/// removed whatever its name is.
const MAX_NAME_LENGTH: usize = 256;

/// The longest `Description` the model accepts: "len 1..2048" on `MetadataDescription`
/// (`smithy_query shape=MetadataDescription`).
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// Create a glossary and return the `GlossaryId` the catalog minted for it.
async fn create_glossary(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateGlossary", request).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossary must report an Id, got {created}"))
        .to_string()
}

/// Create a glossary term in `glossary` and return the `GlossaryTermId` minted for it.
async fn create_term(
    router: &MultiServiceRouter,
    glossary: &str,
    name: &str,
    short_description: Option<&str>,
) -> String {
    let mut request = json!({ "GlossaryIdentifier": glossary, "Name": name });
    if let Some(description) = short_description {
        request["ShortDescription"] = json!(description);
    }
    let created = ok(router, "CreateGlossaryTerm", &request).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossaryTerm must report an Id, got {created}"))
        .to_string()
}

/// Delete the glossary `identifier` names, asserting the modelled 200 and the response's
/// modelled emptiness.
async fn delete(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(router, "DeleteGlossary", &json!({ "Identifier": identifier })).await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteGlossary of {identifier:?} must answer 200, got {status} with {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(Map::is_empty),
        "case {case}: DeleteGlossaryResponse models no members, got {body}"
    );
}

/// Assert that `identifier` resolves to no glossary: 400 `EntityNotFoundException`, the miss
/// `GetGlossary` models, worded the way this surface words it.
async fn assert_read_misses(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(router, "GetGlossary", &json!({ "Identifier": identifier })).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 reading the deleted glossary {identifier:?}, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
    );
    // The refusal's `Message` member, pinned exactly: the business glossary surface's
    // refusal text is a JSON document carrying the sentence under a `Message` key, with the
    // identifier from the request as its one variable part.
    assert_eq!(
        error_message(&body),
        format!(r#"{{"Message":"Business glossary {identifier} not found."}}"#),
        "case {case}: unexpected refusal wording for {identifier:?}, got {body}"
    );
}

/// The `Name` `GetGlossary` reports for `identifier`, which must still resolve.
async fn read_name(router: &MultiServiceRouter, identifier: &str) -> String {
    let read = ok(router, "GetGlossary", &json!({ "Identifier": identifier })).await;
    read.get("Name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("GetGlossary must report a Name, got {read}"))
        .to_string()
}

/// The term names `ListGlossaryTerms` reports for `glossary`, in the order returned.
async fn listed_term_names(router: &MultiServiceRouter, glossary: &str) -> Vec<String> {
    let page = ok(
        router,
        "ListGlossaryTerms",
        &json!({ "GlossaryIdentifier": glossary }),
    )
    .await;
    page.get("Items")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("ListGlossaryTerms must report Items, got {page}"))
        .iter()
        .map(|item| {
            item.get("Name")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("GlossaryTermItem.Name must be a string, got {item}"))
                .to_string()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// The delete removes the one glossary it names
// ---------------------------------------------------------------------------

/// A delete removes the glossary its `Identifier` resolves to and nothing else: a read of
/// the deleted glossary misses, every other stored glossary still reports its own name, the
/// store holds one record fewer per delete, and a create under the deleted name is served
/// again under a NEW identifier — so the delete freed the name rather than latching it.
#[tokio::test]
async fn deleting_a_glossary_removes_only_the_one_it_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // (case, create request) — the rows differ along the form and length of the name and
    // along whether a description was sent and how long it is.
    let rows: Vec<(&str, Value)> = vec![
        (
            // The shape a lifecycle client sends: the one required member alone.
            "the required member only",
            json!({ "Name": "CustomerConcepts" }),
        ),
        (
            "a name and a description",
            json!({
                "Name": "FinanceTerms",
                "Description": "Business concepts the finance domain owns",
            }),
        ),
        (
            "a one-character name and a one-character description",
            json!({ "Name": "g", "Description": "d" }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "Name": "n".repeat(MAX_NAME_LENGTH) }),
        ),
        (
            "a description at the modelled length bound",
            json!({
                "Name": "MaximallyDescribed",
                "Description": "d".repeat(MAX_DESCRIPTION_LENGTH),
            }),
        ),
        (
            "a name carrying spaces, punctuation and non-ASCII text",
            json!({
                "Name": "Ventas — Glosario (ES/MX)",
                "Description": "Términos de negocio\tcon tabulador",
            }),
        ),
        (
            "a glossary created under an idempotency token",
            json!({
                "Name": "TokenedGlossary",
                "ClientToken": "a-client-token",
            }),
        ),
    ];

    // A bystander, to prove the delete is scoped to the glossary it names.
    let bystander = create_glossary(&router, &json!({ "Name": "BystanderGlossary" })).await;

    // Every row is stored FIRST, so each delete runs against a store holding several
    // glossaries and has to reach exactly one of them.
    let mut stored: Vec<(&str, Value, String)> = Vec::new();
    for (case, request) in &rows {
        let id = create_glossary(&router, request).await;
        stored.push((case, request.clone(), id));
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut remaining = state.lock().glossaries.len();
    assert_eq!(
        remaining,
        rows.len() + 1,
        "case every row: the creates must have stored one record each, got {remaining}"
    );

    for (case, request, id) in &stored {
        let name = request["Name"].as_str().expect("the case carries a Name");
        // The record is there to begin with, so the miss asserted below is the delete's
        // doing.
        assert_eq!(
            &read_name(&router, id).await,
            name,
            "case {case}: the glossary must be readable before the delete"
        );

        delete(&router, case, id).await;
        assert_read_misses(&router, case, id).await;

        // Exactly one record left the store for this delete.
        remaining -= 1;
        let held = state.lock().glossaries.len();
        assert_eq!(
            held, remaining,
            "case {case}: a delete must remove exactly one record; expected {remaining} \
             glossaries, got {held}"
        );
    }

    // Nothing a delete did reached the glossary it did not name.
    assert_eq!(
        read_name(&router, &bystander).await,
        "BystanderGlossary",
        "case bystander: a delete must leave every other glossary in place"
    );

    // A create under a deleted name is served again, under its OWN identifier: the delete
    // freed the name.
    for (case, request, old_id) in &stored {
        let new_id = create_glossary(&router, request).await;
        assert_ne!(
            &new_id, old_id,
            "case {case}: a create after the delete must mint its own identifier"
        );
        assert_eq!(
            read_name(&router, &new_id).await,
            request["Name"].as_str().expect("the case carries a Name"),
            "case {case}: the re-created glossary must read back"
        );
        // The identifier the delete removed is still gone: the new record is its own.
        assert_read_misses(&router, case, old_id).await;
    }
}

// ---------------------------------------------------------------------------
// A glossary that still holds terms
// ---------------------------------------------------------------------------

/// A delete is served for a glossary that still holds glossary TERMS, and leaves those terms
/// alone: the glossary itself misses afterwards, while every term is still read back by
/// `GetGlossaryTerm` under its own identifier — reporting the `GlossaryId` its create was
/// given — and is still listed by `ListGlossaryTerms` for that identifier.
///
/// The rows differ along how many terms the glossary holds, whether those terms carry a
/// short description, and whether another glossary holds terms of its own that the delete
/// must not touch.
#[tokio::test]
async fn deleting_a_glossary_that_still_holds_terms_is_served() {
    let router = emulator();

    /// One glossary and the terms it holds when its delete is made.
    struct Row {
        case: &'static str,
        glossary: &'static str,
        /// The term names this glossary holds, in the order they are created.
        terms: &'static [&'static str],
        /// Whether each term carries a short description.
        described: bool,
    }

    let rows = vec![
        Row {
            case: "the one term of a single-term glossary",
            glossary: "SoloGlossary",
            terms: &["Revenue"],
            described: false,
        },
        Row {
            case: "two terms, each described",
            glossary: "PairGlossary",
            terms: &["Churn", "Arpu"],
            described: true,
        },
        Row {
            case: "five terms",
            glossary: "ManyGlossary",
            terms: &["Alpha", "Beta", "Gamma", "Delta", "Epsilon"],
            described: false,
        },
    ];

    // A glossary with terms of its own, which no row's delete may reach.
    let bystander = create_glossary(&router, &json!({ "Name": "TermedBystander" })).await;
    let bystander_term = create_term(&router, &bystander, "UntouchedTerm", None).await;

    for row in &rows {
        let glossary = create_glossary(&router, &json!({ "Name": row.glossary })).await;
        let mut term_ids: Vec<(&str, String)> = Vec::new();
        for name in row.terms {
            let description = if row.described {
                Some(format!("the business concept {name}"))
            } else {
                None
            };
            let id = create_term(&router, &glossary, name, description.as_deref()).await;
            term_ids.push((name, id));
        }

        let listed_before = listed_term_names(&router, &glossary).await;
        assert_eq!(
            listed_before.len(),
            row.terms.len(),
            "case {}: the glossary must hold its terms before the delete, got \
             {listed_before:?}",
            row.case
        );

        delete(&router, row.case, &glossary).await;

        // The glossary is gone...
        assert_read_misses(&router, row.case, &glossary).await;

        // ...and every term it held is still its own record, still naming the glossary its
        // create was given.
        for (name, id) in &term_ids {
            let read = ok(&router, "GetGlossaryTerm", &json!({ "Identifier": id })).await;
            assert_eq!(
                (
                    read.get("Name").and_then(Value::as_str),
                    read.get("GlossaryId").and_then(Value::as_str)
                ),
                (Some(*name), Some(glossary.as_str())),
                "case {}: the term {name} must survive its glossary's delete, got {read}",
                row.case
            );
        }
        let listed_after = listed_term_names(&router, &glossary).await;
        assert_eq!(
            listed_after, listed_before,
            "case {}: the terms of a deleted glossary must still be listed; expected \
             {listed_before:?}, got {listed_after:?}",
            row.case
        );
    }

    // The glossary no row named, and its term, are exactly as their own writes left them.
    assert_eq!(
        read_name(&router, &bystander).await,
        "TermedBystander",
        "case the bystander glossary: a delete must leave every other glossary in place"
    );
    let read = ok(
        &router,
        "GetGlossaryTerm",
        &json!({ "Identifier": bystander_term }),
    )
    .await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("UntouchedTerm"),
        "case the bystander term: a delete must leave every other term in place, got {read}"
    );
}

// ---------------------------------------------------------------------------
// An identifier that resolves to nothing
// ---------------------------------------------------------------------------

/// A delete for an identifier this account holds no glossary under is SERVED: this operation
/// models no not-found error, so the caller that asks for a state the store is already in is
/// answered as though the delete had happened, and nothing is removed.
///
/// The rows differ along how the identifier misses: never minted, a prefix of a stored
/// identifier, a stored identifier with one more character, a stored identifier recased, a
/// glossary that was already deleted, and an identifier no mint could produce.
#[tokio::test]
async fn deleting_a_glossary_the_store_does_not_hold_is_served() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let kept = create_glossary(&router, &json!({ "Name": "KeptGlossary" })).await;
    // A glossary created and then deleted, so one row names an address that WAS live.
    let gone = create_glossary(&router, &json!({ "Name": "GoneGlossary" })).await;
    delete(&router, "the glossary deleted up front", &gone).await;

    let truncated = kept[..kept.len() - 1].to_string();
    let extended = format!("{kept}0");
    let recased = kept.to_uppercase();
    let rows: Vec<(&str, String)> = [
        ("never minted", "no-such-glossary".to_string()),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", extended),
        ("a stored identifier recased", recased),
        ("an already deleted glossary", gone.clone()),
        (
            "an identifier no mint could produce",
            "GLOSSARY/NOT/AN/ID".to_string(),
        ),
    ]
    .into_iter()
    // A minted identifier is lowercase hexadecimal, so one drawn with no letter at all
    // recases to itself; such a row would name the stored record, so it is dropped rather
    // than asserted.
    .filter(|(_, identifier)| identifier != &kept)
    .collect();
    assert!(
        rows.len() >= 5,
        "case the miss table: expected at least five identifiers that miss, got {rows:?}"
    );

    for (case, identifier) in &rows {
        delete(&router, case, identifier).await;
        // Repeating it answers the same way: the delete of something absent is not a state
        // change that a second call could fail on.
        delete(&router, case, identifier).await;
    }

    // Nothing was removed by any of those calls: the glossary the store does hold is still
    // readable, and it is the only record left.
    assert_eq!(
        read_name(&router, &kept).await,
        "KeptGlossary",
        "case the stored glossary: a delete that resolved to nothing must remove nothing"
    );
    let held = engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .glossaries
        .len();
    assert_eq!(
        held, 1,
        "case every miss row: the store must still hold exactly the one glossary, got {held}"
    );
}
