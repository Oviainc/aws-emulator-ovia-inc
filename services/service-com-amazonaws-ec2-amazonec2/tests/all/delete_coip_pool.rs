// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteCoipPool`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeleteCoipPool`, answered with XML (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation DELETES FROM THE STORE and reports what it
//! removed: the pool `CoipPoolId` resolves to leaves the store, the response carries that
//! pool exactly as the describe reported it until this call, every other pool is left
//! alone, and a `CoipPoolId` that resolves to no pool of the account is refused — under
//! `InvalidPoolID.NotFound` when the value is a well-formed pool id, under
//! `InvalidParameterValue` when it reads as one but carries a digit count EC2 never mints,
//! and under `InvalidPoolID.Malformed` when it is no pool id at all — rather than answered
//! as a delete that did nothing.
//!
//! Every table row is a distinct input along what that generalises over — which pool of the
//! seeded set (so: how many address ranges, how many tags, which local gateway route table,
//! including one shared with another pool), in which order the pools are deleted, and each
//! way a value can fail to resolve to a pool — so no single recorded call is the
//! specification. The pools come from the `coip_pools` suite's own fixtures, so a delete is
//! asserted against exactly the pools `DescribeCoipPools` reports.

use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::coip_pools::{REGION, describe_one, described_ids, seed};
use super::ec2_query::{call, element, elements, error_code, error_message};

/// The HTTP status EC2 answers `DryRunOperation` with: the request was refused because a
/// precondition the CALLER set holds, not because anything was wrong with it.
const DRY_RUN_STATUS: u16 = 412;

/// One `DeleteCoipPool` call carrying `params`, as its HTTP status and XML body.
async fn delete(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DeleteCoipPool", params).await
}

/// One `DeleteCoipPool` call naming `pool_id`.
async fn delete_pool(router: &MultiServiceRouter, pool_id: &str) -> (u16, String) {
    delete(router, &[("CoipPoolId".to_string(), pool_id.to_string())]).await
}

/// Every pool `DescribeCoipPools` reports, in the order it reported them — through the
/// `coip_pools` suite's own reader, so the store is read back the one way this test tree
/// reads it.
async fn all_described_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    described_ids(router, case, &[]).await
}

/// Every member of the one modelled `CoipPool` an XML body carries, so what a delete
/// reports can be compared with what the describe reported for the same pool without a
/// member-by-member assertion at each call site.
#[derive(Debug, PartialEq, Eq)]
struct ReportedPool {
    /// `CoipPool$PoolId`.
    pool_id: Option<String>,
    /// `CoipPool$PoolArn`.
    pool_arn: Option<String>,
    /// `CoipPool$LocalGatewayRouteTableId`.
    local_gateway_route_table_id: Option<String>,
    /// `CoipPool$PoolCidrs`, in the order the body wrote them.
    pool_cidrs: Vec<String>,
    /// `CoipPool$Tags`, as `(key, value)` pairs in the order the body wrote them.
    tags: Vec<(String, String)>,
}

/// The one pool `xml` reports. The set members are read out of their own wrapper elements
/// (`poolCidrSet`, `tagSet`), so a pool that carries none reads as an empty list rather
/// than borrowing another set's items.
fn reported_pool(xml: &str) -> ReportedPool {
    let cidr_set = element(xml, "poolCidrSet").unwrap_or_default();
    let tag_set = element(xml, "tagSet").unwrap_or_default();
    let keys = elements(&tag_set, "key");
    let values = elements(&tag_set, "value");
    ReportedPool {
        pool_id: element(xml, "poolId"),
        pool_arn: element(xml, "poolArn"),
        local_gateway_route_table_id: element(xml, "localGatewayRouteTableId"),
        pool_cidrs: elements(&cidr_set, "item"),
        tags: keys.into_iter().zip(values).collect(),
    }
}

/// How many pools the store holds.
fn stored_pools(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().coip_pools.len()
}

// ---------------------------------------------------------------------------
// The store is what is deleted, and what is reported
// ---------------------------------------------------------------------------

/// Deleting a pool removes exactly it from the store and answers with that pool as the
/// describe reported it until this call — every member, including the address ranges and
/// the tags it was created with. One row per seeded pool, so the rows differ in how many
/// address ranges and tags the pool carries and in whether another pool shares its local
/// gateway route table.
#[tokio::test]
async fn deleting_a_pool_removes_exactly_it_and_reports_what_was_stored() {
    // How many pools the shared fixtures seed, so the table has one row per pool without
    // naming a count of its own.
    let pool_count = {
        let (_router, engine) = super::ec2_query::emulator();
        seed(&engine).len()
    };

    for position in 0..pool_count {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let pool_id = seeded[position].clone();
        let case = format!("pool {position} ({pool_id})");

        let described = reported_pool(&describe_one(&router, &case, &pool_id).await);
        let (status, body) = delete_pool(&router, &pool_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let deleted = reported_pool(&body);
        assert_eq!(
            deleted, described,
            "case {case:?}: expected the delete to report the stored pool {described:?}, got \
             {deleted:?}"
        );

        let remaining = all_described_ids(&router, &case).await;
        let expected: Vec<String> = seeded
            .iter()
            .filter(|id| **id != pool_id)
            .cloned()
            .collect();
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the other pools {expected:?} to be left alone, got \
             {remaining:?}"
        );
        assert_eq!(
            stored_pools(&engine),
            expected.len(),
            "case {case:?}: expected {} pools in the store, got {}",
            expected.len(),
            stored_pools(&engine)
        );
    }
}

