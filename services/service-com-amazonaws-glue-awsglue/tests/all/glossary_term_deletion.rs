// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteGlossaryTerm`, the delete of the one business glossary term
//! record `CreateGlossaryTerm` writes and `GetGlossaryTerm` reads.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a delete removes the ONE glossary term record its `Identifier`
//! resolves to — the opaque `GlossaryTermId` the create minted — so a later read of that
//! identifier misses and the name it held is free again inside its glossary, while every
//! other term, the glossary the term belonged to, and the records of any other glossary are
//! left exactly as their own writes left them; and it reports no miss, because the operation
//! models no not-found error (`smithy_query operation=DeleteGlossaryTerm` lists
//! `AccessDeniedException`, `ConcurrentModificationException`, `InternalServiceException`,
//! `InvalidInputException` and `ThrottlingException` and no `EntityNotFoundException`,
//! unlike `GetGlossaryTerm`), so an identifier this account holds no term under is served
//! and removes nothing.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the form and
//! length of the term's name, which of the two descriptions it carries and how long each is,
//! whether the create carried an idempotency token, which glossary holds the term, where in
//! its glossary's set the term sits, and how an identifier misses.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: "len 1..256" on `GlossaryTermName`
/// (`smithy_query operation=GetGlossaryTerm`). A row at this bound proves a term is removed
/// whatever its name is.
const MAX_NAME_LENGTH: usize = 256;

/// The longest `ShortDescription` the model accepts: "len 1..1024" on
/// `GlossaryShortDescription` (`smithy_query operation=GetGlossaryTerm`).
const MAX_SHORT_DESCRIPTION_LENGTH: usize = 1024;

/// The longest `LongDescription` the model accepts: "len 1..4096" on
/// `GlossaryLongDescription` (`smithy_query operation=GetGlossaryTerm`).
const MAX_LONG_DESCRIPTION_LENGTH: usize = 4096;

/// Create a glossary and return the `GlossaryId` the catalog minted for it.
async fn create_glossary(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateGlossary", &json!({ "Name": name })).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossary must report an Id, got {created}"))
        .to_string()
}

/// Create the glossary term `request` describes inside `glossary`, and return the
/// `GlossaryTermId` minted for it.
async fn create_term(router: &MultiServiceRouter, glossary: &str, request: &Value) -> String {
    let mut body = request.clone();
    body["GlossaryIdentifier"] = json!(glossary);
    let created = ok(router, "CreateGlossaryTerm", &body).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossaryTerm must report an Id, got {created}"))
        .to_string()
}

/// Delete the glossary term `identifier` names, asserting the modelled 200 and the
/// response's modelled emptiness (`DeleteGlossaryTermResponse` declares no members).
async fn delete(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(
        router,
        "DeleteGlossaryTerm",
        &json!({ "Identifier": identifier }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteGlossaryTerm of {identifier:?} must answer 200, got {status} with \
         {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(Map::is_empty),
        "case {case}: DeleteGlossaryTermResponse models no members, got {body}"
    );
}

/// The `Name` `GetGlossaryTerm` reports for `identifier`, which must still resolve.
async fn read_name(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let read = ok(router, "GetGlossaryTerm", &json!({ "Identifier": identifier })).await;
    read.get("Name")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case}: GetGlossaryTerm must report a Name, got {read}")
        })
        .to_string()
}

