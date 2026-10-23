// SPDX-License-Identifier: CC0-1.0

//! What `WithdrawByoipCidr` does to a BYOIP address range, and what it refuses — end to end
//! over the wire the service actually speaks: `ec2Query`, an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=WithdrawByoipCidr`,
//! answered with XML (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by
//! the `ec2` signing name in the request's SigV4 credential scope, so every call here
//! carries one.
//!
//! The rule under test: `WithdrawByoipCidr` STOPS THE ADVERTISEMENT OF THE ADDRESS RANGE THE
//! REQUEST NAMES, whatever address family or prefix size that range has — the range leaves
//! `advertised` for the non-terminal `pending-withdrawal` state the response reports, and the
//! clock then settles it into `provisioned` ("it can take a few minutes before traffic to the
//! specified addresses stops routing to Amazon Web Services because of BGP propagation
//! delays"). A range in any other `ByoipCidrState` is refused with `IncorrectState`, a
//! well-formed range the account has not provisioned with `InvalidCidr.NotFound`, an empty
//! `Cidr` with `MissingParameter` and a value that is not an address range with
//! `InvalidParameterValue`; a `DryRun` probe is answered `DryRunOperation` and leaves every
//! stored range exactly as it was.
//!
//! The rows of each table below differ along what the rule generalises over: the address
//! family, the prefix size, the written form of the range, and the state the range is in.
//! The ranges used are from the documentation blocks RFC 5737 and RFC 3849 set aside
//! (`192.0.2.0/24`, `198.51.100.0/24`, `203.0.113.0/24`, `2001:db8::/32`) and from the
//! RFC 2544 benchmarking block (`198.18.0.0/15`), so no row names an address range that
//! belongs to anybody.
//!
//! `ProvisionByoipCidr` is still a scaffold stub, so there is no API path that brings a
//! range into the account: the ranges a row needs are seeded straight into the store the
//! handler reads, the way [`super::byoip_cidr_lifecycle`] (the release of the same ranges)
//! and the describe suites seed theirs.

use std::time::Duration;

use ec2::ops::types::BYOIP_CIDR_STATE_VALUES;
use ec2::state::{
    BYOIP_CIDR_STATE_ADVERTISED, BYOIP_CIDR_STATE_PENDING_WITHDRAWAL, BYOIP_CIDR_STATE_PROVISIONED,
    ByoipCidrRecord, ComAmazonawsEc2Amazonec2State as Ec2State,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, unescaped};
use super::wire::{Param, child_text, document, param};

/// The Region every call here is signed for. A BYOIP range is named by the range itself and
/// this store holds the account's ranges rather than a Region's, so the Region only selects
/// the ec2Query envelope.
const REGION: &str = "us-east-1";

/// EC2's error code for a range whose current state does not admit the withdrawal.
const INCORRECT_STATE: &str = "IncorrectState";

/// EC2's error code for an address range this account has not provisioned.
const NOT_FOUND: &str = "InvalidCidr.NotFound";

/// EC2's error code for a `Cidr` that is not an address range at all.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's error code for a `Cidr` that reached the handler empty.
const MISSING_PARAMETER: &str = "MissingParameter";

/// EC2's error code for a `DryRun` request that would have been served.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// Long enough past [`ec2::state::TRANSITION_DELAY`] that the settlement the clock owes a
/// withdrawn range is due, without depending on the exact span.
fn past_the_transition() -> Duration {
    ec2::state::TRANSITION_DELAY + Duration::from_millis(250)
}

/// One `WithdrawByoipCidr` call for `cidr`, as its HTTP status and XML body.
async fn withdraw(router: &MultiServiceRouter, cidr: &str) -> (u16, String) {
    super::ec2_query::call(router, REGION, "WithdrawByoipCidr", &[param("Cidr", cidr)]).await
}

/// One `WithdrawByoipCidr` call carrying `params`, as its HTTP status and XML body.
async fn withdraw_with(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "WithdrawByoipCidr", params).await
}

