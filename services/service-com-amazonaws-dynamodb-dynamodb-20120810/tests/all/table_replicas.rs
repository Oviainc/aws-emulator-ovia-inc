// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `UpdateTable`'s `ReplicaUpdates` and the legacy global-table
//! surface beside it.
//!
//! The rule these tests pin:
//!
//! > a `ReplicaUpdates` entry acts on ONE Region of a table's replication group: a
//! > `Create` makes a table of its own in the Region it names, holding the group's data
//! > and recorded by every regional table of the group (whose descriptions then report
//! > `Replicas` and the current `GlobalTableVersion`); an `Update` changes that replica's
//! > overrides; a `Delete` removes the replica and its regional table; a `Disassociate`
//! > keeps the local replica as a standalone table; and an entry that cannot apply — no
//! > action or two, one Region named twice, the table's OWN Region, or a Region the group
//! > does not hold — is refused with HTTP 400 `ValidationException`, leaving every
//! > regional table exactly as it was.
//!
//! Beside it, the LEGACY (2017.11.29) global-table operations: the version they address is
//! no longer offered, so `CreateGlobalTable` is refused with `ValidationException` and
//! every operation that addresses one global table of that version answers the modelled
//! `GlobalTableNotFoundException` — both HTTP 400 client errors, never an unmodelled 501.
//!
//! The rows of each table below differ along what the rule generalises over: the Region a
//! replica is created in, the action an entry carries, the operation that is called, and
//! whether the table the legacy call names exists, is replicated, or is absent.
//!
//! Every test drives the operations over the wire (`POST /` with
//! `X-Amz-Target: DynamoDB_20120810.<OperationName>`) through the same
//! `MultiServiceRouter` the emulator's front doors use.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::emulator::{call_in, emulator, error_code, message, ok_in, REGION};

/// Wait out one transient span, so whatever the clock owes a table or a replica is due.
async fn settle() {
    tokio::time::sleep(
        service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION
            + std::time::Duration::from_millis(150),
    )
    .await;
}

/// One table with a global secondary index, created in [`REGION`].
async fn create_table(router: &MultiServiceRouter, name: &str) -> Value {
    ok_in(
        router,
        REGION,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "Id", "AttributeType": "S"},
                {"AttributeName": "Status", "AttributeType": "S"},
            ],
            "BillingMode": "PAY_PER_REQUEST",
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByStatus",
                "KeySchema": [{"AttributeName": "Status", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
            }],
        }),
    )
    .await
}

/// The `Replicas` entry a description carries for `region`, or `Null` when it carries
/// none.
fn replica_in(description: &Value, region: &str) -> Value {
    description["Replicas"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|replica| replica["RegionName"] == json!(region))
        .unwrap_or(Value::Null)
}

