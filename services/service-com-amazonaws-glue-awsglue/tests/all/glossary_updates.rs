// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateGlossary`, the update of the Glue business glossary slice
//! `CreateGlossary` writes and `GetGlossary` reads.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: an update addresses the ONE glossary its `Identifier` resolves to
//! and writes the members the request NAMES, leaving every member it omits where the last
//! write put it; the `GlossaryId` is no member of the request, so the record keeps the
//! identifier its create minted and a later read of that identifier reports what the
//! update stored; a name stays unique over the account's glossaries, so a rename onto a
//! name ANOTHER glossary carries is refused while restating the glossary's own name is
//! served; and an identifier the account holds no glossary under is refused with the miss
//! this operation models (`smithy_query operation=UpdateGlossary` lists
//! `EntityNotFoundException` beside `AlreadyExistsException`).
//!
//! The suites are tables whose rows differ along what the rule generalises over: which
//! members the update names, the form and length of the values it sends, how a requested
//! name relates to the names already stored, and how an identifier misses.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::ComAmazonawsGlueAwsglueState;
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: "len 1..256" on `GlossaryName`
/// (`smithy_query operation=UpdateGlossary`).
const MAX_NAME_LENGTH: usize = 256;

/// The longest `Description` the model accepts: "len 1..2048" on `MetadataDescription`
/// (`smithy_query operation=UpdateGlossary`).
const MAX_DESCRIPTION_LENGTH: usize = 2048;

/// The longest `ClientToken` the model accepts: "len 1..255" on `HashString`
/// (`smithy_query operation=UpdateGlossary`).
const MAX_CLIENT_TOKEN_LENGTH: usize = 255;

/// Create a glossary and return the `GlossaryId` the catalog minted for it.
async fn create_glossary(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok(router, "CreateGlossary", request).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossary must report an Id, got {created}"))
        .to_string()
}

/// `members` sent as an `UpdateGlossary` request for `identifier`, which the member list of
/// a row never carries itself.
fn update_request(identifier: &str, members: &Value) -> Value {
    let mut request = members.clone();
    request["Identifier"] = json!(identifier);
    request
}

/// Update the glossary `identifier` names and return the response body.
async fn update(router: &MultiServiceRouter, identifier: &str, members: &Value) -> Value {
    ok(router, "UpdateGlossary", &update_request(identifier, members)).await
}

/// The `GetGlossary` response for `identifier`, which must still resolve.
async fn get(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetGlossary", &json!({ "Identifier": identifier })).await
}

/// The `(Id, Name, Description)` triple of a glossary body, as the three modelled output
/// members.
fn triple(body: &Value) -> (Option<&str>, Option<&str>, Option<&str>) {
    (
        body.get("Id").and_then(Value::as_str),
        body.get("Name").and_then(Value::as_str),
        body.get("Description").and_then(Value::as_str),
    )
}

// ---------------------------------------------------------------------------
// The update writes the members the request names
// ---------------------------------------------------------------------------

/// One row of the partial-update table: a glossary as created, the members an update names,
/// and the glossary the update must leave behind.
struct UpdateRow {
    case: &'static str,
    /// The `CreateGlossary` request the glossary is stored with.
    create: Value,
    /// The `UpdateGlossary` members, WITHOUT the `Identifier` the harness adds.
    members: Value,
    /// The `Name` the glossary must carry after the update.
    expected_name: String,
    /// The `Description` it must carry, absent when it must carry none.
    expected_description: Option<String>,
}

