// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue asset type slice: `PutAssetType` and the `GetAssetType`
//! that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test is the one `PutAssetType` documents — "Creates or updates an asset
//! type … If an asset type with the given name already exists, it is updated" — read as:
//! the request's `Name` is the key, so a put under a free name registers an asset type and
//! a put under a name already stored replaces that asset type's forms while reporting the
//! same identifier, a read of that identifier — or of the name it is stored under —
//! reports what the latest put stored, and each form entry must reference a
//! form type this account holds. The
//! suites are tables whose rows differ along what the rule generalises over: the form and
//! length of the name, the number and keys of the forms, whether a reference names its
//! form type by the minted identifier or by the name it was registered under, whether two
//! entries share one form type, and whether the put carried an idempotency token.

use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_glue_awsglue::state::{
    AssetTypeEntry, ComAmazonawsGlueAwsglueState, MAX_ASSET_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, ok};

/// The longest `Name` the model accepts: "len 1..128" on `AssetTypeName`
/// (`smithy_query operation=PutAssetType`), whose pattern
/// `^(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$` is what the rows below keep to.
const MAX_NAME_LENGTH: usize = 128;

/// The most form entries the model accepts: "len 1..100" on `AssetTypeFormsMap`
/// (`smithy_query shape=AssetTypeFormsMap`). A row at this bound proves the whole map is
/// stored rather than a prefix of it.
const MAX_FORMS: usize = 100;

/// The widest identifier the model accepts: "len 1..256" on `AssetTypeId`
/// (`smithy_query shape=AssetTypeId`), which every identifier the service mints must sit
/// inside.
const MAX_ASSET_TYPE_ID_LENGTH: usize = 256;

/// Register a form type and return the `FormTypeId` the catalog assigned it, so an asset
/// type's form entries can reference a form type this account really holds.
async fn form_type(router: &MultiServiceRouter, name: &str) -> String {
    let stored = ok(
        router,
        "PutFormType",
        &json!({ "Name": name, "Schema": format!("structure {name} {{ a: String }}") }),
    )
    .await;
    stored
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("PutFormType must report an Id, got {stored}"))
        .to_string()
}

/// A `Forms` map sending every `(form name, form type id)` pair, in the modelled
/// `AssetTypeFormReference` shape.
fn forms(entries: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (form_name, form_type_id) in entries {
        map.insert(
            (*form_name).to_string(),
            json!({ "FormTypeIdentifier": form_type_id }),
        );
    }
    Value::Object(map)
}

/// Put an asset type and return the response body.
async fn put(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "PutAssetType", body).await
}

/// The `GetAssetType` response for the asset type `identifier` names.
async fn get(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetAssetType", &json!({ "Identifier": identifier })).await
}

/// The `Id` of a `PutAssetType`/`GetAssetType` body, asserted to be a usable identifier.
fn asset_type_id(case: &str, body: &Value) -> String {
    let id = body
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the response must carry an Id, got {body}"));
    assert!(
        !id.is_empty() && id.chars().count() <= MAX_ASSET_TYPE_ID_LENGTH,
        "case {case}: the Id must satisfy AssetTypeId's length 1..{MAX_ASSET_TYPE_ID_LENGTH}, \
         got {id:?}"
    );
    id.to_string()
}

/// A name of exactly `length` characters that satisfies `AssetTypeName`'s pattern.
fn name_of_length(length: usize) -> String {
    let mut name = String::from("A");
    name.push_str(&"x".repeat(length - 1));
    name
}

// ---------------------------------------------------------------------------
// Put, then read back
// ---------------------------------------------------------------------------

