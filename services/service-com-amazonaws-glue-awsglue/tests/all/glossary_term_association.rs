// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `AssociateGlossaryTerms`, the operation that attaches business
//! glossary terms to a catalog asset, and for what `GetAsset` then reports.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: an association MERGES the term ids its request names into the
//! glossary term set of the asset its `AssetIdentifier` resolves to and reports that whole
//! set, so a term the asset already carries is not repeated and a later association adds to
//! what an earlier one wrote; the resulting set is what an independent `GetAsset` of the
//! same identifier reports; an identifier that resolves to no asset is refused with the
//! modelled `EntityNotFoundException`, worded the one way this service's asset surface
//! words that miss; and the association is to the term ID, so the glossary term namespace
//! is not cross-locked with it.
//!
//! The suites are tables whose rows differ along what the rule generalises over: the number
//! of term ids one request names, whether they are already held, the form and length of the
//! asset identifier, how many associations are stacked onto one asset, how an identifier
//! misses, and how many associations are in flight against one asset at once.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The most glossary terms one asset can carry: "len 1..10" on `GlossaryTermIdList`
/// (`smithy_query operation=AssociateGlossaryTerms`), the shape both the association's
/// `GlossaryTerms` and `GetAsset$GlossaryTerms` target.
const MAX_ASSET_GLOSSARY_TERMS: usize = 10;

