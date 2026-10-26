// SPDX-License-Identifier: CC0-1.0

//! `UpdateLongTermPricing`, driven end to end over HTTP.
//!
//! One rule, asserted over a TABLE of requests rather than over a single call: an update
//! writes the members the request supplied to the long-term pricing contract its
//! `LongTermPricingId` names — the auto-renew choice, and the job that replaces the device
//! ordered under the contract — and leaves every member the request omitted, and every
//! other contract, exactly as it was; an id that names no record this service holds, whether
//! the contract's or the replacement job's, is refused with `InvalidResourceException` and
//! writes nothing.
//!
//! The rows differ along what the rule generalises over: the `LongTermPricingType` the
//! contract was bought under (every value of the modelled catalog, so every term), the
//! `SnowballType` it covers, whether the create declared the auto-renew flag and which way,
//! whether the update names the auto-renew flag and which way, and whether it names a
//! replacement job. The updates are then applied a second time, so a member named once and
//! omitted next keeps the value it was given, and one named twice takes the newer value.
//!
//! Every call goes through the same front door a client uses — the `awsJson1_1` router
//! `job_lifecycle` drives the service with — and a contract is read back the way a client
//! reads one, through `ListLongTermPricing`, so nothing here reaches into the state struct.

use serde_json::{json, Value};
use service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::ops::types::LONG_TERM_PRICING_TYPE_VALUES;
use smithy_server_runtime::MultiServiceRouter;

use super::cluster_creation::UNKNOWN_LONG_TERM_PRICING_ID;
use super::job_lifecycle::{
    call_in_region, complete_order, create_address, deliverable_address, ok, refused, router,
    string,
};
use super::long_term_pricing::create_pricing;

/// The region every request in this suite signs for, carried in the SigV4 credential scope.
/// A contract is updated by id, which the Region does not bear on, so it is fixed rather
/// than varied here.
const SIGNED_REGION: &str = "us-west-2";

/// The modelled error an id naming no record of this service is refused with
/// (`smithy_query(query="operation", operation="UpdateLongTermPricing")`).
const INVALID_RESOURCE: &str = "InvalidResourceException";

/// A job id in the modelled `JobId` form that names no stored job.
const UNKNOWN_JOB_ID: &str = "JID00000000-0000-0000-0000-000000000000";

/// A main job's id in the modelled `JobId` form that names no stored job — the other prefix
/// the modelled `JobId` pattern admits.
const UNKNOWN_MAIN_JOB_ID: &str = "MID11111111-1111-1111-1111-111111111111";

/// What a row's update asks for the replacement job.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Replacement {
    /// The update names no replacement job.
    Unnamed,
    /// The update names a job this suite ordered for the purpose.
    Named,
}

/// One update row: the contract to buy, and the update to apply to it.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    pricing_type: &'static str,
    snowball_type: &'static str,
    /// What the create asked for the auto-renew flag; absent means it declared none.
    created_auto_renew: Option<bool>,
    /// What the update asks for the auto-renew flag; absent means it names none.
    updated_auto_renew: Option<bool>,
    replacement: Replacement,
}

impl Row {
    /// The auto-renew flag the contract must report once this row's update has been served:
    /// the value the update named, or the one the contract already carried when it named
    /// none — a create that declared no flag bought a contract that does not renew itself.
    fn expected_auto_renew(&self) -> bool {
        self.updated_auto_renew
            .unwrap_or_else(|| self.created_auto_renew.unwrap_or(false))
    }
}

/// Every update row: each `LongTermPricingType` the modelled catalog admits, against each
/// form the auto-renew flag takes on the create (absent, `false`, `true`) and each form it
/// takes on the update (absent, `false`, `true`), with and without a replacement job named.
///
/// The device types cycle through several values of the modelled `SnowballType` catalog, so
/// the rule is exercised on contracts covering different devices as well as different terms.
fn rows() -> Vec<Row> {
    let devices = ["EDGE", "EDGE_S", "SNC1_SSD", "V3_5C"];
    let flags = [None, Some(false), Some(true)];
    let mut rows = Vec::new();
    let mut device = devices.iter().cycle();
    for pricing_type in LONG_TERM_PRICING_TYPE_VALUES {
        for created_auto_renew in flags {
            for updated_auto_renew in flags {
                for replacement in [Replacement::Unnamed, Replacement::Named] {
                    let snowball_type = device.next().expect("the device cycle never ends");
                    rows.push(Row {
                        case: format!(
                            "a {pricing_type} contract on {snowball_type} created with \
                             auto-renew {}, updated with auto-renew {} and a replacement job \
                             {}",
                            describe_flag(created_auto_renew),
                            describe_flag(updated_auto_renew),
                            match replacement {
                                Replacement::Unnamed => "unnamed",
                                Replacement::Named => "named",
                            }
                        ),
                        pricing_type,
                        snowball_type,
                        created_auto_renew,
                        updated_auto_renew,
                        replacement,
                    });
                }
            }
        }
    }
    rows
}

