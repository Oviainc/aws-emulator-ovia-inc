// SPDX-License-Identifier: CC0-1.0

//! The refusals a client branches on, each asserted over a SET of inputs rather than one
//! example.
//!
//! The rules pinned here:
//!
//! 1. a `ClientToken` that already stands for an export is answered by that export when
//!    the call is identical, and is `ExportConflictException` when ANY other member
//!    differs — decided before anything else is asked of the replayed request, so a
//!    conflicting replay is never answered with some other refusal, and the original
//!    export stays the only one the catalogue holds for that token;
//! 2. an `ExportTime` the table's recovery window does not hold is
//!    `InvalidExportTimeException` with ONE fixed sentence, whichever way the instant
//!    falls outside it;
//! 3. `Select=ALL_PROJECTED_ATTRIBUTES` without an `IndexName` is refused in the rule's
//!    own words, the same sentence whichever read made the request;
//! 4. a legacy condition entry given the wrong number of `AttributeValueList` entries is
//!    refused as the ARITY violation it is — the same sentence over `KeyConditions`,
//!    `QueryFilter` and `ScanFilter`, and independent of which operators the member
//!    supports;
//! 5. an index lifecycle action naming an index the table does not carry is the control
//!    plane's lookup miss, and the table's own indexes are left alone;
//! 6. a capacity move that moves NOTHING — the read/write pair already in force — is
//!    refused, for the table and for one of its indexes alike, and the table is not
//!    transitioned for it;
//! 7. a `ConditionalOperator` with no map for it to combine states one fixed sentence,
//!    whichever surface it arrived on;
//! 8. `UpdateTable`'s `AttributeDefinitions` only has to declare the key elements of the
//!    index the call adds: the stored list tracks the LIVE keys, so a declaration no
//!    schema uses is dropped rather than refused.
//!
//! Every call is driven over the wire through the same `MultiServiceRouter` the
//! emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_dynamodb_dynamodb_20120810::state;

use crate::emulator::{
    call, describe_when_active, emulator, error_code, export_arn, index_named, message, ok,
    restorable_window, REGION,
};

/// The bucket a case that is not about the destination exports to. The export's own task
/// writes nothing without that bucket, which is reported ON THE RECORD; every rule here
/// is about how the CALL is answered, so no object store is mounted.
const BUCKET: &str = "export-refusal-rules";

/// The fixed sentence a conflicting `ClientToken` replay is refused with.
const CONFLICT_MESSAGE: &str =
    "Export conflict: Duplicate request detected with conflicting parameters";

/// The fixed sentence an `ExportTime` the window does not hold is refused with.
const INVALID_EXPORT_TIME: &str = "Export Time is invalid";

/// The envelope every clause of a request-member refusal is stated inside.
const ENVELOPE: &str = "1 validation error detected: ";

/// Create one on-demand table, wait out the span the clock owes a new table, and turn
/// point-in-time recovery on — which is what an export needs a point in time from.
async fn exportable_table(router: &MultiServiceRouter, name: &str) -> String {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
    ok(
        router,
        "UpdateContinuousBackups",
        json!({
            "TableName": name,
            "PointInTimeRecoverySpecification": {"PointInTimeRecoveryEnabled": true},
        }),
    )
    .await;
    state::table_arn(REGION, name)
}

/// The window a named instant is held to ([`crate::emulator::restorable_window`]) and the
/// ARN a response's export is recorded under ([`crate::emulator::export_arn`]) come from
/// the harness every suite of this target shares.

