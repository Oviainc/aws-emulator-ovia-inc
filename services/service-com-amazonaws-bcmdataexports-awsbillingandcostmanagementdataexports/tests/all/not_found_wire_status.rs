// SPDX-License-Identifier: CC0-1.0

//! The HTTP STATUS this service answers the not-found condition with, over the awsJson1_1
//! wire.
//!
//! The rule under test: when an operation of this service looks a resource up and the
//! store holds nothing for it, the reply is HTTP 400 — this service's generic client-error
//! status — carrying the modelled `ResourceNotFoundException` body unchanged (`__type`,
//! and the required `Message`, `ResourceId` and `ResourceType` members). The shape's own
//! `@httpError` trait declares 404 (smithy_query shape `ResourceNotFoundException`), and
//! the recorded wire answers 400 on every operation that has such a path, so the status a
//! client branches on is 400 while the error naming the condition is still the modelled
//! one. `tests/all/not_found_message.rs` owns the BODY of that reply (its fixed `Message`
//! per addressed resource); this suite owns the status.
//!
//! It is asserted over a TABLE crossing every operation that addresses a resource
//! (`GetExport`, `DeleteExport`, `UpdateExport`, `ListTagsForResource`, `TagResource`,
//! `UntagResource`, `GetExecution`) with absent resources that differ along what the rule
//! generalises over:
//! an ARN that was never created, one the service minted itself and then deleted, an
//! export name that exists only in another region, resource segments of other forms (a
//! single character, another resource class), a foreign account, and — for `GetExecution`
//! — execution ids of several forms under an export that does exist. The recorded
//! `TagResource` case is one row of it, not the specification.
//!
//! The status is specific to that condition and is not a blanket rewrite of every status:
//! the second test keeps a success at 200 and a refused request at its own 400
//! `ValidationException` with its own code and message. The 403 of an
//! unauthorized caller is asserted in `tests/all/front_door_authorization.rs`.

use serde_json::{json, Value};

use service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::state::{
    EXECUTION_NOT_FOUND_MESSAGE, EXPORT_NOT_FOUND_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE,
    RESOURCE_NOT_TAGGABLE_MESSAGE,
};

use crate::export_lifecycle::{cases, error_code, Client, NOT_FOUND_STATUS};

/// The status this service puts on the wire for the not-found condition: recorded AWS
/// behavior is 400, the generic client-error status, and not the 404 the shape's
/// `@httpError` trait declares. Read from the one place every suite of this target states
/// it (`export_lifecycle::NOT_FOUND_STATUS`).
const NOT_FOUND_WIRE_STATUS: u16 = NOT_FOUND_STATUS;

/// The `ResourceType` a not-found about an export reports.
const EXPORT: &str = "Export";

/// The `ResourceType` a not-found about an execution reports.
const EXECUTION: &str = "Execution";

/// The 0-based position of the region segment of an ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`).
const ARN_REGION_SEGMENT: usize = 3;

/// `arn` with its region segment replaced by `region`: the same export name in a region it
/// was never created in.
fn in_region(arn: &str, region: &str) -> String {
    let mut segments: Vec<&str> = arn.split(':').collect();
    segments[ARN_REGION_SEGMENT] = region;
    segments.join(":")
}

/// An export ARN of this emulator's own account naming `resource`.
fn own_arn(resource: &str) -> String {
    format!(
        "arn:aws:bcm-data-exports:us-east-1:{}:{resource}",
        ::engine::emulator_account_id()
    )
}

/// Assert that `body`/`status` is this service's not-found reply about `resource_id`.
///
/// The `Message` is the fixed sentence of the resource KIND the lookup missed — the export
/// definition for an `ExportArn`, the tagged resource for a `ResourceArn`, the execution
/// for an `ExecutionId`; that rule is `tests/all/not_found_message.rs`'s subject and is
/// only carried along here.
fn check_not_found(
    label: &str,
    status: u16,
    body: &Value,
    resource_id: &str,
    resource_type: &str,
    message: &str,
) {
    assert_eq!(
        (status, error_code(body).as_str()),
        (NOT_FOUND_WIRE_STATUS, "ResourceNotFoundException"),
        "case {label:?}: expected HTTP {NOT_FOUND_WIRE_STATUS} ResourceNotFoundException, got {status} with {body}"
    );
    assert_eq!(
        body["Message"].as_str(),
        Some(message),
        "case {label:?}: the reply must carry this service's fixed not-found message, got {body}"
    );
    assert_eq!(
        body["ResourceId"].as_str(),
        Some(resource_id),
        "case {label:?}: the reply must name what it could not find in ResourceId, got {body}"
    );
    assert_eq!(
        body["ResourceType"].as_str(),
        Some(resource_type),
        "case {label:?}: expected ResourceType {resource_type:?}, got {body}"
    );
}

