// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue metadata form type slice: `PutFormType` and the
//! `GetFormType` that reads back what it stored.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is the HTTP behaviour and not a direct call into a handler.
//!
//! The rule under test is the one `PutFormType` documents — "Creates or updates a form
//! type" — read as: the request's `Name` is the key, so a put under a free name mints a
//! form type under a fresh identifier and a put under a name already stored replaces that
//! form type's schema while keeping its identifier, and a read of that identifier reports
//! what the latest put stored. The suites are tables whose rows differ along what the rule
//! generalises over: the form and length of the name, the form and size of the schema, and
//! whether the put carried an idempotency token.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    ComAmazonawsGlueAwsglueState, FormTypeEntry, MAX_FORM_TYPES,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, emulator_on, error_code, ok};

/// The longest `Name` the model accepts: "len 1..128" on `FormTypeName`
/// (`smithy_query operation=PutFormType`), whose pattern `^[A-Z]\w*$` is what the rows
/// below keep to.
const MAX_NAME_LENGTH: usize = 128;

/// The longest `Schema` the model accepts: "len 1..100000" on `FormTypeSchema`
/// (`smithy_query operation=PutFormType`). A row at this bound proves the whole schema
/// text is stored rather than a prefix of it.
const MAX_SCHEMA_LENGTH: usize = 100_000;

/// The widest identifier the model accepts: "len 1..256" on `FormTypeId`
/// (`smithy_query shape=FormTypeId`), which every identifier the service mints must sit
/// inside.
const MAX_FORM_TYPE_ID_LENGTH: usize = 256;

/// Put a form type and return the response body.
async fn put(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "PutFormType", body).await
}

/// The `GetFormType` response for the form type `identifier` names.
async fn get(router: &MultiServiceRouter, identifier: &str) -> Value {
    ok(router, "GetFormType", &json!({ "Identifier": identifier })).await
}

/// The `Id` of a `PutFormType`/`GetFormType` body, asserted to be a usable identifier.
fn form_type_id(case: &str, body: &Value) -> String {
    let id = body
        .get("Id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case}: the response must carry an Id, got {body}"));
    assert!(
        !id.is_empty() && id.chars().count() <= MAX_FORM_TYPE_ID_LENGTH,
        "case {case}: the Id must satisfy FormTypeId's length 1..{MAX_FORM_TYPE_ID_LENGTH}, \
         got {id:?}"
    );
    id.to_string()
}

/// A name of exactly `length` characters that satisfies `FormTypeName`'s `^[A-Z]\w*$`.
fn name_of_length(length: usize) -> String {
    let mut name = String::from("F");
    name.push_str(&"o".repeat(length - 1));
    name
}

