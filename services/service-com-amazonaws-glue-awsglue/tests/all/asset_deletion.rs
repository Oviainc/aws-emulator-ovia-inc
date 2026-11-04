// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAsset` and `DeleteAssetType`, the deletes of the Glue asset
//! slice `PutAsset`/`PutAssetType` write and `GetAsset`/`GetAssetType` read back.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test: a delete removes the ONE record its `Identifier` resolves to —
//! resolved the way the matching read resolves it, which for an asset is the `AssetId` its
//! put was addressed with and for an asset type is the `AssetTypeId` a put reported, which
//! IS the `AssetTypeName` it was registered under — so a later read of that record misses while
//! every other record, and every record the deleted one merely REFERRED to, still reads
//! back; and it reports no miss, because neither operation models a not-found error
//! (`smithy_query operation=DeleteAsset,DeleteAssetType` lists `AccessDeniedException`,
//! `ConcurrentModificationException`, `InternalServiceException`, `InvalidInputException`
//! and `ThrottlingException` and no `EntityNotFoundException`, unlike `GetAsset` and
//! `GetAssetType`).
//!
//! The suites are tables whose rows differ along what the rule generalises over: the form
//! and length of the identifier, which form the asset type is addressed by, whether the
//! deleted record carried glossary term associations, and how an identifier misses.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, error_code, error_message, ok};

/// The longest `Identifier` the model accepts: "len 1..1087" on `AssetId`
/// (`smithy_query operation=DeleteAsset`), whose pattern `^[a-zA-Z0-9\-\:\/\.\_\*]+$` is
/// what the rows below keep to.
const MAX_ASSET_ID_LENGTH: usize = 1087;

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

/// Register an asset type over one form type and return the identifier it reported.
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

/// Delete the asset `identifier` names, asserting the modelled 200 and the empty response.
async fn delete_asset(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(router, "DeleteAsset", &json!({ "Identifier": identifier })).await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteAsset of {identifier:?} must answer 200, got {status} with {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(Map::is_empty),
        "case {case}: DeleteAssetResponse models no members, got {body}"
    );
}