/// An update writes the members it names and leaves the ones it omits where the last write
/// put them, keeps the identifier the create minted, and reports exactly what a later read
/// of that identifier reports.
#[tokio::test]
async fn an_update_writes_the_members_it_names_and_leaves_the_rest() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let rows = vec![
        UpdateRow {
            case: "the name alone, leaving the description as created",
            create: json!({ "Name": "CustomerConcepts", "Description": "as created" }),
            members: json!({ "Name": "CustomerConceptsV2" }),
            expected_name: "CustomerConceptsV2".to_string(),
            expected_description: Some("as created".to_string()),
        },
        UpdateRow {
            case: "the description alone, leaving the name as created",
            create: json!({ "Name": "FinanceTerms", "Description": "as created" }),
            members: json!({ "Description": "the finance domain's own concepts" }),
            expected_name: "FinanceTerms".to_string(),
            expected_description: Some("the finance domain's own concepts".to_string()),
        },
        UpdateRow {
            case: "both members at once",
            create: json!({ "Name": "SalesTerms", "Description": "as created" }),
            members: json!({ "Name": "SalesGlossary", "Description": "renamed and described" }),
            expected_name: "SalesGlossary".to_string(),
            expected_description: Some("renamed and described".to_string()),
        },
        UpdateRow {
            case: "no updatable member at all",
            create: json!({ "Name": "UntouchedGlossary", "Description": "as created" }),
            members: json!({}),
            expected_name: "UntouchedGlossary".to_string(),
            expected_description: Some("as created".to_string()),
        },
        UpdateRow {
            case: "a description onto a glossary created without one",
            create: json!({ "Name": "LatelyDescribed" }),
            members: json!({ "Description": "described after the fact" }),
            expected_name: "LatelyDescribed".to_string(),
            expected_description: Some("described after the fact".to_string()),
        },
        UpdateRow {
            case: "a name onto a glossary created without a description",
            create: json!({ "Name": "PlainGlossary" }),
            members: json!({ "Name": "PlainGlossaryRenamed" }),
            expected_name: "PlainGlossaryRenamed".to_string(),
            expected_description: None,
        },
        UpdateRow {
            case: "a one-character name and a one-character description",
            create: json!({ "Name": "MinimalGlossary", "Description": "as created" }),
            members: json!({ "Name": "g", "Description": "d" }),
            expected_name: "g".to_string(),
            expected_description: Some("d".to_string()),
        },
        UpdateRow {
            case: "a name at the modelled length bound",
            create: json!({ "Name": "GrowsToTheBound" }),
            members: json!({ "Name": "n".repeat(MAX_NAME_LENGTH) }),
            expected_name: "n".repeat(MAX_NAME_LENGTH),
            expected_description: None,
        },
        UpdateRow {
            case: "a description at the modelled length bound",
            create: json!({ "Name": "MaximallyDescribed", "Description": "short" }),
            members: json!({ "Description": "d".repeat(MAX_DESCRIPTION_LENGTH) }),
            expected_name: "MaximallyDescribed".to_string(),
            expected_description: Some("d".repeat(MAX_DESCRIPTION_LENGTH)),
        },
        UpdateRow {
            case: "a name and description carrying spaces, punctuation and non-ASCII text",
            create: json!({ "Name": "Ventas", "Description": "as created" }),
            members: json!({
                "Name": "Ventas — Glosario (ES/MX)",
                "Description": "Términos de negocio\tcon tabulador\ny salto de línea",
            }),
            expected_name: "Ventas — Glosario (ES/MX)".to_string(),
            expected_description: Some(
                "Términos de negocio\tcon tabulador\ny salto de línea".to_string(),
            ),
        },
        UpdateRow {
            case: "an idempotency token sent with the update",
            create: json!({ "Name": "TokenedUpdate", "Description": "as created" }),
            members: json!({ "Description": "updated under a token", "ClientToken": "a-token" }),
            expected_name: "TokenedUpdate".to_string(),
            expected_description: Some("updated under a token".to_string()),
        },
        UpdateRow {
            case: "an idempotency token at the modelled length bound",
            create: json!({ "Name": "MaximallyTokenedUpdate" }),
            members: json!({
                "Name": "MaximallyTokenedUpdated",
                "ClientToken": "t".repeat(MAX_CLIENT_TOKEN_LENGTH),
            }),
            expected_name: "MaximallyTokenedUpdated".to_string(),
            expected_description: None,
        },
    ];

    // A bystander, to prove an update is scoped to the glossary it names.
    let bystander = create_glossary(&router, &json!({ "Name": "BystanderGlossary" })).await;

    for row in &rows {
        let id = create_glossary(&router, &row.create).await;
        let expected = (
            Some(id.as_str()),
            Some(row.expected_name.as_str()),
            row.expected_description.as_deref(),
        );

        let updated = update(&router, &id, &row.members).await;
        assert_eq!(
            triple(&updated),
            expected,
            "case {}: UpdateGlossary must report the glossary it stored, got {updated}",
            row.case
        );

        // The update reports what the store holds: the read of the identifier the create
        // minted answers with the same three members.
        let read = get(&router, &id).await;
        assert_eq!(
            triple(&read),
            expected,
            "case {}: GetGlossary must report what the update stored, got {read}",
            row.case
        );

        // Re-sending the same update changes nothing further: the second answer is the
        // first, member for member.
        let again = update(&router, &id, &row.members).await;
        assert_eq!(
            triple(&again),
            expected,
            "case {}: re-sending the update must report the same glossary, got {again}",
            row.case
        );
    }

    // An update rewrites a record, it never adds one: the store holds exactly the
    // glossaries the creates wrote.
    let held = engine
        .state::<ComAmazonawsGlueAwsglueState>()
        .lock()
        .glossaries
        .len();
    assert_eq!(
        held,
        rows.len() + 1,
        "case every row: an update must store no second record; expected {} glossaries, got \
         {held}",
        rows.len() + 1
    );

    // Nothing any update did reached the glossary it did not name.
    let read = get(&router, &bystander).await;
    assert_eq!(
        triple(&read),
        (Some(bystander.as_str()), Some("BystanderGlossary"), None),
        "case the bystander glossary: an update must leave every other glossary in place, got \
         {read}"
    );
}