/// Assert that `identifier` resolves to no glossary term: 400 `EntityNotFoundException`, the
/// miss `GetGlossaryTerm` models, worded the way this surface words a term miss.
async fn assert_read_misses(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(
        router,
        "GetGlossaryTerm",
        &json!({ "Identifier": identifier }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 reading the deleted term {identifier:?}, got {status} with \
         {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
    );
    // The refusal's `Message` member, pinned exactly: this surface's refusal text is a JSON
    // document carrying the sentence under a `Message` key, with the identifier from the
    // request as its one variable part.
    let expected = format!(r#"{{"Message":"Business glossary term {identifier} not found."}}"#);
    assert_eq!(
        error_message(&body),
        expected,
        "case {case}: expected Message {expected:?} for {identifier:?}, got {body}"
    );
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

/// The rows every suite here drives: one `(case, create request without its glossary)` pair
/// per input along the axes the rule generalises over.
fn term_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            // The shape a lifecycle client sends: the one required member beside the
            // glossary.
            "the required member only",
            json!({ "Name": "CustomerLifetimeValue" }),
        ),
        (
            "a short description only",
            json!({
                "Name": "ChurnRate",
                "ShortDescription": "Share of customers that left in a period",
            }),
        ),
        (
            "a long description only",
            json!({
                "Name": "NetRevenueRetention",
                "LongDescription": "Revenue kept from the cohort that was already a customer",
            }),
        ),
        (
            "both descriptions",
            json!({
                "Name": "GrossMargin",
                "ShortDescription": "Revenue less cost of revenue",
                "LongDescription": "Revenue less the cost of delivering it, before any \
                                    operating expense",
            }),
        ),
        (
            "a one-character name and one-character descriptions",
            json!({ "Name": "t", "ShortDescription": "s", "LongDescription": "l" }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "Name": "n".repeat(MAX_NAME_LENGTH) }),
        ),
        (
            "descriptions at the modelled length bounds",
            json!({
                "Name": "MaximallyDescribed",
                "ShortDescription": "s".repeat(MAX_SHORT_DESCRIPTION_LENGTH),
                "LongDescription": "l".repeat(MAX_LONG_DESCRIPTION_LENGTH),
            }),
        ),
        (
            "a name carrying spaces, punctuation and non-ASCII text",
            json!({
                "Name": "Ventas — término (ES/MX)",
                "ShortDescription": "Término de negocio\tcon tabulador",
            }),
        ),
        (
            "a term created under an idempotency token",
            json!({ "Name": "TokenedTerm", "ClientToken": "a-client-token" }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// The delete removes the one term it names
// ---------------------------------------------------------------------------

/// A delete removes the glossary term its `Identifier` resolves to and nothing else: the
/// term's own read misses afterwards, the store holds one record fewer per delete, the
/// glossary that held it is untouched and no longer lists it, and the term of the other
/// glossary — created under the SAME name — is still read back.
#[tokio::test]
async fn deleting_a_glossary_term_removes_only_the_one_it_names() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let glossary = create_glossary(&router, "DeletionGlossary").await;
    // A second glossary holding a term of EVERY row's name, so each delete has to reach the
    // one record its identifier addresses rather than everything of that name.
    let other_glossary = create_glossary(&router, "BystanderGlossary").await;

    let rows = term_rows();
    let mut stored: Vec<(&str, Value, String)> = Vec::new();
    let mut bystanders: Vec<(&str, Value, String)> = Vec::new();
    for (case, request) in &rows {
        stored.push((
            case,
            request.clone(),
            create_term(&router, &glossary, request).await,
        ));
        bystanders.push((
            case,
            request.clone(),
            create_term(&router, &other_glossary, request).await,
        ));
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut remaining = state.lock().glossary_terms.len();
    assert_eq!(
        remaining,
        rows.len() * 2,
        "case every row: the creates must have stored one record each, got {remaining}"
    );

    for (case, request, id) in &stored {
        let name = request["Name"].as_str().expect("the case carries a Name");
        // The record is there to begin with, so the miss asserted below is the delete's
        // doing.
        assert_eq!(
            &read_name(&router, case, id).await,
            name,
            "case {case}: the term must be readable before the delete"
        );
        assert!(
            listed_term_names(&router, &glossary)
                .await
                .iter()
                .any(|listed| listed == name),
            "case {case}: the term must be listed by its glossary before the delete"
        );

        delete(&router, case, id).await;
        assert_read_misses(&router, case, id).await;

        // Exactly one record left the store for this delete.
        remaining -= 1;
        let held = state.lock().glossary_terms.len();
        assert_eq!(
            held, remaining,
            "case {case}: a delete must remove exactly one record; expected {remaining} terms, \
             got {held}"
        );

        // The glossary that held it is still a glossary this account holds, and no longer
        // lists the term.
        let read_glossary = ok(
            &router,
            "GetGlossary",
            &json!({ "Identifier": &glossary }),
        )
        .await;
        assert_eq!(
            read_glossary.get("Name").and_then(Value::as_str),
            Some("DeletionGlossary"),
            "case {case}: a term's delete must leave its glossary in place, got {read_glossary}"
        );
        let listed = listed_term_names(&router, &glossary).await;
        assert!(
            !listed.iter().any(|listed| listed == name),
            "case {case}: the deleted term must not be listed, got {listed:?}"
        );
    }

    // Not one call reached the other glossary's identically named terms.
    for (case, request, id) in &bystanders {
        assert_eq!(
            &read_name(&router, case, id).await,
            request["Name"].as_str().expect("the case carries a Name"),
            "case {case}: a delete must leave the same name in another glossary alone"
        );
    }
}

// ---------------------------------------------------------------------------
// The delete frees the name it held
// ---------------------------------------------------------------------------

/// A term's name identifies it within its glossary, so once the delete removed the record
/// the name is free again: the very same create is served, under a NEW identifier, and the
/// identifier the delete removed stays gone. A row whose create carried an idempotency token
/// is answered the same way — the record that token replayed is no longer there to replay.
#[tokio::test]
async fn deleting_a_glossary_term_frees_its_name_within_its_glossary() {
    let router = emulator();
    let glossary = create_glossary(&router, "RecreateGlossary").await;

    for (case, request) in &term_rows() {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let first = create_term(&router, &glossary, request).await;

        // Before the delete the name is taken: the create under it is refused, which is what
        // makes the service after the delete the delete's doing.
        let (status, body) = call(&router, "CreateGlossaryTerm", &{
            let mut body = request.clone();
            body["GlossaryIdentifier"] = json!(&glossary);
            body["ClientToken"] = json!(format!("{case}-second-token"));
            body
        })
        .await;
        assert_eq!(
            status, 400,
            "case {case}: a second create under {name:?} must be refused while the term is \
             stored, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException for {name:?}, got {body}"
        );

        delete(&router, case, &first).await;

        let second = create_term(&router, &glossary, request).await;
        assert_ne!(
            second, first,
            "case {case}: a create after the delete must mint its own identifier"
        );
        assert_eq!(
            read_name(&router, case, &second).await,
            name,
            "case {case}: the re-created term must read back"
        );
        assert_read_misses(&router, case, &first).await;

        // Leave the store as this row found it, so the next row's name is the only one in
        // play.
        delete(&router, case, &second).await;
    }
}

// ---------------------------------------------------------------------------
// An identifier that resolves to nothing
// ---------------------------------------------------------------------------

/// A delete for an identifier this account holds no glossary term under is SERVED: this
/// operation models no not-found error, so the caller that asks for a state the store is
/// already in is answered as though the delete had happened, and nothing is removed.
///
/// The rows differ along how the identifier misses: never minted, a prefix of a stored
/// identifier, a stored identifier with one more character, a stored identifier recased, a
/// term that was already deleted, a GLOSSARY identifier rather than a term's, and an
/// identifier no mint could produce.
#[tokio::test]
async fn deleting_a_glossary_term_the_store_does_not_hold_is_served() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let glossary = create_glossary(&router, "MissGlossary").await;
    let kept = create_term(&router, &glossary, &json!({ "Name": "KeptTerm" })).await;
    // A term created and then deleted, so one row names an address that WAS live.
    let gone = create_term(&router, &glossary, &json!({ "Name": "GoneTerm" })).await;
    delete(&router, "the term deleted up front", &gone).await;

    let truncated = kept[..kept.len() - 1].to_string();
    let extended = format!("{kept}0");
    let recased = kept.to_uppercase();
    let rows: Vec<(&str, String)> = [
        ("never minted", "no-such-glossary-term".to_string()),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", extended),
        ("a stored identifier recased", recased),
        ("an already deleted term", gone.clone()),
        ("a glossary identifier rather than a term's", glossary.clone()),
        (
            "an identifier no mint could produce",
            "GLOSSARY/TERM/NOT/AN/ID".to_string(),
        ),
    ]
    .into_iter()
    // A minted identifier is lowercase hexadecimal, so one drawn with no letter at all
    // recases to itself; such a row would name the stored record, so it is dropped rather
    // than asserted.
    .filter(|(_, identifier)| identifier != &kept)
    .collect();
    assert!(
        rows.len() >= 6,
        "case the miss table: expected at least six identifiers that miss, got {rows:?}"
    );

    for (case, identifier) in &rows {
        delete(&router, case, identifier).await;
        // Repeating it answers the same way: the delete of something absent is not a state
        // change that a second call could fail on.
        delete(&router, case, identifier).await;
    }

    // Nothing was removed by any of those calls: the term the store does hold is still
    // readable, it is the only record left, and the glossary one row named is still there.
    assert_eq!(
        read_name(&router, "the stored term", &kept).await,
        "KeptTerm",
        "case the stored term: a delete that resolved to nothing must remove nothing"
    );
    let held = engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .glossary_terms
        .len();
    assert_eq!(
        held, 1,
        "case every miss row: the store must still hold exactly the one term, got {held}"
    );
    let read_glossary = ok(
        &router,
        "GetGlossary",
        &json!({ "Identifier": &glossary }),
    )
    .await;
    assert_eq!(
        read_glossary.get("Name").and_then(Value::as_str),
        Some("MissGlossary"),
        "case a glossary identifier rather than a term's: the delete of a term must never \
         remove a glossary, got {read_glossary}"
    );
}
