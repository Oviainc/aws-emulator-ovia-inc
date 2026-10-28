// SPDX-License-Identifier: CC0-1.0

//! `UpdateMLTransform`: tuning a machine learning transform this account already holds.
//!
//! RULE: an update addresses one transform by the `TransformId` its create minted and is a
//! PATCH over the stored definition — every member the request carries replaces the stored
//! one, every member it omits is preserved, and a later read reports the merged record and
//! the instant of the write; the three capacity members are resolved together by the rule
//! the create uses ("when the `WorkerType` field is set to a value other than `Standard`,
//! the `MaxCapacity` field is set automatically and becomes read-only"), and an id this
//! account holds no transform under is refused with the operation's modelled
//! `EntityNotFoundException` rather than creating anything.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler. The create helpers, the
//! role and the transform type are the ones [`crate::ml_transforms`] already defines, so
//! every suite over transforms registers one the same way.
//!
//! The rows differ along what the rule generalises over: which members the update names
//! (one, several, all of them), which members the transform already carried, the form of
//! the value sent (an empty description, a description at the published maximum length, a
//! timeout at the published lower bound and one past it, each spelling of a capacity), and
//! — for the refusals — the form of the id and of the role.

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    DEFAULT_ML_TRANSFORM_DPUS, DEFAULT_ML_TRANSFORM_GLUE_VERSION,
    DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES, TRANSFORM_STATUS_NOT_READY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::ml_transforms::{create_body, given_transform, listed, store_table, ROLE, TRANSFORM_TYPE};
use crate::wire::{assert_echoed, call, emulator, error_code, error_message, ok, ACCOUNT_ID};

/// One row of the patch table: the case, the members the create carried beside the required
/// ones, the members the update names, and the members the read must then report.
type PatchRow<'a> = (&'a str, Value, Value, Value);

/// One row of a refusal table: the case and the value sent for the member under test.
type RefusalRow<'a> = (&'a str, Value);

/// Read one transform back by the id its create minted.
async fn get_transform(router: &MultiServiceRouter, transform_id: &str) -> Value {
    ok(
        router,
        "GetMLTransform",
        &json!({ "TransformId": transform_id }),
    )
    .await
}

/// The entry the transform listing reports for `transform_id`, which is the projection a
/// single read has to agree with.
async fn listed_transform(router: &MultiServiceRouter, case: &str, transform_id: &str) -> Value {
    let page = ok(router, "GetMLTransforms", &json!({})).await;
    listed(&page)
        .into_iter()
        .find(|transform| transform["TransformId"].as_str() == Some(transform_id))
        .unwrap_or_else(|| panic!("case {case:?}: {transform_id} must be listed, got {page}"))
}

/// Send one update, assert it answers "the unique identifier for the transform that was
/// updated", and answer the body.
async fn update_transform(
    router: &MultiServiceRouter,
    case: &str,
    transform_id: &str,
    members: &Value,
) -> Value {
    let mut body = json!({ "TransformId": transform_id });
    if let Some(named) = members.as_object() {
        for (member, value) in named {
            body[member] = value.clone();
        }
    }
    let answered = ok(router, "UpdateMLTransform", &body).await;
    assert_eq!(
        answered["TransformId"].as_str(),
        Some(transform_id),
        "case {case:?}: an update must answer the id of the transform it updated, got {answered}"
    );
    answered
}

/// Assert that every member the read reported BEFORE the update, other than the ones the
/// row expects to move, reports the same value after it: an update patches the members it
/// names and leaves the whole rest of the record alone.
fn assert_preserved(case: &str, expected: &Value, before: &Value, after: &Value) {
    for (member, value) in before.as_object().expect("a transform read is a structure") {
        // `LastModifiedOn` is "the last point in time when this machine learning transform
        // was modified", so a write is exactly what moves it; it is asserted on its own.
        if member == "LastModifiedOn" || expected.get(member).is_some() {
            continue;
        }
        assert_eq!(
            after.get(member),
            Some(value),
            "case {case:?}: the update did not name {member}, so it must still report \
             {value}, got {after}"
        );
    }
}