/// The rows every round-trip suite shares: one `(case, request)` pair per input along the
/// axes the rule generalises over.
fn put_rows() -> Vec<(&'static str, Value)> {
    let long_name = name_of_length(MAX_NAME_LENGTH);
    // The schema is opaque text to the catalog, so a row at the modelled bound is built
    // from a Smithy IDL prefix padded with a comment to exactly the maximum length.
    let prefix = "structure Big { ";
    let mut max_schema = String::from(prefix);
    max_schema.push_str(&"a".repeat(MAX_SCHEMA_LENGTH - prefix.len() - 1));
    max_schema.push('}');

    vec![
        (
            // The shape a lifecycle client sends: the two required members alone.
            "required members only",
            json!({
                "Name": "OrderForm",
                "Schema": "structure OrderForm { orderId: String }",
            }),
        ),
        (
            "a one-character name and a one-character schema",
            json!({ "Name": "F", "Schema": "x" }),
        ),
        (
            "a name carrying digits and underscores",
            json!({
                "Name": "Order_Form_2",
                "Schema": "structure Order_Form_2 { total: Integer }",
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({ "Name": long_name, "Schema": "structure Long { a: String }" }),
        ),
        (
            "a schema at the modelled length bound",
            json!({ "Name": "MaximalSchema", "Schema": max_schema }),
        ),
        (
            "a schema carrying newlines, tabs and non-ASCII text",
            json!({
                "Name": "MultilineSchema",
                "Schema": "structure Multiline {\n\t@documentation(\"añejo — café\")\n\tname: String\n}",
            }),
        ),
        (
            "an idempotency token sent with the put",
            json!({
                "Name": "TokenedForm",
                "Schema": "structure TokenedForm { id: String }",
                "ClientToken": "a-client-token",
            }),
        ),
    ]
}

// ---------------------------------------------------------------------------
// Put, then read back
// ---------------------------------------------------------------------------

/// What `PutFormType` persists is what `GetFormType` reports: the put answers with the
/// identifier it assigned plus the name and schema it stored, a read of that identifier
/// reports the same three members, and each form type is its own record.
#[tokio::test]
async fn put_form_types_are_read_back_from_the_store() {
    let router = emulator();
    let cases = put_rows();
    let mut ids: Vec<(&str, String)> = Vec::new();

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let schema = request["Schema"].as_str().expect("the case carries a Schema");

        let stored = put(&router, request).await;
        let id = form_type_id(case, &stored);
        assert_eq!(
            stored.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: PutFormType must report the stored name, got {stored}"
        );
        assert_eq!(
            stored.get("Schema").and_then(Value::as_str),
            Some(schema),
            "case {case}: PutFormType must report the stored schema verbatim, got a schema of \
             {:?} characters",
            stored.get("Schema").and_then(Value::as_str).map(str::len)
        );

        // The put's own identifier addresses the record it wrote, and the read reports
        // every member of it.
        let read = get(&router, &id).await;
        assert_eq!(
            (
                read.get("Id").and_then(Value::as_str),
                read.get("Name").and_then(Value::as_str),
                read.get("Schema").and_then(Value::as_str)
            ),
            (Some(id.as_str()), Some(name), Some(schema)),
            "case {case}: GetFormType must report the form type the put stored, got {read}"
        );

        // Reading it again reports the same identifier: the id is stored, not minted per
        // read.
        let again = get(&router, &id).await;
        assert_eq!(
            again.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: two reads of one form type must report one identifier, got {again}"
        );
        ids.push((case, id));
    }

    // Each put minted its own identifier, and each record is still its own after every
    // put has landed: no put overwrote another's.
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
            "case {case}: each form type keeps its own name, got {read}"
        );
        assert_eq!(
            read.get("Schema").and_then(Value::as_str),
            expected["Schema"].as_str(),
            "case {case}: each form type keeps its own schema"
        );
    }
    let distinct: std::collections::BTreeSet<&String> = ids.iter().map(|(_, id)| id).collect();
    assert_eq!(
        distinct.len(),
        ids.len(),
        "case every row: each form type must carry its own identifier, got {ids:?}"
    );
}