/// The tags a `TagResource` row submits: one well-formed pair, so the row fails on the
/// lookup and not on its tag list.
fn one_tag() -> Value {
    json!([{ "Key": "env", "Value": "dev" }])
}

/// Every operation that hands an ARN of THIS account to this service, for `arn`.
///
/// `GetExecution`'s row asks about an execution of `arn`: the export lookup comes first,
/// and because the resource that operation addresses is the EXECUTION it reports the
/// execution as absent while naming the ARN that resolved to nothing.
fn own_account_requests(arn: &str, export: &Value) -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "GetExport",
            json!({ "ExportArn": arn }),
            EXPORT_NOT_FOUND_MESSAGE,
        ),
        (
            "DeleteExport",
            json!({ "ExportArn": arn }),
            EXPORT_NOT_FOUND_MESSAGE,
        ),
        (
            "UpdateExport",
            json!({ "ExportArn": arn, "Export": export }),
            EXPORT_NOT_FOUND_MESSAGE,
        ),
        (
            "ListTagsForResource",
            json!({ "ResourceArn": arn }),
            RESOURCE_NOT_FOUND_MESSAGE,
        ),
        (
            "TagResource",
            json!({ "ResourceArn": arn, "ResourceTags": one_tag() }),
            RESOURCE_NOT_FOUND_MESSAGE,
        ),
        (
            "UntagResource",
            json!({ "ResourceArn": arn, "ResourceTagKeys": ["env"] }),
            RESOURCE_NOT_FOUND_MESSAGE,
        ),
        (
            "GetExecution",
            json!({ "ExportArn": arn, "ExecutionId": "00000000-0000-0000-0000-000000000000" }),
            EXECUTION_NOT_FOUND_MESSAGE,
        ),
    ]
}

/// The operations a FOREIGN-account ARN reaches the lookup through. The tagging operations
/// are excluded on purpose: they judge the ARN's account before any lookup and refuse
/// another account's ARN with the modelled `ValidationException`, which
/// `tests/all/tagging_arn_validation.rs` asserts.
fn foreign_account_requests(arn: &str, export: &Value) -> Vec<(&'static str, Value, &'static str)> {
    vec![
        (
            "GetExport",
            json!({ "ExportArn": arn }),
            EXPORT_NOT_FOUND_MESSAGE,
        ),
        (
            "DeleteExport",
            json!({ "ExportArn": arn }),
            EXPORT_NOT_FOUND_MESSAGE,
        ),
        (
            "UpdateExport",
            json!({ "ExportArn": arn, "Export": export }),
            EXPORT_NOT_FOUND_MESSAGE,
        ),
        (
            "GetExecution",
            json!({ "ExportArn": arn, "ExecutionId": "any" }),
            EXECUTION_NOT_FOUND_MESSAGE,
        ),
    ]
}