/// Delete the asset type `identifier` names, asserting the modelled 200 and the empty
/// response.
async fn delete_asset_type(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(
        router,
        "DeleteAssetType",
        &json!({ "Identifier": identifier }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: DeleteAssetType of {identifier:?} must answer 200, got {status} with {body}"
    );
    assert!(
        body.is_null() || body.as_object().is_some_and(Map::is_empty),
        "case {case}: DeleteAssetTypeResponse models no members, got {body}"
    );
}

/// Assert `identifier` resolves to no asset: 400 `EntityNotFoundException`, worded the one
/// way this service's asset surface words that miss.
async fn assert_asset_misses(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(router, "GetAsset", &json!({ "Identifier": identifier })).await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 reading the deleted asset {identifier:?}, got {status} with \
         {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
    );
    // The refusal's `Message` member, pinned exactly: this surface's refusal text is itself
    // a JSON document carrying the sentence under a `Message` key, with the identifier from
    // the request as its one variable part.
    assert_eq!(
        error_message(&body),
        format!(r#"{{"Message":"Asset not found: {identifier}"}}"#),
        "case {case}: unexpected refusal wording for {identifier:?}, got {body}"
    );
}

/// Assert `identifier` resolves to no asset type: 400 `EntityNotFoundException`.
async fn assert_asset_type_misses(router: &MultiServiceRouter, case: &str, identifier: &str) {
    let (status, body) = call(
        router,
        "GetAssetType",
        &json!({ "Identifier": identifier }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: expected 400 reading the deleted asset type {identifier:?}, got {status} \
         with {body}"
    );
    assert!(
        error_code(&body).contains("EntityNotFoundException"),
        "case {case}: expected EntityNotFoundException for {identifier:?}, got {body}"
    );
}

// ---------------------------------------------------------------------------
// DeleteAsset
// ---------------------------------------------------------------------------

/// A delete removes the asset its `Identifier` names and nothing else: a read of that
/// identifier misses, every other asset still reads back, the asset type the deleted asset
/// was an instance of and the glossary terms it was associated with are untouched, and a
/// put under the freed identifier stores the asset afresh, carrying no association of its
/// own.
///
/// The rows differ along the form and length of the identifier and along whether the
/// deleted asset carried glossary term associations.
#[tokio::test]
async fn deleting_an_asset_removes_only_the_one_it_names() {
    let router = emulator();
    let form = form_type(&router, "DeleteForm").await;
    let asset_type_id = asset_type(&router, "DeleteAssetType", &form).await;
    let glossary = ok(
        &router,
        "CreateGlossary",
        &json!({ "Name": "DeletionGlossary" }),
    )
    .await;
    let glossary_id = glossary
        .get("Id")
        .and_then(Value::as_str)
        .expect("CreateGlossary must mint an Id")
        .to_string();

    let longest = "a".repeat(MAX_ASSET_ID_LENGTH);
    // (case, identifier, how many glossary terms the asset is associated with)
    let rows: Vec<(&str, String, usize)> = vec![
        ("the shape a lifecycle client stores", "order-1".to_string(), 0),
        ("a one-character identifier", "a".to_string(), 1),
        (
            "an identifier carrying every character class the pattern allows",
            "Order-2:v1/p.0_x*".to_string(),
            2,
        ),
        (
            "an identifier at the modelled length bound",
            longest,
            0,
        ),
    ];

    // A bystander, to prove each delete is scoped to the asset it names.
    put_asset(&router, "bystander", &asset_type_id).await;

    // Every row is stored FIRST, so each delete runs against a store holding several
    // assets and has to reach exactly one of them.
    let mut associated: Vec<(usize, Vec<String>)> = Vec::new();
    for (index, (case, identifier, term_count)) in rows.iter().enumerate() {
        put_asset(&router, identifier, &asset_type_id).await;
        let mut terms: Vec<String> = Vec::new();
        for term_index in 0..*term_count {
            let created = ok(
                &router,
                "CreateGlossaryTerm",
                &json!({
                    "GlossaryIdentifier": &glossary_id,
                    "Name": format!("Term{index}_{term_index}"),
                }),
            )
            .await;
            terms.push(
                created
                    .get("Id")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| panic!("case {case}: CreateGlossaryTerm must mint an Id"))
                    .to_string(),
            );
        }
        if !terms.is_empty() {
            ok(
                &router,
                "AssociateGlossaryTerms",
                &json!({ "AssetIdentifier": identifier, "GlossaryTermIdentifiers": &terms }),
            )
            .await;
        }
        associated.push((index, terms));
    }

    for ((case, identifier, _), (_, terms)) in rows.iter().zip(associated.iter()) {
        // The record is there to begin with, so the miss asserted below is the delete's
        // doing.
        let read = ok(&router, "GetAsset", &json!({ "Identifier": identifier })).await;
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(identifier.as_str()),
            "case {case}: the asset must be readable before the delete, got {read}"
        );

        delete_asset(&router, case, identifier).await;
        assert_asset_misses(&router, case, identifier).await;

        // The records the deleted asset merely referred to are untouched: the asset type it
        // was an instance of, and every glossary term its association named.
        let asset_type_read = ok(
            &router,
            "GetAssetType",
            &json!({ "Identifier": &asset_type_id }),
        )
        .await;
        assert_eq!(
            asset_type_read.get("Id").and_then(Value::as_str),
            Some(asset_type_id.as_str()),
            "case {case}: the asset type must survive the asset's delete, got {asset_type_read}"
        );
        for term in terms {
            let term_read = ok(
                &router,
                "GetGlossaryTerm",
                &json!({ "Identifier": term }),
            )
            .await;
            assert_eq!(
                term_read.get("Id").and_then(Value::as_str),
                Some(term.as_str()),
                "case {case}: the glossary term must survive the asset's delete, got {term_read}"
            );
        }

        // A put under the freed identifier stores the asset afresh — the delete freed the
        // identifier, it did not latch it — and the fresh record carries no association,
        // because the association surface is the only writer of one.
        put_asset(&router, identifier, &asset_type_id).await;
        let reborn = ok(&router, "GetAsset", &json!({ "Identifier": identifier })).await;
        assert_eq!(
            reborn.get("Id").and_then(Value::as_str),
            Some(identifier.as_str()),
            "case {case}: a put after the delete must store the asset afresh, got {reborn}"
        );
        assert!(
            reborn.get("GlossaryTerms").is_none(),
            "case {case}: a freshly stored asset carries no glossary term, got {reborn}"
        );
    }

    // Nothing any delete did reached the asset it did not name.
    let bystander = ok(&router, "GetAsset", &json!({ "Identifier": "bystander" })).await;
    assert_eq!(
        bystander.get("Id").and_then(Value::as_str),
        Some("bystander"),
        "case bystander: a delete must leave every other asset in place, got {bystander}"
    );
}

/// A delete for an identifier this account holds no asset under is SERVED: this operation
/// models no not-found error, so the caller that asks for the state the store is already in
/// is answered as though the delete had happened, and nothing is removed.
///
/// The rows differ along how the identifier misses: never stored, a prefix of a stored one,
/// a stored one with one more character, a stored one recased, an already deleted asset,
/// and the identifier of a resource of another kind.
#[tokio::test]
async fn deleting_an_asset_the_store_does_not_hold_is_served() {
    let router = emulator();
    let form = form_type(&router, "KeptForm").await;
    let asset_type_id = asset_type(&router, "KeptAssetType", &form).await;
    put_asset(&router, "kept-asset", &asset_type_id).await;
    // An asset stored and then deleted, so one row names an address that WAS live.
    put_asset(&router, "gone-asset", &asset_type_id).await;
    delete_asset(&router, "the asset deleted up front", "gone-asset").await;

    let rows: Vec<(&str, String)> = vec![
        ("never stored", "no-such-asset".to_string()),
        ("a prefix of a stored identifier", "kept-asse".to_string()),
        (
            "a stored identifier with one more character",
            "kept-asset0".to_string(),
        ),
        ("a stored identifier recased", "Kept-Asset".to_string()),
        ("an already deleted asset", "gone-asset".to_string()),
        ("the identifier of an asset type", asset_type_id.clone()),
        ("the identifier of a form type", form.clone()),
        (
            "an identifier at the modelled length bound",
            "z".repeat(MAX_ASSET_ID_LENGTH),
        ),
    ];
    // No row may name the one asset the store holds, or it would assert a delete that
    // happened rather than one that resolved to nothing.
    for (case, identifier) in &rows {
        assert_ne!(
            identifier, "kept-asset",
            "case {case}: the row must name an identifier the store does not hold"
        );
    }

    for (case, identifier) in &rows {
        delete_asset(&router, case, identifier).await;
        // Repeating it answers the same way: the delete of something absent is not a state
        // change that a second call could fail on.
        delete_asset(&router, case, identifier).await;
    }

    // Nothing was removed by any of those calls: the asset the store does hold is still
    // readable, and so is the asset type a row addressed.
    let kept = ok(&router, "GetAsset", &json!({ "Identifier": "kept-asset" })).await;
    assert_eq!(
        kept.get("Id").and_then(Value::as_str),
        Some("kept-asset"),
        "case the stored asset: a delete that resolved to nothing must remove nothing, got {kept}"
    );
    let kept_type = ok(
        &router,
        "GetAssetType",
        &json!({ "Identifier": &asset_type_id }),
    )
    .await;
    assert_eq!(
        kept_type.get("Id").and_then(Value::as_str),
        Some(asset_type_id.as_str()),
        "case the stored asset type: an asset delete must not reach an asset type, got {kept_type}"
    );
}

// ---------------------------------------------------------------------------
// DeleteAssetType
// ---------------------------------------------------------------------------

/// A delete removes the asset type its `Identifier` resolves to and nothing else, by EITHER
/// form a client addresses one by: a read of that asset type then misses under both forms,
/// every other asset type still reads back, and the form types its `Forms` entries named
/// are untouched.
///
/// The rows differ along which form the delete addresses the asset type by — the
/// `AssetTypeId` the put reported or the `AssetTypeName` it was registered under, which are
/// one value, because the identifier an asset type is addressed by IS the name it was
/// registered under — and along how many forms the asset type holds.
#[tokio::test]
async fn deleting_an_asset_type_removes_only_the_one_it_names() {
    let router = emulator();
    let shared_form = form_type(&router, "SharedForm").await;

    /// How the row's delete addresses the asset type.
    enum AddressedBy {
        ReportedId,
        RegisteredName,
    }

    let rows: Vec<(&str, &str, AddressedBy)> = vec![
        (
            "by the identifier the put reported",
            "MintedAssetType",
            AddressedBy::ReportedId,
        ),
        (
            "by the registered name",
            "NamedAssetType",
            AddressedBy::RegisteredName,
        ),
    ];

    // A bystander, to prove each delete is scoped to the asset type it names.
    let bystander = asset_type(&router, "BystanderAssetType", &shared_form).await;

    for (case, name, addressed_by) in &rows {
        let reported = asset_type(&router, name, &shared_form).await;
        assert_eq!(
            &reported, name,
            "case {case}: the identifier a put reports is the name it registered the asset \
             type under"
        );
        let identifier = match addressed_by {
            AddressedBy::ReportedId => reported.clone(),
            AddressedBy::RegisteredName => (*name).to_string(),
        };

        delete_asset_type(&router, case, &identifier).await;

        // The record is gone under BOTH addresses, so the delete reached the record itself
        // and not one of its two names.
        assert_asset_type_misses(&router, case, &reported).await;
        assert_asset_type_misses(&router, case, name).await;

        // The form type the asset type's entries named is its own record and is untouched.
        let form_read = ok(
            &router,
            "GetFormType",
            &json!({ "Identifier": &shared_form }),
        )
        .await;
        assert_eq!(
            form_read.get("Id").and_then(Value::as_str),
            Some(shared_form.as_str()),
            "case {case}: the form type must survive the asset type's delete, got {form_read}"
        );

        // A put under the freed name stores the asset type afresh, under the same
        // identifier: the identity of an asset type is the name it is registered under, so
        // what the second put makes new is the record and not its address.
        let reborn = asset_type(&router, name, &shared_form).await;
        assert_eq!(
            &reborn, name,
            "case {case}: a put after the delete registers the same name, so it reports the \
             same identifier"
        );
        let read_again = ok(
            &router,
            "GetAssetType",
            &json!({ "Identifier": &reborn }),
        )
        .await;
        assert_eq!(
            read_again.get("Id").and_then(Value::as_str),
            Some(*name),
            "case {case}: the re-registered asset type must read back, got {read_again}"
        );
        delete_asset_type(&router, case, &reborn).await;
    }

    // Nothing any delete did reached the asset type it did not name.
    let read = ok(
        &router,
        "GetAssetType",
        &json!({ "Identifier": &bystander }),
    )
    .await;
    assert_eq!(
        read.get("Id").and_then(Value::as_str),
        Some(bystander.as_str()),
        "case bystander: a delete must leave every other asset type in place, got {read}"
    );
}

/// A delete for an identifier this account holds no asset type under is SERVED, for the
/// reason `DeleteAsset`'s is: the operation models no not-found error. The rows differ along
/// how the identifier misses.
#[tokio::test]
async fn deleting_an_asset_type_the_store_does_not_hold_is_served() {
    let router = emulator();
    let form = form_type(&router, "StillForm").await;
    let kept = asset_type(&router, "KeptType", &form).await;
    let gone = asset_type(&router, "GoneType", &form).await;
    delete_asset_type(&router, "the asset type deleted up front", &gone).await;

    let rows: Vec<(&str, String)> = vec![
        ("never registered", "NoSuchAssetType".to_string()),
        (
            "a prefix of an identifier the store holds",
            kept[..kept.len() - 1].to_string(),
        ),
        (
            "an identifier the store holds with one more character",
            format!("{kept}0"),
        ),
        ("a registered name recased", "KEPTTYPE".to_string()),
        ("an already deleted asset type", gone.clone()),
        ("the identifier of a form type", form.clone()),
        (
            "an identifier at the modelled length bound",
            "Z".repeat(256),
        ),
    ];
    for (case, identifier) in &rows {
        assert_ne!(
            identifier, &kept,
            "case {case}: the row must name an identifier the store does not hold"
        );
    }

    for (case, identifier) in &rows {
        delete_asset_type(&router, case, identifier).await;
        delete_asset_type(&router, case, identifier).await;
    }

    // Nothing was removed: the asset type the store does hold is still readable under the
    // identifier its put reported and under its registered name.
    for address in [kept.as_str(), "KeptType"] {
        let read = ok(&router, "GetAssetType", &json!({ "Identifier": address })).await;
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(kept.as_str()),
            "case {address}: a delete that resolved to nothing must remove nothing, got {read}"
        );
    }
}
