// SPDX-License-Identifier: CC0-1.0

//! `Connection$billingTier` / `ConnectionSummary$billingTier`, the one optional member
//! of a stored connection.
//!
//! The model says only this much about it: the target is `BillingTier`, an integer
//! `@range(min: 0, max: 8)`, the member is NOT `@required`, and its documentation is
//! the single sentence "The billing tier this connection is currently assigned"
//! (`smithy_query(query="shape",shape="Connection,ConnectionSummary,BillingTier")` and
//! `smithy_query(query="doc",shape="Connection",member="billingTier")`). No rule for
//! choosing the ordinal is published, in the model or in the service's docs, and the
//! model's own worked examples return connections WITHOUT the member — the example
//! output of `AcceptConnectionProposal`, `CreateConnection`, `GetConnection` and
//! `DeleteConnection` each carries every required member of `Connection` and no
//! `billingTier` — so the member is one the service does not always send.
//!
//! The rule this suite pins is therefore the one the spec does state: the tier belongs
//! to the stored RECORD, not to a response, so no operation invents one and no
//! operation changes one. For every connection, every operation that returns it — the
//! create that minted it, a read, the list view's summary, an update, the delete —
//! reports one and the same value, that value is what the record holds (absent while
//! nothing has assigned it) and never anything outside the modelled range, and two
//! reads of one record never differ.
//!
//! The rows span the class the rule generalises over rather than one request: every
//! placement the live seeded catalog offers (both provider kinds, every environment
//! that takes connections, every bandwidth each one offers), each driven through all
//! five paths, plus a bandwidth change — the one mutation that could plausibly re-grade
//! a tier — on every environment that offers a second bandwidth.

use serde_json::{json, Map, Value};

use crate::emulator::{
    available_bandwidths, creatable_environments, placements, text, Emulator,
};

use service_com_amazonaws_interconnect_interconnect::state::BILLING_TIER_RANGE;

/// The gateway the rows attach their connections to; an attach point IS a Direct
/// Connect gateway of the account, so the suite creates one of its own.
const GATEWAY_ROWS: [(&str, Option<i64>); 1] = [("ic-billing-tier", None)];

/// The member every assertion below is about.
const MEMBER: &str = "billingTier";

/// Whatever a returned `Connection` or `ConnectionSummary` holds for the member, as
/// the wire carries it — `Value::Null` when the record holds no tier, which is a
/// value this comparison treats like any other.
fn reported(connection: &Value) -> Value {
    connection
        .get(MEMBER)
        .cloned()
        .unwrap_or(Value::Null)
}

/// A reported tier is either absent or an integer inside the modelled range. Nothing
/// else is a value this member may carry.
fn assert_modelled(case: &str, path: &str, value: &Value) {
    let (min, max) = BILLING_TIER_RANGE;
    assert!(
        value.is_null()
            || value
                .as_i64()
                .is_some_and(|tier| (i64::from(min)..=i64::from(max)).contains(&tier)),
        "case {case:?} on {path}: expected {MEMBER} absent or an integer in \
         {min}..={max}, got {value:?}"
    );
}