/// Over the whole table, the not-found condition answers HTTP 400 and the modelled
/// `ResourceNotFoundException`.
#[tokio::test]
async fn an_absent_resource_answers_the_services_client_error_status() {
    let client = Client::new();
    let table = cases();
    let export = table[0].create_request()["Export"].clone();

    // One export the service mints and then deletes, so the table also covers an ARN this
    // very emulator issued, and one that stays, so a live name in another region is absent.
    let deleted = client.ok("CreateExport", &table[0].create_request()).await["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();
    client
        .ok("DeleteExport", &json!({ "ExportArn": &deleted }))
        .await;
    let live = client.ok("CreateExport", &table[1].create_request()).await["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    let own_account: Vec<(String, String)> = vec![
        ("never created".to_string(), own_arn("export/never_created")),
        ("deleted export".to_string(), deleted),
        (
            "a live export name in another region".to_string(),
            in_region(&live, "eu-west-1"),
        ),
        (
            "a single-character resource".to_string(),
            own_arn("a"),
        ),
        (
            "another resource class".to_string(),
            own_arn("table/COST_AND_USAGE_REPORT"),
        ),
        (
            "a deeper resource path".to_string(),
            own_arn("export/absent/00000000-0000-0000-0000-000000000000"),
        ),
    ];
    for (row, arn) in &own_account {
        for (operation, request, message) in own_account_requests(arn, &export) {
            let label = format!("{row} / {operation}");
            let (status, body) = client.call(operation, &request).await;
            check_not_found(&label, status, &body, arn, EXPORT, message);
        }
    }

    // Foreign accounts and partitions: the lookup misses there too, with the same status.
    let foreign = [
        (
            "foreign account".to_string(),
            "arn:aws:bcm-data-exports:us-east-1:111122223333:export/isolated-test".to_string(),
        ),
        (
            "gov partition".to_string(),
            "arn:aws-us-gov:bcm-data-exports:us-gov-west-1:444455556666:export/absent".to_string(),
        ),
        (
            "china partition".to_string(),
            "arn:aws-cn:bcm-data-exports:cn-north-1:777788889999:export/absent".to_string(),
        ),
    ];
    for (row, arn) in &foreign {
        for (operation, request, message) in foreign_account_requests(arn, &export) {
            let label = format!("{row} / {operation}");
            let (status, body) = client.call(operation, &request).await;
            check_not_found(&label, status, &body, arn, EXPORT, message);
        }
    }

    // An execution id that names no execution OF AN EXPORT THAT EXISTS is the same
    // condition about a different resource: the id forms walk what `ExecutionId` admits
    // (`@length 0..1024`, `@pattern ^[\S\s]*$` — smithy_query operation `GetExecution`,
    // member `ExecutionId`).
    for id in [
        "00000000-0000-0000-0000-000000000000".to_string(),
        "not-an-execution".to_string(),
        String::new(),
        " ".to_string(),
        "x".repeat(1024),
    ] {
        let label = format!("live export / execution {id:?}");
        let (status, body) = client
            .call(
                "GetExecution",
                &json!({ "ExportArn": &live, "ExecutionId": &id }),
            )
            .await;
        check_not_found(
            &label,
            status,
            &body,
            &id,
            EXECUTION,
            EXECUTION_NOT_FOUND_MESSAGE,
        );
    }

    // None of those refusals wrote anything: the live export is still the only one, and it
    // is still readable.
    let listed = client.ok("ListExports", &json!({})).await;
    assert_eq!(
        listed["Exports"].as_array().map(Vec::len),
        Some(1),
        "a refused lookup must not create or remove an export, got {listed}"
    );
    let fetched = client.ok("GetExport", &json!({ "ExportArn": &live })).await;
    assert_eq!(
        fetched["Export"]["ExportArn"].as_str(),
        Some(live.as_str()),
        "the live export must still be readable after the refusals, got {fetched}"
    );
}

/// The status of every OTHER answer this service gives is untouched: only the not-found
/// condition is collapsed onto the client-error status.
#[tokio::test]
async fn every_other_answer_keeps_its_own_status() {
    let client = Client::new();
    let case = cases().remove(0);
    let created = client.ok("CreateExport", &case.create_request()).await;
    let arn = created["ExportArn"]
        .as_str()
        .expect("CreateExport returns an ExportArn")
        .to_string();

    // 200 — a resource that exists is served, not refused.
    let fetched = client.ok("GetExport", &json!({ "ExportArn": &arn })).await;
    assert_eq!(
        fetched["Export"]["ExportArn"].as_str(),
        Some(arn.as_str()),
        "an existing export must be reported, got {fetched}"
    );

    // 400 `ValidationException` — its own modelled status, and still its own code and its
    // own message: a refusal must not be renamed into the not-found error by the status
    // they now share.
    let foreign = "arn:aws:bcm-data-exports:us-east-1:111122223333:export/other_tenant";
    for (label, operation, request, message) in [
        (
            "a foreign ARN handed to the tagging path",
            "TagResource",
            json!({ "ResourceArn": foreign, "ResourceTags": one_tag() }),
            RESOURCE_NOT_TAGGABLE_MESSAGE.to_string(),
        ),
        (
            "a table the catalog does not publish",
            "GetTable",
            json!({ "TableName": "NOT_A_REAL_TABLE_XYZ" }),
            // The request-derived unknown-table sentence; the table of names it holds for
            // is in `tests/all/validation_message_rendering.rs`.
            "Table NOT_A_REAL_TABLE_XYZ not found".to_string(),
        ),
    ] {
        let (status, body) = client.call(operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {label:?}: expected HTTP 400 ValidationException, got {status} with {body}"
        );
        assert_eq!(
            body["Message"].as_str(),
            Some(message.as_str()),
            "case {label:?}: a refusal must keep its own message, got {body}"
        );
    }

    // 200 again — a tagging call against the resource that DOES exist succeeds, so the
    // refusals above are about the lookup and not about the tagging path as a whole.
    let (status, body) = client
        .call(
            "UntagResource",
            &json!({ "ResourceArn": &arn, "ResourceTagKeys": ["env"] }),
        )
        .await;
    assert_eq!(
        status, 200,
        "an existing resource must be untagged, got {status} with {body}"
    );
}