/// The Regions `ListGlobalTables` reports for the group named `name`, in the ascending
/// order a page carries them in.
async fn listed_group(router: &MultiServiceRouter, name: &str) -> Vec<String> {
    let page = ok_in(router, REGION, "ListGlobalTables", json!({})).await;
    page["GlobalTables"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|table| table["GlobalTableName"] == json!(name))
        .map(|table| {
            table["ReplicationGroup"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter_map(|replica| replica["RegionName"].as_str().map(ToString::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// The two Regions of a group, in the ascending order a listing reports them in.
fn group_of(first: &str, second: &str) -> Vec<String> {
    let mut regions = vec![first.to_string(), second.to_string()];
    regions.sort();
    regions
}

/// The Regions a replica is created in, each row a Region code of a different shape — one
/// in the same continent group as [`REGION`], one in another, and one whose code carries
/// three words.
const TARGET_REGIONS: [(&str, &str); 3] = [
    ("another Region of the same continent", "us-west-2"),
    ("a Region on another continent", "eu-west-1"),
    ("a Region whose code has three words", "us-gov-east-1"),
];

#[tokio::test]
async fn a_created_replica_is_a_table_of_its_own_holding_the_group_s_data() {
    let router = emulator();
    for (case, target) in TARGET_REGIONS {
        let name = format!("Replicated.{target}");
        create_table(&router, &name).await;
        assert_eq!(
            ok_in(&router, REGION, "CreateTable", json!({
                "TableName": format!("{name}.probe"),
                "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Id", "AttributeType": "S"}],
                "BillingMode": "PAY_PER_REQUEST",
            }))
            .await["TableDescription"]["Replicas"],
            Value::Null,
            "case {case}: a table created on its own is no global table"
        );
    }
    settle().await;

    for (case, target) in TARGET_REGIONS {
        let name = format!("Replicated.{target}");
        ok_in(
            &router,
            REGION,
            "PutItem",
            json!({
                "TableName": name,
                "Item": {"Id": {"S": "before"}, "Status": {"S": "OPEN"}},
            }),
        )
        .await;
        let updated = ok_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "ReplicaUpdates": [{"Create": {"RegionName": target}}],
            }),
        )
        .await;
        let description = &updated["TableDescription"];
        assert_eq!(
            (
                replica_in(description, target)["ReplicaStatus"].clone(),
                description["GlobalTableVersion"].clone(),
                description["TableStatus"].clone(),
            ),
            (json!("CREATING"), json!("2019.11.21"), json!("UPDATING")),
            "case {case}: the replica is reported CREATING on a table now replicated, got \
             {updated}"
        );
    }
    settle().await;

    for (case, target) in TARGET_REGIONS {
        let name = format!("Replicated.{target}");
        // The replica is a table of its own in its own Region: it answers DescribeTable
        // there, with the group's schema and an ARN of that Region.
        let described = ok_in(&router, target, "DescribeTable", json!({"TableName": name})).await;
        let replica = &described["Table"];
        assert_eq!(
            (
                replica["TableStatus"].clone(),
                replica["KeySchema"].clone(),
                replica["TableArn"].as_str().unwrap_or_default().contains(target),
                replica["GlobalTableVersion"].clone(),
                replica_in(replica, REGION)["ReplicaStatus"].clone(),
            ),
            (
                json!("ACTIVE"),
                json!([{"AttributeName": "Id", "KeyType": "HASH"}]),
                true,
                json!("2019.11.21"),
                json!("ACTIVE"),
            ),
            "case {case}: the replica answers for itself and reports the group, got {described}"
        );
        // The data the group already held is in the replica, and so is its index.
        let read = ok_in(
            &router,
            target,
            "GetItem",
            json!({"TableName": name, "Key": {"Id": {"S": "before"}}}),
        )
        .await;
        let queried = ok_in(
            &router,
            target,
            "Query",
            json!({
                "TableName": name,
                "IndexName": "ByStatus",
                "KeyConditionExpression": "#s = :open",
                "ExpressionAttributeNames": {"#s": "Status"},
                "ExpressionAttributeValues": {":open": {"S": "OPEN"}},
            }),
        )
        .await;
        assert_eq!(
            (read["Item"]["Status"].clone(), queried["Count"].clone()),
            (json!({"S": "OPEN"}), json!(1)),
            "case {case}: the replica holds the group's data and indexes it, got {read} and \
             {queried}"
        );
        // The local table reports the settled replica, and the group is a global table.
        let local = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            (
                replica_in(&local["Table"], target)["ReplicaStatus"].clone(),
                replica_in(&local["Table"], target)["ReplicaArn"]
                    .as_str()
                    .unwrap_or_default()
                    .contains(target),
                listed_group(&router, &name).await,
            ),
            (
                json!("ACTIVE"),
                true,
                group_of(REGION, target),
            ),
            "case {case}: the local table reports the replica and the group is listed, got \
             {local}"
        );
    }
}

