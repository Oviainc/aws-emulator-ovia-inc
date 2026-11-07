// SPDX-License-Identifier: CC0-1.0

//! What `DeprovisionIpamPoolCidr` takes back out of an IPAM pool, and how a deprovision that
//! has nothing to take back is refused — driven over `ec2Query` (README.md / OPERATIONS.md
//! "Wire protocol").
//!
//! One rule: a deprovision acts on the CIDRs the pool ACTUALLY holds. A range the pool holds a
//! provisioned CIDR for is taken back and left in the transient state of the
//! `IpamPoolCidrState` catalog, with the clock owing its removal; a range the pool holds no
//! provisioned CIDR for is nothing this call can act on, so it is refused for the STATE of the
//! resource the request names — HTTP 400 `IncorrectState` — and NOTHING is written.
//!
//! The rows of each table differ along what that rule generalises over: the address family,
//! the size of the range, whether the range was never provisioned, lies inside or around one
//! the pool holds, belongs to another pool, or has already been taken back, how the request
//! spells the range it names, how many ranges the pool holds, and how much of the
//! deprovision's own span the clock has run.
//!
//! The suite keeps its own `call`/`ok`/`member` wrappers over the shared [`super::ec2_query`]
//! harness, which is how every other suite in this crate is arranged.

use std::time::Duration;

use ec2::state::{
    ADDRESS_FAMILY_IPV4, ADDRESS_FAMILY_IPV6, INCORRECT_STATE,
    IPAM_POOL_CIDR_STATE_PENDING_DEPROVISION, IPAM_POOL_CIDR_STATE_PENDING_PROVISION,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, emulator, error_code, error_message};
use super::wire::{Param, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// Long enough past [`TRANSITION_DELAY`] that a removal the clock owes is certainly due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(250)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case and showing the body.
fn member(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// Assert that `action` was refused HTTP 400 with `code` and exactly `message`.
async fn refused(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
    code: &str,
    message: &str,
) {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {action}, got {status} with body {body}"
    );
    let actual_code = error_code(&body);
    assert_eq!(
        actual_code, code,
        "case {case:?}: expected the code {code:?} from {action}, got {actual_code:?} in {body}"
    );
    let actual_message = error_message(&body);
    assert_eq!(
        actual_message, message,
        "case {case:?}: expected the message {message:?}, got {actual_message:?}"
    );
}

/// The sentence EC2 refuses a deprovision of a range the pool holds no provisioned CIDR for
/// with, naming the range as the REQUEST spelled it.
fn no_such_pool_cidr(cidr: &str) -> String {
    format!("The specified action is not valid.The pool CIDR {cidr} does not exist.")
}

/// Create one IPAM and answer its default private scope id, the scope a pool of this suite is
/// created in.
async fn create_scope(case: &str, router: &MultiServiceRouter) -> String {
    let body = ok(case, router, "CreateIpam", &[]).await;
    member(case, &body, "privateDefaultScopeId")
}

/// Create one IPAM pool of `family` in `scope_id`, and answer the `IpamPoolId` the service
/// minted.
async fn create_pool(
    case: &str,
    router: &MultiServiceRouter,
    scope_id: &str,
    family: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateIpamPool",
        &[
            param("IpamScopeId", scope_id),
            param("AddressFamily", family),
        ],
    )
    .await;
    member(case, &body, "ipamPoolId")
}

/// Provision `cidr` to the pool `pool_id` names, asserting the provision took.
async fn provision(case: &str, router: &MultiServiceRouter, pool_id: &str, cidr: &str) {
    let body = ok(
        case,
        router,
        "ProvisionIpamPoolCidr",
        &[param("IpamPoolId", pool_id), param("Cidr", cidr)],
    )
    .await;
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(IPAM_POOL_CIDR_STATE_PENDING_PROVISION),
        "case {case:?}: expected the provision to answer the transient state, got {body}"
    );
}

/// The `cidr` elements `GetIpamPoolCidrs` reports for the pool, sorted.
async fn held_ranges(case: &str, router: &MultiServiceRouter, pool_id: &str) -> Vec<String> {
    let body = ok(
        case,
        router,
        "GetIpamPoolCidrs",
        &[param("IpamPoolId", pool_id)],
    )
    .await;
    let mut reported = elements(&body, "cidr");
    reported.sort();
    reported
}