#[tokio::test]
async fn an_update_replaces_the_members_it_names_and_preserves_the_rest() {
    let router = emulator();
    store_table(
        &router,
        "sales",
        "orders",
        &[("order_id", "bigint"), ("total", "double")],
    )
    .await;

    // (case, the members the create carried, the members the update names, the members the
    // read must then report)
    let rows: &[PatchRow<'_>] = &[
        (
            "only the description",
            Value::Null,
            json!({ "Description": "dedupes the order records" }),
            json!({ "Description": "dedupes the order records" }),
        ),
        (
            // "The default is an empty string" (`UpdateMLTransform$Description`): an empty
            // description is a description, stored as the create stores one.
            "an empty description",
            json!({ "Description": "replaced" }),
            json!({ "Description": "" }),
            json!({ "Description": "" }),
        ),
        (
            // `DescriptionString` is `length 0..2048`, so the longest description the model
            // accepts is stored whole.
            "a description at the published maximum length",
            Value::Null,
            json!({ "Description": "d".repeat(2048) }),
            json!({ "Description": "d".repeat(2048) }),
        ),
        (
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`).
            "only the name",
            Value::Null,
            json!({ "Name": "renamed-transform" }),
            json!({ "Name": "renamed-transform" }),
        ),
        (
            "only the Glue version",
            Value::Null,
            json!({ "GlueVersion": "3.0" }),
            json!({ "GlueVersion": "3.0" }),
        ),
        (
            // `Timeout` is `range 1..`, so one minute is the published lower bound.
            "the timeout at the published lower bound",
            json!({ "Timeout": 60 }),
            json!({ "Timeout": 1 }),
            json!({ "Timeout": 1 }),
        ),
        (
            "only the retry count",
            Value::Null,
            json!({ "MaxRetries": 4 }),
            json!({ "MaxRetries": 4 }),
        ),
        (
            // "Call this operation to tune the algorithm parameters to achieve better
            // results" (`UpdateMLTransform`): the tuned parameters replace the stored ones.
            "only the algorithm parameters",
            Value::Null,
            json!({
                "Parameters": {
                    "TransformType": TRANSFORM_TYPE,
                    "FindMatchesParameters": {
                        "PrimaryKeyColumnName": "customer_id",
                        "PrecisionRecallTradeoff": 0.8,
                        "AccuracyCostTradeoff": 0.4,
                        "EnforceProvidedLabels": true,
                    },
                },
            }),
            json!({
                "Parameters": {
                    "TransformType": TRANSFORM_TYPE,
                    "FindMatchesParameters": {
                        "PrimaryKeyColumnName": "customer_id",
                        "PrecisionRecallTradeoff": 0.8,
                        "AccuracyCostTradeoff": 0.4,
                        "EnforceProvidedLabels": true,
                    },
                },
            }),
        ),
        (
            "only the role",
            Value::Null,
            json!({ "Role": "arn:aws:iam::000000000000:role/another-glue-role" }),
            json!({ "Role": "arn:aws:iam::000000000000:role/another-glue-role" }),
        ),
        (
            // "You can allocate from 2 to 100 DPUs" (`UpdateMLTransform$MaxCapacity`), and
            // a transform carrying no worker configuration reports the figure it is given.
            "a DPU figure over the default one",
            Value::Null,
            json!({ "MaxCapacity": 48.0 }),
            json!({ "MaxCapacity": 48.0, "WorkerType": Value::Null }),
        ),
        (
            // "When the WorkerType field is set to a value other than Standard, the
            // MaxCapacity field is set automatically": a `G.2X` worker "maps to 2 DPU"
            // (`CreateJob$WorkerType`), so three of them are 6.
            "a worker configuration over a DPU figure",
            json!({ "MaxCapacity": 48.0 }),
            json!({ "WorkerType": "G.2X", "NumberOfWorkers": 3 }),
            json!({ "WorkerType": "G.2X", "NumberOfWorkers": 3, "MaxCapacity": 6.0 }),
        ),
        (
            // The same sentence's other half: with a worker type other than `Standard` set,
            // `MaxCapacity` "becomes read-only", so a figure sent against it moves nothing.
            "a DPU figure against a worker configuration",
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 5 }),
            json!({ "MaxCapacity": 99.0 }),
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 5, "MaxCapacity": 5.0 }),
        ),
        (
            // The worker count alone, over a worker type already stored: five `G.1X`
            // workers are 5 DPUs, two of them are 2.
            "a smaller worker count on the stored worker type",
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 5 }),
            json!({ "NumberOfWorkers": 2 }),
            json!({ "WorkerType": "G.1X", "NumberOfWorkers": 2, "MaxCapacity": 2.0 }),
        ),
        (
            // `Standard` is the one worker type the model gives no per-worker DPU figure
            // for, so it is excepted from the sentence above and its DPU figure is the one
            // the request names.
            "a DPU figure on the Standard worker type",
            json!({ "WorkerType": "Standard", "NumberOfWorkers": 2 }),
            json!({ "MaxCapacity": 30.0 }),
            json!({ "WorkerType": "Standard", "NumberOfWorkers": 2, "MaxCapacity": 30.0 }),
        ),
        (
            "every member the update can name",
            json!({ "Description": "replaced", "Timeout": 60, "MaxRetries": 1 }),
            json!({
                "Name": "fully-tuned",
                "Description": "every member replaced",
                "Role": "arn:aws:iam::000000000000:role/tuned-glue-role",
                "GlueVersion": "2.0",
                "Timeout": 120,
                "MaxRetries": 7,
                "WorkerType": "G.4X",
                "NumberOfWorkers": 2,
                "Parameters": {
                    "TransformType": TRANSFORM_TYPE,
                    "FindMatchesParameters": { "PrimaryKeyColumnName": "order_id" },
                },
            }),
            json!({
                "Name": "fully-tuned",
                "Description": "every member replaced",
                "Role": "arn:aws:iam::000000000000:role/tuned-glue-role",
                "GlueVersion": "2.0",
                "Timeout": 120,
                "MaxRetries": 7,
                "WorkerType": "G.4X",
                "NumberOfWorkers": 2,
                // A `G.4X` worker "maps to 4 DPU" (`CreateJob$WorkerType`).
                "MaxCapacity": 8.0,
                "Parameters": {
                    "TransformType": TRANSFORM_TYPE,
                    "FindMatchesParameters": { "PrimaryKeyColumnName": "order_id" },
                },
            }),
        ),
        (
            // `UpdateMLTransformRequest` carries no `InputRecordTables`, so the tables the
            // transform reads and the catalog schema they decide are not an update's to
            // move: the preservation check below is what pins that.
            "a transform over a catalogued table",
            json!({ "InputRecordTables": [{ "DatabaseName": "sales", "TableName": "orders" }] }),
            json!({ "Description": "over the orders table" }),
            json!({
                "Description": "over the orders table",
                "Schema": [
                    { "Name": "order_id", "DataType": "bigint" },
                    { "Name": "total", "DataType": "double" },
                ],
            }),
        ),
    ];

    for (index, (case, created, update, expected)) in rows.iter().enumerate() {
        // Each row gets its own transform, so a row's write cannot be read as the previous
        // row's.
        let id = given_transform(&router, &format!("patched-{index}"), created.clone()).await;
        let before = get_transform(&router, &id).await;

        update_transform(&router, case, &id, update).await;

        let after = get_transform(&router, &id).await;
        assert_echoed(case, expected, &after);
        assert_preserved(case, expected, &before, &after);

        // The id is the whole address of the transform and an update has no member for it,
        // so the merged record still lives under it.
        assert_eq!(
            after["TransformId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: expected the transform addressed by {id}, got {after}"
        );
        // The members no update can move: the creation instant, the status of a transform
        // nothing has taught and the label count.
        let created_on = before["CreatedOn"]
            .as_f64()
            .unwrap_or_else(|| panic!("case {case:?}: CreatedOn must be a timestamp, got {before}"));
        assert_eq!(
            after["CreatedOn"].as_f64(),
            Some(created_on),
            "case {case:?}: an update must not move the creation instant, got {after}"
        );
        assert_eq!(
            after["Status"].as_str(),
            Some(TRANSFORM_STATUS_NOT_READY),
            "case {case:?}: an update teaches the transform nothing, so its status is \
             {TRANSFORM_STATUS_NOT_READY}, got {after}"
        );
        assert_eq!(
            after["LabelCount"].as_i64(),
            Some(0),
            "case {case:?}: an update generates no labelling file, got {after}"
        );

        // The write is what moves "the last point in time when this machine learning
        // transform was modified", which was the creation instant until now.
        let last_modified = after["LastModifiedOn"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: LastModifiedOn must be a timestamp, got {after}")
        });
        assert!(
            last_modified > created_on,
            "case {case:?}: expected the update's instant to be later than the creation \
             instant {created_on}, got {last_modified}"
        );

        // One record, one projection: the single read and the listing report the same
        // merged transform member for member.
        let entry = listed_transform(&router, case, &id).await;
        assert_eq!(
            after, entry,
            "case {case:?}: a single read and the listing must report the same updated \
             transform"
        );
    }
}

#[tokio::test]
async fn an_update_naming_no_member_to_change_writes_nothing() {
    let router = emulator();

    // (case, the name, the members the create carried)
    for (case, name, created) in [
        (
            "a transform carrying only the required members",
            "untouched-bare",
            Value::Null,
        ),
        (
            "a transform carrying every optional member",
            "untouched-full",
            json!({
                "Description": "untouched",
                "GlueVersion": "2.0",
                "Timeout": 45,
                "MaxRetries": 2,
                "WorkerType": "G.1X",
                "NumberOfWorkers": 4,
            }),
        ),
    ] {
        let id = given_transform(&router, name, created).await;
        let before = get_transform(&router, &id).await;

        // Every member of the request other than the `TransformId` that addresses the
        // transform is optional, so a request naming none of them asks for no change.
        update_transform(&router, case, &id, &Value::Null).await;

        let after = get_transform(&router, &id).await;
        assert_eq!(
            after, before,
            "case {case:?}: an update naming no member must leave the transform, \
             LastModifiedOn included, exactly as it was"
        );
    }
}

#[tokio::test]
async fn an_id_this_account_holds_no_transform_under_is_refused() {
    let router = emulator();
    let stored = given_transform(&router, "stored", json!({ "Timeout": 30 })).await;
    let before = get_transform(&router, &stored).await;

    // (case, the TransformId the update addresses)
    let rows: &[RefusalRow<'_>] = &[
        (
            "an id nothing has minted",
            json!("0123456789abcdef0123456789abcdef"),
        ),
        (
            // "Names are not guaranteed unique and can be changed at any time"
            // (`MLTransform$Name`), so a name addresses no transform.
            "the stored transform's name used as an id",
            json!("stored"),
        ),
        (
            "the stored id with one character changed",
            json!(format!("{}x", &stored[..stored.len() - 1])),
        ),
        ("the stored id in upper case", json!(stored.to_uppercase())),
        (
            // The longest and shortest `TransformId` the model accepts (`HashString`,
            // `length 1..255`), so the refusal is the answer at the published bounds too.
            "the longest id the model accepts",
            json!("z".repeat(255)),
        ),
        ("the shortest id the model accepts", json!("z")),
    ];

    for (case, transform_id) in rows {
        let (status, body) = call(
            &router,
            "UpdateMLTransform",
            &json!({ "TransformId": transform_id, "Description": "written nowhere" }),
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
        // The sentence is the crate's one wording for a transform this account holds none
        // of, and its variable parts are the account and the id the request itself sent.
        assert_eq!(
            error_message(&body),
            format!(
                "Cannot find MLTransform in account {ACCOUNT_ID} with handle {}",
                transform_id.as_str().expect("the id is a string")
            ),
            "case {case:?}: expected the refusal to name the requested id, got {body}"
        );
    }

    // A refused update wrote nothing: the one transform this account holds still reads back
    // as it was, and no transform was created for the ids that addressed none.
    let after = get_transform(&router, &stored).await;
    assert_eq!(
        after, before,
        "case a refused update: the stored transform must be unchanged"
    );
    assert_eq!(
        listed(&ok(&router, "GetMLTransforms", &json!({})).await).len(),
        1,
        "case a refused update: a missed update must register no transform"
    );
}

#[tokio::test]
async fn an_update_that_would_leave_the_transform_without_a_role_is_refused() {
    let router = emulator();
    let id = given_transform(&router, "roled", Value::Null).await;
    let before = get_transform(&router, &id).await;

    // `Role` is `@required` on the create, so no update may leave a transform without one.
    // (case, the Role sent)
    let rows: &[RefusalRow<'_>] = &[
        ("an empty role", json!("")),
        ("a role of one space", json!(" ")),
        ("a role of whitespace", json!("  \t ")),
    ];

    for (case, role) in rows {
        let (status, body) = call(
            &router,
            "UpdateMLTransform",
            &json!({ "TransformId": &id, "Role": role, "Timeout": 99 }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the modelled InvalidInputException's 400, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("InvalidInputException"),
            "case {case:?}: expected InvalidInputException, got {}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            "Role is required and cannot be blank",
            "case {case:?}: expected this crate's refusal for a blank role, got {body}"
        );
    }

    // A refused update is a whole refusal: the `Timeout` it carried beside the blank role
    // was not written either.
    let after = get_transform(&router, &id).await;
    assert_eq!(
        after, before,
        "case a blank role: a refused update must write none of its members"
    );
}

#[tokio::test]
async fn a_timeout_outside_the_modelled_range_is_refused_at_the_boundary() {
    let router = emulator();
    let id = given_transform(&router, "bounded", json!({ "Timeout": 30 })).await;

    // One step past the one published bound of `Timeout` (`range 1..`).
    for (case, timeout) in [("below the lower bound", 0), ("negative", -1)] {
        let (status, body) = call(
            &router,
            "UpdateMLTransform",
            &json!({ "TransformId": &id, "Timeout": timeout }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a timeout of {timeout} is outside the modelled range, got {status} \
             with {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {case:?}: a modelled constraint is refused at the boundary, got {} with {}",
            error_code(&body),
            error_message(&body)
        );
    }

    let read = get_transform(&router, &id).await;
    assert_eq!(
        read["Timeout"].as_i64(),
        Some(30),
        "case a refused timeout: the stored timeout must be unchanged, got {read}"
    );
}

#[tokio::test]
async fn a_rename_is_not_refused_for_a_name_another_transform_already_carries() {
    let router = emulator();
    // "Names are not guaranteed unique and can be changed at any time"
    // (`MLTransform$Name`): a transform is addressed by its minted id, so an update may
    // move a name onto one another transform already carries — which is exactly what a
    // CREATE under that name is refused for.
    let first = given_transform(&router, "shared-name", Value::Null).await;
    let second = given_transform(&router, "own-name", Value::Null).await;

    update_transform(
        &router,
        "a rename onto a name in use",
        &second,
        &json!({ "Name": "shared-name" }),
    )
    .await;

    for (case, id) in [("the first transform", &first), ("the renamed one", &second)] {
        let read = get_transform(&router, id).await;
        assert_eq!(
            read["Name"].as_str(),
            Some("shared-name"),
            "case {case:?}: both transforms must report the shared name, got {read}"
        );
        assert_eq!(
            read["TransformId"].as_str(),
            Some(id.as_str()),
            "case {case:?}: each transform is still addressed by its own id, got {read}"
        );
    }

    // The name the rename VACATED is free again, so a create may take it; the name both
    // transforms now carry is not.
    let vacated = ok(&router, "CreateMLTransform", &create_body("own-name", Value::Null)).await;
    assert!(
        vacated["TransformId"].is_string(),
        "case the vacated name: a create must be able to take it, got {vacated}"
    );
    let (status, body) = call(
        &router,
        "CreateMLTransform",
        &create_body("shared-name", Value::Null),
    )
    .await;
    assert_eq!(
        status, 400,
        "case the name in use: a create naming it must be refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).ends_with("AlreadyExistsException"),
        "case the name in use: expected AlreadyExistsException, got {}",
        error_code(&body)
    );
}

#[tokio::test]
async fn an_update_leaves_the_documented_defaults_a_create_resolved_in_place() {
    let router = emulator();
    // A create resolves three documented defaults — "if the value is not set, the Glue
    // compatibility defaults to Glue 0.9" (`$GlueVersion`), "the default is 10" DPUs
    // (`$MaxCapacity`) and "the default is 2,880 minutes (48 hours)" (`$Timeout`) — and an
    // update is a patch, so a member it does not name keeps the value the create resolved
    // rather than being resolved again.
    let id = given_transform(&router, "defaulted", Value::Null).await;
    update_transform(
        &router,
        "a patch naming none of the defaulted members",
        &id,
        &json!({ "Description": "left the defaults alone" }),
    )
    .await;

    let read = get_transform(&router, &id).await;
    assert_echoed(
        "a patch naming none of the defaulted members",
        &json!({
            "GlueVersion": DEFAULT_ML_TRANSFORM_GLUE_VERSION,
            "MaxCapacity": DEFAULT_ML_TRANSFORM_DPUS,
            "Timeout": DEFAULT_ML_TRANSFORM_TIMEOUT_MINUTES,
            "Role": ROLE,
            "Description": "left the defaults alone",
        }),
        &read,
    );
}