#[tokio::test]
async fn an_item_change_in_one_region_of_a_global_table_reaches_the_others() {
    let router = emulator();
    let name = "Replicated.Items";
    create_table(&router, name).await;
    settle().await;
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Create": {"RegionName": "us-west-2"}}],
        }),
    )
    .await;
    settle().await;

    // Rows differ in the write that makes the change and in the Region it is made in: a
    // global table's data is the same in every Region, whichever one the write reached.
    let cases: [(&str, &str, &str, Value, Value); 4] = [
        (
            "a put in the Region the table was created in",
            REGION,
            "PutItem",
            json!({"Item": {"Id": {"S": "one"}, "Status": {"S": "OPEN"}}}),
            json!({"S": "OPEN"}),
        ),
        (
            "a put in the replica's Region",
            "us-west-2",
            "PutItem",
            json!({"Item": {"Id": {"S": "two"}, "Status": {"S": "HELD"}}}),
            json!({"S": "HELD"}),
        ),
        (
            "an update in the replica's Region",
            "us-west-2",
            "UpdateItem",
            json!({
                "Key": {"Id": {"S": "one"}},
                "UpdateExpression": "SET #s = :done",
                "ExpressionAttributeNames": {"#s": "Status"},
                "ExpressionAttributeValues": {":done": {"S": "DONE"}},
            }),
            json!({"S": "DONE"}),
        ),
        (
            "a delete in the Region the table was created in",
            REGION,
            "DeleteItem",
            json!({"Key": {"Id": {"S": "two"}}}),
            Value::Null,
        ),
    ];

    for (case, region, operation, members, expected) in cases {
        let mut request = json!({"TableName": name});
        for (member, value) in members.as_object().cloned().unwrap_or_default() {
            request[member] = value;
        }
        ok_in(&router, region, operation, request.clone()).await;
        let key = request["Key"]
            .as_object()
            .cloned()
            .map(Value::Object)
            .unwrap_or_else(|| json!({"Id": request["Item"]["Id"].clone()}));
        // Read the change back from EVERY Region of the group, the one it was made in
        // included.
        for reader in [REGION, "us-west-2"] {
            let read = ok_in(
                &router,
                reader,
                "GetItem",
                json!({"TableName": name, "Key": key, "ConsistentRead": true}),
            )
            .await;
            let status = read
                .get("Item")
                .map(|item| item["Status"].clone())
                .unwrap_or(Value::Null);
            assert_eq!(
                status, expected,
                "case {case}: {reader} must see the change, got {read}"
            );
        }
    }
}

#[tokio::test]
async fn a_replica_action_that_cannot_apply_is_refused_and_changes_nothing() {
    let router = emulator();
    let name = "Replicated.Refusals";
    create_table(&router, name).await;
    settle().await;
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Create": {"RegionName": "us-west-2"}}],
        }),
    )
    .await;
    settle().await;

    // Rows differ in WHY the entry cannot apply: the action it carries, the Region it
    // names, how many actions or Regions one request states, and whether its per-index
    // overrides name indexes the table carries.
    let cases: [(&str, Value); 11] = [
        (
            "a Create naming the table's own Region",
            json!([{"Create": {"RegionName": REGION}}]),
        ),
        (
            "a Delete naming the table's own Region",
            json!([{"Delete": {"RegionName": REGION}}]),
        ),
        (
            "a Create naming a Region the group already holds",
            json!([{"Create": {"RegionName": "us-west-2"}}]),
        ),
        (
            "a Delete naming a Region the group does not hold",
            json!([{"Delete": {"RegionName": "eu-west-1"}}]),
        ),
        (
            "an Update naming a Region the group does not hold",
            json!([{"Update": {"RegionName": "ap-south-1"}}]),
        ),
        (
            "an entry carrying no action at all",
            json!([{}]),
        ),
        (
            "an entry carrying two actions",
            json!([{
                "Create": {"RegionName": "eu-west-1"},
                "Delete": {"RegionName": "eu-west-1"},
            }]),
        ),
        (
            "two entries naming one Region",
            json!([
                {"Create": {"RegionName": "eu-west-1"}},
                {"Delete": {"RegionName": "eu-west-1"}},
            ]),
        ),
        (
            "a Create overriding an index the table does not carry",
            json!([{"Create": {
                "RegionName": "eu-west-1",
                "GlobalSecondaryIndexes": [{"IndexName": "NoSuchIndex"}],
            }}]),
        ),
        (
            "an Update overriding an index the table does not carry",
            json!([{"Update": {
                "RegionName": "us-west-2",
                "GlobalSecondaryIndexes": [{"IndexName": "NoSuchIndex"}],
            }}]),
        ),
        (
            "an Update overriding one index twice",
            json!([{"Update": {
                "RegionName": "us-west-2",
                "GlobalSecondaryIndexes": [
                    {"IndexName": "ByStatus", "ProvisionedThroughputOverride": {"ReadCapacityUnits": 2}},
                    {"IndexName": "ByStatus", "ProvisionedThroughputOverride": {"ReadCapacityUnits": 3}},
                ],
            }}]),
        ),
    ];

    for (case, updates) in cases {
        let (status, body) = call_in(
            &router,
            REGION,
            "UpdateTable",
            json!({"TableName": name, "ReplicaUpdates": updates.clone()}),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case}: the entry cannot apply, so it is a client error, got {status} with \
             {body} for {updates}"
        );
        assert!(
            !message(&body).is_empty() && message(&body) != "<no message>",
            "case {case}: the refusal says what was wrong, got {body}"
        );
    }

    // Nothing moved: the group still spans exactly the two Regions it did, and no table
    // was created in a Region a refused entry named.
    let described = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
    let (status, absent) = call_in(&router, "eu-west-1", "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        (
            replica_in(&described["Table"], "us-west-2")["ReplicaStatus"].clone(),
            described["Table"]["Replicas"].as_array().map(Vec::len),
            listed_group(&router, name).await,
            (status, error_code(&absent)),
        ),
        (
            json!("ACTIVE"),
            Some(1),
            group_of(REGION, "us-west-2"),
            (400, "ResourceNotFoundException".to_string()),
        ),
        "case after the refusals: the group is untouched, got {described} and {absent}"
    );
}