/// The longest `AssetIdentifier` the model accepts: "len 1..1087" on `AssetId`
/// (`smithy_query operation=AssociateGlossaryTerms`), whose pattern
/// `^[a-zA-Z0-9\-\:\/\.\_\*]+$` is what the rows below keep to.
const MAX_ASSET_ID_LENGTH: usize = 1087;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Register a form type and return the identifier the catalog assigned it.
async fn form_type(router: &MultiServiceRouter, name: &str) -> String {
    let stored = ok(
        router,
        "PutFormType",
        &json!({ "Name": name, "Schema": format!("structure {name} {{ note: String }}") }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutFormType must report an Id, got {stored}"))
        .to_string()
}

/// Register an asset type over one form type and return the identifier it minted.
async fn asset_type(router: &MultiServiceRouter, name: &str, form_type_id: &str) -> String {
    let mut map = Map::new();
    map.insert(
        "Details".to_string(),
        json!({ "FormTypeIdentifier": form_type_id }),
    );
    let stored = ok(
        router,
        "PutAssetType",
        &json!({ "Name": name, "Forms": Value::Object(map) }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutAssetType must report an Id, got {stored}"))
        .to_string()
}

/// Store an asset under `identifier`, an instance of `asset_type_id`.
async fn put_asset(router: &MultiServiceRouter, identifier: &str, asset_type_id: &str) {
    ok(
        router,
        "PutAsset",
        &json!({
            "Identifier": identifier,
            "AssetTypeId": asset_type_id,
            "Name": identifier,
            "Forms": {},
        }),
    )
    .await;
}

/// Create a business glossary and return the identifier it minted.
async fn glossary(router: &MultiServiceRouter, name: &str) -> String {
    let created = ok(router, "CreateGlossary", &json!({ "Name": name })).await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossary must mint an Id, got {created}"))
        .to_string()
}

/// Create a glossary term inside `glossary_id` and return the identifier it minted.
async fn glossary_term(router: &MultiServiceRouter, glossary_id: &str, name: &str) -> String {
    let created = ok(
        router,
        "CreateGlossaryTerm",
        &json!({ "GlossaryIdentifier": glossary_id, "Name": name }),
    )
    .await;
    created
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("CreateGlossaryTerm must mint an Id, got {created}"))
        .to_string()
}

/// A whole fixture: one asset ready to be associated with, and the glossary its terms are
/// created in.
struct Catalog {
    asset: String,
    glossary: String,
}

/// Register a form type, an asset type, an asset under `asset` and a glossary, so a suite
/// can get to the association in one line.
async fn catalog(router: &MultiServiceRouter, asset: &str) -> Catalog {
    let form = form_type(router, "AssociationForm").await;
    let asset_type_id = asset_type(router, "AssociationAssetType", &form).await;
    put_asset(router, asset, &asset_type_id).await;
    Catalog {
        asset: asset.to_string(),
        glossary: glossary(router, "AssociationGlossary").await,
    }
}

/// The `GlossaryTerms` of a response, in the order it reported them.
fn reported_terms(case: &str, body: &Value) -> Vec<String> {
    body.get("GlossaryTerms")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case}: the response must carry GlossaryTerms, got {body}"))
        .iter()
        .map(|term| {
            term.as_str()
                .unwrap_or_else(|| panic!("case {case}: a term id is a string, got {term}"))
                .to_string()
        })
        .collect()
}

/// The `GlossaryTerms` an independent `GetAsset` reports for `identifier`, or an empty list
/// when the asset carries none.
async fn read_terms(router: &MultiServiceRouter, identifier: &str) -> Vec<String> {
    let read = ok(router, "GetAsset", &json!({ "Identifier": identifier })).await;
    read.get("GlossaryTerms")
        .and_then(Value::as_array)
        .map(|terms| {
            terms
                .iter()
                .map(|term| term.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Associate `terms` with the asset `asset` names, asserting the modelled 200.
async fn associate(
    router: &MultiServiceRouter,
    case: &str,
    asset: &str,
    terms: &[String],
) -> Value {
    let (status, body) = call(
        router,
        "AssociateGlossaryTerms",
        &json!({ "AssetIdentifier": asset, "GlossaryTermIdentifiers": terms }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: associating {terms:?} with {asset:?} must answer 200, got {status} with \
         {body}"
    );
    body
}

// ---------------------------------------------------------------------------
// The merge, and the read-back
// ---------------------------------------------------------------------------

/// One association reports exactly the set the asset carries afterwards, and an independent
/// `GetAsset` reports the same set. The rows differ along how many term ids the request
/// names, whether an id is repeated within the request, and whether the terms come from one
/// glossary or several.
#[tokio::test]
async fn an_association_reports_the_set_the_asset_carries() {
    let router = emulator();
    let fixture = catalog(&router, "order-1").await;
    let other_glossary = glossary(&router, "SecondGlossary").await;

    let mut minted: Vec<String> = Vec::new();
    for name in ["Revenue", "Churn", "Margin", "Cohort"] {
        minted.push(glossary_term(&router, &fixture.glossary, name).await);
    }
    let elsewhere = glossary_term(&router, &other_glossary, "Elsewhere").await;

    // (case, the ids the request names, the ids the asset must carry afterwards)
    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            // The shape a governance client sends: one term on one asset.
            "one term",
            vec![minted[0].clone()],
            vec![minted[0].clone()],
        ),
        (
            "two more terms in one request",
            vec![minted[1].clone(), minted[2].clone()],
            vec![minted[0].clone(), minted[1].clone(), minted[2].clone()],
        ),
        (
            "a term the asset already carries",
            vec![minted[0].clone()],
            vec![minted[0].clone(), minted[1].clone(), minted[2].clone()],
        ),
        (
            "one held and one new term together",
            vec![minted[2].clone(), minted[3].clone()],
            vec![
                minted[0].clone(),
                minted[1].clone(),
                minted[2].clone(),
                minted[3].clone(),
            ],
        ),
        (
            "the same id twice in one request",
            vec![elsewhere.clone(), elsewhere.clone()],
            vec![
                minted[0].clone(),
                minted[1].clone(),
                minted[2].clone(),
                minted[3].clone(),
                elsewhere.clone(),
            ],
        ),
    ];

    for (case, requested, expected) in &rows {
        let body = associate(&router, case, &fixture.asset, requested).await;
        assert_eq!(
            body.get("AssetIdentifier").and_then(Value::as_str),
            Some(fixture.asset.as_str()),
            "case {case}: the response must name the asset it wrote to, got {body}"
        );
        for absent in ["IterableFormName", "ItemIdentifier"] {
            assert!(
                body.get(absent).is_none(),
                "case {case}: an association to the asset itself carries no {absent}, got {body}"
            );
        }
        let reported = reported_terms(case, &body);
        assert_eq!(
            &reported, expected,
            "case {case}: expected the asset to carry {expected:?}, got {reported:?}"
        );

        // An independent read reports the same set, so the merge was persisted and not
        // only answered.
        let read = read_terms(&router, &fixture.asset).await;
        assert_eq!(
            &read, expected,
            "case {case}: GetAsset must report {expected:?}, got {read:?}"
        );
    }
}

/// The merge holds whatever the asset's identifier looks like and however many terms are
/// stacked onto it, up to the modelled bound: the rows differ along the form and length of
/// the identifier and along how many associations build the set.
#[tokio::test]
async fn the_merge_holds_across_identifiers_and_up_to_the_modelled_bound() {
    let router = emulator();
    let form = form_type(&router, "StackForm").await;
    let asset_type_id = asset_type(&router, "StackAssetType", &form).await;
    let glossary_id = glossary(&router, "StackGlossary").await;

    let longest = "a".repeat(MAX_ASSET_ID_LENGTH);
    // (case, asset identifier, how many terms to associate, terms per request)
    let rows: Vec<(&str, String, usize, usize)> = vec![
        ("a one-character identifier", "a".to_string(), 1, 1),
        (
            "an identifier carrying every character class the pattern allows",
            "Order-2:v1/p.0_x*".to_string(),
            3,
            3,
        ),
        (
            "an identifier at the modelled length bound",
            longest,
            2,
            1,
        ),
        (
            "the modelled maximum, in one request",
            "bulk-at-once".to_string(),
            MAX_ASSET_GLOSSARY_TERMS,
            MAX_ASSET_GLOSSARY_TERMS,
        ),
        (
            "the modelled maximum, one request at a time",
            "bulk-one-by-one".to_string(),
            MAX_ASSET_GLOSSARY_TERMS,
            1,
        ),
    ];

    for (case, identifier, term_count, per_request) in &rows {
        put_asset(&router, identifier, &asset_type_id).await;
        let mut terms: Vec<String> = Vec::new();
        for index in 0..*term_count {
            terms.push(
                glossary_term(
                    &router,
                    &glossary_id,
                    &format!("{}Term{index}", case.replace(|c: char| !c.is_alphanumeric(), "")),
                )
                .await,
            );
        }

        for chunk in terms.chunks(*per_request) {
            associate(&router, case, identifier, chunk).await;
        }

        let read = read_terms(&router, identifier).await;
        assert_eq!(
            read, terms,
            "case {case}: expected the asset to carry {terms:?}, got {read:?}"
        );
    }
}

/// An association that would take an asset past the modelled bound on the list that
/// reports its terms is refused with `InvalidInputException` and changes nothing, while
/// re-associating terms a full asset already carries is still served.
#[tokio::test]
async fn an_association_past_the_modelled_bound_is_refused() {
    let router = emulator();
    let fixture = catalog(&router, "full-asset").await;

    let mut held: Vec<String> = Vec::new();
    for index in 0..MAX_ASSET_GLOSSARY_TERMS {
        held.push(glossary_term(&router, &fixture.glossary, &format!("Held{index}")).await);
    }
    associate(&router, "the asset filled to the bound", &fixture.asset, &held).await;
    let one_more = glossary_term(&router, &fixture.glossary, "OneMore").await;

    let (status, body) = call(
        &router,
        "AssociateGlossaryTerms",
        &json!({
            "AssetIdentifier": &fixture.asset,
            "GlossaryTermIdentifiers": [&one_more],
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case one past the bound: expected 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one past the bound: expected InvalidInputException, got {body}"
    );
    assert_eq!(
        read_terms(&router, &fixture.asset).await,
        held,
        "case one past the bound: a refused association must change nothing"
    );

    // The bound is on the union, so an association naming only terms the asset already
    // carries is served however full it is.
    let body = associate(
        &router,
        "re-associating a full asset's own terms",
        &fixture.asset,
        &held[..2],
    )
    .await;
    assert_eq!(
        reported_terms("re-associating a full asset's own terms", &body),
        held,
        "case re-associating a full asset's own terms: the set must be unchanged, got {body}"
    );
}

// ---------------------------------------------------------------------------
// An identifier that resolves to no asset
// ---------------------------------------------------------------------------

/// An `AssetIdentifier` this account holds no asset under is refused with HTTP 400
/// `EntityNotFoundException`, worded the one way this service's asset surface words that
/// miss — the same wording `GetAsset` answers the same identifier with — and nothing is
/// created under it.
///
/// The rows differ along how the identifier misses: never stored, a prefix of a stored one,
/// a stored one with one more character, a stored one recased, the identifier of a resource
/// of another kind, and one at the modelled length bound.
#[tokio::test]
async fn associating_with_an_unknown_asset_is_refused() {
    let router = emulator();
    let form = form_type(&router, "MissForm").await;
    let asset_type_id = asset_type(&router, "MissAssetType", &form).await;
    put_asset(&router, "stored-asset", &asset_type_id).await;
    let glossary_id = glossary(&router, "MissGlossary").await;
    let term = glossary_term(&router, &glossary_id, "Term").await;

    let longest = "z".repeat(MAX_ASSET_ID_LENGTH);
    let rows: Vec<(&str, String)> = vec![
        ("never stored", "no-such-asset".to_string()),
        ("a prefix of a stored identifier", "stored-asse".to_string()),
        (
            "a stored identifier with one more character",
            "stored-asset0".to_string(),
        ),
        ("a stored identifier recased", "Stored-Asset".to_string()),
        ("the identifier of an asset type", asset_type_id.clone()),
        ("the identifier of a form type", form.clone()),
        ("the identifier of a glossary term", term.clone()),
        ("an identifier at the modelled length bound", longest),
    ];

    for (case, identifier) in &rows {
        let (status, body) = call(
            &router,
            "AssociateGlossaryTerms",
            &json!({
                "AssetIdentifier": identifier,
                "GlossaryTermIdentifiers": [&term],
            }),
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
        // The refusal's `Message` member, pinned exactly: this surface's refusal text is
        // itself a JSON document carrying the sentence under a `Message` key, with the
        // identifier from the request as its one variable part.
        let expected = format!(r#"{{"Message":"Asset not found: {identifier}"}}"#);
        assert_eq!(
            error_message(&body),
            expected,
            "case {case}: expected Message {expected:?}, got {body}"
        );

        // `GetAsset` of the same identifier answers the same code and the same wording, so
        // the two readings of one miss agree.
        let (read_status, read_body) =
            call(&router, "GetAsset", &json!({ "Identifier": identifier })).await;
        assert_eq!(
            read_status, 400,
            "case {case}: GetAsset of {identifier:?} must miss too, got {read_status} with \
             {read_body}"
        );
        assert_eq!(
            error_message(&read_body),
            expected,
            "case {case}: GetAsset must word the miss the same way, got {read_body}"
        );
    }

    // The stored asset still resolves, so the misses above are misses and not a broken
    // lookup.
    let body = associate(
        &router,
        "the stored identifier",
        "stored-asset",
        std::slice::from_ref(&term),
    )
    .await;
    assert_eq!(
        reported_terms("the stored identifier", &body),
        vec![term],
        "case the stored identifier: the association must be served, got {body}"
    );
}

/// `IterableFormName` and `ItemIdentifier` name ONE target together — "Required when
/// iterableFormName is specified" (`smithy_query operation=AssociateGlossaryTerms`) — so a
/// request carrying one without the other is refused with `InvalidInputException`, and a
/// request naming an iterable form the asset does not carry is refused with the modelled
/// `EntityNotFoundException`. Nothing is associated by any of them.
#[tokio::test]
async fn an_association_naming_an_item_is_refused() {
    let router = emulator();
    let fixture = catalog(&router, "item-asset").await;
    let term = glossary_term(&router, &fixture.glossary, "ItemTerm").await;

    // (case, the members beside the asset and the terms, expected error code)
    let rows: Vec<(&str, Value, &str)> = vec![
        (
            "a form name with no item identifier",
            json!({ "IterableFormName": "Lineitems" }),
            "InvalidInputException",
        ),
        (
            "an item identifier with no form name",
            json!({ "ItemIdentifier": "line-1" }),
            "InvalidInputException",
        ),
        (
            "both, naming an iterable form the asset does not carry",
            json!({ "IterableFormName": "Lineitems", "ItemIdentifier": "line-1" }),
            "EntityNotFoundException",
        ),
        (
            "both, with a namespaced form name",
            json!({ "IterableFormName": "Orders::Lineitems", "ItemIdentifier": "line-2" }),
            "EntityNotFoundException",
        ),
    ];

    for (case, extra, expected) in &rows {
        let mut request = json!({
            "AssetIdentifier": &fixture.asset,
            "GlossaryTermIdentifiers": [&term],
        });
        for (member, value) in extra.as_object().expect("the row's members are an object") {
            request
                .as_object_mut()
                .expect("the request is an object")
                .insert(member.clone(), value.clone());
        }

        let (status, body) = call(&router, "AssociateGlossaryTerms", &request).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains(expected),
            "case {case}: expected {expected}, got {body}"
        );
        assert!(
            read_terms(&router, &fixture.asset).await.is_empty(),
            "case {case}: a refused association must associate nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// The glossary term namespace is not cross-locked with the association
// ---------------------------------------------------------------------------

/// An association and the DELETE of a term it named are independent: both are served, the
/// term's own reader then misses with the wording the business glossary surface words a
/// term miss with, and the asset still names the deleted id. The rows differ along whether
/// the deleted term is the asset's only one and along where in the set it sits.
#[tokio::test]
async fn deleting_an_associated_term_leaves_its_id_on_the_asset() {
    let router = emulator();
    let fixture = catalog(&router, "lifecycle-asset").await;

    let mut terms: Vec<String> = Vec::new();
    for name in ["First", "Middle", "Last"] {
        terms.push(glossary_term(&router, &fixture.glossary, name).await);
    }
    associate(&router, "the whole set", &fixture.asset, &terms).await;

    // (case, which of the asset's terms is deleted)
    let rows: Vec<(&str, usize)> = vec![
        ("the first of three", 0),
        ("the last of what is left", 2),
        ("the only one left", 1),
    ];

    for (case, index) in &rows {
        let doomed = terms[*index].clone();
        ok(
            &router,
            "DeleteGlossaryTerm",
            &json!({ "Identifier": &doomed }),
        )
        .await;

        // The term itself is gone, worded the one way this surface words a term miss.
        let (status, body) = call(
            &router,
            "GetGlossaryTerm",
            &json!({ "Identifier": &doomed }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 reading the deleted {doomed:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {doomed:?}, got {body}"
        );
        let expected = format!(r#"{{"Message":"Business glossary term {doomed} not found."}}"#);
        assert_eq!(
            error_message(&body),
            expected,
            "case {case}: expected Message {expected:?}, got {body}"
        );

        // ...and the asset still names every id its association wrote.
        let read = read_terms(&router, &fixture.asset).await;
        assert_eq!(
            read, terms,
            "case {case}: the asset must still carry {terms:?}, got {read:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Simultaneous associations against one asset
// ---------------------------------------------------------------------------

/// Associations fired at one asset at the same time leave state that matches exactly what
/// they reported: AT MOST ONE is refused, any refusal is the modelled
/// `ConcurrentModificationException`, every winner's term is on the asset afterwards and no
/// loser's is, and a loser retried on its own then succeeds. The rows differ along how many
/// associations are in flight.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_associations_against_one_asset_agree_with_the_state() {
    for racers in [2usize, 8] {
        let case = format!("{racers} associations at once");
        let router = Arc::new(emulator());
        let fixture = catalog(&router, "contended-asset").await;

        let mut terms: Vec<String> = Vec::new();
        for index in 0..racers {
            terms.push(glossary_term(&router, &fixture.glossary, &format!("Racer{index}")).await);
        }

        let mut racing = Vec::new();
        for term in &terms {
            let router = Arc::clone(&router);
            let asset = fixture.asset.clone();
            let term = term.clone();
            racing.push(tokio::spawn(async move {
                let (status, body) = call(
                    &router,
                    "AssociateGlossaryTerms",
                    &json!({
                        "AssetIdentifier": asset,
                        "GlossaryTermIdentifiers": [&term],
                    }),
                )
                .await;
                (term, status, body)
            }));
        }

        let mut winners: BTreeSet<String> = BTreeSet::new();
        let mut losers: Vec<String> = Vec::new();
        for racer in racing {
            let (term, status, body) = racer.await.expect("a racing association must not panic");
            if status == 200 {
                assert!(
                    reported_terms(&case, &body).contains(&term),
                    "case {case}: a winner must report its own term, got {body}"
                );
                winners.insert(term);
            } else {
                assert_eq!(
                    status, 400,
                    "case {case}: a refused association answers 400, got {status} with {body}"
                );
                assert!(
                    error_code(&body).contains("ConcurrentModificationException"),
                    "case {case}: a loser must be refused with ConcurrentModificationException, \
                     got {body}"
                );
                losers.push(term);
            }
        }

        assert!(
            losers.len() <= 1,
            "case {case}: at most one association may lose, got {losers:?}"
        );
        assert!(
            !winners.is_empty(),
            "case {case}: at least one association must be served"
        );

        // This store merges under its ONE state lock, so the round is fully serialized and
        // has no loser at all — the zero-loser outcome the recording names as correct. The
        // refusal branch above is kept because it pins the one error a refusal would have
        // to be, and this is the outcome the emulator actually produces: every racer served
        // and the asset carrying the union of what they all named.
        assert!(
            losers.is_empty(),
            "case {case}: serialized merges lose nothing, got losers {losers:?}"
        );
        let all: BTreeSet<String> = terms.iter().cloned().collect();
        assert_eq!(
            winners, all,
            "case {case}: every racer must be served, got winners {winners:?}"
        );

        // State matches the reported outcomes exactly: every winner's term is there and no
        // loser's is.
        let held: BTreeSet<String> = read_terms(&router, &fixture.asset).await.into_iter().collect();
        assert_eq!(
            held, winners,
            "case {case}: the asset must carry exactly the winners' terms, got {held:?}"
        );
    }
}
