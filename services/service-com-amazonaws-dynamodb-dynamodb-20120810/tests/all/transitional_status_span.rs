// SPDX-License-Identifier: CC0-1.0

//! The SPAN a clock-owed table transition lasts, as a caller OBSERVES it.
//!
//! `tests/all/table_control_plane.rs` already pins what each transition answers — a
//! second `DeleteTable` on a `DELETING` table conflicts, and the table really
//! disappears afterwards. What it leaves implicit, and what this suite is for, is the
//! span those answers last: a transition the clock owes must outlive a caller's own
//! SEQUENCE of requests. A client that accepts a delete and then polls the table and
//! retries its delete — several real round trips, not one — must keep finding the table
//! `DELETING` and its retry refused throughout, rather than have the removal fall due
//! between two of its own calls and turn the conflict into a not-found.
//!
//! The property is asserted over the ROUND TRIPS themselves — every poll inside the span
//! must answer, and at least several must fit — so it holds for whatever the span is
//! rather than for a particular number of milliseconds, and the same loop also pins the
//! other half of the rule: the clock does settle the transition once the span is past.
//!
//! The emulator, the signed request, the success assertion, the error code and the
//! create-then-wait waiter all come from the one harness this test target shares
//! (`crate::emulator`), so nothing about how this crate is driven is restated here.

use serde_json::json;
use service_com_amazonaws_dynamodb_dynamodb_20120810::state::STATUS_TRANSITION;

use crate::emulator::{call, describe_when_active, emulator, error_code, ok};

/// How long after the span this suite waits before asserting the settled state: enough
/// slack that a loaded machine is not read as a defect.
const SLACK: std::time::Duration = std::time::Duration::from_millis(400);

/// How long the loop below pauses between a caller's round trips.
const BETWEEN_ROUND_TRIPS: std::time::Duration = std::time::Duration::from_millis(50);

#[tokio::test]
async fn a_deleting_table_outlasts_the_callers_own_requests() {
    let router = emulator();
    let name = "OutlastingTheCaller";
    ok(
        &router,
        "CreateTable",
        json!({
            "TableName": name,
            "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
            "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
            "BillingMode": "PAY_PER_REQUEST",
        }),
    )
    .await;
    describe_when_active(&router, "an accepted delete", name).await;

    let started = std::time::Instant::now();
    let accepted = ok(&router, "DeleteTable", json!({"TableName": name})).await;
    assert_eq!(
        accepted["TableDescription"]["TableStatus"],
        json!("DELETING"),
        "case accepted delete: the delete reports DELETING, got {accepted}"
    );

    // Half the span rather than all of it, so the loop cannot race the settling it is
    // asserting is still owed.
    let mut round_trips = 0_u32;
    while started.elapsed() < STATUS_TRANSITION / 2 {
        let polled = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        assert_eq!(
            polled["Table"]["TableStatus"],
            json!("DELETING"),
            "case poll {round_trips} at {:?}: the table is still reported DELETING, got {polled}",
            started.elapsed()
        );
        let (retry_status, retried) = call(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            (retry_status, error_code(&retried).as_str()),
            (400, "ResourceInUseException"),
            "case retry {round_trips}: a second delete still conflicts, got {retry_status} \
             with {retried}"
        );
        round_trips += 1;
        tokio::time::sleep(BETWEEN_ROUND_TRIPS).await;
    }
    assert!(
        round_trips >= 2,
        "case span: the span must outlast several of a caller's round trips, got \
         {round_trips} inside {:?}",
        STATUS_TRANSITION / 2
    );

    // And the clock does settle it: past the span the table is gone, so the read is a
    // not-found rather than a conflict.
    tokio::time::sleep(STATUS_TRANSITION + SLACK).await;
    let (gone_status, gone) = call(&router, "DescribeTable", json!({"TableName": name})).await;
    assert_eq!(
        (gone_status, error_code(&gone).as_str()),
        (400, "ResourceNotFoundException"),
        "case settled: the removed table answers a not-found, got {gone_status} with {gone}"
    );
}

/// The other transitional statuses a table can be in, over the same rule: a
/// `DeleteTable` inside the span of a transition already in flight is a conflict with
/// the work the clock still owes, and it leaves the table exactly where it was.
///
/// `table_control_plane.rs` pins the `DELETING` case and `table_items.rs` the `CREATING`
/// one; the row here that neither covers is `UPDATING` — a capacity move in flight —
/// and the table spans all three so the rule is stated once over the whole class rather
/// than per status.
#[tokio::test]
async fn a_delete_inside_any_transition_conflicts_and_changes_nothing() {
    /// Which call puts the table into the transition a row is about.
    #[derive(Debug, Clone, Copy)]
    enum Starts {
        /// The create itself: the table is `CREATING` until the span is past.
        Create,
        /// A capacity move on a settled table: `UPDATING` until the span is past.
        CapacityMove,
        /// An accepted delete: `DELETING` until the removal falls due.
        Delete,
    }

    for (case, starts, expected) in [
        ("a fresh create", Starts::Create, "CREATING"),
        ("a capacity move", Starts::CapacityMove, "UPDATING"),
        ("an accepted delete", Starts::Delete, "DELETING"),
    ] {
        let router = emulator();
        let name = "TransitionalDelete";
        let created = ok(
            &router,
            "CreateTable",
            json!({
                "TableName": name,
                "KeySchema": [{"AttributeName": "Part", "KeyType": "HASH"}],
                "AttributeDefinitions": [{"AttributeName": "Part", "AttributeType": "S"}],
                "BillingMode": "PROVISIONED",
                "ProvisionedThroughput": {"ReadCapacityUnits": 5, "WriteCapacityUnits": 5},
            }),
        )
        .await;
        let description = match starts {
            Starts::Create => created["TableDescription"].clone(),
            Starts::CapacityMove => {
                describe_when_active(&router, case, name).await;
                ok(
                    &router,
                    "UpdateTable",
                    json!({
                        "TableName": name,
                        "ProvisionedThroughput": {
                            "ReadCapacityUnits": 11,
                            "WriteCapacityUnits": 12,
                        },
                    }),
                )
                .await["TableDescription"]
                    .clone()
            }
            Starts::Delete => {
                describe_when_active(&router, case, name).await;
                ok(&router, "DeleteTable", json!({"TableName": name})).await
                    ["TableDescription"]
                    .clone()
            }
        };
        assert_eq!(
            description["TableStatus"],
            json!(expected),
            "case {case}: the call that starts the transition reports {expected}, got \
             {description}"
        );

        let (status, body) = call(&router, "DeleteTable", json!({"TableName": name})).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ResourceInUseException"),
            "case {case}: a delete inside the span is refused as a conflict, got {status} \
             with {body}"
        );
        // The refusal changed nothing: the table is still there, in the status the
        // starting call reported.
        let described = ok(&router, "DescribeTable", json!({"TableName": name})).await;
        let still = match starts {
            // `DescribeTable` answers a create's poll once the creation has fallen due
            // (see that handler), so a table whose transition was the create reports the
            // settled status — what matters is that it is still THERE.
            Starts::Create => json!("ACTIVE"),
            _ => json!(expected),
        };
        assert_eq!(
            described["Table"]["TableStatus"], still,
            "case {case}: the refused delete left the table where it was, got {described}"
        );
    }
}