/// A put under a name the store already holds UPDATES that form type: its schema becomes
/// the one the later put sent, its identifier is the one the first put minted, and no
/// other form type changes.
///
/// The rows differ along what the update changes about the schema — a larger one, a
/// smaller one, the identical one, and one at the modelled length bound — and along
/// whether the update carried an idempotency token.
#[tokio::test]
async fn a_put_under_a_stored_name_updates_that_form_type() {
    let router = emulator();

    // A bystander form type, to prove an update is scoped to the name it names.
    let bystander = put(
        &router,
        &json!({ "Name": "Bystander", "Schema": "structure Bystander { a: String }" }),
    )
    .await;
    let bystander_id = form_type_id("bystander", &bystander);

    let max_schema = "b".repeat(MAX_SCHEMA_LENGTH);
    let cases: Vec<(&str, &str, Value, Value)> = vec![
        (
            "a longer schema",
            "GrowingForm",
            json!({ "Name": "GrowingForm", "Schema": "structure GrowingForm { a: String }" }),
            json!({
                "Name": "GrowingForm",
                "Schema": "structure GrowingForm { a: String, b: Integer, c: Timestamp }",
            }),
        ),
        (
            "a shorter schema",
            "ShrinkingForm",
            json!({
                "Name": "ShrinkingForm",
                "Schema": "structure ShrinkingForm { a: String, b: Integer }",
            }),
            json!({ "Name": "ShrinkingForm", "Schema": "structure ShrinkingForm {}" }),
        ),
        (
            "the identical schema",
            "UnchangedForm",
            json!({ "Name": "UnchangedForm", "Schema": "structure UnchangedForm { a: String }" }),
            json!({ "Name": "UnchangedForm", "Schema": "structure UnchangedForm { a: String }" }),
        ),
        (
            "a schema at the modelled length bound",
            "MaximalUpdate",
            json!({ "Name": "MaximalUpdate", "Schema": "structure MaximalUpdate { a: String }" }),
            json!({ "Name": "MaximalUpdate", "Schema": max_schema }),
        ),
        (
            "an update carrying an idempotency token",
            "TokenedUpdate",
            json!({ "Name": "TokenedUpdate", "Schema": "structure TokenedUpdate { a: String }" }),
            json!({
                "Name": "TokenedUpdate",
                "Schema": "structure TokenedUpdate { b: String }",
                "ClientToken": "another-client-token",
            }),
        ),
    ];

    for (case, name, first, second) in &cases {
        let created = put(&router, first).await;
        let id = form_type_id(case, &created);

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
            updated.get("Schema").and_then(Value::as_str),
            second["Schema"].as_str(),
            "case {case}: an update must report the schema it stored"
        );

        // The identifier from BEFORE the update still resolves, and resolves to the
        // updated schema: the update replaced the record rather than adding a second one.
        let read = get(&router, &id).await;
        assert_eq!(
            read.get("Schema").and_then(Value::as_str),
            second["Schema"].as_str(),
            "case {case}: a read after the update must report the updated schema"
        );
        assert_eq!(
            read.get("Id").and_then(Value::as_str),
            Some(id.as_str()),
            "case {case}: a read after the update must report the same identifier, got {read}"
        );
    }

    // Nothing an update did reached the form type it did not name.
    let read = get(&router, &bystander_id).await;
    assert_eq!(
        read.get("Schema"),
        bystander.get("Schema"),
        "case bystander: an update must change only the form type it names, got {read}"
    );
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A read for an identifier this account holds no form type under is refused with the
/// modelled `EntityNotFoundException`. The rows differ along how the identifier misses:
/// never minted, a prefix of a stored identifier, a stored identifier with one more
/// character, and a stored identifier recased.
#[tokio::test]
async fn reading_an_unknown_form_type_is_refused() {
    let router = emulator();
    let stored = put(
        &router,
        &json!({ "Name": "StoredForm", "Schema": "structure StoredForm { a: String }" }),
    )
    .await;
    let id = form_type_id("stored", &stored);

    let truncated = &id[..id.len() - 1];
    let extended = format!("{id}0");
    let recased = id.to_uppercase();
    // Every row must name an identifier the store does NOT hold. A minted identifier is
    // lowercase hexadecimal, so one drawn with no letter at all recases to itself; such a
    // row would name the stored record, so it is dropped rather than asserted.
    let rows: Vec<(&str, &str)> = [
        ("never minted", "no-such-form-type"),
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
        let (status, body) = call(&router, "GetFormType", &json!({ "Identifier": identifier })).await;
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
        Some("StoredForm"),
        "case the minted identifier: expected the stored form type, got {read}"
    );
}

/// The store holds a bounded number of form types: the put at the ceiling succeeds, the
/// one that would add a record past it is refused, an UPDATE of a stored form type still
/// succeeds at the ceiling (it adds no record), and freeing a slot lets a create succeed
/// again — so the ceiling bounds what is stored rather than latching on how many puts were
/// ever made.
#[tokio::test]
async fn a_put_that_would_grow_the_store_past_the_form_type_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would
    // exercise the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_FORM_TYPES - 1 {
            let name = format!("Seeded{index}");
            store.form_types.insert(
                name.clone(),
                FormTypeEntry::new(name, "structure Seeded {}".to_string()),
            );
        }
    }

    let last = json!({ "Name": "LastForm", "Schema": "structure LastForm { a: String }" });
    let past = json!({ "Name": "OneTooMany", "Schema": "structure OneTooMany { a: String }" });

    let created = put(&router, &last).await;
    let last_id = form_type_id("the form type at the ceiling", &created);

    let (status, body) = call(&router, "PutFormType", &past).await;
    assert_eq!(
        status,
        400,
        "case form type {} of {MAX_FORM_TYPES}: expected 400, got {status} with {body}",
        MAX_FORM_TYPES + 1
    );
    assert!(
        error_code(&body).contains("InvalidInputException"),
        "case one form type too many: expected InvalidInputException, got {body}"
    );

    // An update stores no new record, so it is served at the ceiling.
    let updated = put(
        &router,
        &json!({ "Name": "LastForm", "Schema": "structure LastForm { a: String, b: Integer }" }),
    )
    .await;
    assert_eq!(
        updated.get("Id").and_then(Value::as_str),
        Some(last_id.as_str()),
        "case an update at the ceiling: expected the stored identifier, got {updated}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().form_types.remove("Seeded0");
    }
    put(&router, &past).await;
}