/// What `PutAssetType` persists is what `GetAssetType` reports: the put answers with the
/// identifier it assigned plus the name and the forms it stored, a read of that identifier
/// reports the same three members, and each asset type is its own record.
#[tokio::test]
async fn put_asset_types_are_read_back_from_the_store() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let audit_form = form_type(&router, "AuditForm").await;

    // The resolution an identifier goes through is shared: a client that registered a form
    // type under a name addresses it by that name through the form type's own reader as
    // well as through an asset type's form reference, and reaches the record whose
    // identifier the put minted.
    let by_name = ok(&router, "GetFormType", &json!({ "Identifier": "OrderForm" })).await;
    assert_eq!(
        by_name.get("Id").and_then(Value::as_str),
        Some(order_form.as_str()),
        "case a form type read by its name: expected the minted identifier, got {by_name}"
    );

    let long_name = name_of_length(MAX_NAME_LENGTH);
    // One entry per modelled slot, every one referencing a form type this account holds:
    // several form names may share one form type, since a reference carries a form type
    // identifier and nothing else (`smithy_query shape=AssetTypeFormReference`).
    let many: Vec<(String, String)> = (0..MAX_FORMS)
        .map(|index| (format!("Form{index}"), order_form.clone()))
        .collect();
    let many_rows: Vec<(&str, &str)> = many
        .iter()
        .map(|(form_name, id)| (form_name.as_str(), id.as_str()))
        .collect();

    let cases: Vec<(&str, Value)> = vec![
        (
            // The shape a lifecycle client sends: the two required members alone.
            "one form",
            json!({ "Name": "OrderAsset", "Forms": forms(&[("Details", &order_form)]) }),
        ),
        (
            "two forms referencing two form types",
            json!({
                "Name": "AuditedOrderAsset",
                "Forms": forms(&[("Details", &order_form), ("Audit", &audit_form)]),
            }),
        ),
        (
            "two forms referencing ONE form type",
            json!({
                "Name": "TwiceFormedAsset",
                "Forms": forms(&[("Primary", &order_form), ("Secondary", &order_form)]),
            }),
        ),
        (
            "a one-character name and a one-character form key",
            json!({ "Name": "A", "Forms": forms(&[("f", &order_form)]) }),
        ),
        (
            "a name and a form key carrying digits and underscores",
            json!({
                "Name": "Order_Asset_2",
                "Forms": forms(&[("details_2", &order_form)]),
            }),
        ),
        (
            "a name starting with an underscore",
            json!({ "Name": "_internalAsset", "Forms": forms(&[("Details", &audit_form)]) }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "Name": long_name, "Forms": forms(&[("Details", &order_form)]) }),
        ),
        (
            "forms at the modelled count bound",
            json!({ "Name": "MaximalAsset", "Forms": forms(&many_rows) }),
        ),
        (
            "a form reference naming its form type by name",
            json!({ "Name": "NamedRefAsset", "Forms": forms(&[("Details", "OrderForm")]) }),
        ),
        (
            "one form referenced by name and one by minted identifier",
            json!({
                "Name": "MixedRefAsset",
                "Forms": forms(&[("ByName", "AuditForm"), ("ById", &order_form)]),
            }),
        ),
        (
            "an idempotency token sent with the put",
            json!({
                "Name": "TokenedAsset",
                "Forms": forms(&[("Details", &order_form)]),
                "ClientToken": "a-client-token",
            }),
        ),
    ];

    let mut ids: Vec<(&str, String)> = Vec::new();
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let sent_forms = &request["Forms"];

        let stored = put(&router, request).await;
        let id = asset_type_id(case, &stored);
        assert_eq!(
            stored.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: PutAssetType must report the stored name, got {stored}"
        );
        assert_eq!(
            stored.get("Forms"),
            Some(sent_forms),
            "case {case}: PutAssetType must report the forms it stored, got {:?} entries",
            stored.get("Forms").and_then(Value::as_object).map(Map::len)
        );

        // The put's own identifier addresses the record it wrote, and the read reports
        // every member of it.
        let read = get(&router, &id).await;
        assert_eq!(
            (
                read.get("Id").and_then(Value::as_str),
                read.get("Name").and_then(Value::as_str),
                read.get("Forms")
            ),
            (Some(id.as_str()), Some(name), Some(sent_forms)),
            "case {case}: GetAssetType must report the asset type the put stored, got {read}"
        );

        // Reading it again reports the same identifier: the id is stored, not minted per
        // read.
        let again = get(&router, &id).await;
        assert_eq!(
            again.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: two reads of one asset type must report one identifier, got {again}"
        );
        // A client that put the asset type under a name addresses it by that name too,
        // and reads the same record.
        let by_name = get(&router, name).await;
        assert_eq!(
            by_name, read,
            "case {case}: a read by name must report the asset type the minted identifier \
             reports, got {by_name}"
        );
        ids.push((case, id));
    }

    // Each record is still its own after every put has landed: no put overwrote another's.
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
            "case {case}: each asset type keeps its own name, got {read}"
        );
        assert_eq!(
            read.get("Forms"),
            expected.get("Forms"),
            "case {case}: each asset type keeps its own forms, got {read}"
        );
    }
    let distinct: std::collections::BTreeSet<&String> = ids.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "case every row: each asset type must carry its own identifier, got {ids:?}"
    );
}

