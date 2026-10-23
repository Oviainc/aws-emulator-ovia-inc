// SPDX-License-Identifier: CC0-1.0

//! Two rules of the `TableDescription` projection every table operation returns, both
//! about the table's read/write throughput mode and the capacity it keeps warm:
//!
//! 1. a table's read/write mode is reported by BOTH summary members a description
//!    carries — `BillingModeSummary` and `TableThroughputModeSummary` — and the instant
//!    `PAY_PER_REQUEST` was LAST SET is reported by both, in both cases only once that
//!    mode is in effect. One stored mode and one stored switch instant feed both
//!    summaries, so a client that reads the mode from either sees the same answer, from
//!    `CreateTable`, `DescribeTable`, `UpdateTable` and `DeleteTable` alike;
//! 2. warm throughput is "the minimum provisioned capacity maintained for immediate data
//!    access" (the `TableWarmThroughputDescription` doc), so a table — and every global
//!    secondary index on it — keeps warm the capacity it is provisioned for; WITHDRAWING
//!    that capacity (a switch to `PAY_PER_REQUEST`, which leaves table and indexes with
//!    none) does not un-warm it, so the pair last in force is what each goes on
//!    reporting, and provisioning capacity again moves an unnamed pair to the new
//!    capacity while a pair a request NAMED stands.
//!
//! Each rule is asserted over a TABLE of inputs spanning what it generalises over: the
//! mode a table is created in (named either way, or left out), the mode it is switched to
//! and back, and — for the warm pair — equal and unequal capacities per level and per
//! dimension, with the table's pair named in full, in part or not at all.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;

use crate::emulator::{emulator, index_named, ok};

/// Wait out a transition the clock owes, so what follows observes the settled state.
fn wait_out_transition() {
    std::thread::sleep(STATUS_TRANSITION + Duration::from_millis(200));
}

/// The mode a row switches its table to once the table is `ACTIVE`.
struct Switch {
    /// The `BillingMode` the `UpdateTable` asks for.
    mode: &'static str,
    /// The `ProvisionedThroughput` it must carry when that mode is `PROVISIONED`.
    capacity: Option<(i64, i64)>,
    /// Whether the switch's OWN echo already reports the `PAY_PER_REQUEST` instant: a
    /// switch TO on-demand records an instant that falls due when the update settles, so
    /// its own echo carries none, while a switch back to `PROVISIONED` leaves the instant
    /// an earlier call recorded standing.
    echo_reports_instant: bool,
}

/// One row of the throughput-mode table: how the table is created, whether it is then
/// switched, and what mode every later description must report.
struct ModeRow {
    case: &'static str,
    name: &'static str,
    /// The `BillingMode` the create asks for; `None` leaves the member out, which
    /// resolves to `PROVISIONED`.
    created_mode: Option<&'static str>,
    created_capacity: Option<(i64, i64)>,
    switch: Option<Switch>,
    /// The mode every description taken after the dust settles must report.
    expected_mode: &'static str,
    /// Whether those descriptions must report the instant `PAY_PER_REQUEST` was last set:
    /// true once that mode has been in effect, and it stays true after a switch BACK to
    /// `PROVISIONED` — "the time when PAY_PER_REQUEST was LAST set as the read/write
    /// capacity mode" (`BillingModeSummary$LastUpdateToPayPerRequestDateTime`).
    expected_instant: bool,
}

/// The rows span the mode a table can be in and how it got there: created on-demand,
/// created provisioned, created with no mode member at all, switched to on-demand, and
/// switched back to provisioned.
const MODE_TABLE: &[ModeRow] = &[
    ModeRow {
        case: "a table created on-demand",
        name: "mode_on_demand",
        created_mode: Some("PAY_PER_REQUEST"),
        created_capacity: None,
        switch: None,
        expected_mode: "PAY_PER_REQUEST",
        expected_instant: true,
    },
    ModeRow {
        case: "a table created provisioned",
        name: "mode_provisioned",
        created_mode: Some("PROVISIONED"),
        created_capacity: Some((3, 2)),
        switch: None,
        expected_mode: "PROVISIONED",
        // A table that has never been on-demand has no instant to report.
        expected_instant: false,
    },
    ModeRow {
        case: "a table created with no BillingMode member at all",
        name: "mode_defaulted",
        created_mode: None,
        created_capacity: Some((1, 1)),
        switch: None,
        expected_mode: "PROVISIONED",
        expected_instant: false,
    },
    ModeRow {
        case: "a provisioned table switched to on-demand",
        name: "mode_switched_to_on_demand",
        created_mode: Some("PROVISIONED"),
        created_capacity: Some((2, 2)),
        switch: Some(Switch {
            mode: "PAY_PER_REQUEST",
            capacity: None,
            echo_reports_instant: false,
        }),
        expected_mode: "PAY_PER_REQUEST",
        expected_instant: true,
    },
    ModeRow {
        case: "an on-demand table switched back to provisioned",
        name: "mode_switched_back",
        created_mode: Some("PAY_PER_REQUEST"),
        created_capacity: None,
        switch: Some(Switch {
            mode: "PROVISIONED",
            capacity: Some((4, 4)),
            echo_reports_instant: true,
        }),
        expected_mode: "PROVISIONED",
        // The switch back does not erase when PAY_PER_REQUEST was last set.
        expected_instant: true,
    },
];