/// The address pool id this FIXTURE gives a seeded range: an `ipv4pool-ec2-…` id for an IPv4
/// range and an `ipv6pool-ec2-…` one for an IPv6 range (the `ipv4pool-ec2` / `ipv6pool-ec2`
/// resource types of the model's own `ResourceType` enum), suffixed with the range itself so
/// one range always gets one id and the id names the range it belongs to.
///
/// It is fixture data, not a value the service computes: what the tables below assert is that
/// a withdrawal reports the pool id the STORE holds for the range it moved.
fn fixture_pool_id(canonical_cidr: &str) -> String {
    let family = if canonical_cidr.contains(':') {
        "ipv6pool-ec2"
    } else {
        "ipv4pool-ec2"
    };
    let suffix: String = canonical_cidr
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("{family}-{suffix}")
}

/// Seed one BYOIP range in `state` and answer the canonical form the store keys it by.
///
/// The record carries a description and a pool id as well as the state, so a response built
/// from it reports more than the one member the move changed.
fn seed(engine: &engine::InMemoryEngine, cidr: &str, state: &str) -> String {
    let canonical = ec2::state::canonical_byoip_cidr(cidr)
        .unwrap_or_else(|problem| panic!("{cidr:?} must be an address range: {problem:?}"));
    let mut record = ByoipCidrRecord::in_state(&canonical, state);
    record.byoip_cidr.description = Some(format!("seeded {state} range"));
    record.byoip_cidr.pool_id = Some(fixture_pool_id(&canonical));
    engine.state::<Ec2State>().lock().insert_byoip_cidr(record);
    canonical
}

/// The state the store reports for `cidr` right now, with every move the clock already owed
/// applied — `None` when the store holds no such range.
fn stored_state(engine: &engine::InMemoryEngine, cidr: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard.materialize_due_transitions();
    guard
        .byoip_cidrs
        .get(cidr)
        .map(|range| range.state().to_string())
}

/// The text of one member of the `<byoipCidr>` a successful response carries.
fn reported(case: &str, body: &str, member: &str) -> String {
    let parsed = document(case, body);
    let range = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "byoipCidr")
        .unwrap_or_else(|| panic!("case {case:?}: the response must carry a <byoipCidr>: {body}"));
    child_text(range, member)
        .unwrap_or_else(|| panic!("case {case:?}: <byoipCidr> must carry <{member}>: {body}"))
}

/// The `<Message>` of a failure body, with the XML entities resolved.
fn message(body: &str) -> String {
    unescaped(&element(body, "Message").unwrap_or_default())
}

// ---------------------------------------------------------------------------
// The withdrawal itself
// ---------------------------------------------------------------------------

