// SPDX-License-Identifier: CC0-1.0

//! The two summaries a `TableDescription` carries about how a table is CHARGED, and when
//! each of them is there at all.
//!
//! RULE 1 — `BillingModeSummary` records the `PAY_PER_REQUEST` switch, so a table that has
//! never been on-demand carries NONE: "you may need to switch to on-demand mode at least
//! once in order to return a `BillingModeSummary` response"
//! (`smithy_query(query="shape",shape="BillingModeSummary")`). A table that HAS been
//! on-demand carries it from then on, reporting the mode in force beside
//! `LastUpdateToPayPerRequestDateTime`, "the time when `PAY_PER_REQUEST` was last set as
//! the read/write capacity mode" — the instant of the CALL that set it, which for a table
//! created on-demand is that table's own `CreationDateTime`. The mode of EVERY table is
//! still reported, by `TableThroughputModeSummary`, which carries no such note.
//!
//! RULE 2 — `TableClassSummary.LastUpdateDateTime` is "the date and time at which the table
//! class was last updated"
//! (`smithy_query(query="shape",shape="TableClassSummary")`): a table whose class no call
//! has updated carries the class alone, and an `UpdateTable` that changes it stamps the
//! instant of that call.
//!
//! The rows span what each rule generalises over: the mode a table is created in (named
//! either way, or left out) and the class it is created with and moved to, both classes the
//! `TableClass` enum names included.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;

use crate::emulator::{emulator, ok};

/// Wait out a transition the clock owes, so what follows observes the settled state.
async fn settle() {
    tokio::time::sleep(STATUS_TRANSITION + Duration::from_millis(200)).await;
}

/// One row of the capacity-mode table: how the table is created, and whether a description
/// of it must carry a `BillingModeSummary` at all.
struct ModeRow {
    case: &'static str,
    name: &'static str,
    /// The `BillingMode` the create names; `None` leaves the member out.
    created_mode: Option<&'static str>,
    /// The mode every description must report.
    expected_mode: &'static str,
    /// Whether a description carries `BillingModeSummary`: only a table that has been
    /// on-demand does.
    summary: bool,
}

const MODE_ROWS: &[ModeRow] = &[
    ModeRow {
        case: "created on-demand",
        name: "SummaryOnDemand",
        created_mode: Some("PAY_PER_REQUEST"),
        expected_mode: "PAY_PER_REQUEST",
        summary: true,
    },
    ModeRow {
        case: "created provisioned",
        name: "SummaryProvisioned",
        created_mode: Some("PROVISIONED"),
        expected_mode: "PROVISIONED",
        summary: false,
    },
    ModeRow {
        case: "created with no BillingMode member at all",
        name: "SummaryDefaulted",
        created_mode: None,
        expected_mode: "PROVISIONED",
        summary: false,
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
    if row.expected_mode == "PROVISIONED" {
        body["ProvisionedThroughput"] = json!({"ReadCapacityUnits": 2, "WriteCapacityUnits": 2});
    }
    body
}

#[tokio::test]
async fn only_a_table_that_has_been_on_demand_carries_a_billing_mode_summary() {
    let router = emulator();

    for row in MODE_ROWS {
        let case = row.case;
        let created = ok(&router, "CreateTable", create_body(row)).await;
        let described = &created["TableDescription"];
        assert_eq!(
            described["TableThroughputModeSummary"]["TableThroughputMode"],
            json!(row.expected_mode),
            "case {case:?}: every description reports the mode in force, got {described}"
        );
        assert_eq!(
            described.get("BillingModeSummary").is_some(),
            row.summary,
            "case {case:?}: expected BillingModeSummary present={}, got {described}",
            row.summary
        );
        if row.summary {
            assert_eq!(
                described["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"],
                described["CreationDateTime"],
                "case {case:?}: a table created on-demand had PAY_PER_REQUEST set when it \
                 was created, got {described}"
            );
        }

        settle().await;
        let read = ok(&router, "DescribeTable", json!({"TableName": row.name})).await;
        let read = &read["Table"];
        assert_eq!(
            (
                read.get("BillingModeSummary").is_some(),
                read["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"].clone(),
            ),
            (
                row.summary,
                described["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"].clone(),
            ),
            "case {case:?}: a later description reports the same summary, got {read}"
        );
    }

    // A PROVISIONED table that is SWITCHED to on-demand carries the summary from the
    // switch on, and the instant it reports is the one the switching call was made at —
    // not the one the capacity move settles at, which is later.
    let switched = ok(
        &router,
        "UpdateTable",
        json!({"TableName": "SummaryProvisioned", "BillingMode": "PAY_PER_REQUEST"}),
    )
    .await;
    let switched = &switched["TableDescription"];
    let set_at = switched["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"].as_f64();
    assert!(
        set_at.is_some_and(|instant| instant > 0.0),
        "case switched to on-demand: the switch's own echo carries the instant it was made, \
         got {switched}"
    );
    assert_eq!(
        switched["TableStatus"],
        json!("UPDATING"),
        "case switched to on-demand: the capacity move is still in flight, got {switched}"
    );

    settle().await;
    // Switching BACK to provisioned does not erase it: the member records when
    // PAY_PER_REQUEST was LAST SET, not which mode is current.
    let back = ok(
        &router,
        "UpdateTable",
        json!({
            "TableName": "SummaryProvisioned",
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 3, "WriteCapacityUnits": 3},
        }),
    )
    .await;
    let back = &back["TableDescription"];
    assert_eq!(
        (
            back["BillingModeSummary"]["BillingMode"].clone(),
            back["BillingModeSummary"]["LastUpdateToPayPerRequestDateTime"].as_f64(),
        ),
        (json!("PROVISIONED"), set_at),
        "case switched back to provisioned: the summary reports the mode in force and the \
         instant on-demand was last set, got {back}"
    );
}