/// A deprovision of a range the pool holds no provisioned CIDR for is refused HTTP 400
/// `IncorrectState`, naming the range the request asked about, and writes nothing.
///
/// The rows differ along the address family, the size of the range and how it relates to the
/// space the pool does hold — none of which the recorded instance varies.
#[tokio::test]
async fn a_deprovision_of_a_range_the_pool_does_not_hold_is_refused_for_the_state() {
    // (case, the pool's family, the ranges provisioned to it, the range the request names)
    let rows: [(&str, &str, &[&str], &str); 7] = [
        (
            "a pool with nothing provisioned to it",
            ADDRESS_FAMILY_IPV4,
            &[],
            "10.0.0.0/16",
        ),
        (
            "a range the pool was never given",
            ADDRESS_FAMILY_IPV4,
            &["10.1.0.0/16"],
            "10.2.0.0/16",
        ),
        (
            "a block inside a range the pool holds",
            ADDRESS_FAMILY_IPV4,
            &["10.3.0.0/16"],
            "10.3.4.0/24",
        ),
        (
            "a block the pool's own range lies inside",
            ADDRESS_FAMILY_IPV4,
            &["10.4.0.0/16"],
            "10.0.0.0/8",
        ),
        (
            "a single address the pool was never given",
            ADDRESS_FAMILY_IPV4,
            &["10.5.0.0/16", "10.6.0.0/24"],
            "192.0.2.7/32",
        ),
        (
            "an IPv6 range the pool was never given",
            ADDRESS_FAMILY_IPV6,
            &["2001:db8::/32"],
            "2001:db9::/32",
        ),
        (
            "an IPv6 block inside a range the pool holds",
            ADDRESS_FAMILY_IPV6,
            &["2001:db8::/32"],
            "2001:db8:a::/48",
        ),
    ];

    for (case, family, held, requested) in rows {
        let (router, _engine) = emulator();
        let scope = create_scope(case, &router).await;
        let pool_id = create_pool(case, &router, &scope, family).await;
        for cidr in held {
            provision(case, &router, &pool_id, cidr).await;
        }

        refused(
            case,
            &router,
            "DeprovisionIpamPoolCidr",
            &[param("IpamPoolId", &pool_id), param("Cidr", requested)],
            INCORRECT_STATE,
            &no_such_pool_cidr(requested),
        )
        .await;

        // Nothing was written: the pool still holds exactly the ranges it was given, and each
        // is still on its way to `provisioned` rather than on its way out.
        let reported = held_ranges(case, &router, &pool_id).await;
        let mut expected: Vec<String> = held.iter().map(|cidr| (*cidr).to_string()).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the pool to still hold {expected:?}, got {reported:?}"
        );
        let body = ok(
            case,
            &router,
            "GetIpamPoolCidrs",
            &[param("IpamPoolId", &pool_id)],
        )
        .await;
        for state in elements(&body, "state") {
            assert_eq!(
                state, IPAM_POOL_CIDR_STATE_PENDING_PROVISION,
                "case {case:?}: expected the refused call to have moved no range, got {body}"
            );
        }
    }
}

/// A range the pool DOES hold is taken back: the call answers it in the transient
/// `pending-deprovision` state, a read reports it there, the clock removes it one span later,
/// and a second deprovision of the same range is refused as a range the pool does not hold.
///
/// The rows differ along the family, the size of the range and how the request SPELLS the range
/// it names — a range is the addresses it covers, so a spelling with host bits set names the
/// range it lies in.
#[tokio::test]
async fn a_held_range_is_taken_back_and_then_no_longer_exists() {
    // (case, the pool's family, the range provisioned, the spelling the request names it by)
    let rows: [(&str, &str, &str, &str); 4] = [
        (
            "an IPv4 /16 named exactly",
            ADDRESS_FAMILY_IPV4,
            "10.10.0.0/16",
            "10.10.0.0/16",
        ),
        (
            "an IPv4 /24 named with its host bits set",
            ADDRESS_FAMILY_IPV4,
            "10.11.2.0/24",
            "10.11.2.37/24",
        ),
        (
            "an IPv4 /32",
            ADDRESS_FAMILY_IPV4,
            "192.0.2.7/32",
            "192.0.2.7/32",
        ),
        (
            "an IPv6 /56",
            ADDRESS_FAMILY_IPV6,
            "2001:db8:1::/56",
            "2001:db8:1::/56",
        ),
    ];

    for (case, family, provisioned, spelling) in rows {
        let (router, _engine) = emulator();
        let scope = create_scope(case, &router).await;
        let pool_id = create_pool(case, &router, &scope, family).await;
        provision(case, &router, &pool_id, provisioned).await;

        let body = ok(
            case,
            &router,
            "DeprovisionIpamPoolCidr",
            &[param("IpamPoolId", &pool_id), param("Cidr", spelling)],
        )
        .await;
        // The range is answered as the pool holds it, not as the request spelled it.
        assert_eq!(
            element(&body, "cidr").as_deref(),
            Some(provisioned),
            "case {case:?}: expected the deprovision to answer {provisioned:?}, got {body}"
        );
        // A deprovision is work AWS reports as under way, so the range is left in the
        // transient state of its catalog.
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(IPAM_POOL_CIDR_STATE_PENDING_DEPROVISION),
            "case {case:?}: expected the transient deprovision state, got {body}"
        );

        // Read back out of the store straight away: the range is still reported, in the state
        // the call left it in.
        let read = ok(
            case,
            &router,
            "GetIpamPoolCidrs",
            &[param("IpamPoolId", &pool_id)],
        )
        .await;
        assert_eq!(
            element(&read, "cidr").as_deref(),
            Some(provisioned),
            "case {case:?}: expected the pool to still report the range, got {read}"
        );
        assert_eq!(
            element(&read, "state").as_deref(),
            Some(IPAM_POOL_CIDR_STATE_PENDING_DEPROVISION),
            "case {case:?}: expected the read to report the transient state, got {read}"
        );

        // The pool holds no provisioned CIDR for it any more, so asking again is refused.
        refused(
            case,
            &router,
            "DeprovisionIpamPoolCidr",
            &[param("IpamPoolId", &pool_id), param("Cidr", spelling)],
            INCORRECT_STATE,
            &no_such_pool_cidr(spelling),
        )
        .await;

        // The clock owes the removal, which it performs whether or not anybody called.
        past_the_transition().await;
        let settled = held_ranges(case, &router, &pool_id).await;
        assert!(
            settled.is_empty(),
            "case {case:?}: expected the range to have left the pool, got {settled:?}"
        );
        refused(
            case,
            &router,
            "DeprovisionIpamPoolCidr",
            &[param("IpamPoolId", &pool_id), param("Cidr", spelling)],
            INCORRECT_STATE,
            &no_such_pool_cidr(spelling),
        )
        .await;
    }
}