/// The rule: a withdrawal of an advertised range answers that range in `pending-withdrawal`,
/// persists that state, and the clock settles it into `provisioned` — so a second withdrawal
/// of the same range is refused both while it is pending and once it has settled.
///
/// The rows differ in the address family, the prefix size and the written form of the range
/// the request names: the canonical range is what the store is keyed by, so a request may
/// spell it with host bits set or with an expanded IPv6 address.
#[tokio::test]
async fn withdrawal_moves_an_advertised_range_to_pending_withdrawal_and_settles_it_provisioned() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, the range as it is provisioned, the range as the request spells it)
    let cases: [(&str, &str, &str); 5] = [
        ("an IPv4 /24", "198.51.100.0/24", "198.51.100.0/24"),
        ("an IPv4 /22", "203.0.112.0/22", "203.0.112.0/22"),
        (
            "an IPv4 /24 the request spells with host bits set",
            "192.0.2.0/24",
            "192.0.2.37/24",
        ),
        ("an IPv6 /48", "2001:db8::/48", "2001:db8::/48"),
        (
            "an IPv6 /56 the request spells in expanded form",
            "2001:db8:1:100::/56",
            "2001:0db8:0001:0100:0000:0000:0000:0000/56",
        ),
    ];

    // A bystander range no row withdraws: every call must leave it advertised.
    let bystander = seed(&engine, "198.18.0.0/15", BYOIP_CIDR_STATE_ADVERTISED);

    for (case, provisioned, requested) in cases {
        let key = seed(&engine, provisioned, BYOIP_CIDR_STATE_ADVERTISED);
        assert_eq!(
            key, provisioned,
            "case {case:?}: the store must key a range by its canonical form, got {key:?}"
        );

        let (status, body) = withdraw(&router, requested).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from WithdrawByoipCidr, got {status} \
             with body {body}"
        );
        let answered_cidr = reported(case, &body, "cidr");
        assert_eq!(
            answered_cidr, provisioned,
            "case {case:?}: expected the response to name the range {provisioned:?}, \
             got {answered_cidr:?}"
        );
        let answered_state = reported(case, &body, "state");
        assert_eq!(
            answered_state, BYOIP_CIDR_STATE_PENDING_WITHDRAWAL,
            "case {case:?}: expected the withdrawn range to be reported \
             pending-withdrawal, got {answered_state:?} in {body}"
        );
        // The response is the STORED range, not just the member the move changed.
        let pool_id = reported(case, &body, "poolId");
        assert_eq!(
            pool_id,
            fixture_pool_id(provisioned),
            "case {case:?}: expected the response to carry the pool id the store holds \
             for the range, got {pool_id:?}"
        );

        let persisted = stored_state(&engine, provisioned);
        assert_eq!(
            persisted.as_deref(),
            Some(BYOIP_CIDR_STATE_PENDING_WITHDRAWAL),
            "case {case:?}: the withdrawal must persist pending-withdrawal, \
             got {persisted:?}"
        );

        // Nothing is left to withdraw while the withdrawal propagates.
        let (status, body) = withdraw(&router, requested).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second withdrawal must be refused, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INCORRECT_STATE,
            "case {case:?}: expected IncorrectState for a range already withdrawing, \
             got {code:?} in {body}"
        );
    }

    // One wait covers every row: each range's settlement is due a fixed span after its own
    // withdrawal, and the loop above took less than that span's worth of margin.
    tokio::time::sleep(past_the_transition()).await;

    for (case, provisioned, requested) in cases {
        let settled = stored_state(&engine, provisioned);
        assert_eq!(
            settled.as_deref(),
            Some(BYOIP_CIDR_STATE_PROVISIONED),
            "case {case:?}: the clock must settle a withdrawn range into provisioned, \
             got {settled:?}"
        );
        // A settled range is provisioned, not advertised, so it still has no advertisement
        // to stop.
        let (status, body) = withdraw(&router, requested).await;
        let code = error_code(&body);
        assert_eq!(
            (status, code.as_str()),
            (400, INCORRECT_STATE),
            "case {case:?}: expected 400 IncorrectState for a settled range, \
             got {status} / {code:?} in {body}"
        );
    }

    let untouched = stored_state(&engine, &bystander);
    assert_eq!(
        untouched.as_deref(),
        Some(BYOIP_CIDR_STATE_ADVERTISED),
        "a withdrawal must leave every range it does not name alone, got {untouched:?}"
    );
}

