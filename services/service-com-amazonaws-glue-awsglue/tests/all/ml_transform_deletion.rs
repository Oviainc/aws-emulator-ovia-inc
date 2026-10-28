// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteMLTransform`: the delete that removes one machine learning
//! transform from the per-account namespace `CreateMLTransform` wrote it into.
//!
//! RULE: a delete addresses ONE transform by the `TransformId` its create minted and
//! removes that whole record — "if you no longer need a transform, you can delete it" — so
//! every later read of that id misses, the listings stop reporting it, the task runs the
//! record held are gone with it and the `Name` it carried is free for a create again; the
//! delete answers the id it removed, and an id this account holds no transform under
//! removes nothing and is refused with the operation's modelled `EntityNotFoundException`
//! at HTTP 400.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The transforms are
//! registered through [`crate::ml_transforms::given_transform`], so this suite and the
//! other transform suites create one the same way.
//!
//! The rows differ along what the rule generalises over: which members the deleted
//! transform was created with (only the required ones, a description, each spelling of a
//! capacity, tags, a catalogued input table whose columns become the transform's schema),
//! the form and length of its name, whether it had task runs — and, for the refusals, the
//! form of the `TransformId` that addresses no transform (including both published length
//! bounds of `HashString`).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ml_task_runs::start_labeling_run;
use crate::ml_transforms::{create_body, given_transform, listed, store_table, TRANSFORM_TYPE};
use crate::wire::{call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// The longest `Name` a create accepts: `NameString` is `length 1..255` (`smithy_query
/// operation=CreateMLTransform`).
const MAX_NAME_LENGTH: usize = 255;

/// The longest `TransformId` a request may carry: `HashString` is `length 1..255`
/// (`smithy_query operation=DeleteMLTransform`).
const MAX_TRANSFORM_ID_LENGTH: usize = 255;

/// One row of the deletion table: the case, the transform's name and the members its
/// create carried beside the required ones.
type DeletionRow<'a> = (&'a str, String, Value);

/// One row of a refusal table: the case and the `TransformId` the delete addresses.
type RefusalRow<'a> = (&'a str, String);

/// Delete the transform `transform_id` addresses, which must succeed, and answer the body.
async fn delete(router: &MultiServiceRouter, case: &str, transform_id: &str) -> Value {
    let deleted = ok(
        router,
        "DeleteMLTransform",
        &json!({ "TransformId": transform_id }),
    )
    .await;
    // "The unique identifier of the transform that was deleted."
    assert_eq!(
        deleted["TransformId"].as_str(),
        Some(transform_id),
        "case {case:?}: a delete must answer the id of the transform it deleted, got {deleted}"
    );
    deleted
}

/// The ids `GetMLTransforms` reports, in the order the service returned them.
async fn listed_ids(router: &MultiServiceRouter) -> Vec<String> {
    listed(&ok(router, "GetMLTransforms", &json!({})).await)
        .iter()
        .map(|transform| {
            transform["TransformId"]
                .as_str()
                .expect("MLTransform.TransformId is a string")
                .to_string()
        })
        .collect()
}

/// The ids `ListMLTransforms` reports, which is the other listing every reader resolves
/// against the same store through.
async fn listed_id_only(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(router, "ListMLTransforms", &json!({})).await;
    page["TransformIds"]
        .as_array()
        .unwrap_or_else(|| panic!("ListMLTransforms must report TransformIds, got {page}"))
        .iter()
        .map(|id| id.as_str().expect("a TransformId is a string").to_string())
        .collect()
}