#[tokio::test]
async fn a_table_class_change_stamps_the_instant_it_was_made() {
    let router = emulator();

    // The rows differ along the class a table is created with and the class it is moved
    // to: both values `TableClass` names
    // (`smithy_query(query="shape",shape="TableClassSummary")`: "valid values are STANDARD
    // and STANDARD_INFREQUENT_ACCESS"), including a create that names none at all.
    let rows: [(&str, &str, Option<&str>, &str); 3] = [
        (
            "created with no class, moved to infrequent access",
            "ClassDefaulted",
            None,
            "STANDARD_INFREQUENT_ACCESS",
        ),
        (
            "created STANDARD, moved to infrequent access",
            "ClassStandard",
            Some("STANDARD"),
            "STANDARD_INFREQUENT_ACCESS",
        ),
        (
            "created infrequent access, moved to STANDARD",
            "ClassInfrequent",
            Some("STANDARD_INFREQUENT_ACCESS"),
            "STANDARD",
        ),
    ];

    for (case, name, created_class, moved_to) in rows {
        let mut create = json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "pk", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "pk", "AttributeType": "S"}],
            "BillingMode": "PROVISIONED",
            "ProvisionedThroughput": {"ReadCapacityUnits": 1, "WriteCapacityUnits": 1},
        });
        if let Some(class) = created_class {
            create["TableClass"] = json!(class);
        }
        let created = ok(&router, "CreateTable", create).await;
        let described = &created["TableDescription"];
        // The class a create NAMED is reported, and no update instant with it: nothing has
        // updated the class.
        match created_class {
            Some(class) => assert_eq!(
                (
                    described["TableClassSummary"]["TableClass"].clone(),
                    described["TableClassSummary"].get("LastUpdateDateTime"),
                ),
                (json!(class), None),
                "case {case:?}: a class nothing has updated carries no update instant, got \
                 {described}"
            ),
            None => assert_eq!(
                described.get("TableClassSummary"),
                None,
                "case {case:?}: a table created with no class carries no summary, got \
                 {described}"
            ),
        }

        settle().await;
        let moved = ok(
            &router,
            "UpdateTable",
            json!({"TableName": name, "TableClass": moved_to}),
        )
        .await;
        let moved = &moved["TableDescription"];
        let stamped = moved["TableClassSummary"]["LastUpdateDateTime"].as_f64();
        assert_eq!(
            moved["TableClassSummary"]["TableClass"],
            json!(moved_to),
            "case {case:?}: the update reports the class it moved to, got {moved}"
        );
        assert!(
            stamped.is_some_and(|instant| instant > 0.0),
            "case {case:?}: a class change stamps the instant it was made, got {moved}"
        );

        settle().await;
        let read = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            (
                read["Table"]["TableClassSummary"]["TableClass"].clone(),
                read["Table"]["TableClassSummary"]["LastUpdateDateTime"].as_f64(),
            ),
            (json!(moved_to), stamped),
            "case {case:?}: a later description reports the class and the instant the \
             change stamped, got {read}"
        );

        // A request that RE-STATES the class the table is already in updates nothing, so
        // the instant the real change stamped stands.
        let restated = ok(
            &router,
            "UpdateTable",
            json!({"TableName": name, "TableClass": moved_to}),
        )
        .await;
        assert_eq!(
            restated["TableDescription"]["TableClassSummary"]["LastUpdateDateTime"].as_f64(),
            stamped,
            "case {case:?}: re-stating the class updates nothing, got {restated}"
        );
        settle().await;
    }
}