/// The rule: only a range that IS being advertised has an advertisement to stop, so a range
/// in any other state is refused with `IncorrectState` and keeps the state it was in.
///
/// The rows are every other value of the model's own `ByoipCidrState` enum
/// (`BYOIP_CIDR_STATE_VALUES` in `ops::types`, the generated catalog), so a state the model
/// adds is covered here without a second list being written.
#[tokio::test]
async fn withdrawal_refuses_a_range_that_is_not_being_advertised() {
    let (router, engine) = super::ec2_query::emulator();

    let other_states: Vec<&str> = BYOIP_CIDR_STATE_VALUES
        .iter()
        .copied()
        .filter(|state| *state != BYOIP_CIDR_STATE_ADVERTISED)
        .collect();
    assert_eq!(
        other_states.len(),
        BYOIP_CIDR_STATE_VALUES.len() - 1,
        "the state catalog must hold `advertised` exactly once, got \
         {BYOIP_CIDR_STATE_VALUES:?}"
    );

    for (index, state) in other_states.into_iter().enumerate() {
        // One range per row, so a refusal cannot be read off a range another row moved.
        let cidr = format!("198.19.{index}.0/24");
        let key = seed(&engine, &cidr, state);

        let (status, body) = withdraw(&router, &cidr).await;
        assert_eq!(
            status, 400,
            "case {state:?}: expected HTTP 400 for a range that is not advertised, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INCORRECT_STATE,
            "case {state:?}: expected IncorrectState, got {code:?} in {body}"
        );
        let said = message(&body);
        assert!(
            said.contains(state),
            "case {state:?}: the refusal must name the state the range is in, got {said:?}"
        );
        let after = stored_state(&engine, &key);
        assert_eq!(
            after.as_deref(),
            Some(state),
            "case {state:?}: a refused withdrawal must leave the range's state alone, \
             got {after:?}"
        );
    }
}

/// The rule: a well-formed address range the account has not provisioned is refused with
/// `InvalidCidr.NotFound`, and the range a request names is the range it provisioned — not a
/// part of one, not one that contains it, and not the same block at another prefix length.
#[tokio::test]
async fn withdrawal_refuses_a_range_the_account_has_not_provisioned() {
    let (router, engine) = super::ec2_query::emulator();

    // The one provisioned range every row below must NOT be read as the range it named.
    let provisioned = seed(&engine, "203.0.113.0/24", BYOIP_CIDR_STATE_ADVERTISED);

    // (case, the range the request names)
    let cases: [(&str, &str); 5] = [
        ("an IPv4 range nothing provisioned", "198.51.100.0/24"),
        ("an IPv6 range nothing provisioned", "2001:db8:dead::/48"),
        // Inside the provisioned /24, but not the range that was provisioned.
        ("half of a provisioned range", "203.0.113.128/25"),
        ("a range that contains a provisioned one", "203.0.112.0/22"),
        ("the same address with another prefix length", "203.0.113.0/32"),
    ];

    for (case, cidr) in cases {
        let (status, body) = withdraw(&router, cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for an unprovisioned range, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, NOT_FOUND,
            "case {case:?}: expected InvalidCidr.NotFound, got {code:?} in {body}"
        );
        let said = message(&body);
        assert!(
            said.contains(cidr),
            "case {case:?}: the refusal must name the range the request asked about, \
             got {said:?}"
        );
    }

    let after = stored_state(&engine, &provisioned);
    assert_eq!(
        after.as_deref(),
        Some(BYOIP_CIDR_STATE_ADVERTISED),
        "a refused withdrawal must leave the provisioned range advertised, got {after:?}"
    );
}