/// A range provisioned to ANOTHER pool is not one this pool holds: the deprovision is refused
/// for this pool, and the pool that does hold the range still holds it.
#[tokio::test]
async fn a_range_of_another_pool_is_not_one_this_pool_can_deprovision() {
    let case = "a range provisioned to a sibling pool";
    let (router, _engine) = emulator();
    let scope = create_scope(case, &router).await;
    let holder = create_pool(case, &router, &scope, ADDRESS_FAMILY_IPV4).await;
    let other = create_pool(case, &router, &scope, ADDRESS_FAMILY_IPV4).await;
    let cidr = "10.20.0.0/16";
    provision(case, &router, &holder, cidr).await;

    refused(
        case,
        &router,
        "DeprovisionIpamPoolCidr",
        &[param("IpamPoolId", &other), param("Cidr", cidr)],
        INCORRECT_STATE,
        &no_such_pool_cidr(cidr),
    )
    .await;

    let held = held_ranges(case, &router, &holder).await;
    assert_eq!(
        held,
        vec![cidr.to_string()],
        "case {case:?}: expected the holding pool to still hold {cidr:?}, got {held:?}"
    );
}

/// A request that names no `Cidr` asks for every range the pool holds: each is taken back, and
/// a pool that holds none is refused for its state with nothing written.
///
/// The rows differ along the number of ranges the pool holds, which is what "every range"
/// generalises over.
#[tokio::test]
async fn a_request_naming_no_cidr_takes_back_every_range_the_pool_holds() {
    // (case, the ranges provisioned to the pool)
    let rows: [(&str, &[&str]); 3] = [
        ("a pool holding nothing", &[]),
        ("a pool holding one range", &["10.30.0.0/16"]),
        (
            "a pool holding three ranges",
            &["10.31.0.0/16", "10.32.0.0/24", "10.33.0.0/20"],
        ),
    ];

    for (case, held) in rows {
        let (router, _engine) = emulator();
        let scope = create_scope(case, &router).await;
        let pool_id = create_pool(case, &router, &scope, ADDRESS_FAMILY_IPV4).await;
        for cidr in held {
            provision(case, &router, &pool_id, cidr).await;
        }

        if held.is_empty() {
            refused(
                case,
                &router,
                "DeprovisionIpamPoolCidr",
                &[param("IpamPoolId", &pool_id)],
                INCORRECT_STATE,
                &format!(
                    "The specified action is not valid for {pool_id}. There are no pool cidrs \
                     provisioned in the pool."
                ),
            )
            .await;
            continue;
        }

        let body = ok(
            case,
            &router,
            "DeprovisionIpamPoolCidr",
            &[param("IpamPoolId", &pool_id)],
        )
        .await;
        // "The deprovisioned pool CIDR" is ONE range, and it is one of the pool's own.
        let answered = member(case, &body, "cidr");
        assert!(
            held.contains(&answered.as_str()),
            "case {case:?}: expected the answer to name one of {held:?}, got {answered:?}"
        );

        // Every range the pool held is on its way out.
        let read = ok(
            case,
            &router,
            "GetIpamPoolCidrs",
            &[param("IpamPoolId", &pool_id)],
        )
        .await;
        for state in elements(&read, "state") {
            assert_eq!(
                state, IPAM_POOL_CIDR_STATE_PENDING_DEPROVISION,
                "case {case:?}: expected every range to be on its way out, got {read}"
            );
        }
        // And asking again, with the pool now holding none, is the refusal above.
        refused(
            case,
            &router,
            "DeprovisionIpamPoolCidr",
            &[param("IpamPoolId", &pool_id)],
            INCORRECT_STATE,
            &format!(
                "The specified action is not valid for {pool_id}. There are no pool cidrs \
                 provisioned in the pool."
            ),
        )
        .await;
    }
}