/// The `CreateTable` body for one mode row.
fn create_body(row: &ModeRow) -> Value {
    let mut body = json!({
        "TableName": row.name,
        "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
        "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
    });
    if let Some(mode) = row.created_mode {
        body["BillingMode"] = json!(mode);
    }
    if let Some((read, write)) = row.created_capacity {
        body["ProvisionedThroughput"] =
            json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write});
    }
    body
}

/// Rule 1 over one description: both summaries report `mode`, and both report the
/// `PAY_PER_REQUEST` instant exactly when `instant` says they must — the same instant in
/// both, since one stored value feeds them.
fn assert_mode(case: &str, origin: &str, description: &Value, mode: &str, instant: bool) {
    let billing = &description["BillingModeSummary"];
    let throughput = &description["TableThroughputModeSummary"];
    assert_eq!(
        (
            billing["BillingMode"].clone(),
            throughput["TableThroughputMode"].clone(),
        ),
        (json!(mode), json!(mode)),
        "case {case:?} from {origin}: expected both summaries to report {mode}, got \
         {description}"
    );
    let billing_instant = billing["LastUpdateToPayPerRequestDateTime"].clone();
    let throughput_instant = throughput["LastUpdateToPayPerRequestDateTime"].clone();
    assert_eq!(
        billing_instant, throughput_instant,
        "case {case:?} from {origin}: expected one switch instant in both summaries, got \
         {description}"
    );
    assert_eq!(
        billing_instant.is_number(),
        instant,
        "case {case:?} from {origin}: expected the PAY_PER_REQUEST instant \
         present={instant}, got {description}"
    );
}

#[tokio::test]
async fn every_table_description_reports_the_tables_throughput_mode_in_both_summaries() {
    let router = emulator();
    for row in MODE_TABLE {
        let case = row.case;
        let created = ok(&router, "CreateTable", create_body(row)).await;
        // The create echo reports the mode the table was created in. A table created
        // on-demand has PAY_PER_REQUEST set by this very call, and the mode takes effect
        // when the table does, so no instant is reported yet.
        assert_mode(
            case,
            "CreateTable",
            &created["TableDescription"],
            row.created_mode.unwrap_or("PROVISIONED"),
            false,
        );

        wait_out_transition();
        if let Some(switch) = &row.switch {
            let mut body = json!({"TableName": row.name, "BillingMode": switch.mode});
            if let Some((read, write)) = switch.capacity {
                body["ProvisionedThroughput"] =
                    json!({"ReadCapacityUnits": read, "WriteCapacityUnits": write});
            }
            let switched = ok(&router, "UpdateTable", body).await;
            assert_mode(
                case,
                "the switching UpdateTable",
                &switched["TableDescription"],
                switch.mode,
                switch.echo_reports_instant,
            );
            wait_out_transition();
        }

        let described = ok(&router, "DescribeTable", json!({"TableName": row.name})).await;
        assert_mode(
            case,
            "DescribeTable",
            &described["Table"],
            row.expected_mode,
            row.expected_instant,
        );

        // An UpdateTable that moves no capacity at all still projects the same table.
        let updated = ok(
            &router,
            "UpdateTable",
            json!({"TableName": row.name, "DeletionProtectionEnabled": false}),
        )
        .await;
        assert_mode(
            case,
            "UpdateTable",
            &updated["TableDescription"],
            row.expected_mode,
            row.expected_instant,
        );

        let deleted = ok(&router, "DeleteTable", json!({"TableName": row.name})).await;
        assert_mode(
            case,
            "DeleteTable",
            &deleted["TableDescription"],
            row.expected_mode,
            row.expected_instant,
        );
        wait_out_transition();
    }
}