#[tokio::test]
async fn a_replica_s_index_overrides_are_reported_and_applied_in_its_region() {
    let router = emulator();
    let name = "Replicated.IndexOverrides";
    // A PROVISIONED table, so the read capacity an override states is observable on the
    // replica's own table and index.
    ok_in(
        &router,
        REGION,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Id", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "Id", "AttributeType": "S"},
                {"AttributeName": "Status", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            "GlobalSecondaryIndexes": [{
                "IndexName": "ByStatus",
                "KeySchema": [{"AttributeName": "Status", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {"ReadCapacityUnits": 2, "WriteCapacityUnits": 2},
            }],
        }),
    )
    .await;
    settle().await;
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Create": {
                "RegionName": "us-west-2",
                "ProvisionedThroughputOverride": {"ReadCapacityUnits": 50},
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByStatus",
                    "ProvisionedThroughputOverride": {"ReadCapacityUnits": 20},
                }],
            }}],
        }),
    )
    .await;
    settle().await;

    // The group reports what the replica overrides, per index, and the replica's own
    // table and index carry the capacity the override states while the source table keeps
    // its own.
    let local = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
    let replica_table = ok_in(
        &router,
        "us-west-2",
        "DescribeTable",
        json!({"TableName": name}),
    )
    .await;
    assert_eq!(
        (
            replica_in(&local["Table"], "us-west-2")["GlobalSecondaryIndexes"].clone(),
            replica_table["Table"]["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            crate::emulator::index_named(&replica_table["Table"], "ByStatus")
                ["ProvisionedThroughput"]["ReadCapacityUnits"]
                .clone(),
            local["Table"]["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
            crate::emulator::index_named(&local["Table"], "ByStatus")["ProvisionedThroughput"]
                ["ReadCapacityUnits"]
                .clone(),
        ),
        (
            json!([{
                "IndexName": "ByStatus",
                "ProvisionedThroughputOverride": {"ReadCapacityUnits": 20},
            }]),
            json!(50),
            json!(20),
            json!(5),
            json!(2),
        ),
        "case a create's index override: the replica carries it and the source table does \
         not, got {local} and {replica_table}"
    );

    // An Update states the replica's parameters again, so the new override replaces the
    // one in force — reported and applied the same way.
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Update": {
                "RegionName": "us-west-2",
                "GlobalSecondaryIndexes": [{
                    "IndexName": "ByStatus",
                    "ProvisionedThroughputOverride": {"ReadCapacityUnits": 30},
                    "OnDemandThroughputOverride": {"MaxReadRequestUnits": 70},
                }],
            }}],
        }),
    )
    .await;
    settle().await;
    let changed = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
    let changed_replica = ok_in(
        &router,
        "us-west-2",
        "DescribeTable",
        json!({"TableName": name}),
    )
    .await;
    assert_eq!(
        (
            replica_in(&changed["Table"], "us-west-2")["GlobalSecondaryIndexes"].clone(),
            crate::emulator::index_named(&changed_replica["Table"], "ByStatus")
                ["ProvisionedThroughput"]["ReadCapacityUnits"]
                .clone(),
        ),
        (
            json!([{
                "IndexName": "ByStatus",
                "ProvisionedThroughputOverride": {"ReadCapacityUnits": 30},
                "OnDemandThroughputOverride": {"MaxReadRequestUnits": 70},
            }]),
            json!(30),
        ),
        "case an update's index override: the new values are in force, got {changed} and \
         {changed_replica}"
    );
}