/// The rule: `Cidr` is "the address range, in CIDR notation" and the model puts no pattern on
/// it, so the handler is the only place that can refuse a value that is not one — an empty
/// value with `MissingParameter` and anything else that is not an address range with
/// `InvalidParameterValue`, never a not-found and never a 501.
///
/// The rows differ in HOW the value fails to be a range: empty, no prefix at all, a prefix
/// past the address width of either family, a non-numeric prefix, an address outside its
/// family's range, and a value that is not an address.
#[tokio::test]
async fn withdrawal_refuses_a_value_that_is_not_an_address_range() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine, "198.51.100.0/24", BYOIP_CIDR_STATE_ADVERTISED);

    // (case, the value the request carries as `Cidr`, the expected error code)
    let cases: [(&str, &str, &str); 8] = [
        ("an empty value", "", MISSING_PARAMETER),
        ("not an address at all", "not-a-cidr", INVALID_PARAMETER_VALUE),
        (
            "an address with no prefix length",
            "198.51.100.0",
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv4 prefix past 32",
            "198.51.100.0/33",
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a negative prefix length",
            "198.51.100.0/-1",
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an octet past 255",
            "198.51.100.256/24",
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv6 prefix past 128",
            "2001:db8::/129",
            INVALID_PARAMETER_VALUE,
        ),
        (
            "two prefix lengths",
            "198.51.100.0/24/24",
            INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, cidr, expected_code) in cases {
        let (status, body) = withdraw(&router, cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a value that is not a CIDR range, \
             got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?} \
             in {body}"
        );
    }

    let after = stored_state(&engine, "198.51.100.0/24");
    assert_eq!(
        after.as_deref(),
        Some(BYOIP_CIDR_STATE_ADVERTISED),
        "a refused withdrawal must leave the provisioned range advertised, got {after:?}"
    );
}

/// The rule: `DryRun` "checks whether you have the required permissions for the action,
/// without actually making the request" — so the probe is answered `DryRunOperation` (HTTP
/// 412) whatever the account's BYOIP registry holds, and changes nothing in it. A request
/// that is not a probe (`DryRun=false`, or no `DryRun` at all) performs the withdrawal.
///
/// The rows differ in what the named range is: advertised, present in another state, and
/// absent — plus the syntax refusal, which the request is read for before the probe is
/// answered.
#[tokio::test]
async fn a_dry_run_reports_dry_run_operation_and_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    let advertised = seed(&engine, "198.51.100.0/24", BYOIP_CIDR_STATE_ADVERTISED);
    let provisioned = seed(&engine, "192.0.2.0/24", BYOIP_CIDR_STATE_PROVISIONED);

    // (case, the range the request names, the expected status, the expected error code)
    let cases: [(&str, &str, u16, &str); 4] = [
        ("an advertised range", &advertised, 412, DRY_RUN_OPERATION),
        (
            "a range that is not being advertised",
            &provisioned,
            412,
            DRY_RUN_OPERATION,
        ),
        (
            "a range the account has not provisioned",
            "203.0.113.0/24",
            412,
            DRY_RUN_OPERATION,
        ),
        (
            "a value that is not an address range",
            "not-a-cidr",
            400,
            INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, cidr, expected_status, expected_code) in cases {
        let params = [param("Cidr", cidr), param("DryRun", "true")];
        let (status, body) = withdraw_with(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status} from a dry run, got {status} \
             with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?} \
             in {body}"
        );
    }

    for (case, cidr, expected) in [
        (
            "the advertised range",
            advertised.as_str(),
            BYOIP_CIDR_STATE_ADVERTISED,
        ),
        (
            "the provisioned range",
            provisioned.as_str(),
            BYOIP_CIDR_STATE_PROVISIONED,
        ),
    ] {
        let after = stored_state(&engine, cidr);
        assert_eq!(
            after.as_deref(),
            Some(expected),
            "case {case:?}: a dry run must leave the range's state alone, got {after:?}"
        );
    }
    assert!(
        stored_state(&engine, "203.0.113.0/24").is_none(),
        "a dry run must not store the range it named: {:?}",
        stored_state(&engine, "203.0.113.0/24")
    );

    // `DryRun=false` is not a probe: the withdrawal is performed.
    let params = [param("Cidr", &advertised), param("DryRun", "false")];
    let (status, body) = withdraw_with(&router, &params).await;
    assert_eq!(
        status, 200,
        "DryRun=false must perform the withdrawal, got {status} with body {body}"
    );
    let reported_state = reported("DryRun=false", &body, "state");
    assert_eq!(
        reported_state, BYOIP_CIDR_STATE_PENDING_WITHDRAWAL,
        "DryRun=false must withdraw the range, got {reported_state:?} in {body}"
    );
}
