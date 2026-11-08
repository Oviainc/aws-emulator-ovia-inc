// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue business glossary slice: `CreateGlossary` and the
//! `GetGlossary` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a create stores one glossary under a freshly minted `GlossaryId`
//! and answers with the three members it stored, a read of that identifier reports the
//! same three, a name is unique over the account's glossaries, and a retry carrying the
//! create's own idempotency token is answered with the record that create already wrote.
//! The suites are tables whose rows differ along what the rule generalises over: the form
//! and length of the name, whether a description was sent and how long it is, and whether
//! the create carried an idempotency token.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, GlossaryEntry, MAX_GLOSSARIES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, ok};

/// The longest `Name` the model accepts: "len 1..256" on `GlossaryName`
/// (`smithy_query operation=CreateGlossary`).
const MAX_NAME_LENGTH: usize = 256;

/// The longest `Description` the model accepts: "len 1..2048" on `MetadataDescription`
/// (`smithy_query shape=MetadataDescription`). A row at this bound proves the whole
/// description is stored rather than a prefix of it.
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// The longest `ClientToken` the model accepts: "len 1..255" on `HashString`
/// (`smithy_query operation=CreateGlossary`).
const MAX_CLIENT_TOKEN_LENGTH: usize = 255;

/// Create a glossary and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateGlossary", body).await
}

/// The `GetGlossary` response for the glossary `identifier` names.
async fn get(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetGlossary", &json!({ "Identifier": identifier })).await
}

/// The `Id` of a `CreateGlossary`/`GetGlossary` body, asserted to be a usable identifier.
///
/// `GlossaryId` carries no length or pattern constraint of its own
/// (`smithy_query shape=GlossaryId`), so what is asserted is that an identifier was
/// assigned at all.
fn glossary_id(case: &str, body: &Value) -> String {
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

/// The rows every round-trip suite shares: one `(case, request)` pair per input along the
/// axes the rule generalises over.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
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
                "Description": "Términos de negocio\tcon tabulador\ny salto de línea",
            }),
        ),
        (
            "an idempotency token sent with the create",
            json!({
                "Name": "TokenedGlossary",
                "Description": "created with a client token",
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

/// What `CreateGlossary` persists is what `GetGlossary` reports: the create answers with
/// the identifier it assigned plus the name and description it stored, a read of that
/// identifier reports the same three members, two reads agree, and each glossary is its
/// own record under its own identifier.
#[tokio::test]
async fn created_glossaries_are_read_back_from_the_store() {
    let router = emulator();
    let cases = create_rows();
    let mut ids: Vec<(&str, String)> = Vec::new();

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let description = request.get("Description").and_then(Value::as_str);

        let created = create(&router, request).await;
        let id = glossary_id(case, &created);
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: CreateGlossary must report the stored name, got {created}"
        );
        assert_eq!(
            created.get("Description").and_then(Value::as_str),
            description,
            "case {case}: CreateGlossary must report the description it stored, got a \
             description of {:?} characters",
            created
                .get("Description")
                .and_then(Value::as_str)
                .map(str::len)
        );

        // The create's own identifier addresses the record it wrote, and the read reports
        // every member of it.
        let read = get(&router, &id).await;
        assert_eq!(
            (
                read.get("Id").and_then(Value::as_str),
                read.get("Name").and_then(Value::as_str),
                read.get("Description").and_then(Value::as_str)
            ),
            (Some(id.as_str()), Some(name), description),
            "case {case}: GetGlossary must report the glossary the create stored, got {read}"
        );

        // A read changes nothing a later reader sees: the second read reports the first's
        // body member for member.
        let again = get(&router, &id).await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one glossary must agree; expected {read}, got {again}"
        );
        ids.push((case, id));
    }

    // Each create minted its own identifier, and each record is still its own after every
    // create has landed: no create overwrote another's.
    for (case, id) in &ids {
        let read = get(&router, id).await;
        let expected = cases
            .iter()
            .find(|(row, _)| row == case)
            .map(|(_, request)| request)
            .expect("the case is one of the rows");
        assert_eq!(
            read.get("Name"),
            expected.get("Name"),
            "case {case}: each glossary keeps its own name, got {read}"
        );
        assert_eq!(
            read.get("Description").and_then(Value::as_str),
            expected.get("Description").and_then(Value::as_str),
            "case {case}: each glossary keeps its own description, got {read}"
        );
    }
    let distinct: std::collections::BTreeSet<&String> = ids.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "case every row: each glossary must carry its own identifier, got {ids:?}"
    );
}