#[tokio::test]
async fn an_update_action_applies_the_overrides_the_replica_reports() {
    let router = emulator();
    let name = "Replicated.Overrides";
    create_table(&router, name).await;
    settle().await;
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Create": {
                "RegionName": "us-west-2",
                "OnDemandThroughputOverride": {"MaxReadRequestUnits": 11},
                "TableClassOverride": "STANDARD_INFREQUENT_ACCESS",
            }}],
        }),
    )
    .await;
    settle().await;

    // What the create's overrides left, read back from the Region the call was made in.
    let created = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        (
            replica_in(&created["Table"], "us-west-2")["OnDemandThroughputOverride"].clone(),
            replica_in(&created["Table"], "us-west-2")["ReplicaTableClassSummary"]["TableClass"]
                .clone(),
        ),
        (
            json!({"MaxReadRequestUnits": 11}),
            json!("STANDARD_INFREQUENT_ACCESS"),
        ),
        "case the create's overrides: the replica reports what it overrides, got {created}"
    );

    // An Update changes them, and the replica's own table reports the class it was given.
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Update": {
                "RegionName": "us-west-2",
                "OnDemandThroughputOverride": {"MaxReadRequestUnits": 22},
                "TableClassOverride": "STANDARD",
            }}],
        }),
    )
    .await;
    settle().await;
    let changed = ok_in(&router, REGION, "DescribeTable", json!({"TableName": name})).await;
    let replica_table = ok_in(&router, "us-west-2", "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        (
            replica_in(&changed["Table"], "us-west-2")["OnDemandThroughputOverride"].clone(),
            replica_in(&changed["Table"], "us-west-2")["ReplicaStatus"].clone(),
            replica_table["Table"]["TableClassSummary"]["TableClass"].clone(),
        ),
        (
            json!({"MaxReadRequestUnits": 22}),
            json!("ACTIVE"),
            json!("STANDARD"),
        ),
        "case the update's overrides: the new values are in force, got {changed} and \
         {replica_table}"
    );
}

#[tokio::test]
async fn a_deleted_replica_and_its_table_are_gone_and_a_disassociated_one_stays() {
    let router = emulator();
    for name in ["Replicated.Deleted", "Replicated.Disassociated"] {
        create_table(&router, name).await;
    }
    settle().await;
    for name in ["Replicated.Deleted", "Replicated.Disassociated"] {
        ok_in(
            &router,
            REGION,
            "PutItem",
            json!({
                "TableName": name,
                "Item": {"Id": {"S": "kept"}, "Status": {"S": "OPEN"}},
            }),
        )
        .await;
        ok_in(
            &router,
            REGION,
            "UpdateTable",
            json!({
                "TableName": name,
                "ReplicaUpdates": [{"Create": {"RegionName": "us-west-2"}}],
            }),
        )
        .await;
    }
    settle().await;

    // A Delete reports the replica DELETING and then removes it, with the table it had in
    // its own Region.
    let deleting = ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": "Replicated.Deleted",
            "ReplicaUpdates": [{"Delete": {"RegionName": "us-west-2"}}],
        }),
    )
    .await;
    // A Disassociate keeps the LOCAL table and takes it out of the group.
    let disassociating = ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": "Replicated.Disassociated",
            "ReplicaUpdates": [{"Disassociate": {
                "RegionName": REGION,
                "DisassociationMode": "WAIT_FOR_REPLICATION",
            }}],
        }),
    )
    .await;
    assert_eq!(
        (
            replica_in(&deleting["TableDescription"], "us-west-2")["ReplicaStatus"].clone(),
            disassociating["TableDescription"]["Replicas"].clone(),
        ),
        (json!("DELETING"), Value::Null),
        "case the two removals: the delete is in flight and the disassociation is done, got \
         {deleting} and {disassociating}"
    );
    settle().await;

    let deleted = ok_in(
        &router,
        REGION,
        "DescribeTable",
        json!({"TableName": "Replicated.Deleted"}),
    )
    .await;
    let (status, gone) = call_in(
        &router,
        "us-west-2",
        "DescribeTable",
        json!({"TableName": "Replicated.Deleted"}),
    )
    .await;
    assert_eq!(
        (
            deleted["Table"]["Replicas"].clone(),
            deleted["Table"]["GlobalTableVersion"].clone(),
            (status, error_code(&gone)),
            listed_group(&router, "Replicated.Deleted").await,
        ),
        (
            Value::Null,
            Value::Null,
            (400, "ResourceNotFoundException".to_string()),
            Vec::<String>::new(),
        ),
        "case the deleted replica: the replica, its table and the group are gone, got \
         {deleted} and {gone}"
    );

    // The disassociated local table kept its data, and the replica it left behind is a
    // standalone table of its own Region holding the same data.
    let standalone = ok_in(
        &router,
        REGION,
        "GetItem",
        json!({
            "TableName": "Replicated.Disassociated",
            "Key": {"Id": {"S": "kept"}},
        }),
    )
    .await;
    let former_replica = ok_in(
        &router,
        "us-west-2",
        "DescribeTable",
        json!({"TableName": "Replicated.Disassociated"}),
    )
    .await;
    assert_eq!(
        (
            standalone["Item"]["Status"].clone(),
            former_replica["Table"]["Replicas"].clone(),
            listed_group(&router, "Replicated.Disassociated").await,
        ),
        (json!({"S": "OPEN"}), Value::Null, Vec::<String>::new()),
        "case the disassociated replica: both tables stand alone with their data, got \
         {standalone} and {former_replica}"
    );
}

