// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue glossary term slice: `CreateGlossaryTerm` and the
//! `GetGlossaryTerm` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a create stores one term under a freshly minted `GlossaryTermId`
//! against the glossary its `GlossaryIdentifier` names, answers with the five members it
//! stored, and a read of that identifier reports the same five; the glossary must be one
//! this account holds; a term's name is unique within that one glossary and free in any
//! other; and a retry carrying the create's own idempotency token is answered with the
//! record that create already wrote. The suites are tables whose rows differ along what the
//! rule generalises over: the form and length of the name, which of the two descriptions
//! was sent and how long each is, whether the create carried an idempotency token, and
//! which glossary the term was created in.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, GlossaryEntry, GlossaryTermEntry, MAX_GLOSSARY_TERMS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, ok};

/// The longest `Name` the model accepts: "len 1..256" on `GlossaryTermName`
/// (`smithy_query operation=CreateGlossaryTerm`).
const MAX_NAME_LENGTH: usize = 256;

/// The longest `ShortDescription` the model accepts: "len 1..1024" on
/// `GlossaryShortDescription` (`smithy_query shape=GlossaryShortDescription`). A row at
/// this bound proves the whole description is stored rather than a prefix of it.
const MAX_SHORT_DESCRIPTION_LENGTH: usize = 1024;

/// The longest `LongDescription` the model accepts: "len 1..4096" on
/// `GlossaryLongDescription` (`smithy_query shape=GlossaryLongDescription`).
const MAX_LONG_DESCRIPTION_LENGTH: usize = 4096;

/// The longest `ClientToken` the model accepts: "len 1..255" on `HashString`
/// (`smithy_query operation=CreateGlossaryTerm`).
const MAX_CLIENT_TOKEN_LENGTH: usize = 255;

/// Create a glossary and return the identifier it minted.
async fn glossary(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateGlossary", &json!({ "Name": name })).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossary must mint an Id, got {created}"))
        .to_string()
}

/// The `GetGlossaryTerm` response for the term `identifier` names.
async fn get_term(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetGlossaryTerm", &json!({ "Identifier": identifier })).await
}

/// The `Id` of a `CreateGlossaryTerm`/`GetGlossaryTerm` body, asserted to be a usable
/// identifier.
///
/// `GlossaryTermId` carries no length or pattern constraint of its own
/// (`smithy_query shape=GlossaryTermId`), so what is asserted is that an identifier was
/// assigned at all.
fn term_id(case: &str, body: &Value) -> String {
    let id = body
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the response must carry an Id, got {body}"));
    assert!(
        !id.is_empty(),
        "case {case}: the Id must be a usable identifier, got {id:?}"
    );
    id.to_string()
}