#[tokio::test]
async fn a_client_token_replayed_with_any_changed_member_is_the_modelled_conflict() {
    let router = emulator();
    let table_arn = exportable_table(&router, "Replayed").await;

    // Rows differ in WHICH member of the replay differs from the call the token first
    // started. Each member is one the model declares for this operation
    // (`smithy_query(query="operation",operation="ExportTableToPointInTime")`), including
    // the ones the emulator writes no export data for at all — the conflict is a fact
    // about the TOKEN, so it is settled before anything is asked of the member that
    // changed.
    let (earliest, latest) = restorable_window(&router, "Replayed").await;
    let changes: [(&str, Value); 9] = [
        ("the bucket", json!({"S3Bucket": "another-bucket"})),
        ("a prefix added", json!({"S3Prefix": "nightly"})),
        ("the owner of the bucket", json!({"S3BucketOwner": "123456789012"})),
        ("the encryption asked for", json!({"S3SseAlgorithm": "AES256"})),
        ("a point in time named", json!({"ExportTime": latest})),
        ("the export format", json!({"ExportFormat": "ION"})),
        ("the kind of export", json!({"ExportType": "INCREMENTAL_EXPORT"})),
        (
            "a filter added",
            json!({"FilterSpecification": {"FilterExpression": "attribute_exists(Part)"}}),
        ),
        (
            "an incremental window added",
            json!({"IncrementalExportSpecification": {
                "ExportFromTime": earliest,
                "ExportToTime": latest,
            }}),
        ),
    ];

    for (case, change) in &changes {
        let token = format!("token-{}", case.replace(' ', "-"));
        let first = json!({"TableArn": table_arn, "S3Bucket": BUCKET, "ClientToken": token});

        let started = ok(&router, "ExportTableToPointInTime", first.clone()).await;
        let original = export_arn(&started);

        // The identical replay is the idempotent one: the same export, not a second.
        let replayed = ok(&router, "ExportTableToPointInTime", first.clone()).await;
        assert_eq!(
            export_arn(&replayed),
            original,
            "case {case:?}: an identical replay answers the export the token already \
             stands for, got {replayed}"
        );

        let mut conflicting = first.clone();
        for (member, value) in change.as_object().cloned().unwrap_or_default() {
            conflicting[member] = value;
        }
        let (status, answered) = call(&router, "ExportTableToPointInTime", conflicting).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ExportConflictException"),
            "case {case:?}: a replay that changed a member is the modelled conflict, got \
             {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            CONFLICT_MESSAGE,
            "case {case:?}: expected {CONFLICT_MESSAGE:?}, got {}",
            message(&answered)
        );
    }

    // One export per token and not one per call: every replay above started nothing.
    let listed = ok(
        &router,
        "ListExports",
        json!({"TableArn": table_arn, "MaxResults": 25}),
    )
    .await;
    let recorded = listed["ExportSummaries"]
        .as_array()
        .map(Vec::len)
        .unwrap_or_default();
    assert_eq!(
        recorded,
        changes.len(),
        "case the catalogue: one export per token and no more, got {recorded} in {listed}"
    );
}

#[tokio::test]
async fn every_export_time_outside_the_window_states_the_one_fixed_sentence() {
    let router = emulator();
    let table_arn = exportable_table(&router, "Windowless").await;
    let (earliest, latest) = restorable_window(&router, "Windowless").await;

    // Rows differ in HOW the named instant falls outside the window: long before the
    // table existed, just outside each published bound, and in the future — the three
    // classes the model's prose names, which AWS refuses alike.
    //
    // `LatestRestorableDateTime` is the window's MOVING end: it advances with the clock,
    // so the row just past it clears that bound by a margin wider than any wall clock the
    // calls below can take, rather than landing back inside the window on a slow machine.
    // The oldest point only ever moves forward, so a row before it stays before it.
    let instants: [(&str, f64); 5] = [
        ("long before the table existed", earliest - 86_400.0),
        ("just before the oldest point", earliest - 1.0),
        ("just past the newest point", latest + 120.0),
        ("an hour in the future", latest + 3_600.0),
        ("a year in the future", latest + 31_536_000.0),
    ];

    for (case, instant) in instants {
        let (status, answered) = call(
            &router,
            "ExportTableToPointInTime",
            json!({"TableArn": table_arn, "S3Bucket": BUCKET, "ExportTime": instant}),
        )
        .await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "InvalidExportTimeException"),
            "case {case:?}: expected the modelled refusal, got {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            INVALID_EXPORT_TIME,
            "case {case:?}: expected {INVALID_EXPORT_TIME:?}, got {}",
            message(&answered)
        );
    }

    // The control: a point the window DOES hold is exported, so the rows above are
    // refused for the instant they named and not for the member being named at all.
    let inside = json!({
        "TableArn": table_arn,
        "S3Bucket": BUCKET,
        "ExportTime": (earliest + latest) / 2.0,
    });
    let served = ok(&router, "ExportTableToPointInTime", inside).await;
    assert!(
        !export_arn(&served).is_empty(),
        "case a point inside the window: the export is recorded, got {served}"
    );
}