#[tokio::test]
async fn the_legacy_global_table_operations_answer_their_modelled_client_errors() {
    let router = emulator();
    let name = "Legacy.Surface";
    create_table(&router, name).await;
    settle().await;
    ok_in(
        &router,
        REGION,
        "UpdateTable",
        json!({
            "TableName": name,
            "ReplicaUpdates": [{"Create": {"RegionName": "us-west-2"}}],
        }),
    )
    .await;
    settle().await;

    // Rows differ in the operation called, in what the request names beside the global
    // table, and in whether that name is a replicated table, a standalone table or no
    // table at all: the version these operations address is not offered, so the answer is
    // the same client error for all of them.
    let cases: [(&str, &str, Value, &str); 7] = [
        (
            "a create naming a replicated table",
            "CreateGlobalTable",
            json!({
                "GlobalTableName": name,
                "ReplicationGroup": [{"RegionName": "us-west-2"}],
            }),
            "ValidationException",
        ),
        (
            "a create naming no table at all",
            "CreateGlobalTable",
            json!({
                "GlobalTableName": "Legacy.Absent",
                "ReplicationGroup": [{"RegionName": "eu-west-1"}],
            }),
            "ValidationException",
        ),
        (
            "a describe of a replicated table",
            "DescribeGlobalTable",
            json!({"GlobalTableName": name}),
            "GlobalTableNotFoundException",
        ),
        (
            "a describe of a name no table carries",
            "DescribeGlobalTable",
            json!({"GlobalTableName": "Legacy.Absent"}),
            "GlobalTableNotFoundException",
        ),
        (
            "a settings describe",
            "DescribeGlobalTableSettings",
            json!({"GlobalTableName": name}),
            "GlobalTableNotFoundException",
        ),
        (
            "a settings update naming an index the table does not carry",
            "UpdateGlobalTableSettings",
            json!({
                "GlobalTableName": name,
                "GlobalTableGlobalSecondaryIndexSettingsUpdate": [{
                    "IndexName": "NoSuchIndex",
                    "ProvisionedWriteCapacityUnits": 5,
                }],
            }),
            "GlobalTableNotFoundException",
        ),
        (
            "a replica update through the legacy operation",
            "UpdateGlobalTable",
            json!({
                "GlobalTableName": name,
                "ReplicaUpdates": [{"Create": {"RegionName": "eu-west-1"}}],
            }),
            "GlobalTableNotFoundException",
        ),
    ];

    for (case, operation, request, expected) in cases {
        let (status, body) = call_in(&router, REGION, operation, request.clone()).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, expected),
            "case {case} on {operation}: expected the modelled client error, got {status} with \
             {body} for {request}"
        );
    }

    // The refused calls created nothing: the group is still the one `ReplicaUpdates` made,
    // and no table appeared in the Region the legacy replica update named.
    let (status, absent) = call_in(
        &router,
        "eu-west-1",
        "DescribeTable",
        json!({"TableName": name}),
    )
    .await;
    assert_eq!(
        (listed_group(&router, name).await, status, error_code(&absent)),
        (
            group_of(REGION, "us-west-2"),
            400,
            "ResourceNotFoundException".to_string(),
        ),
        "case after the legacy refusals: nothing was created, got {absent}"
    );
}