/// A put under a name the store already holds UPDATES that asset type: its forms become
/// the ones the later put sent, its identifier is the one the first put minted, and no
/// other asset type changes.
///
/// The rows differ along what the update changes about the forms — more entries, fewer
/// entries, the identical map, a re-pointed reference, a renamed form key, and a map at
/// the modelled count bound — and along whether the update carried an idempotency token.
#[tokio::test]
async fn a_put_under_a_stored_name_updates_that_asset_type() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let audit_form = form_type(&router, "AuditForm").await;

    // A bystander asset type, to prove an update is scoped to the name it names.
    let bystander = put(
        &router,
        &json!({ "Name": "BystanderAsset", "Forms": forms(&[("Details", &order_form)]) }),
    )
    .await;
    let bystander_id = asset_type_id("bystander", &bystander);

    let many: Vec<(String, String)> = (0..MAX_FORMS)
        .map(|index| (format!("Form{index}"), audit_form.clone()))
        .collect();
    let many_rows: Vec<(&str, &str)> = many
        .iter()
        .map(|(form_name, id)| (form_name.as_str(), id.as_str()))
        .collect();

    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "more form entries",
            "GrowingAsset",
            json!({ "Name": "GrowingAsset", "Forms": forms(&[("Details", &order_form)]) }),
            json!({
                "Name": "GrowingAsset",
                "Forms": forms(&[("Details", &order_form), ("Audit", &audit_form)]),
            }),
        ),
        (
            "fewer form entries",
            "ShrinkingAsset",
            json!({
                "Name": "ShrinkingAsset",
                "Forms": forms(&[("Details", &order_form), ("Audit", &audit_form)]),
            }),
            json!({ "Name": "ShrinkingAsset", "Forms": forms(&[("Audit", &audit_form)]) }),
        ),
        (
            "the identical forms",
            "UnchangedAsset",
            json!({ "Name": "UnchangedAsset", "Forms": forms(&[("Details", &order_form)]) }),
            json!({ "Name": "UnchangedAsset", "Forms": forms(&[("Details", &order_form)]) }),
        ),
        (
            "one form key re-pointed at another form type",
            "RepointedAsset",
            json!({ "Name": "RepointedAsset", "Forms": forms(&[("Details", &order_form)]) }),
            json!({ "Name": "RepointedAsset", "Forms": forms(&[("Details", &audit_form)]) }),
        ),
        (
            "the form entry renamed",
            "RekeyedAsset",
            json!({ "Name": "RekeyedAsset", "Forms": forms(&[("Details", &order_form)]) }),
            json!({ "Name": "RekeyedAsset", "Forms": forms(&[("Summary", &order_form)]) }),
        ),
        (
            "forms at the modelled count bound",
            "MaximalUpdateAsset",
            json!({ "Name": "MaximalUpdateAsset", "Forms": forms(&[("Details", &order_form)]) }),
            json!({ "Name": "MaximalUpdateAsset", "Forms": forms(&many_rows) }),
        ),
        (
            "an update carrying an idempotency token",
            "TokenedUpdateAsset",
            json!({ "Name": "TokenedUpdateAsset", "Forms": forms(&[("Details", &order_form)]) }),
            json!({
                "Name": "TokenedUpdateAsset",
                "Forms": forms(&[("Details", &audit_form)]),
                "ClientToken": "another-client-token",
            }),
        ),
    ];

    for (case, name, first, second) in &cases {
        let created = put(&router, first).await;
        let id = asset_type_id(case, &created);

        let updated = put(&router, second).await;
        assert_eq!(
            updated.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: an update must keep the identifier the first put minted, got {updated}"
        );
        assert_eq!(
            updated.get("Name").and_then(Value::as_str),
            Some(*name),
            "case {case}: an update must report the name it is stored under, got {updated}"
        );
        assert_eq!(
            updated.get("Forms"),
            second.get("Forms"),
            "case {case}: an update must report the forms it stored, got {:?} entries",
            updated.get("Forms").and_then(Value::as_object).map(Map::len)
        );

        // The identifier from BEFORE the update still resolves, and resolves to the
        // updated forms: the update replaced the record rather than adding a second one.
        let read = get(&router, &id).await;
        assert_eq!(
            read.get("Forms"),
            second.get("Forms"),
            "case {case}: a read after the update must report the updated forms, got {:?} entries",
            read.get("Forms").and_then(Value::as_object).map(Map::len)
        );
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: a read after the update must report the same identifier, got {read}"
        );
    }

    // Nothing an update did reached the asset type it did not name.
    let read = get(&router, &bystander_id).await;
    assert_eq!(
        read.get("Forms"),
        bystander.get("Forms"),
        "case bystander: an update must change only the asset type it names, got {read}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A read for an identifier this account holds no asset type under is refused with the
/// modelled `EntityNotFoundException`. The rows differ along how the identifier misses:
/// never minted, a prefix of a stored identifier, a stored identifier with one more
/// character, a stored identifier recased, the stored NAME recased, and the identifier of a
/// DIFFERENT resource kind (a form type), which this namespace does not hold.
#[tokio::test]
async fn reading_an_unknown_asset_type_is_refused() {
    let router = emulator();
    let order_form = form_type(&router, "OrderForm").await;
    let stored = put(
        &router,
        &json!({ "Name": "StoredAsset", "Forms": forms(&[("Details", &order_form)]) }),
    )
    .await;
    let id = asset_type_id("stored", &stored);

    let truncated = &id[..id.len() - 1];
    let extended = format!("{id}0");
    let recased = id.to_uppercase();
    // Every row must name an identifier the store does NOT hold. The identifier is the
    // name the put registered it under, so a row built by recasing it is a miss only while
    // that name has a letter to recase; a row that comes out equal to the stored
    // identifier would name the stored record, so it is dropped rather than asserted.
    let rows: Vec<(&str, &str)> = [
        ("never minted", "no-such-asset-type"),
        ("a prefix of a stored identifier", truncated),
        ("a stored identifier with one more character", &extended),
        ("a stored identifier recased", &recased),
        ("the stored name recased", "storedasset"),
        ("the identifier of a form type", &order_form),
    ]
    .into_iter()
    .filter(|(_, identifier)| *identifier != id.as_str())
    .collect();
    assert!(
        rows.len() >= 4,
        "case the miss table: expected at least four identifiers that miss, got {rows:?}"
    );

    for (case, identifier) in rows {
        let (status, body) =
            call(&router, "GetAssetType", &json!({ "Identifier": identifier })).await;
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
        Some("StoredAsset"),
        "case the minted identifier: expected the stored asset type, got {read}"
    );
}

/// A form entry referencing a form type this account does not hold is refused with the
/// modelled `EntityNotFoundException`, and nothing is stored under the name the refused put
/// named. The rows differ along how the reference misses and along where in the map the
/// missing reference sits.
#[tokio::test]
async fn a_put_referencing_an_unregistered_form_type_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let order_form = form_type(&router, "OrderForm").await;

    // An asset type identifier is NOT a form type identifier, so a reference carrying one
    // misses as surely as an identifier that was never minted.
    let sibling = put(
        &router,
        &json!({ "Name": "SiblingAsset", "Forms": forms(&[("Details", &order_form)]) }),
    )
    .await;
    let sibling_id = asset_type_id("sibling", &sibling);
    let truncated = &order_form[..order_form.len() - 1];
    let extended = format!("{order_form}0");

    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "a form type identifier that was never minted",
            "NeverMintedRefAsset",
            forms(&[("Details", "no-such-form-type")]),
        ),
        (
            "a prefix of a registered form type identifier",
            "TruncatedRefAsset",
            forms(&[("Details", truncated)]),
        ),
        (
            "a registered form type identifier with one more character",
            "ExtendedRefAsset",
            forms(&[("Details", &extended)]),
        ),
        (
            "the identifier of an asset type rather than a form type",
            "WrongKindRefAsset",
            forms(&[("Details", &sibling_id)]),
        ),
        (
            "a form type name recased",
            "RecasedNameRefAsset",
            forms(&[("Details", "orderform")]),
        ),
        (
            "one entry of several missing, beside entries that resolve",
            "PartlyResolvingRefAsset",
            forms(&[
                ("Details", order_form.as_str()),
                ("Audit", "another-missing-form-type"),
            ]),
        ),
    ];

    for (case, name, form_map) in &rows {
        let (status, body) = call(
            &router,
            "PutAssetType",
            &json!({ "Name": name, "Forms": form_map }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException, got {body}"
        );

        // A refused put stores nothing: the name it named is still free, so a later put
        // under it would create rather than update.
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let held = state.lock().asset_types.contains_key(*name);
        assert!(
            !held,
            "case {case}: a refused put must store no asset type under {name:?}"
        );
    }

    // A put whose every reference resolves is still served, so the refusals above are the
    // reference check and not a broken write path.
    let served = put(
        &router,
        &json!({
            "Name": "ResolvingRefAsset",
            "Forms": forms(&[("Details", &order_form)]),
        }),
    )
    .await;
    let served_id = asset_type_id("every reference resolves", &served);
    let read = get(&router, &served_id).await;
    assert_eq!(
        read.get("Forms"),
        served.get("Forms"),
        "case every reference resolves: expected the stored forms, got {read}"
    );
}