/// How a row spells the form the auto-renew flag takes.
fn describe_flag(flag: Option<bool>) -> String {
    flag.map_or_else(|| "absent".to_string(), |value| value.to_string())
}

/// The `CreateLongTermPricing` body a row buys its contract with.
fn create_body(row: &Row) -> Value {
    let mut body = json!({
        "LongTermPricingType": row.pricing_type,
        "SnowballType": row.snowball_type,
    });
    if let Some(auto_renew) = row.created_auto_renew {
        body["IsLongTermPricingAutoRenew"] = json!(auto_renew);
    }
    body
}

/// The `UpdateLongTermPricing` body a row applies to `pricing_id`, naming `replacement_job`
/// when the row asks for one.
fn update_body(row: &Row, pricing_id: &str, replacement_job: &str) -> Value {
    let mut body = json!({ "LongTermPricingId": pricing_id });
    if let Some(auto_renew) = row.updated_auto_renew {
        body["IsLongTermPricingAutoRenew"] = json!(auto_renew);
    }
    if row.replacement == Replacement::Named {
        body["ReplacementJob"] = json!(replacement_job);
    }
    body
}

/// Order one job, returning the `JobId` the service minted for it.
async fn create_job(router: &MultiServiceRouter, address_id: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateJob",
        complete_order(&json!({ "AddressId": address_id })),
        case,
    )
    .await;
    string(&created, "JobId", case)
}