/// Assert that `transform_id` resolves through neither read nor either listing, and that
/// the miss is the modelled `EntityNotFoundException` worded for a transform.
async fn assert_gone(router: &MultiServiceRouter, case: &str, transform_id: &str) {
    let (status, body) = call(
        router,
        "GetMLTransform",
        &json!({ "TransformId": transform_id }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a read after the delete must answer the modelled \
         EntityNotFoundException's 400, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("EntityNotFoundException"),
        "case {case:?}: expected EntityNotFoundException after the delete, got {}",
        error_code(&body)
    );
    assert_eq!(
        error_message(&body),
        format!("Cannot find MLTransform in account {ACCOUNT_ID} with handle {transform_id}"),
        "case {case:?}: expected the miss to name the requested id, got {body}"
    );

    let listed = listed_ids(router).await;
    assert!(
        !listed.contains(&transform_id.to_string()),
        "case {case:?}: GetMLTransforms must stop reporting {transform_id}, got {listed:?}"
    );
    let ids = listed_id_only(router).await;
    assert!(
        !ids.contains(&transform_id.to_string()),
        "case {case:?}: ListMLTransforms must stop reporting {transform_id}, got {ids:?}"
    );
}

/// The transforms every suite here deletes over: one row per axis the delete generalises
/// over, each a distinct record in the one per-account namespace.
fn deletion_rows() -> Vec<DeletionRow<'static>> {
    vec![
        (
            // The shape a lifecycle client sends: the four `@required` members alone.
            "only the required members",
            "bare-transform".to_string(),
            Value::Null,
        ),
        (
            "a description and a retry count",
            "described-transform".to_string(),
            json!({ "Description": "dedupes the order records", "MaxRetries": 3 }),
        ),
        (
            // "You can allocate from 2 to 100 DPUs" (`CreateMLTransform$MaxCapacity`).
            "a DPU figure",
            "dpu-transform".to_string(),
            json!({ "MaxCapacity": 42.0 }),
        ),
        (
            // The other spelling of a capacity: a worker type and a worker count.
            "a worker configuration",
            "worker-transform".to_string(),
            json!({ "WorkerType": "G.2X", "NumberOfWorkers": 3, "Timeout": 45 }),
        ),
        (
            "tags sent with the create",
            "tagged-transform".to_string(),
            json!({ "Tags": { "owner": "data-engineering" } }),
        ),
        (
            // A transform over a catalogued table reports that table's columns as its
            // schema, so this row deletes a record carrying one.
            "a catalogued input table",
            "catalogued-transform".to_string(),
            json!({
                "InputRecordTables": [{ "DatabaseName": "sales", "TableName": "orders" }],
                "GlueVersion": "3.0",
            }),
        ),
        (
            "a name at the published maximum length",
            "n".repeat(MAX_NAME_LENGTH),
            Value::Null,
        ),
        (
            "a name of one character",
            "x".to_string(),
            json!({ "Description": "" }),
        ),
        (
            "a name carrying a space and a tab",
            "transform one\ttwo".to_string(),
            json!({ "Description": "named with whitespace" }),
        ),
    ]
}

/// Register every row of [`deletion_rows`] and answer the id each create minted, paired
/// with its case.
async fn given_rows(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    store_table(
        router,
        "sales",
        "orders",
        &[("order_id", "bigint"), ("total", "double")],
    )
    .await;

    let mut stored = Vec::new();
    for (case, name, extra) in deletion_rows() {
        stored.push((case, given_transform(router, &name, extra).await));
    }
    stored
}