/// Whatever ORDER the pools are deleted in, the store is left holding exactly the pools
/// that were not deleted, in store order: a delete removes the pool it names and nothing
/// else. The rows are distinct orders over the same three positions, plus one that deletes
/// every pool.
#[tokio::test]
async fn deleting_in_any_order_leaves_exactly_the_pools_not_deleted() {
    // Rows: ascending, descending, interleaved, and the whole store.
    let orders: [&[usize]; 4] = [
        &[0, 1, 2],
        &[4, 3, 2],
        &[3, 0, 5],
        &[0, 1, 2, 3, 4, 5],
    ];

    for order in orders {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let case = format!("order {order:?}");

        for position in order {
            let pool_id = &seeded[*position];
            let (status, body) = delete_pool(&router, pool_id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: deleting {pool_id} expected HTTP 200, got {status} with body \
                 {body}"
            );
            let reported = element(&body, "poolId").unwrap_or_default();
            assert_eq!(
                &reported, pool_id,
                "case {case:?}: expected the response to name {pool_id}, got {reported:?}"
            );
        }

        let expected: Vec<String> = seeded
            .iter()
            .enumerate()
            .filter(|(position, _)| !order.contains(position))
            .map(|(_, id)| id.clone())
            .collect();
        let remaining = all_described_ids(&router, &case).await;
        assert_eq!(
            remaining, expected,
            "case {case:?}: expected the pools not deleted {expected:?}, got {remaining:?}"
        );
    }
}

/// A delete is not idempotent: the pool is gone after the first call, so a second call
/// naming the same id resolves to no pool and is refused — the same refusal any other
/// unknown id gets.
#[tokio::test]
async fn deleting_the_same_pool_twice_refuses_the_second_call() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let pool_id = seeded[0].clone();

    let (status, body) = delete_pool(&router, &pool_id).await;
    assert_eq!(
        status, 200,
        "first delete: expected HTTP 200, got {status} with body {body}"
    );

    let (status, body) = delete_pool(&router, &pool_id).await;
    assert_eq!(
        status, 400,
        "second delete: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        ec2::state::INVALID_POOL_ID_NOT_FOUND,
        "second delete: the id is well formed, so expected InvalidPoolID.NotFound, got {body}"
    );
    assert_eq!(
        stored_pools(&engine),
        seeded.len() - 1,
        "second delete: the refused call must remove nothing, got {} pools",
        stored_pools(&engine)
    );
}

// ---------------------------------------------------------------------------
// A CoipPoolId that resolves to no pool
// ---------------------------------------------------------------------------