/// The store holds a bounded number of asset types: the put at the ceiling succeeds, the
/// one that would add a record past it is refused, an UPDATE of a stored asset type still
/// succeeds at the ceiling (it adds no record), and freeing a slot lets a create succeed
/// again — so the ceiling bounds what is stored rather than latching on how many puts were
/// ever made.
#[tokio::test]
async fn a_put_that_would_grow_the_store_past_the_asset_type_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());
    let order_form = form_type(&router, "OrderForm").await;

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_ASSET_TYPES - 1 {
            let name = format!("Seeded{index}");
            let mut seeded_forms = std::collections::BTreeMap::new();
            seeded_forms.insert("Details".to_string(), order_form.clone());
            store
                .asset_types
                .insert(name.clone(), AssetTypeEntry::new(name, seeded_forms));
        }
    }

    let last = json!({ "Name": "LastAsset", "Forms": forms(&[("Details", &order_form)]) });
    let past = json!({ "Name": "OneTooManyAsset", "Forms": forms(&[("Details", &order_form)]) });

    let created = put(&router, &last).await;
    let last_id = asset_type_id("the asset type at the ceiling", &created);

    let (status, body) = call(&router, "PutAssetType", &past).await;
    assert_eq!(
        status,
        400,
        "case asset type {} of {MAX_ASSET_TYPES}: expected 400, got {status} with {body}",
        MAX_ASSET_TYPES + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one asset type too many: expected InvalidInputException, got {body}"
    );

    // An update stores no new record, so it is served at the ceiling.
    let updated = put(
        &router,
        &json!({
            "Name": "LastAsset",
            "Forms": forms(&[("Details", &order_form), ("Audit", &order_form)]),
        }),
    )
    .await;
    assert_eq!(
        updated.get("Id").and_then(Value::as_str),
        Some(last_id.as_str()),
        "case an update at the ceiling: expected the stored identifier, got {updated}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().asset_types.remove("Seeded0");
    }
    put(&router, &past).await;
}