#[tokio::test]
async fn deleting_a_transform_removes_it_and_reports_its_id() {
    let router = emulator();
    let stored = given_rows(&router).await;

    for (case, id) in &stored {
        // The transform resolves BEFORE the delete, so the miss afterwards is the delete's
        // doing and not a lookup that never worked.
        let read = ok(&router, "GetMLTransform", &json!({ "TransformId": id })).await;
        assert_eq!(
            read["TransformId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: the transform must resolve before the delete, got {read}"
        );

        delete(&router, case, id).await;
        assert_gone(&router, case, id).await;
    }

    // Every record is gone: the namespace was the only place they were held.
    let listed = listed_ids(&router).await;
    assert!(
        listed.is_empty(),
        "case every transform deleted: expected an empty listing, got {listed:?}"
    );
}

#[tokio::test]
async fn a_delete_removes_only_the_transform_it_addresses() {
    let router = emulator();
    let stored = given_rows(&router).await;

    // What every surviving transform must still read back as: the record its own create
    // wrote, member for member.
    let mut surviving: Vec<(&str, String, Value)> = Vec::new();
    for (case, id) in &stored {
        let read = ok(&router, "GetMLTransform", &json!({ "TransformId": id })).await;
        surviving.push((case, id.clone(), read));
    }

    for (case, id) in &stored {
        delete(&router, case, id).await;
        surviving.retain(|(_, stored_id, _)| stored_id != id);

        let listed = listed_ids(&router).await;
        let mut expected: Vec<String> = surviving
            .iter()
            .map(|(_, stored_id, _)| stored_id.clone())
            .collect();
        expected.sort();
        let mut reported = listed.clone();
        reported.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: the listing must report exactly the transforms not yet deleted, \
             got {listed:?}"
        );

        for (other_case, other_id, before) in &surviving {
            let read = ok(
                &router,
                "GetMLTransform",
                &json!({ "TransformId": other_id }),
            )
            .await;
            assert_eq!(
                &read, before,
                "case {case:?} then {other_case:?}: a surviving transform must be untouched by \
                 another transform's delete, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_deleted_transforms_name_is_free_for_a_create_again() {
    let router = emulator();

    // (case, the members the second create carries)
    for (case, name, second) in [
        (
            "the same definition again",
            "recreated-identical",
            Value::Null,
        ),
        (
            "a different definition",
            "recreated-changed",
            json!({ "Description": "the second definition", "MaxCapacity": 30.0 }),
        ),
    ] {
        let first = given_transform(&router, name, json!({ "Description": "the first" })).await;
        delete(&router, case, &first).await;

        // The name the delete VACATED is free: a create under it is served rather than
        // refused with `AlreadyExistsException`, and it mints a NEW id — the delete removed
        // the record, it did not reserve the name or recycle the address.
        let created = ok(&router, "CreateMLTransform", &create_body(name, second.clone())).await;
        let second_id = created["TransformId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: a create must answer a TransformId, got {created}"))
            .to_string();
        assert_ne!(
            second_id, first,
            "case {case:?}: a create after the delete must mint its own id, got {second_id}"
        );

        let read = ok(
            &router,
            "GetMLTransform",
            &json!({ "TransformId": &second_id }),
        )
        .await;
        assert_eq!(
            read["Name"].as_str(),
            Some(name),
            "case {case:?}: the re-created transform must carry the name, got {read}"
        );
        assert_eq!(
            read["Description"].as_str(),
            second.get("Description").and_then(Value::as_str),
            "case {case:?}: the re-created transform must carry the second create's \
             description, got {read}"
        );
        // The first record is still gone: the create under its old name resurrected nothing.
        assert_gone(&router, case, &first).await;
    }
}

#[tokio::test]
async fn deleting_a_transform_removes_the_task_runs_it_held() {
    let router = emulator();

    // (case, how many task runs the transform had started of it)
    for (case, runs) in [
        ("a transform with no task run", 0),
        ("a transform with one task run", 1),
        ("a transform with several task runs", 3),
    ] {
        let id = given_transform(&router, &format!("with-{runs}-runs"), Value::Null).await;
        for run in 0..runs {
            start_labeling_run(&router, &id, &format!("s3://bucket/labels/{run}/")).await;
        }
        let history = ok(&router, "GetMLTaskRuns", &json!({ "TransformId": &id })).await;
        assert_eq!(
            history["TaskRuns"].as_array().map(Vec::len),
            Some(runs),
            "case {case:?}: the run history must hold {runs} run(s) before the delete, got \
             {history}"
        );

        delete(&router, case, &id).await;

        // A task run is addressed through its parent transform, so the history goes with
        // the record: the listing of it misses the transform rather than reporting an empty
        // history.
        let (status, body) = call(&router, "GetMLTaskRuns", &json!({ "TransformId": &id })).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the run history of a deleted transform must answer the modelled \
             EntityNotFoundException's 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException for the deleted transform, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            format!("Cannot find MLTransform in account {ACCOUNT_ID} with handle {id}"),
            "case {case:?}: expected the miss to name the deleted id, got {body}"
        );
    }
}

#[tokio::test]
async fn deleting_an_id_this_account_holds_no_transform_under_is_refused() {
    let router = emulator();
    let stored = given_transform(&router, "stored", json!({ "Timeout": 30 })).await;
    let before = ok(
        &router,
        "GetMLTransform",
        &json!({ "TransformId": &stored }),
    )
    .await;
    let deleted = given_transform(&router, "already-deleted", Value::Null).await;
    delete(&router, "the row that sets up an already deleted id", &deleted).await;

    let rows: &[RefusalRow<'_>] = &[
        (
            "an id nothing has minted",
            "0123456789abcdef0123456789abcdef".to_string(),
        ),
        (
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`), so a name addresses no transform.
            "the stored transform's name used as an id",
            "stored".to_string(),
        ),
        (
            "the stored id with one character changed",
            format!("{}x", &stored[..stored.len() - 1]),
        ),
        ("the stored id in upper case", stored.to_uppercase()),
        (
            "an id whose transform was already deleted",
            deleted.clone(),
        ),
        (
            "the longest id the model accepts",
            "z".repeat(MAX_TRANSFORM_ID_LENGTH),
        ),
        ("the shortest id the model accepts", "z".to_string()),
    ];

    for (case, transform_id) in rows {
        let (status, body) = call(
            &router,
            "DeleteMLTransform",
            &json!({ "TransformId": transform_id }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled EntityNotFoundException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("EntityNotFoundException"),
            "case {case:?}: expected EntityNotFoundException, got {}",
            error_code(&body)
        );
        // The miss is worded the way every read of a transform's own miss is: the sentence
        // names the account whose namespace holds none and the id the request itself sent,
        // which is the whole address of a transform.
        assert_eq!(
            error_message(&body),
            format!("Cannot find MLTransform in account {ACCOUNT_ID} with handle {transform_id}"),
            "case {case:?}: expected the refusal to name the requested id, got {body}"
        );

        // A refused delete removed nothing: the one transform this account still holds
        // reads back exactly as it did, and it is the only one listed.
        let after = ok(
            &router,
            "GetMLTransform",
            &json!({ "TransformId": &stored }),
        )
        .await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused delete must leave the stored transform untouched"
        );
        let listed = listed_ids(&router).await;
        assert_eq!(
            listed,
            vec![stored.clone()],
            "case {case:?}: a refused delete must remove nothing, got {listed:?}"
        );
    }

    // The id as minted still deletes, so the misses above are misses and not a broken
    // lookup.
    delete(&router, "the stored id", &stored).await;
    assert_gone(&router, "the stored id", &stored).await;
}

#[tokio::test]
async fn a_delete_leaves_the_rest_of_the_catalog_alone() {
    let router = emulator();
    store_table(
        &router,
        "sales",
        "orders",
        &[("order_id", "bigint"), ("total", "double")],
    )
    .await;
    let id = given_transform(
        &router,
        "over-the-orders-table",
        json!({
            "InputRecordTables": [{ "DatabaseName": "sales", "TableName": "orders" }],
            "Parameters": {
                "TransformType": TRANSFORM_TYPE,
                "FindMatchesParameters": { "PrimaryKeyColumnName": "order_id" },
            },
        }),
    )
    .await;

    delete(&router, "a transform over a catalogued table", &id).await;

    // "Any Glue jobs that still reference the deleted transform will no longer succeed" —
    // the delete removes the TRANSFORM, so the table it read is untouched by it.
    let table = ok(
        &router,
        "GetTable",
        &json!({ "DatabaseName": "sales", "Name": "orders" }),
    )
    .await;
    assert_eq!(
        table["Table"]["Name"].as_str(),
        Some("orders"),
        "case a transform over a catalogued table: the input table must survive the \
         transform's delete, got {table}"
    );
}