#[tokio::test]
async fn all_projected_attributes_without_an_index_is_refused_in_the_rules_own_words() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Projected",
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the base table", "Projected").await;

    // The sentence is the RULE's — "Allowed only when querying an index"
    // (`Query$Select`) — so it carries the same verb whichever read was refused.
    let reads: [(&str, &str, Value); 2] = [
        (
            "a query of the base table",
            "Query",
            json!({
                "TableName": "Projected",
                "Select": "ALL_PROJECTED_ATTRIBUTES",
                "KeyConditions": {
                    "Part": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "one"}]},
                },
            }),
        ),
        (
            "a scan of the base table",
            "Scan",
            json!({"TableName": "Projected", "Select": "ALL_PROJECTED_ATTRIBUTES"}),
        ),
    ];

    for (case, operation, body) in reads {
        let (status, answered) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            format!("{ENVELOPE}ALL_PROJECTED_ATTRIBUTES can be used only when Querying using \
                     an IndexName"),
            "case {case:?}: got {}",
            message(&answered)
        );
    }
}

#[tokio::test]
async fn a_legacy_condition_given_the_wrong_operand_count_states_the_arity_rule() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Arity",
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the filtered table", "Arity").await;

    // The operand counts each operator takes come from `ComparisonOperator`'s own doc
    // (`smithy_query(query="doc",shape="ComparisonOperator")`): `EQ`/`LE`/`LT`/`GE`/`GT`/
    // `BEGINS_WITH`/`CONTAINS`/`NOT_CONTAINS`/`NE` take one, `BETWEEN` takes two,
    // `NULL`/`NOT_NULL` take none, and `IN` takes one or more. A row sends a count that
    // is not the operator's, and the refusal states the arity rule over that operator —
    // including for operators the member it was sent in does not support at all.
    let operators: [(&str, usize); 8] = [
        ("EQ", 0),
        ("EQ", 2),
        ("BETWEEN", 1),
        ("BETWEEN", 3),
        ("NULL", 1),
        ("NOT_NULL", 2),
        ("IN", 0),
        ("BEGINS_WITH", 3),
    ];

    for (operator, count) in operators {
        let values: Vec<Value> = (0..count).map(|at| json!({"S": format!("v{at}")})).collect();
        let condition = json!({
            "ComparisonOperator": operator,
            "AttributeValueList": values,
        });
        // The three legacy members that carry a condition map: a key condition, a query's
        // result filter and a scan's. The rule is the operator's, so all three state it.
        let requests: [(&str, &str, Value); 3] = [
            (
                "KeyConditions",
                "Query",
                json!({"TableName": "Arity", "KeyConditions": {"Part": condition.clone()}}),
            ),
            (
                "QueryFilter",
                "Query",
                json!({
                    "TableName": "Arity",
                    "KeyConditions": {
                        "Part": {
                            "ComparisonOperator": "EQ",
                            "AttributeValueList": [{"S": "one"}],
                        },
                    },
                    "QueryFilter": {"Label": condition.clone()},
                }),
            ),
            (
                "ScanFilter",
                "Scan",
                json!({"TableName": "Arity", "ScanFilter": {"Label": condition.clone()}}),
            ),
        ];
        for (member, operation, body) in requests {
            let case = format!("{member}/{operator} with {count} operand(s)");
            let (status, answered) = call(&router, operation, body).await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {status} with {answered}"
            );
            assert_eq!(
                message(&answered),
                format!(
                    "One or more parameter values were invalid: Invalid number of argument(s) \
                     for the {operator} ComparisonOperator"
                ),
                "case {case:?}: got {}",
                message(&answered)
            );
        }
    }
}