/// One row of the warm-throughput table: the capacity a table and its index are created
/// with, the pair the request names for the table, and the pairs each level must report
/// before and after its capacity is withdrawn and provisioned again.
struct WarmRow {
    case: &'static str,
    name: &'static str,
    table_capacity: (i64, i64),
    index_capacity: (i64, i64),
    /// The `WarmThroughput` the create names for the TABLE, member by member; `None`
    /// leaves the member out, and `None` for the row names no pair at all. No request may
    /// name one per index — that member is refused with the scaffold's 501.
    named: Option<(Option<i64>, Option<i64>)>,
    /// The pair the table reports while it is provisioned, and still reports once its
    /// provisioned capacity has been withdrawn.
    table_warm: (i64, i64),
    /// The same for its index.
    index_warm: (i64, i64),
    /// The capacity the table and the index are provisioned with again, after the switch
    /// back to `PROVISIONED`.
    restored_table_capacity: (i64, i64),
    restored_index_capacity: (i64, i64),
    /// What the table then keeps warm: the new capacity when nobody named a pair, and the
    /// named pair where a request did name one — a named pair stands until another
    /// request names a new one.
    restored_table_warm: (i64, i64),
}

/// The rows span what the rule generalises over: equal and unequal capacities on the
/// table and on its index, each dimension moving independently, and a warm pair named in
/// full, in part or not at all.
const WARM_TABLE: &[WarmRow] = &[
    WarmRow {
        case: "equal capacities, no warm pair named",
        name: "warm_withdrawn_equal",
        table_capacity: (5, 5),
        index_capacity: (5, 5),
        named: None,
        table_warm: (5, 5),
        index_warm: (5, 5),
        restored_table_capacity: (9, 9),
        restored_index_capacity: (6, 6),
        restored_table_warm: (9, 9),
    },
    WarmRow {
        case: "capacities differing per level and per dimension",
        name: "warm_withdrawn_uneven",
        table_capacity: (7, 2),
        index_capacity: (1, 9),
        named: None,
        table_warm: (7, 2),
        index_warm: (1, 9),
        restored_table_capacity: (3, 11),
        restored_index_capacity: (8, 1),
        restored_table_warm: (3, 11),
    },
    WarmRow {
        case: "a warm pair named for the table in full",
        name: "warm_withdrawn_named",
        table_capacity: (2, 2),
        index_capacity: (3, 4),
        named: Some((Some(24_000), Some(8_000))),
        table_warm: (24_000, 8_000),
        index_warm: (3, 4),
        restored_table_capacity: (10, 10),
        restored_index_capacity: (5, 5),
        // The pair the request named stands: no later request named another one.
        restored_table_warm: (24_000, 8_000),
    },
    WarmRow {
        case: "a warm pair named only in its read half",
        name: "warm_withdrawn_named_read",
        table_capacity: (6, 3),
        index_capacity: (2, 2),
        named: Some((Some(30_000), None)),
        // The member the request leaves out keeps the value in force, which for a table
        // nobody had named a pair for is its own write capacity.
        table_warm: (30_000, 3),
        index_warm: (2, 2),
        restored_table_capacity: (12, 4),
        restored_index_capacity: (7, 7),
        restored_table_warm: (30_000, 3),
    },
];

/// The warm-throughput structure a description must carry for `(read, write)` units. The
/// pair is in force, so the status of the SETTING is `ACTIVE`.
fn expected_warm((read, write): (i64, i64)) -> Value {
    json!({
        "ReadUnitsPerSecond": read,
        "WriteUnitsPerSecond": write,
        "Status": "ACTIVE",
    })
}

/// Rule 2 over one description: the table and its index each report the pair expected of
/// them.
fn assert_warm(
    case: &str,
    origin: &str,
    description: &Value,
    table_warm: (i64, i64),
    index_warm: (i64, i64),
) {
    assert_eq!(
        description["WarmThroughput"],
        expected_warm(table_warm),
        "case {case:?} from {origin}: expected the table to keep {table_warm:?} warm, got \
         {description}"
    );
    assert_eq!(
        index_named(description, "by-gsi_pk")["WarmThroughput"],
        expected_warm(index_warm),
        "case {case:?} from {origin}: expected the index to keep {index_warm:?} warm, got \
         {description}"
    );
}