/// The `LongTermPricingListEntry` `ListLongTermPricing` reports for `pricing_id`.
///
/// The listing is walked page by page with the cursor the service mints, so the entry is
/// found however many contracts the store holds.
async fn entry_for(router: &MultiServiceRouter, pricing_id: &str, case: &str) -> Value {
    let mut body = json!({});
    loop {
        let listed = ok(router, "ListLongTermPricing", body.clone(), case).await;
        let entries = listed
            .get("LongTermPricingEntries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        if let Some(entry) = entries
            .iter()
            .find(|entry| entry.get("LongTermPricingId").and_then(Value::as_str) == Some(pricing_id))
        {
            return entry.clone();
        }
        match listed.get("NextToken").and_then(Value::as_str) {
            Some(token) => body = json!({ "NextToken": token }),
            None => panic!("case {case:?}: expected the listing to report {pricing_id}, got {listed}"),
        }
    }
}

/// The auto-renew flag an entry reports, reading an absent member as `false`: the member
/// carries a `false` default, which the protocol may leave off the wire.
fn reported_auto_renew(entry: &Value) -> bool {
    entry
        .get("IsLongTermPricingAutoRenew")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// The replacement job an entry reports, `None` for an entry that names none.
fn reported_replacement(entry: &Value) -> Option<String> {
    entry
        .get("ReplacementJob")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Assert `entry` reports the contract `created` bought, unchanged in everything an update
/// does not take: the term, the device type, the window and the status.
fn assert_contract_preserved(entry: &Value, created: &Value, case: &str) {
    for member in [
        "LongTermPricingType",
        "SnowballType",
        "LongTermPricingStartDate",
        "LongTermPricingEndDate",
        "LongTermPricingStatus",
    ] {
        assert_eq!(
            entry.get(member),
            created.get(member),
            "case {case:?}: expected the update to leave {member} as the create left it, got \
             {entry} against {created}"
        );
    }
}

#[tokio::test]
async fn update_long_term_pricing_writes_the_members_the_request_names() {
    let router = router();
    let address_id = create_address(
        &router,
        deliverable_address("Replacement Receiver", "4 Replacement Row", json!({})),
        "the address every order in this suite ships to",
    )
    .await;

    for row in rows() {
        let case = row.case.clone();
        let pricing_id = create_pricing(&router, create_body(&row), &case).await;
        let replacement_job = create_job(&router, &address_id, &case).await;
        let created = entry_for(&router, &pricing_id, &case).await;

        // A contract no update has touched reports no replacement device.
        assert_eq!(
            reported_replacement(&created),
            None,
            "case {case:?}: expected no ReplacementJob before any update, got {created}"
        );

        ok(
            &router,
            "UpdateLongTermPricing",
            update_body(&row, &pricing_id, &replacement_job),
            &case,
        )
        .await;

        let updated = entry_for(&router, &pricing_id, &case).await;
        assert_eq!(
            reported_auto_renew(&updated),
            row.expected_auto_renew(),
            "case {case:?}: expected the auto-renew flag {}, got {updated}",
            row.expected_auto_renew()
        );
        let expected_replacement = match row.replacement {
            Replacement::Named => Some(replacement_job.clone()),
            Replacement::Unnamed => None,
        };
        assert_eq!(
            reported_replacement(&updated),
            expected_replacement,
            "case {case:?}: expected the replacement job {expected_replacement:?}, got {updated}"
        );
        assert_contract_preserved(&updated, &created, &case);

        // The same update applied again names the same members, so it leaves the contract
        // reporting the same values: an update is the contract's state, not a counter.
        ok(
            &router,
            "UpdateLongTermPricing",
            update_body(&row, &pricing_id, &replacement_job),
            &case,
        )
        .await;
        let repeated = entry_for(&router, &pricing_id, &case).await;
        assert_eq!(
            repeated, updated,
            "case {case:?}: expected the same update applied twice to report the same entry, \
             got {repeated} against {updated}"
        );

        // An update naming NEITHER member leaves both as the update above wrote them, which
        // is what makes the update partial.
        ok(
            &router,
            "UpdateLongTermPricing",
            json!({ "LongTermPricingId": pricing_id }),
            &case,
        )
        .await;
        let untouched = entry_for(&router, &pricing_id, &case).await;
        assert_eq!(
            untouched, updated,
            "case {case:?}: expected an update naming no member to change nothing, got \
             {untouched} against {updated}"
        );
    }
}

#[tokio::test]
async fn a_later_update_replaces_only_the_members_it_names() {
    let router = router();
    let case = "a contract updated twice";
    let address_id = create_address(
        &router,
        deliverable_address("Second Update Receiver", "9 Second Street", json!({})),
        case,
    )
    .await;
    let pricing_id = create_pricing(
        &router,
        json!({ "LongTermPricingType": "OneYear", "SnowballType": "EDGE" }),
        case,
    )
    .await;
    let first_job = create_job(&router, &address_id, case).await;
    let second_job = create_job(&router, &address_id, case).await;

    ok(
        &router,
        "UpdateLongTermPricing",
        json!({
            "LongTermPricingId": pricing_id,
            "IsLongTermPricingAutoRenew": true,
            "ReplacementJob": first_job,
        }),
        case,
    )
    .await;

    // The second update names the replacement job alone, so the auto-renew flag the first
    // update wrote stays and the replacement job takes the newer value.
    ok(
        &router,
        "UpdateLongTermPricing",
        json!({ "LongTermPricingId": pricing_id, "ReplacementJob": second_job }),
        case,
    )
    .await;
    let entry = entry_for(&router, &pricing_id, case).await;
    assert!(
        reported_auto_renew(&entry),
        "case {case:?}: expected the auto-renew flag the first update wrote to stand, got {entry}"
    );
    assert_eq!(
        reported_replacement(&entry).as_deref(),
        Some(second_job.as_str()),
        "case {case:?}: expected the newer replacement job {second_job}, got {entry}"
    );

    // The third names the auto-renew flag alone, so the replacement job stands and the flag
    // takes the newer value.
    ok(
        &router,
        "UpdateLongTermPricing",
        json!({ "LongTermPricingId": pricing_id, "IsLongTermPricingAutoRenew": false }),
        case,
    )
    .await;
    let entry = entry_for(&router, &pricing_id, case).await;
    assert!(
        !reported_auto_renew(&entry),
        "case {case:?}: expected the auto-renew flag the third update wrote, got {entry}"
    );
    assert_eq!(
        reported_replacement(&entry).as_deref(),
        Some(second_job.as_str()),
        "case {case:?}: expected the replacement job to stand, got {entry}"
    );
}

#[tokio::test]
async fn an_update_reaches_only_the_contract_it_names() {
    let router = router();
    let case = "one contract of several updated";
    let address_id = create_address(
        &router,
        deliverable_address("Single Contract Receiver", "1 Single Lane", json!({})),
        case,
    )
    .await;
    let replacement_job = create_job(&router, &address_id, case).await;

    // One contract per term the modelled catalog admits, so the rule is checked against
    // neighbours of every term rather than one.
    let mut pricing_ids = Vec::new();
    for pricing_type in LONG_TERM_PRICING_TYPE_VALUES {
        pricing_ids.push(
            create_pricing(
                &router,
                json!({ "LongTermPricingType": pricing_type, "SnowballType": "EDGE_C" }),
                case,
            )
            .await,
        );
    }

    for target in &pricing_ids {
        let before: Vec<Value> = {
            let mut entries = Vec::new();
            for pricing_id in &pricing_ids {
                entries.push(entry_for(&router, pricing_id, case).await);
            }
            entries
        };
        ok(
            &router,
            "UpdateLongTermPricing",
            json!({
                "LongTermPricingId": target,
                "IsLongTermPricingAutoRenew": true,
                "ReplacementJob": replacement_job,
            }),
            case,
        )
        .await;
        for (pricing_id, was) in pricing_ids.iter().zip(before.iter()) {
            let now = entry_for(&router, pricing_id, case).await;
            if pricing_id == target {
                assert_eq!(
                    reported_replacement(&now).as_deref(),
                    Some(replacement_job.as_str()),
                    "case {case:?}: expected the named contract {pricing_id} to carry the \
                     replacement job, got {now}"
                );
            } else {
                assert_eq!(
                    &now, was,
                    "case {case:?}: expected the contract {pricing_id} the update did not name \
                     to be unchanged, got {now} against {was}"
                );
            }
        }
    }
}

#[tokio::test]
async fn update_long_term_pricing_refuses_an_id_no_record_answers_to() {
    let router = router();
    let case = "the records the refusals below are checked against";
    let address_id = create_address(
        &router,
        deliverable_address("Refusal Receiver", "6 Refusal Road", json!({})),
        case,
    )
    .await;
    let known_job = create_job(&router, &address_id, case).await;
    let pricing_id = create_pricing(
        &router,
        json!({
            "LongTermPricingType": "ThreeYear",
            "SnowballType": "EDGE",
            "IsLongTermPricingAutoRenew": true,
        }),
        case,
    )
    .await;
    let before = entry_for(&router, &pricing_id, case).await;

    // Each row names an id of the modelled form that answers to no record of this service:
    // the contract's, the replacement job's under either prefix the `JobId` pattern admits,
    // and both at once.
    for (row_case, body) in [
        (
            "a LongTermPricingId no create minted",
            json!({ "LongTermPricingId": UNKNOWN_LONG_TERM_PRICING_ID }),
        ),
        (
            "a LongTermPricingId no create minted, with a known replacement job",
            json!({
                "LongTermPricingId": UNKNOWN_LONG_TERM_PRICING_ID,
                "ReplacementJob": known_job,
            }),
        ),
        (
            "a ReplacementJob no order minted",
            json!({ "LongTermPricingId": pricing_id, "ReplacementJob": UNKNOWN_JOB_ID }),
        ),
        (
            "a main job's ReplacementJob no order minted",
            json!({ "LongTermPricingId": pricing_id, "ReplacementJob": UNKNOWN_MAIN_JOB_ID }),
        ),
        (
            "neither id answering to a record",
            json!({
                "LongTermPricingId": UNKNOWN_LONG_TERM_PRICING_ID,
                "ReplacementJob": UNKNOWN_JOB_ID,
            }),
        ),
    ] {
        refused(
            &router,
            "UpdateLongTermPricing",
            body,
            INVALID_RESOURCE,
            row_case,
        )
        .await;
    }

    // A refused update writes nothing: the contract reads back exactly as it did before,
    // including the auto-renew flag the refused rows asked to change.
    let after = entry_for(&router, &pricing_id, case).await;
    assert_eq!(
        after, before,
        "case {case:?}: expected a refused update to persist nothing, got {after} against \
         {before}"
    );
    assert_eq!(
        reported_replacement(&after),
        None,
        "case {case:?}: expected no replacement job after every refusal, got {after}"
    );
}

#[tokio::test]
async fn update_long_term_pricing_refuses_a_request_outside_the_modelled_constraints() {
    let router = router();

    // A request whose required member is missing, or whose ids do not satisfy the modelled
    // `LongTermPricingId` / `JobId` length and pattern, is refused at the boundary before
    // the handler runs, so only the status is asserted for these rows.
    for (case, body) in [
        ("no LongTermPricingId at all", json!({})),
        (
            "a LongTermPricingId outside the modelled pattern",
            json!({ "LongTermPricingId": "not-a-long-term-pricing-id" }),
        ),
        (
            "a LongTermPricingId one character short",
            json!({ "LongTermPricingId": "LTPIDffffffff-ffff-ffff-ffff-fffffffffff" }),
        ),
        (
            "a ReplacementJob outside the modelled pattern",
            json!({
                "LongTermPricingId": UNKNOWN_LONG_TERM_PRICING_ID,
                "ReplacementJob": "not-a-job-id",
            }),
        ),
    ] {
        let (status, parsed) =
            call_in_region(&router, SIGNED_REGION, "UpdateLongTermPricing", body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a refusal, got {status} with {parsed}"
        );
    }
}