/// A `ConditionalOperator` with no map for it to combine, on every surface the member
/// arrives on: the two reads whose legacy filter it combines, and the three writes whose
/// legacy `Expected` map it combines.
#[tokio::test]
async fn a_conditional_operator_with_no_map_to_combine_states_one_sentence() {
    let router = emulator();
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": "Combined",
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "the combined table", "Combined").await;

    let key = json!({"Part": {"S": "one"}});
    // Rows differ in the operation, and so in WHICH legacy map the member would have
    // combined; the sentence states the rule, so it is the same for all of them. Both
    // values the enum declares are sent, because the rule is about the member being
    // there at all.
    let calls: [(&str, Value); 5] = [
        (
            "Query",
            json!({
                "TableName": "Combined",
                "ConditionalOperator": "AND",
                "KeyConditions": {
                    "Part": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "one"}]},
                },
            }),
        ),
        (
            "Scan",
            json!({"TableName": "Combined", "ConditionalOperator": "OR"}),
        ),
        (
            "PutItem",
            json!({"TableName": "Combined", "Item": key, "ConditionalOperator": "AND"}),
        ),
        (
            "DeleteItem",
            json!({"TableName": "Combined", "Key": key, "ConditionalOperator": "OR"}),
        ),
        (
            "UpdateItem",
            json!({
                "TableName": "Combined",
                "Key": key,
                "AttributeUpdates": {"Label": {"Action": "PUT", "Value": {"S": "x"}}},
                "ConditionalOperator": "AND",
            }),
        ),
    ];

    for (operation, body) in calls {
        let (status, answered) = call(&router, operation, body).await;
        assert_eq!(
            (status, error_code(&answered).as_str(), message(&answered)),
            (
                400,
                "ValidationException",
                "ConditionalOperator cannot be used without Filter or Expected".to_string()
            ),
            "case {operation:?}: expected the one sentence the rule states, got {status} with \
             {answered}"
        );
    }
}