/// Every path that returns a connection reports the record's own tier: the same value
/// the create reported, inside the modelled range, and unchanged by a read.
#[tokio::test]
async fn every_connection_path_reports_the_records_own_billing_tier() {
    let emulator = Emulator::new();
    let gateway = emulator.ready_gateways(&GATEWAY_ROWS).await.remove(0);
    let table = placements(&emulator).await;
    assert!(
        table.len() >= 4,
        "expected the seeded catalog to offer several placements to span, got {table:?}"
    );

    for (row, (environment_id, bandwidth)) in table.iter().enumerate() {
        let case = format!("{environment_id} at {bandwidth}");
        let created = emulator
            .create_connection(
                environment_id,
                bandwidth,
                &gateway,
                &format!("billing tier row {row}"),
            )
            .await;
        let id = text(&created, "id");
        let expected = reported(&created);
        assert_modelled(&case, "CreateConnection", &expected);

        // Two reads of one record, so a read that invented a tier of its own — or
        // that assigned one as a side effect — would differ from the create or from
        // its own repeat.
        for attempt in 1..=2 {
            let fetched = emulator
                .ic_ok("GetConnection", json!({ "identifier": id }))
                .await;
            let actual = reported(&fetched["connection"]);
            assert_eq!(
                actual, expected,
                "case {case:?} on GetConnection (read {attempt}): expected the record's \
                 {MEMBER} {expected:?}, got {actual:?}"
            );
        }

        // The summary view of the same record, which is a projection of it rather
        // than a second opinion about it.
        let listed = emulator
            .ic_ok("ListConnections", json!({ "environmentId": environment_id }))
            .await;
        let summary = listed["connections"]
            .as_array()
            .expect("connections is a list")
            .iter()
            .find(|summary| summary["id"] == json!(id))
            .unwrap_or_else(|| panic!("case {case:?}: the created connection is listed"))
            .clone();
        let actual = reported(&summary);
        assert_eq!(
            actual, expected,
            "case {case:?} on the ConnectionSummary: expected the record's {MEMBER} \
             {expected:?}, got {actual:?}"
        );

        // An update of a member the tier is not a function of leaves it alone.
        let updated = emulator
            .ic_ok(
                "UpdateConnection",
                json!({ "identifier": id, "description": format!("renamed row {row}") }),
            )
            .await;
        let actual = reported(&updated["connection"]);
        assert_eq!(
            actual, expected,
            "case {case:?} on UpdateConnection: expected a description change to leave \
             {MEMBER} at {expected:?}, got {actual:?}"
        );

        let deleted = emulator
            .ic_ok("DeleteConnection", json!({ "identifier": id }))
            .await;
        let actual = reported(&deleted["connection"]);
        assert_eq!(
            actual, expected,
            "case {case:?} on DeleteConnection: expected the record's {MEMBER} \
             {expected:?}, got {actual:?}"
        );
    }
}

/// A bandwidth change is the one mutation that could plausibly re-grade a tier. No
/// rule for grading one is published, so the change moves the connection's bandwidth
/// and leaves its tier exactly as the record holds it — on every environment that
/// offers a second bandwidth, in both directions.
#[tokio::test]
async fn changing_the_bandwidth_leaves_the_billing_tier_as_the_record_holds_it() {
    let emulator = Emulator::new();
    let gateway = emulator.ready_gateways(&GATEWAY_ROWS).await.remove(0);
    let catalog = emulator.environments().await;

    // Every ordered pair of distinct bandwidths one environment offers, so the table
    // covers a change up and a change down.
    let mut rows: Vec<(String, String, String)> = Vec::new();
    for environment in creatable_environments(&catalog) {
        let environment_id = text(&environment, "environmentId");
        let offered = available_bandwidths(&environment);
        for from in &offered {
            for to in &offered {
                if from != to {
                    rows.push((environment_id.clone(), from.clone(), to.clone()));
                }
            }
        }
    }
    assert!(
        !rows.is_empty(),
        "expected an environment offering two bandwidths to move a connection between"
    );

    for (row, (environment_id, from, to)) in rows.iter().enumerate() {
        let case = format!("{environment_id}: {from} -> {to}");
        let created = emulator
            .create_connection(
                environment_id,
                from,
                &gateway,
                &format!("billing tier change {row}"),
            )
            .await;
        let id = text(&created, "id");
        let expected = reported(&created);
        assert_modelled(&case, "CreateConnection", &expected);

        let mut request = Map::new();
        request.insert("identifier".to_string(), json!(id));
        request.insert("bandwidth".to_string(), json!(to));
        let updated = emulator
            .ic_ok("UpdateConnection", Value::Object(request))
            .await;
        assert_eq!(
            text(&updated["connection"], "bandwidth"),
            *to,
            "case {case:?}: expected the update to have applied the new bandwidth, got {:?}",
            updated["connection"]["bandwidth"]
        );
        let actual = reported(&updated["connection"]);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the bandwidth change to leave {MEMBER} at \
             {expected:?}, got {actual:?}"
        );

        // And that is what the store now holds, not just what the update echoed.
        let fetched = emulator
            .ic_ok("GetConnection", json!({ "identifier": id }))
            .await;
        let actual = reported(&fetched["connection"]);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored {MEMBER} to still be {expected:?}, got {actual:?}"
        );

        // Each row cleans up after itself: a quota counts the connections the account
        // holds, and the rows share one account.
        emulator
            .ic_ok("DeleteConnection", json!({ "identifier": id }))
            .await;
    }
}