/// The rows the round-trip suite drives: one `(case, request-without-the-glossary)` pair per
/// input along the axes the rule generalises over.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            // The shape a lifecycle client sends: the one required member beside the
            // glossary.
            "the required members only",
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
                "LongDescription": "Revenue kept from the cohort that was already a \
                                    customer, expansion included and new business excluded",
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
            "descriptions at their modelled length bounds",
            json!({
                "Name": "MaximallyDescribed",
                "ShortDescription": "s".repeat(MAX_SHORT_DESCRIPTION_LENGTH),
                "LongDescription": "l".repeat(MAX_LONG_DESCRIPTION_LENGTH),
            }),
        ),
        (
            "a name carrying spaces, punctuation and non-ASCII text",
            json!({
                "Name": "Margen bruto — ventas (ES/MX)",
                "ShortDescription": "Términos de negocio\tcon tabulador\ny salto de línea",
            }),
        ),
        (
            "an idempotency token sent with the create",
            json!({
                "Name": "TokenedTerm",
                "ShortDescription": "created with a client token",
                "ClientToken": "a-client-token",
            }),
        ),
        (
            "an idempotency token at the modelled length bound",
            json!({
                "Name": "MaximallyTokened",
                "ClientToken": "t".repeat(MAX_CLIENT_TOKEN_LENGTH),
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// What `CreateGlossaryTerm` persists is what `GetGlossaryTerm` reports: the create answers
/// with the identifier it assigned, the glossary it recorded the term against and the three
/// members it stored, a read of that identifier reports the same five, two reads agree, and
/// each term is its own record under its own identifier.
#[tokio::test]
async fn created_glossary_terms_are_read_back_from_the_store() {
    let router = emulator();
    let glossary_id = glossary(&router, "BusinessConcepts").await;
    let cases = create_rows();
    let mut ids: Vec<(&str, String)> = Vec::new();

    for (case, row) in &cases {
        let mut request = row.clone();
        request["GlossaryIdentifier"] = json!(glossary_id);
        let name = request["Name"].as_str().expect("the case carries a Name");
        let short = request.get("ShortDescription").and_then(Value::as_str);
        let long = request.get("LongDescription").and_then(Value::as_str);

        let created = ok(&router, "CreateGlossaryTerm", &request).await;
        let id = term_id(case, &created);
        assert_eq!(
            (
                created.get("GlossaryId").and_then(Value::as_str),
                created.get("Name").and_then(Value::as_str),
                created.get("ShortDescription").and_then(Value::as_str),
                created.get("LongDescription").and_then(Value::as_str)
            ),
            (Some(glossary_id.as_str()), Some(name), short, long),
            "case {case}: CreateGlossaryTerm must report what it stored, got {created}"
        );

        // The create's own identifier addresses the record it wrote, and the read reports
        // every member of it.
        let read = get_term(&router, &id).await;
        assert_eq!(
            (
                read.get("Id").and_then(Value::as_str),
                read.get("GlossaryId").and_then(Value::as_str),
                read.get("Name").and_then(Value::as_str),
                read.get("ShortDescription").and_then(Value::as_str),
                read.get("LongDescription").and_then(Value::as_str)
            ),
            (
                Some(id.as_str()),
                Some(glossary_id.as_str()),
                Some(name),
                short,
                long
            ),
            "case {case}: GetGlossaryTerm must report the term the create stored, got {read}"
        );

        // A read changes nothing a later reader sees: the second read reports the first's
        // body member for member.
        let again = get_term(&router, &id).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one term must agree; expected {read}, got {again}"
        );
        ids.push((case, id));
    }

    // Each create minted its own identifier, and each record is still its own after every
    // create has landed: no create overwrote another's.
    for (case, id) in &ids {
        let read = get_term(&router, id).await;
        let expected = cases
            .iter()
            .find(|(row, _)| row == case)
            .map(|(_, request)| request)
            .expect("the case is one of the rows");
        assert_eq!(
            read.get("Name"),
            expected.get("Name"),
            "case {case}: each term keeps its own name, got {read}"
        );
        assert_eq!(
            read.get("ShortDescription").and_then(Value::as_str),
            expected.get("ShortDescription").and_then(Value::as_str),
            "case {case}: each term keeps its own short description, got {read}"
        );
        assert_eq!(
            read.get("LongDescription").and_then(Value::as_str),
            expected.get("LongDescription").and_then(Value::as_str),
            "case {case}: each term keeps its own long description, got {read}"
        );
    }
    let distinct: std::collections::BTreeSet<&String> = ids.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "case every row: each term must carry its own identifier, got {ids:?}"
    );
}

/// A term's name is unique WITHIN one glossary, so the same name is free in every other
/// glossary: each create mints its own term, recorded against the glossary it named, and a
/// read of each reports that glossary.
///
/// The rows differ along the glossary a term of one shared name is created in.
#[tokio::test]
async fn one_name_is_its_own_term_in_each_glossary() {
    let router = emulator();
    let shared_name = "SharedConcept";

    let glossaries = [
        ("the first glossary", "FinanceGlossary"),
        ("the second glossary", "SalesGlossary"),
        ("a third glossary", "SupplyChainGlossary"),
    ];

    let mut terms: Vec<(&str, String, String)> = Vec::new();
    for (case, glossary_name) in &glossaries {
        let glossary_id = glossary(&router, glossary_name).await;
        let created = ok(
            &router,
            "CreateGlossaryTerm",
            &json!({
                "GlossaryIdentifier": glossary_id,
                "Name": shared_name,
                "ShortDescription": format!("the concept as {glossary_name} defines it"),
            }),
        )
        .await;
        let id = term_id(case, &created);
        terms.push((case, glossary_id, id));
    }

    for (case, glossary_id, id) in &terms {
        let read = get_term(&router, id).await;
        assert_eq!(
            read.get("GlossaryId").and_then(Value::as_str),
            Some(glossary_id.as_str()),
            "case {case}: a term must report the glossary it was created in, got {read}"
        );
        assert_eq!(
            read.get("Name").and_then(Value::as_str),
            Some(shared_name),
            "case {case}: the shared name must be stored as sent, got {read}"
        );
    }

    let distinct: std::collections::BTreeSet<&String> =
        terms.iter().map(|(_, _, id)| id).collect();
    assert_eq!(
        distinct.len(),
        terms.len(),
        "case every glossary: one name per glossary must mint its own term, got {terms:?}"
    );
}

/// A create carrying the SAME `ClientToken`, glossary and `Name` as one already served is
/// the retry of that request: it is answered with the record the first create wrote —
/// identifier included — and no second record is stored.
///
/// The rows differ along what the first create carried beside the token: neither
/// description, one, both, and a token at the modelled length bound.
#[tokio::test]
async fn a_retry_carrying_the_creates_own_token_reports_the_stored_term() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let glossary_id = glossary(&router, "RetriedGlossary").await;

    let cases = [
        (
            "a token and no description",
            json!({ "Name": "RetriedPlain", "ClientToken": "token-plain" }),
        ),
        (
            "a token and a short description",
            json!({
                "Name": "RetriedShort",
                "ShortDescription": "retried with a short description",
                "ClientToken": "token-short",
            }),
        ),
        (
            "a token and both descriptions",
            json!({
                "Name": "RetriedBoth",
                "ShortDescription": "retried",
                "LongDescription": "retried with both descriptions",
                "ClientToken": "token-both",
            }),
        ),
        (
            "a token at the modelled length bound",
            json!({
                "Name": "RetriedMaximalToken",
                "ClientToken": "r".repeat(MAX_CLIENT_TOKEN_LENGTH),
            }),
        ),
    ];

    for (case, row) in &cases {
        let mut request = row.clone();
        request["GlossaryIdentifier"] = json!(glossary_id);

        let created = ok(&router, "CreateGlossaryTerm", &request).await;
        let id = term_id(case, &created);

        let retried = ok(&router, "CreateGlossaryTerm", &request).await;
        assert_eq!(
            retried, created,
            "case {case}: a retry must report the term the create stored; expected \
             {created}, got {retried}"
        );

        let read = get_term(&router, &id).await;
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: the identifier the create minted must still resolve, got {read}"
        );
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let stored = state.lock().glossary_terms.len();
    assert_eq!(
        stored,
        cases.len(),
        "case every row: a retry must store no second record; expected {} terms, got {stored}",
        cases.len()
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A term is created within a glossary, so a create naming a glossary this account does not
/// hold is refused with the modelled `EntityNotFoundException` and stores nothing.
///
/// The rows differ along how the identifier misses: never minted, a prefix of a stored
/// identifier, a stored identifier with one more character, and a stored identifier recased.
#[tokio::test]
async fn creating_a_term_in_an_unknown_glossary_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let glossary_id = glossary(&router, "StoredGlossary").await;

    let truncated = &glossary_id[..glossary_id.len() - 1];
    let extended = format!("{glossary_id}0");
    let recased = glossary_id.to_uppercase();
    // Every row must name an identifier the store does NOT hold. A minted identifier is
    // lowercase hexadecimal, so one drawn with no letter at all recases to itself; such a
    // row would name the stored glossary, so it is dropped rather than asserted.
    let rows: Vec<(&str, &str)> = [
        ("never minted", "no-such-glossary"),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", &extended),
        ("a stored identifier recased", &recased),
    ]
    .into_iter()
    .filter(|(_, identifier)| *identifier != glossary_id.as_str())
    .collect();
    assert!(
        rows.len() >= 3,
        "case the miss table: expected at least three identifiers that miss, got {rows:?}"
    );

    for (case, identifier) in rows {
        let (status, body) = call(
            &router,
            "CreateGlossaryTerm",
            &json!({ "GlossaryIdentifier": identifier, "Name": "Orphan" }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {identifier:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
        );
    }

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let store = state.lock();
        assert!(
            store.glossary_terms.is_empty(),
            "case every refused row: a refused create stores nothing, got {:?}",
            store
                .glossary_terms
                .values()
                .map(|term| &term.name)
                .collect::<Vec<_>>()
        );
    }

    // The glossary as minted still accepts a create, so the misses above are misses and not
    // a broken lookup.
    let served = ok(
        &router,
        "CreateGlossaryTerm",
        &json!({ "GlossaryIdentifier": glossary_id, "Name": "Adopted" }),
    )
    .await;
    term_id("the minted glossary identifier", &served);
}

/// A term's name is unique within its glossary: a second create of that name in that
/// glossary is refused with the modelled `AlreadyExistsException`, the stored term is
/// untouched, and a name that only resembles a stored one is free.
///
/// The rows differ along how the second create's name relates to the stored one: identical,
/// identical under different descriptions, identical under an idempotency token of its own,
/// and (as the free rows) recased, a prefix, and one character longer.
#[tokio::test]
async fn a_second_create_of_a_stored_name_in_one_glossary_is_refused() {
    let router = emulator();
    let glossary_id = glossary(&router, "UniqueNameGlossary").await;
    let stored_name = "SharedTermName";

    let created = ok(
        &router,
        "CreateGlossaryTerm",
        &json!({
            "GlossaryIdentifier": glossary_id,
            "Name": stored_name,
            "ShortDescription": "the first create's description",
        }),
    )
    .await;
    let id = term_id("the stored term", &created);

    let clashing = [
        (
            "the identical request",
            json!({ "Name": stored_name, "ShortDescription": "the first create's description" }),
        ),
        (
            "the same name under different descriptions",
            json!({
                "Name": stored_name,
                "ShortDescription": "a different description",
                "LongDescription": "and a long one",
            }),
        ),
        (
            "the same name with no description at all",
            json!({ "Name": stored_name }),
        ),
        (
            "the same name under an idempotency token of its own",
            json!({ "Name": stored_name, "ClientToken": "a-fresh-token" }),
        ),
    ];
    for (case, row) in &clashing {
        let mut request = row.clone();
        request["GlossaryIdentifier"] = json!(glossary_id);
        let (status, body) = call(&router, "CreateGlossaryTerm", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
    }

    // The refused creates left the stored term exactly as the first create wrote it.
    let read = get_term(&router, &id).await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some(stored_name),
        "case the stored term: a refused create must change nothing, got {read}"
    );
    assert_eq!(
        read.get("ShortDescription").and_then(Value::as_str),
        Some("the first create's description"),
        "case the stored term: a refused create must change nothing, got {read}"
    );

    // A name is matched as sent, so one that only resembles the stored name is free.
    let free = [
        ("the stored name recased", stored_name.to_uppercase()),
        (
            "a prefix of the stored name",
            stored_name[..stored_name.len() - 1].to_string(),
        ),
        (
            "the stored name with one more character",
            format!("{stored_name}X"),
        ),
    ];
    for (case, name) in &free {
        let other = ok(
            &router,
            "CreateGlossaryTerm",
            &json!({ "GlossaryIdentifier": glossary_id, "Name": name }),
        )
        .await;
        let other_id = term_id(case, &other);
        assert_ne!(
            other_id, id,
            "case {case}: a free name must mint its own term, got {other}"
        );
    }
}

/// A read for an identifier this account holds no term under is refused with the modelled
/// `EntityNotFoundException`. The rows differ along how the identifier misses: never minted,
/// a prefix of a stored identifier, a stored identifier with one more character, a stored
/// identifier recased, and the identifier of the GLOSSARY the term lives in — a glossary id
/// is not a term id.
#[tokio::test]
async fn reading_an_unknown_glossary_term_is_refused() {
    let router = emulator();
    let glossary_id = glossary(&router, "ReadGlossary").await;
    let created = ok(
        &router,
        "CreateGlossaryTerm",
        &json!({ "GlossaryIdentifier": glossary_id, "Name": "StoredTerm" }),
    )
    .await;
    let id = term_id("stored", &created);

    let truncated = &id[..id.len() - 1];
    let extended = format!("{id}0");
    let recased = id.to_uppercase();
    let rows: Vec<(&str, &str)> = [
        ("never minted", "no-such-term"),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", &extended),
        ("a stored identifier recased", &recased),
        ("the identifier of the containing glossary", &glossary_id),
    ]
    .into_iter()
    .filter(|(_, identifier)| *identifier != id.as_str())
    .collect();
    assert!(
        rows.len() >= 4,
        "case the miss table: expected at least four identifiers that miss, got {rows:?}"
    );

    for (case, identifier) in rows {
        let (status, body) = call(
            &router,
            "GetGlossaryTerm",
            &json!({ "Identifier": identifier }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {identifier:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
        );
    }

    // The identifier as minted still resolves, so the misses above are misses and not a
    // broken lookup.
    let read = get_term(&router, &id).await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("StoredTerm"),
        "case the minted identifier: expected the stored term, got {read}"
    );
}

/// One past each published bound on the create's own members is refused, and nothing is
/// stored for it: a name longer than the modelled length, a name of no characters, each
/// description past its own length, and a client token past its length.
#[tokio::test]
async fn an_input_past_a_published_bound_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let glossary_id = glossary(&router, "BoundedGlossary").await;

    let cases = [
        (
            "a name one character past the length bound",
            json!({ "Name": "n".repeat(MAX_NAME_LENGTH + 1) }),
        ),
        ("a name of no characters at all", json!({ "Name": "" })),
        (
            "a short description one character past its length bound",
            json!({
                "Name": "ShortTooFar",
                "ShortDescription": "s".repeat(MAX_SHORT_DESCRIPTION_LENGTH + 1),
            }),
        ),
        (
            "a short description of no characters at all",
            json!({ "Name": "ShortEmptily", "ShortDescription": "" }),
        ),
        (
            "a long description one character past its length bound",
            json!({
                "Name": "LongTooFar",
                "LongDescription": "l".repeat(MAX_LONG_DESCRIPTION_LENGTH + 1),
            }),
        ),
        (
            "a long description of no characters at all",
            json!({ "Name": "LongEmptily", "LongDescription": "" }),
        ),
        (
            "a client token one character past its length bound",
            json!({
                "Name": "TokenedTooFar",
                "ClientToken": "t".repeat(MAX_CLIENT_TOKEN_LENGTH + 1),
            }),
        ),
    ];

    for (case, row) in &cases {
        let mut request = row.clone();
        request["GlossaryIdentifier"] = json!(glossary_id);
        let (status, refusal) = call(&router, "CreateGlossaryTerm", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refusal}"
        );
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    assert!(
        store.glossary_terms.is_empty(),
        "case every refused row: a refused create stores nothing, got {:?}",
        store
            .glossary_terms
            .values()
            .map(|term| &term.name)
            .collect::<Vec<_>>()
    );
}

/// The store is resident memory and this create is reachable by any caller, so the number of
/// glossary terms one process holds is bounded: the create that fits at the ceiling is
/// served, the one that would add a record past it is refused with the closest client error
/// this operation models, and freeing a slot lets a create succeed again — so the ceiling
/// bounds what is stored rather than latching on how many creates were ever made.
#[tokio::test]
async fn a_create_past_the_glossary_term_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would exercise
    // the transport rather than the bound.
    let mut seeded_term_id = String::new();
    let glossary_id = {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        let glossary = GlossaryEntry::new(
            "SeededGlossary".to_string(),
            None,
            None,
            std::time::SystemTime::now(),
        );
        let glossary_id = glossary.id.clone();
        store.glossaries.insert(glossary_id.clone(), glossary);
        for index in 0..MAX_GLOSSARY_TERMS - 1 {
            let term = GlossaryTermEntry::new(
                glossary_id.clone(),
                format!("Seeded{index}"),
                None,
                None,
                None,
                std::time::SystemTime::now(),
            );
            seeded_term_id = term.id.clone();
            store.glossary_terms.insert(term.id.clone(), term);
        }
        glossary_id
    };

    let last = json!({ "GlossaryIdentifier": glossary_id, "Name": "LastTerm" });
    let past = json!({ "GlossaryIdentifier": glossary_id, "Name": "OneTooMany" });

    ok(&router, "CreateGlossaryTerm", &last).await;

    let (status, body) = call(&router, "CreateGlossaryTerm", &past).await;
    assert_eq!(
        status,
        400,
        "case term {} of {MAX_GLOSSARY_TERMS}: expected 400, got {status} with {body}",
        MAX_GLOSSARY_TERMS + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one term too many: expected InvalidInputException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().glossary_terms.remove(&seeded_term_id);
    }
    let served = ok(&router, "CreateGlossaryTerm", &past).await;
    term_id("the create after a slot was freed", &served);
}