#[tokio::test]
async fn a_gsi_added_to_a_settled_table_reaches_active_with_the_warm_pair_it_keeps() {
    let router = emulator();

    // Rows differ in the capacity mode the table is in when the index is added, and so in
    // the pair the new index keeps warm: an index with no provisioned capacity of its own
    // keeps the documented default pair ("Warm throughput", Developer Guide), while a
    // provisioned one keeps warm the capacity it is provisioned for. They also differ in
    // the index's key shape and projection, which the rule is indifferent to.
    //
    // One row: the case, the table's billing mode, the index the update creates, the
    // table's own throughput (`Null` for an on-demand table) and the warm pair the index
    // must then report.
    type WarmRow = (&'static str, &'static str, Value, Value, (i64, i64));
    let rows: [WarmRow; 2] = [
        (
            "an on-demand table",
            "PAY_PER_REQUEST",
            json!({
                "IndexName": "WorkflowIndex",
                "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }),
            Value::Null,
            (12_000, 4_000),
        ),
        (
            "a provisioned table",
            "PROVISIONED",
            json!({
                "IndexName": "WorkflowIndex",
                "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"},
                              {"AttributeName": "Part", "KeyType": "RANGE"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 8, "WriteCapacityUnits": 2},
            }),
            json!({"ReadCapacityUnits": 4, "WriteCapacityUnits": 4}),
            (8, 2),
        ),
    ];

    for (case, mode, create, table_throughput, (warm_read, warm_write)) in rows {
        let name = format!("Workflow{mode}");
        let mut body = json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": mode,
        });
        if !table_throughput.is_null() {
            body["ProvisionedThroughput"] = table_throughput;
        }
        ok(&router, "CreateTable", body).await;
        describe_when_active(&router, case, &name).await;

        // The index is added to a table that is already ACTIVE, which is the call a
        // client makes when it needs one more access pattern.
        let updated = ok(
            &router,
            "UpdateTable",
            json!({
                "TableName": name,
                "AttributeDefinitions": [
                    {"AttributeName": "Part", "AttributeType": "S"},
                    {"AttributeName": "Owner", "AttributeType": "S"},
                ],
                "GlobalSecondaryIndexUpdates": [{"Create": create}],
            }),
        )
        .await;
        assert_eq!(
            index_named(&updated["TableDescription"], "WorkflowIndex")["IndexStatus"],
            json!("CREATING"),
            "case {case:?}: the index the call added is CREATING while it backfills, got \
             {updated}"
        );

        // The client's own waiter: poll until the index reports ACTIVE. It must get
        // there, and the pair it keeps warm is readable then.
        let started = std::time::Instant::now();
        let index = loop {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "case {case:?}: the index never reached ACTIVE"
            );
            let described = ok(&router, "DescribeTable", json!({"TableName": name})).await;
            let index = index_named(&described["Table"], "WorkflowIndex");
            if index["IndexStatus"] == json!("ACTIVE") {
                break index;
            }
            assert_eq!(
                index["IndexStatus"],
                json!("CREATING"),
                "case {case:?}: an index goes CREATING then ACTIVE and nowhere else, got \
                 {described}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        assert_eq!(
            index["WarmThroughput"],
            json!({
                "ReadUnitsPerSecond": warm_read,
                "WriteUnitsPerSecond": warm_write,
                "Status": "ACTIVE",
            }),
            "case {case:?}: the index reports the pair it keeps warm, got {index}"
        );
        // The table it was added to settles with it, and the index is usable: a query
        // through it is served from the table's items.
        let settled = describe_when_active(&router, case, &name).await;
        assert_eq!(
            index_named(&settled, "WorkflowIndex")["IndexStatus"],
            json!("ACTIVE"),
            "case {case:?}: the settled table reports the index ACTIVE, got {settled}"
        );
        ok(
            &router,
            "PutItem",
            json!({
                "TableName": name,
                "Item": {"Part": {"S": "one"}, "Owner": {"S": "me"}},
            }),
        )
        .await;
        let queried = ok(
            &router,
            "Query",
            json!({
                "TableName": name,
                "IndexName": "WorkflowIndex",
                "KeyConditions": {
                    "Owner": {"ComparisonOperator": "EQ", "AttributeValueList": [{"S": "me"}]},
                },
            }),
        )
        .await;
        assert_eq!(
            queried["Count"],
            json!(1),
            "case {case:?}: the index the update added is read through, got {queried}"
        );
    }
}

/// Create the `PROVISIONED` table the capacity rules are asserted on: its own pair, and
/// one global secondary index with a pair of its own.
async fn provisioned_table(router: &MultiServiceRouter, name: &str) {
    ok(
        router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "Part", "AttributeType": "S"},
                {"AttributeName": "Owner", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 7},
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByOwner",
                "KeySchema": [{"AttributeName": "Owner", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 4},
            }],
        }),
    )
    .await;
    describe_when_active(router, name, name).await;
}