#[tokio::test]
async fn a_withdrawn_provisioned_capacity_leaves_the_warm_throughput_it_kept_in_force() {
    let router = emulator();
    for row in WARM_TABLE {
        let case = row.case;
        let mut body = json!({
            "TableName": row.name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [
                {"AttributeName": "pk", "AttributeType": "S"},
                {"AttributeName": "gsi_pk", "AttributeType": "S"},
            ],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {
                "ReadCapacityUnits": row.table_capacity.0,
                "WriteCapacityUnits": row.table_capacity.1,
            },
            "GlobalSecondaryIndexes": [{
                "IndexName": "by-gsi_pk",
                "KeySchema": [{"AttributeName": "gsi_pk", "KeyType": "HASH"}],
                "Projection": {"ProjectionType": "ALL"},
                "ProvisionedThroughput": {
                    "ReadCapacityUnits": row.index_capacity.0,
                    "WriteCapacityUnits": row.index_capacity.1,
                },
            }],
        });
        if let Some((read, write)) = row.named {
            let mut warm = json!({});
            if let Some(read) = read {
                warm["ReadUnitsPerSecond"] = json!(read);
            }
            if let Some(write) = write {
                warm["WriteUnitsPerSecond"] = json!(write);
            }
            body["WarmThroughput"] = warm;
        }
        let created = ok(&router, "CreateTable", body).await;
        assert_warm(
            case,
            "CreateTable",
            &created["TableDescription"],
            row.table_warm,
            row.index_warm,
        );
        wait_out_transition();

        // The switch to on-demand WITHDRAWS the provisioned capacity of the table and of
        // its index — both report zero capacity from here on — and leaves what each was
        // keeping warm exactly as it was.
        let switched = ok(
            &router,
            "UpdateTable",
            json!({"TableName": row.name, "BillingMode": "PAY_PER_REQUEST"}),
        )
        .await;
        let description = &switched["TableDescription"];
        assert_eq!(
            (
                description["ProvisionedThroughput"]["ReadCapacityUnits"].clone(),
                description["ProvisionedThroughput"]["WriteCapacityUnits"].clone(),
                index_named(description, "by-gsi_pk")["ProvisionedThroughput"]
                    ["ReadCapacityUnits"]
                    .clone(),
            ),
            (json!(0), json!(0), json!(0)),
            "case {case:?}: expected the on-demand switch to withdraw every provisioned \
             capacity, got {description}"
        );
        assert_warm(
            case,
            "the switching UpdateTable",
            description,
            row.table_warm,
            row.index_warm,
        );
        wait_out_transition();

        // Read back through DescribeTable: the pairs are stored, not an artefact of the
        // response the switch wrote.
        let described = ok(&router, "DescribeTable", json!({"TableName": row.name})).await;
        assert_warm(
            case,
            "DescribeTable",
            &described["Table"],
            row.table_warm,
            row.index_warm,
        );

        // Provisioning capacity again moves an unnamed pair to the new capacity, at both
        // levels; a pair a request named stands.
        let restored = ok(
            &router,
            "UpdateTable",
            json!({
                "TableName": row.name,
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {
                    "ReadCapacityUnits": row.restored_table_capacity.0,
                    "WriteCapacityUnits": row.restored_table_capacity.1,
                },
                // An index of a PROVISIONED table must be given capacity too: "initial
                // provisioned capacity values must be set" (`UpdateTable$BillingMode`).
                "GlobalSecondaryIndexUpdates": [{
                    "Update": {
                        "IndexName": "by-gsi_pk",
                        "ProvisionedThroughput": {
                            "ReadCapacityUnits": row.restored_index_capacity.0,
                            "WriteCapacityUnits": row.restored_index_capacity.1,
                        },
                    },
                }],
            }),
        )
        .await;
        assert_warm(
            case,
            "the restoring UpdateTable",
            &restored["TableDescription"],
            row.restored_table_warm,
            row.restored_index_capacity,
        );
        wait_out_transition();

        // And the delete echo projects the same stored table one last time.
        let deleted = ok(&router, "DeleteTable", json!({"TableName": row.name})).await;
        assert_warm(
            case,
            "DeleteTable",
            &deleted["TableDescription"],
            row.restored_table_warm,
            row.restored_index_capacity,
        );
        wait_out_transition();
    }
}