/// A create carrying the SAME `ClientToken` and `Name` as one already served is the retry
/// of that request: it is answered with the record the first create wrote — identifier
/// included — and no second record is stored.
///
/// The rows differ along what the first create carried beside the token: no description, a
/// description, and a token at the modelled length bound.
#[tokio::test]
async fn a_retry_carrying_the_creates_own_token_reports_the_stored_glossary() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let cases = [
        (
            "a token and no description",
            json!({ "Name": "RetriedPlain", "ClientToken": "token-plain" }),
        ),
        (
            "a token and a description",
            json!({
                "Name": "RetriedDescribed",
                "Description": "retried with a description",
                "ClientToken": "token-described",
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

    for (case, request) in &cases {
        let created = create(&router, request).await;
        let id = glossary_id(case, &created);

        let retried = create(&router, request).await;
        assert_eq!(
            retried, created,
            "case {case}: a retry must report the glossary the create stored; expected \
             {created}, got {retried}"
        );

        let read = get(&router, &id).await;
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: the identifier the create minted must still resolve, got {read}"
        );
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let stored = state.lock().glossaries.len();
    assert_eq!(
        stored,
        cases.len(),
        "case every row: a retry must store no second record; expected {} glossaries, got \
         {stored}",
        cases.len()
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A glossary's name is unique over this account's glossaries: a second create under a
/// name already stored is refused with the modelled `AlreadyExistsException`, the stored
/// glossary is untouched, and a name that only resembles a stored one is free.
///
/// The rows differ along how the second create's name relates to the stored one: identical,
/// identical with a different description, identical under a different idempotency token,
/// and (as the free rows) recased, a prefix, and one character longer.
#[tokio::test]
async fn a_second_create_under_a_stored_name_is_refused() {
    let router = emulator();
    let stored_name = "SharedGlossaryName";
    let created = create(
        &router,
        &json!({ "Name": stored_name, "Description": "the first create's description" }),
    )
    .await;
    let id = glossary_id("the stored glossary", &created);
    // The whole record as the create left it, so the refusals below are compared against
    // every member a read reports and not only the ones the create answers with.
    let before = get(&router, &id).await;

    let clashing = [
        (
            "the identical request",
            json!({ "Name": stored_name, "Description": "the first create's description" }),
        ),
        (
            "the same name under a different description",
            json!({ "Name": stored_name, "Description": "a different description" }),
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
    for (case, request) in &clashing {
        let (status, body) = call(&router, "CreateGlossary", request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
    }

    // The refused creates left the stored glossary exactly as the first create wrote it:
    // every member a read reports, the two instants included, and the three members the
    // create itself answered with.
    let read = get(&router, &id).await;
    assert_eq!(
        read, before,
        "case the stored glossary: a refused create must change nothing; expected {before}, \
         got {read}"
    );
    assert_eq!(
        (read.get("Id"), read.get("Name"), read.get("Description")),
        (
            created.get("Id"),
            created.get("Name"),
            created.get("Description")
        ),
        "case the stored glossary: the record must still be the one the create reported; \
         expected {created}, got {read}"
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
        let other = create(&router, &json!({ "Name": name })).await;
        let other_id = glossary_id(case, &other);
        assert_ne!(
            other_id, id,
            "case {case}: a free name must mint its own glossary, got {other}"
        );
    }
}

/// A read for an identifier this account holds no glossary under is refused with the
/// modelled `EntityNotFoundException`. The rows differ along how the identifier misses:
/// never minted, a prefix of a stored identifier, a stored identifier with one more
/// character, and a stored identifier recased.
#[tokio::test]
async fn reading_an_unknown_glossary_is_refused() {
    let router = emulator();
    let created = create(&router, &json!({ "Name": "StoredGlossary" })).await;
    let id = glossary_id("stored", &created);

    let truncated = &id[..id.len() - 1];
    let extended = format!("{id}0");
    let recased = id.to_uppercase();
    // Every row must name an identifier the store does NOT hold. A minted identifier is
    // lowercase hexadecimal, so one drawn with no letter at all recases to itself; such a
    // row would name the stored record, so it is dropped rather than asserted.
    let rows: Vec<(&str, &str)> = [
        ("never minted", "no-such-glossary"),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", &extended),
        ("a stored identifier recased", &recased),
    ]
    .into_iter()
    .filter(|(_, identifier)| *identifier != id.as_str())
    .collect();
    assert!(
        rows.len() >= 3,
        "case the miss table: expected at least three identifiers that miss, got {rows:?}"
    );

    for (case, identifier) in rows {
        let (status, body) = call(&router, "GetGlossary", &json!({ "Identifier": identifier })).await;
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
    let read = get(&router, &id).await;
    assert_eq!(
        read.get("Name").and_then(Value::as_str),
        Some("StoredGlossary"),
        "case the minted identifier: expected the stored glossary, got {read}"
    );
}

/// One past each published bound on the create's own members is refused, and nothing is
/// stored for it: a name longer than the modelled length, a name of no characters, a
/// description past its length, and a client token past its length.
#[tokio::test]
async fn an_input_past_a_published_bound_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let cases = [
        (
            "a name one character past the length bound",
            json!({ "Name": "n".repeat(MAX_NAME_LENGTH + 1) }),
        ),
        ("a name of no characters at all", json!({ "Name": "" })),
        (
            "a description one character past its length bound",
            json!({
                "Name": "DescribedTooFar",
                "Description": "d".repeat(MAX_DESCRIPTION_LENGTH + 1),
            }),
        ),
        (
            "a description of no characters at all",
            json!({ "Name": "DescribedEmptily", "Description": "" }),
        ),
        (
            "a client token one character past its length bound",
            json!({
                "Name": "TokenedTooFar",
                "ClientToken": "t".repeat(MAX_CLIENT_TOKEN_LENGTH + 1),
            }),
        ),
    ];

    for (case, body) in &cases {
        let (status, refusal) = call(&router, "CreateGlossary", body).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {refusal}"
        );
    }

    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let store = state.lock();
    assert!(
        store.glossaries.is_empty(),
        "case every refused row: a refused create stores nothing, got {:?}",
        store.glossaries.values().map(|g| &g.name).collect::<Vec<_>>()
    );
}

/// The store is resident memory and this create is reachable by any caller, so the number
/// of glossaries one process holds is bounded: the create that fits at the ceiling is
/// served, the one that would add a record past it is refused with the closest client error
/// this operation models, and freeing a slot lets a create succeed again — so the ceiling
/// bounds what is stored rather than latching on how many creates were ever made.
#[tokio::test]
async fn a_create_past_the_glossary_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    let mut seeded_id = String::new();
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_GLOSSARIES - 1 {
            let entry = GlossaryEntry::new(
                format!("Seeded{index}"),
                None,
                None,
                std::time::SystemTime::now(),
            );
            seeded_id = entry.id.clone();
            store.glossaries.insert(entry.id.clone(), entry);
        }
    }

    let last = json!({ "Name": "LastGlossary" });
    let past = json!({ "Name": "OneTooMany" });

    create(&router, &last).await;

    let (status, body) = call(&router, "CreateGlossary", &past).await;
    assert_eq!(
        status,
        400,
        "case glossary {} of {MAX_GLOSSARIES}: expected 400, got {status} with {body}",
        MAX_GLOSSARIES + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one glossary too many: expected InvalidInputException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().glossaries.remove(&seeded_id);
    }
    let served = create(&router, &past).await;
    glossary_id("the create after a slot was freed", &served);
}