// ---------------------------------------------------------------------------
// The name namespace after a rename
// ---------------------------------------------------------------------------

/// A rename moves the glossary's place in the account's one-glossary-per-name namespace:
/// the name it gave up is free for a create again, and the name it took is refused to one.
///
/// The rows differ along the form and length of the two names: plain text, a one-character
/// name, a name at the modelled length bound, and a name carrying non-ASCII text.
#[tokio::test]
async fn a_rename_frees_the_old_name_and_takes_the_new_one() {
    let router = emulator();

    let rows: Vec<(&str, String, String)> = vec![
        (
            "plain names",
            "RenamedPlain".to_string(),
            "RenamedPlainV2".to_string(),
        ),
        (
            "a one-character new name",
            "RenamedToOne".to_string(),
            "x".to_string(),
        ),
        (
            "a new name at the modelled length bound",
            "RenamedToBound".to_string(),
            "b".repeat(MAX_NAME_LENGTH),
        ),
        (
            "names carrying non-ASCII text",
            "Glosario".to_string(),
            "Glosario — Ventas (ES/MX)".to_string(),
        ),
    ];

    for (case, old_name, new_name) in &rows {
        let id = create_glossary(&router, &json!({ "Name": old_name })).await;
        update(&router, &id, &json!({ "Name": new_name })).await;

        // The name the glossary gave up is free: a create under it is served, under its own
        // identifier.
        let reused = create_glossary(&router, &json!({ "Name": old_name })).await;
        assert_ne!(
            reused, id,
            "case {case}: a create under the freed name must mint its own glossary"
        );

        // The name the glossary took is held by it: a create under that name is refused.
        let (status, body) = call(&router, "CreateGlossary", &json!({ "Name": new_name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 creating under the taken name, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException for the taken name, got {body}"
        );

        // The renamed glossary is still its own record, under the identifier its create
        // minted.
        let read = get(&router, &id).await;
        assert_eq!(
            triple(&read),
            (Some(id.as_str()), Some(new_name.as_str()), None),
            "case {case}: the renamed glossary must read back under its own identifier, got \
             {read}"
        );
    }
}

/// A name is unique over this account's glossaries, so an update that would move one onto a
/// name ANOTHER glossary carries is refused with the modelled `AlreadyExistsException` and
/// writes nothing; a request restating the glossary's OWN name collides with nothing and is
/// served, and a name that only resembles a stored one is free.
///
/// The rows differ along how the requested name relates to the names already stored:
/// identical to another glossary's, identical beside a description, identical under an
/// idempotency token, the glossary's own name, its own name beside a new description, and
/// (as the free rows) another's name recased, a prefix of it, and one character longer.
#[tokio::test]
async fn an_update_onto_another_glossarys_name_is_refused() {
    let router = emulator();

    let other_name = "OccupiedGlossaryName";
    let other = create_glossary(
        &router,
        &json!({ "Name": other_name, "Description": "the other glossary" }),
    )
    .await;
    let subject_name = "SubjectGlossary";
    let subject = create_glossary(
        &router,
        &json!({ "Name": subject_name, "Description": "the subject" }),
    )
    .await;
    let before = get(&router, &subject).await;

    let refused = [
        ("another glossary's name", json!({ "Name": other_name })),
        (
            "another glossary's name beside a description",
            json!({ "Name": other_name, "Description": "a new description" }),
        ),
        (
            "another glossary's name under an idempotency token",
            json!({ "Name": other_name, "ClientToken": "a-fresh-token" }),
        ),
    ];
    for (case, members) in &refused {
        let (status, body) = call(
            &router,
            "UpdateGlossary",
            &update_request(&subject, members),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );
        // A refused update writes nothing: the subject is exactly as its create left it.
        let read = get(&router, &subject).await;
        assert_eq!(
            read, before,
            "case {case}: a refused update must change nothing; expected {before}, got {read}"
        );
    }

    // The other glossary was never touched by any refused update either.
    let read = get(&router, &other).await;
    assert_eq!(
        triple(&read),
        (
            Some(other.as_str()),
            Some(other_name),
            Some("the other glossary")
        ),
        "case the occupied glossary: a refused update must leave it in place, got {read}"
    );

    // Restating the glossary's own name collides with nothing, so it is served.
    let served = update(&router, &subject, &json!({ "Name": subject_name })).await;
    assert_eq!(
        triple(&served),
        (
            Some(subject.as_str()),
            Some(subject_name),
            Some("the subject")
        ),
        "case the glossary's own name: expected the stored glossary, got {served}"
    );
    let served = update(
        &router,
        &subject,
        &json!({ "Name": subject_name, "Description": "restated with a new description" }),
    )
    .await;
    assert_eq!(
        triple(&served),
        (
            Some(subject.as_str()),
            Some(subject_name),
            Some("restated with a new description")
        ),
        "case its own name beside a new description: expected the new description, got {served}"
    );

    // A name is matched as sent, so one that only resembles the occupied name is free.
    let free = [
        ("the occupied name recased", other_name.to_uppercase()),
        (
            "a prefix of the occupied name",
            other_name[..other_name.len() - 1].to_string(),
        ),
        (
            "the occupied name with one more character",
            format!("{other_name}X"),
        ),
    ];
    for (case, name) in &free {
        let served = update(&router, &subject, &json!({ "Name": name })).await;
        assert_eq!(
            triple(&served).1,
            Some(name.as_str()),
            "case {case}: a free name must be written, got {served}"
        );
    }
}

// ---------------------------------------------------------------------------
// An identifier that resolves to nothing
// ---------------------------------------------------------------------------

/// An update for an identifier this account holds no glossary under is refused with the
/// modelled `EntityNotFoundException`, worded the way this surface words a glossary miss,
/// and writes nothing — the glossary a concurrent delete already removed included.
///
/// The rows differ along how the identifier misses: never minted, a prefix of a stored
/// identifier, a stored identifier with one more character, a stored identifier recased, an
/// identifier no mint could produce, and a glossary that was deleted.
#[tokio::test]
async fn updating_a_glossary_the_store_does_not_hold_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    let kept = create_glossary(
        &router,
        &json!({ "Name": "KeptGlossary", "Description": "as created" }),
    )
    .await;
    // A glossary created and then deleted, so one row names an address that WAS live — the
    // shape a client that updates a glossary another caller has just deleted sends.
    let gone = create_glossary(&router, &json!({ "Name": "GoneGlossary" })).await;
    ok(
        &router,
        "DeleteGlossary",
        &json!({ "Identifier": gone.clone() }),
    )
    .await;

    let truncated = kept[..kept.len() - 1].to_string();
    let extended = format!("{kept}0");
    let recased = kept.to_uppercase();
    let rows: Vec<(&str, String)> = [
        ("never minted", "no-such-glossary".to_string()),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", extended),
        ("a stored identifier recased", recased),
        (
            "an identifier no mint could produce",
            "GLOSSARY/NOT/AN/ID".to_string(),
        ),
        ("a glossary that was deleted", gone.clone()),
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

    // The members the update names make no difference to a miss: the identifier is resolved
    // first.
    let member_sets = [
        ("a name alone", json!({ "Name": "RenamedByAMiss" })),
        (
            "a description alone",
            json!({ "Description": "described by a miss" }),
        ),
        ("no updatable member at all", json!({})),
    ];

    for (case, identifier) in &rows {
        for (members_case, members) in &member_sets {
            let (status, body) = call(
                &router,
                "UpdateGlossary",
                &update_request(identifier, members),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case} with {members_case}: expected 400 for {identifier:?}, got {status} \
                 with {body}"
            );
            assert!(
                error_code(&body).contains("EntityNotFoundException"),
                "case {case} with {members_case}: expected EntityNotFoundException for \
                 {identifier:?}, got {body}"
            );
            // The refusal's `Message` member, pinned exactly: this surface's refusal text is
            // a JSON document carrying the sentence under a `Message` key, with the
            // identifier from the request as its one variable part — the same sentence
            // `GetGlossary` words the same miss with.
            assert_eq!(
                error_message(&body),
                format!(r#"{{"Message":"Business glossary {identifier} not found."}}"#),
                "case {case} with {members_case}: unexpected refusal wording for \
                 {identifier:?}, got {body}"
            );
        }
    }

    // No refused update wrote anything: the one stored glossary is as its create left it,
    // and it is the only record the store holds.
    let read = get(&router, &kept).await;
    assert_eq!(
        triple(&read),
        (Some(kept.as_str()), Some("KeptGlossary"), Some("as created")),
        "case the stored glossary: an update that resolved to nothing must write nothing, got \
         {read}"
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

/// A value one past a published bound on the update's own members is refused, and the stored
/// glossary is left exactly as its create wrote it: a name past its length, a name of no
/// characters, a description past its length, a description of no characters, and a client
/// token past its length.
#[tokio::test]
async fn an_update_past_a_published_bound_is_refused() {
    let router = emulator();

    let id = create_glossary(
        &router,
        &json!({ "Name": "BoundedGlossary", "Description": "as created" }),
    )
    .await;
    let before = get(&router, &id).await;

    let rows = [
        (
            "a name one character past the length bound",
            json!({ "Name": "n".repeat(MAX_NAME_LENGTH + 1) }),
        ),
        ("a name of no characters at all", json!({ "Name": "" })),
        (
            "a description one character past its length bound",
            json!({ "Description": "d".repeat(MAX_DESCRIPTION_LENGTH + 1) }),
        ),
        (
            "a description of no characters at all",
            json!({ "Description": "" }),
        ),
        (
            "a client token one character past its length bound",
            json!({
                "Name": "TokenedTooFar",
                "ClientToken": "t".repeat(MAX_CLIENT_TOKEN_LENGTH + 1),
            }),
        ),
    ];

    for (case, members) in &rows {
        let (status, body) = call(&router, "UpdateGlossary", &update_request(&id, members)).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        let read = get(&router, &id).await;
        assert_eq!(
            read, before,
            "case {case}: a refused update must write nothing; expected {before}, got {read}"
        );
    }
}