/// A `CoipPoolId` that resolves to no pool of the account is refused and nothing is
/// removed, and the CODE says HOW the value failed: a well-formed pool id nothing was
/// created for is `InvalidPoolID.NotFound`, a value that READS as a pool id but carries a
/// digit count EC2 never mints is `InvalidParameterValue`, and a value that is no pool id
/// at all is `InvalidPoolID.Malformed`. The rows are each way a value can fail to resolve:
/// both published EC2 id suffix lengths with digits nothing was created for, then three
/// hexadecimal suffixes of other lengths (one digit, between the two, far past the longer),
/// an upper-case suffix, a non-hex suffix, another resource's id, free text, and the empty
/// value.
#[tokio::test]
async fn a_pool_id_that_resolves_to_no_pool_is_refused() {
    let prefix = ec2::state::COIP_POOL_ID_PREFIX;
    let not_found = ec2::state::INVALID_POOL_ID_NOT_FOUND;
    let bad_value = ec2::state::INVALID_PARAMETER_VALUE;
    let malformed = ec2::state::INVALID_POOL_ID_MALFORMED;
    // The forms a pool id takes are `COIP_POOL_ID_PREFIX` plus 8 or 17 lowercase hex
    // digits, which `ec2::state::coip_pool_id_is_well_formed` is the one reader of.
    let cases = [
        ("the long form", format!("{prefix}00000000000000000"), not_found),
        ("the short form", format!("{prefix}00000000"), not_found),
        // The middle class is about the digit COUNT, not about any one value: a hexadecimal
        // suffix one digit long, one between the two mintable lengths, and one far past the
        // longer of them.
        ("one digit", format!("{prefix}0"), bad_value),
        ("neither length", format!("{prefix}0000000000000000"), bad_value),
        ("far too many digits", format!("{prefix}{}", "a".repeat(32)), bad_value),
        ("an upper-case suffix", format!("{prefix}0ABCDEF01"), malformed),
        ("a non-hex suffix", format!("{prefix}zzzzzzzz"), malformed),
        (
            "another resource's id",
            "subnet-00000000000000000".to_string(),
            malformed,
        ),
        ("free text", "not-a-pool-at-all".to_string(), malformed),
        ("the empty value", String::new(), malformed),
    ];

    for (case, pool_id, expected_code) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);

        let (status, body) = delete_pool(&router, &pool_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code:?}, got {body}"
        );
        let message = error_message(&body);
        // Either refusal names the value the request sent, in the quoting of the ONE
        // template this surface words a `CoipPoolId` it could not resolve with: the pool id
        // single-quoted, with that class's own verdict after it.
        let expected_message = if expected_code == malformed {
            format!("The pool ID '{pool_id}' is invalid.")
        } else {
            format!("The pool ID '{pool_id}' does not exist.")
        };
        assert!(
            message.contains(&format!("'{pool_id}'")),
            "case {case:?}: expected the message to name '{pool_id}', got {message:?}"
        );
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the surface's sentence for its class, got {message:?}"
        );
        assert_eq!(
            stored_pools(&engine),
            seeded.len(),
            "case {case:?}: a refused delete must remove nothing, got {} pools",
            stored_pools(&engine)
        );
        let remaining = all_described_ids(&router, case).await;
        assert_eq!(
            remaining, seeded,
            "case {case:?}: expected every pool to be left alone, got {remaining:?}"
        );
    }
}

/// A refused `CoipPoolId` is named in the message only up to the echo bound this crate
/// keeps on it: `Ipv4PoolCoipId` carries no `@length`, so an arbitrarily long value is
/// bounded before it is quoted back. Rows: a value AT the bound, which is named in full, and
/// one past it, which is named up to the bound.
#[tokio::test]
async fn an_over_long_pool_id_is_named_only_up_to_the_echo_bound() {
    let bound = ec2::state::MAX_ECHOED_COIP_POOL_ID_CHARS;
    let prefix = ec2::state::COIP_POOL_ID_PREFIX;
    for (case, digits) in [
        ("at the bound", bound - prefix.len()),
        ("past the bound", bound),
    ] {
        let (router, engine) = super::ec2_query::emulator();
        let seeded = seed(&engine);
        let pool_id = format!("{prefix}{}", "a".repeat(digits));

        let (status, body) = delete_pool(&router, &pool_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let message = error_message(&body);
        let echoed: String = pool_id.chars().take(bound).collect();
        assert_eq!(
            message,
            format!("The pool ID '{echoed}' does not exist."),
            "case {case:?}: expected the message to name at most {bound} characters, got \
             {message:?}"
        );
        assert_eq!(
            stored_pools(&engine),
            seeded.len(),
            "case {case:?}: a refused delete must remove nothing, got {} pools",
            stored_pools(&engine)
        );
    }
}

/// A request that carries no `CoipPoolId` at all names no pool to delete, so it is refused
/// rather than answered with an empty success. The member is `@required`, so the boundary
/// validator is what refuses it — before this operation's handler runs.
#[tokio::test]
async fn a_request_that_names_no_pool_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let (status, body) = delete(&router, &[]).await;
    assert_eq!(
        status, 400,
        "no CoipPoolId: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        stored_pools(&engine),
        seeded.len(),
        "no CoipPoolId: a refused delete must remove nothing, got {} pools",
        stored_pools(&engine)
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and deletes
/// nothing, whether or not the pool it names exists; `DryRun=false` is an ordinary delete.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_deleting() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let unknown = format!("{}00000000000000000", ec2::state::COIP_POOL_ID_PREFIX);

    // Rows: a pool the store holds, and an id it does not — the flag asks whether the
    // caller is permitted, which is not a question about the pool.
    for (case, pool_id) in [
        ("a stored pool", seeded[0].as_str()),
        ("an unknown pool", unknown.as_str()),
    ] {
        let (status, body) = delete(
            &router,
            &[
                ("CoipPoolId".to_string(), pool_id.to_string()),
                ("DryRun".to_string(), "true".to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert_eq!(
            stored_pools(&engine),
            seeded.len(),
            "case {case:?}: a dry run must delete nothing, got {} pools",
            stored_pools(&engine)
        );
    }

    // `DryRun=false` is no dry run at all: the delete happens.
    let (status, body) = delete(
        &router,
        &[
            ("CoipPoolId".to_string(), seeded[0].clone()),
            ("DryRun".to_string(), "false".to_string()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_pools(&engine),
        seeded.len() - 1,
        "DryRun=false: expected the pool to be deleted, got {} pools",
        stored_pools(&engine)
    );
}