#[tokio::test]
async fn an_index_lifecycle_action_naming_no_index_of_the_table_is_a_lookup_miss() {
    let router = emulator();
    provisioned_table(&router, "Lifecycle").await;

    // Rows differ in HOW the name misses: a name of its own, one differing only in case,
    // one with a trailing character, and the two actions that address an index — a
    // `Delete` and an `Update`, which are the same lookup.
    let names = ["by-nothing", "byowner", "ByOwnerr", "ByOwner.2"];
    // Which action addresses the index: a `Delete` and an `Update` make the same lookup.
    type Action = (&'static str, fn(&str) -> Value);
    let actions: [Action; 2] = [
        ("Delete", |index| json!({"Delete": {"IndexName": index}})),
        (
            "Update",
            |index| {
                json!({"Update": {
                    "IndexName": index,
                    "ProvisionedThroughput": {"ReadCapacityUnits": 9, "WriteCapacityUnits": 9},
                }})
            },
        ),
    ];

    for (action, build) in actions {
        for absent in names {
            let case = format!("{action} of the absent index {absent:?}");
            let (status, answered) = call(
                &router,
                "UpdateTable",
                json!({
                    "TableName": "Lifecycle",
                    "GlobalSecondaryIndexUpdates": [build(absent)],
                }),
            )
            .await;
            assert_eq!(
                (status, error_code(&answered).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: expected the control plane's lookup miss, got {status} with \
                 {answered}"
            );
            assert_eq!(
                message(&answered),
                format!("Requested resource not found: Index {absent} for table Lifecycle"),
                "case {case:?}: got {}",
                message(&answered)
            );
        }
    }

    // Nothing above changed the table: the index it does carry is still there, ACTIVE,
    // with the capacity it was created with.
    let described = ok(&router, "DescribeTable", json!({"TableName": "Lifecycle"})).await;
    let index = index_named(&described["Table"], "ByOwner");
    assert_eq!(
        (
            index["IndexStatus"].clone(),
            index["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            described["Table"]["TableStatus"].clone()
        ),
        (json!("ACTIVE"), json!(3), json!("ACTIVE")),
        "case the surviving index: a refused action leaves the table as it was, got \
         {described}"
    );
}

/// The sentence a capacity move that moves nothing is refused with, over the resource it
/// addressed and the four figures the rule reports.
fn unchanged_message(subject: &str, holder: &str, read: i64, write: i64) -> String {
    format!(
        "The provisioned throughput for {subject} will not change. The requested value equals \
         the current value. Current ReadCapacityUnits provisioned for {holder}: {read}. \
         Requested ReadCapacityUnits: {read}. Current WriteCapacityUnits provisioned for \
         {holder}: {write}. Requested WriteCapacityUnits: {write}. Refer to the Amazon \
         DynamoDB Developer Guide for current limits and how to request higher limits."
    )
}

#[tokio::test]
async fn a_capacity_pair_already_in_force_is_refused_for_the_table_and_for_its_index() {
    let router = emulator();
    provisioned_table(&router, "NoMove").await;

    // Rows differ in WHICH resource the pair was re-stated for, and in whether the
    // request restates the capacity mode beside it: the rule is about the pair, so each
    // is refused, and neither spends one of the table's counted decreases.
    let rows: [(&str, Value, String); 3] = [
        (
            "the table's own pair",
            json!({
                "TableName": "NoMove",
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 7},
            }),
            unchanged_message("the table", "the table", 5, 7),
        ),
        (
            "the table's pair beside the mode it is already in",
            json!({
                "TableName": "NoMove",
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 7},
            }),
            unchanged_message("the table", "the table", 5, 7),
        ),
        (
            "an index's pair",
            json!({
                "TableName": "NoMove",
                "GlobalSecondaryIndexUpdates": [{"Update": {
                    "IndexName": "ByOwner",
                    "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 4},
                }}],
            }),
            unchanged_message("the index ByOwner", "the index", 3, 4),
        ),
    ];

    for (case, body, expected) in rows {
        let (status, answered) = call(&router, "UpdateTable", body).await;
        assert_eq!(
            (status, error_code(&answered).as_str()),
            (400, "ValidationException"),
            "case {case:?}: a move that moves nothing is refused, got {status} with {answered}"
        );
        assert_eq!(
            message(&answered),
            expected,
            "case {case:?}: got {}",
            message(&answered)
        );
        // "AWS never transitions the table for a no-op": the refused call left it
        // settled, so the next control-plane call is served rather than conflicting.
        let described = ok(&router, "DescribeTable", json!({"TableName": "NoMove"})).await;
        assert_eq!(
            described["Table"]["TableStatus"],
            json!("ACTIVE"),
            "case {case:?}: the refused update must not transition the table, got {described}"
        );
    }

    // The control: a pair that DOES differ — in one dimension, then in both — is applied,
    // and the pair it put in force is then the one a repeat of it is refused for.
    let moves: [(&str, i64, i64); 2] = [("one dimension", 6, 7), ("both dimensions", 9, 11)];
    for (case, read, write) in moves {
        ok(
            &router,
            "UpdateTable",
            json!({
                "TableName": "NoMove",
                "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write},
            }),
        )
        .await;
        let settled = describe_when_active(&router, case, "NoMove").await;
        assert_eq!(
            (
                settled["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
                settled["ProvisionedThroughput"]["WriteCapacityUnits"].clone()
            ),
            (json!(read), json!(write)),
            "case {case:?}: the move the request asked for is in force, got {settled}"
        );
        let (status, answered) = call(
            &router,
            "UpdateTable",
            json!({
                "TableName": "NoMove",
                "ProvisionedThroughput": {"ReadCapacityUnits": read, "WriteCapacityUnits": write},
            }),
        )
        .await;
        assert_eq!(
            (status, message(&answered)),
            (400, unchanged_message("the table", "the table", read, write)),
            "case {case:?}: re-stating the pair just applied is the no-op refusal, got \
             {status} with {answered}"
        );
    }
}

#[tokio::test]
async fn the_stored_attribute_definitions_track_the_live_keys() {
    let router = emulator();
    provisioned_table(&router, "Tracking").await;

    /// The attribute names a description declares definitions for, sorted.
    fn declared(description: &Value) -> Vec<String> {
        let mut names: Vec<String> = description["AttributeDefinitions"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|definition| {
                definition["AttributeName"].as_str().map(str::to_string)
            })
            .collect();
        names.sort();
        names
    }

    // Rows differ in WHAT the update declares beside the change it asks for: a definition
    // no schema uses, several of them, and one for the key of the index the same call
    // deletes. None of them is a contradiction — the member only has to "include the key
    // element(s) of the new index" (`UpdateTable$AttributeDefinitions`) — so each call is
    // served, and the stored list is exactly the live keys either way.
    let rows: [(&str, Value, Vec<&str>); 3] = [
        (
            "one definition no schema uses",
            json!({
                "TableName": "Tracking",
                "AttributeDefinitions": [{"AttributeName": "unused", "AttributeType": "S"}],
                "ProvisionedThroughput": {"ReadCapacityUnits": 6, "WriteCapacityUnits": 7},
            }),
            vec!["Owner", "Part"],
        ),
        (
            "several, of every type",
            json!({
                "TableName": "Tracking",
                "AttributeDefinitions": [
                    {"AttributeName": "unused", "AttributeType": "S"},
                    {"AttributeName": "counter", "AttributeType": "N"},
                    {"AttributeName": "blob", "AttributeType": "B"},
                ],
                "ProvisionedThroughput": {"ReadCapacityUnits": 7, "WriteCapacityUnits": 7},
            }),
            vec!["Owner", "Part"],
        ),
        (
            "the key of the index the same call deletes",
            json!({
                "TableName": "Tracking",
                "AttributeDefinitions": [{"AttributeName": "Owner", "AttributeType": "S"}],
                "GlobalSecondaryIndexUpdates": [{"Delete": {"IndexName": "ByOwner"}}],
            }),
            vec!["Part"],
        ),
    ];

    for (case, body, expected) in rows {
        let answered = ok(&router, "UpdateTable", body).await;
        assert_eq!(
            declared(&answered["TableDescription"]),
            expected,
            "case {case:?}: the update reports the live keys and no more, got {answered}"
        );
        let settled = describe_when_active(&router, case, "Tracking").await;
        assert_eq!(
            declared(&settled),
            expected,
            "case {case:?}: the settled table reports the same list, got {settled}"
        );
    }

    // The converse rule is untouched: an index this call CREATES must have its key
    // elements declared by this call, so a create whose key the request declares no
    // definition for is still refused — over the two lists the rule is about.
    let (status, answered) = call(
        &router,
        "UpdateTable",
        json!({
            "TableName": "Tracking",
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "GlobalSecondaryIndexUpdates": [{"Create": {
                "IndexName": "ByLabel",
                "KeySchema": [{"AttributeName": "Label", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "KEYS_ONLY"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
            }}],
        }),
    )
    .await;
    assert_eq!(
        (status, error_code(&answered).as_str(), message(&answered)),
        (
            400,
            "ValidationException",
            "One or more parameter values were invalid: Some index key attributes are not \
             defined in AttributeDefinitions. Keys: [Label], AttributeDefinitions: [Part]"
                .to_string()
        ),
        "case an index created with no definition for its key: expected the refusal naming \
         both lists, got {status} with {answered}"
    );
}
