// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AssociateIpamByoasn`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=AssociateIpamByoasn`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: `AssociateIpamByoasn` ASSOCIATES AN ASN THE ACCOUNT HAS BROUGHT TO
//! IPAM WITH A BYOIP ADDRESS RANGE IPAM MANAGES — the association is written onto that
//! range, where `DescribeByoipCidrs` reports it ("You can view the association with
//! DescribeByoipCidrs", `AssociateIpamByoasn`), and it starts in the non-terminal
//! `pending-association` the clock then moves to `associated` ("AFTER the association
//! succeeds, the ASN is eligible for advertisement"). A range IPAM does not manage — one the
//! account never brought, or one it brought but never moved to IPAM — is refused as not
//! found; an ASN the account has not brought to IPAM is refused as not found; an ASN whose
//! own provision has not finished is refused as being in the wrong state; a range-and-ASN
//! pair that already holds an association is refused as already associated; and a request
//! whose `Asn` or `Cidr` is not one of those at all is refused before anything is looked up,
//! as is a `DryRun` request.
//!
//! Every table row is a distinct input along what that generalises over — the ASN's width,
//! which address family and prefix length the range has, whether the `Cidr` is written in
//! canonical form, which `AsnState` the ASN sits in, which `AsnAssociationState` an existing
//! association sits in — so no single recorded request is the specification.
//!
//! The ASNs used are from the blocks RFC 5398 sets aside for documentation (64496-64511 for
//! 2-byte ASNs, 65536-65551 for 4-byte ASNs) and the ranges from RFC 5737 (`192.0.2.0/24`,
//! `198.51.100.0/24`, `203.0.113.0/24`), RFC 3849 (`2001:db8::/32`) and the RFC 2544
//! benchmarking block (`198.18.0.0/15`), so no row names an ASN or an address range that
//! belongs to anybody.
//!
//! The store is seeded directly, the way `byoip_cidr_lifecycle` and `describe_vpcs` seed
//! theirs: these rows need an ASN sitting in a chosen `AsnState` and an association sitting
//! in a chosen `AsnAssociationState`, and `ProvisionIpamByoasn` — the operation that brings
//! an ASN to IPAM — is still a scaffold, so the API has no path to either.
//!
//! Each refusal's MESSAGE is checked only for the substitution the service makes — the value
//! the request sent, named back — never for wording this emulator has no rule for.

use std::time::Duration;

use ec2::ops::types::{ASN_ASSOCIATION_STATE_VALUES, ASN_STATE_VALUES};
use ec2::state::{
    BYOIP_CIDR_STATE_PROVISIONED, ByoasnRecord, ByoipCidrRecord,
    ComAmazonawsEc2Amazonec2State as Ec2State, MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR,
    PendingStateMove, TRANSITION_DELAY, canonical_byoip_cidr,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, unescaped};

/// The Region every call here is signed for. A BYOIP range and a BYOASN are both named by
/// themselves and this store holds the account's, not a Region's, so the Region only selects
/// the ec2Query envelope.
const REGION: &str = "us-west-2";

/// The IPAM pool a seeded range is managed by — `ByoipCidr$IpamPoolId` is what says IPAM
/// manages a range, and `MoveByoipCidrToIpam` is the call that sets it.
const POOL_ID: &str = "ipam-pool-00000000000000001";

/// The IPAM a seeded ASN was brought to (`Byoasn$IpamId`).
const IPAM_ID: &str = "ipam-00000000000000001";

/// The `AsnState` of an ASN IPAM has finished bringing to the account — the one state an
/// association can be made from. Spelled out here rather than imported, so the service
/// cannot make this suite pass by agreeing with itself.
const PROVISIONED: &str = "provisioned";

/// The `AsnAssociationState` an association starts in.
const PENDING_ASSOCIATION: &str = "pending-association";

/// The `AsnAssociationState` the clock moves it to.
const ASSOCIATED: &str = "associated";

/// The `AsnAssociationState`s in which a range-and-ASN pair holds NO association, so the
/// pair can be associated again: the one whose disassociation finished and the one that
/// never took (`smithy_query(query="shape",shape="AsnAssociationState")`).
const UNHELD_ASSOCIATION_STATES: &[&str] = &["disassociated", "failed-association"];

/// EC2's code for an address range the request named that IPAM does not manage.
const CIDR_NOT_FOUND: &str = "InvalidCidr.NotFound";

/// EC2's code for an ASN the account has not brought to IPAM.
const BYOASN_NOT_FOUND: &str = "InvalidByoasn.NotFound";

/// EC2's code for a resource that exists but is not in a state admitting the request.
const INCORRECT_STATE: &str = "IncorrectState";

/// EC2's code for an association the request asked for that is already in place.
const ALREADY_ASSOCIATED: &str = "Resource.AlreadyAssociated";

/// EC2's code for a parameter value the service cannot read.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's code for a parameter that reached the handler empty.
const MISSING_PARAMETER: &str = "MissingParameter";

/// The code the boundary validator refuses a request missing a modelled `@required` member
/// with, before any handler logic runs.
const MALFORMED_INPUT: &str = "MalformedInput";

/// EC2's code for a `DryRun` request that would otherwise have been served, and the status
/// this emulator answers it with (`Precondition Failed`, the status EC2's own dry-run
/// refusal carries).
const DRY_RUN_OPERATION: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;

/// EC2's code for a request that would take the store past a ceiling of this emulator's own.
const RESOURCE_LIMIT_EXCEEDED: &str = "ResourceLimitExceeded";

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `AssociateIpamByoasn` call carrying `params`, as its HTTP status and XML body.
async fn associate(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "AssociateIpamByoasn", params).await
}

/// One `AssociateIpamByoasn` call for `asn` and `cidr`.
async fn associate_pair(router: &MultiServiceRouter, asn: &str, cidr: &str) -> (u16, String) {
    associate(router, &[p("Asn", asn), p("Cidr", cidr)]).await
}

/// Seed one BYOIP range the account holds, managed by IPAM when `in_ipam`, and answer its
/// canonical CIDR.
fn seed_range(engine: &engine::InMemoryEngine, cidr: &str, in_ipam: bool) -> String {
    let mut record = ByoipCidrRecord::in_state(cidr, BYOIP_CIDR_STATE_PROVISIONED);
    if in_ipam {
        record.byoip_cidr.ipam_pool_id = Some(POOL_ID.to_string());
    }
    let stored = engine.state::<Ec2State>().lock().insert_byoip_cidr(record);
    assert!(stored, "the store has room for the seeded range {cidr}");
    canonical_byoip_cidr(cidr).unwrap_or_else(|_| panic!("{cidr} is an address range"))
}

/// Seed one BYOIP range in IPAM that already holds an association for `asn` in
/// `association_state`, and answer its canonical CIDR.
fn seed_range_with_association(
    engine: &engine::InMemoryEngine,
    cidr: &str,
    asn: &str,
    association_state: &str,
) -> String {
    let canonical = seed_range(engine, cidr, true);
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut record = guard
        .byoip_cidrs
        .get(&canonical)
        .cloned()
        .expect("the range was just seeded");
    record.byoip_cidr.asn_associations = Some(vec![ec2::ops::types::AsnAssociation {
        asn: Some(asn.to_string()),
        cidr: Some(canonical.clone()),
        state: Some(association_state.to_string()),
        status_message: None,
    }]);
    assert!(
        guard.insert_byoip_cidr(record),
        "the seeded range {canonical} is already a key, so it must be stored"
    );
    canonical
}

/// Seed one ASN the account has brought to IPAM, sitting in `state` with nothing owed to it
/// by the clock.
fn seed_asn(engine: &engine::InMemoryEngine, asn: &str, state: &str) {
    let stored = engine
        .state::<Ec2State>()
        .lock()
        .insert_byoasn(ByoasnRecord::in_state(asn, IPAM_ID, state));
    assert!(stored, "the store has room for the seeded ASN {asn}");
}

/// Seed one ASN whose provision is still under way, with the clock owing it `provisioned`.
fn seed_asn_being_provisioned(engine: &engine::InMemoryEngine, asn: &str) {
    let mut record = ByoasnRecord::in_state(asn, IPAM_ID, "pending-provision");
    record.pending = Some(PendingStateMove::starting_now(PROVISIONED));
    let stored = engine.state::<Ec2State>().lock().insert_byoasn(record);
    assert!(stored, "the store has room for the seeded ASN {asn}");
}

/// The `AsnAssociationState` the STORE holds for the pair `(cidr, asn)`, read straight from
/// it so it reports what a later caller would be told without a call of its own
/// materialising anything.
fn stored_association(engine: &engine::InMemoryEngine, cidr: &str, asn: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .asn_association(cidr, asn)
        .and_then(|association| association.state.clone())
}

/// Every association the store holds for `cidr`, as `(asn, state)` pairs.
fn stored_associations(engine: &engine::InMemoryEngine, cidr: &str) -> Vec<(String, String)> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .byoip_cidrs
        .get(cidr)
        .and_then(|record| record.byoip_cidr.asn_associations.clone())
        .unwrap_or_default()
        .into_iter()
        .map(|association| {
            (
                association.asn.unwrap_or_default(),
                association.state.unwrap_or_default(),
            )
        })
        .collect()
}

/// Every association `DescribeByoipCidrs` REPORTS, over every range it answers with, as
/// `(asn, state)` pairs — the reader the operation's own documentation points at.
async fn described_associations(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeByoipCidrs",
        &[p("MaxResults", "100")],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeByoipCidrs: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "asnAssociationSet")
        .iter()
        .flat_map(|set| elements(set, "item"))
        .map(|item| {
            (
                element(&item, "asn").unwrap_or_default(),
                element(&item, "state").unwrap_or_default(),
            )
        })
        .collect()
}

/// Wait until a move started before now has certainly fallen due. Derived from the service's
/// own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// The association itself
// ---------------------------------------------------------------------------

/// "Associates your Autonomous System Number (ASN) with a BYOIP CIDR that you own" — the
/// association is answered in `pending-association`, carries the ASN and the range it was
/// made for, and is on the range from that instant on, where `DescribeByoipCidrs` reports
/// it.
#[tokio::test]
async fn an_asn_brought_to_ipam_is_associated_with_a_range_ipam_manages() {
    // `(case, the Asn, the Cidr as sent, the range the association is made for)`. The ASNs
    // are RFC 5398 documentation ASNs of both widths; the ranges span both families, three
    // prefix lengths and a block written with its host bits set.
    let rows: &[(&str, &str, &str, &str)] = &[
        (
            "a 2-byte ASN and an IPv4 /24",
            "64496",
            "192.0.2.0/24",
            "192.0.2.0/24",
        ),
        (
            "the top of the documentation 2-byte block",
            "64511",
            "198.51.100.0/24",
            "198.51.100.0/24",
        ),
        (
            "a 4-byte ASN and an IPv6 /32",
            "65536",
            "2001:db8::/32",
            "2001:db8::/32",
        ),
        (
            "the top of the documentation 4-byte block and an IPv4 /15",
            "65551",
            "198.18.0.0/15",
            "198.18.0.0/15",
        ),
        (
            "a Cidr written with host bits set",
            "64500",
            "203.0.113.5/24",
            "203.0.113.0/24",
        ),
    ];

    for (case, asn, sent, expected_cidr) in rows {
        let (router, engine) = emulator();
        let canonical = seed_range(&engine, sent, true);
        assert_eq!(
            canonical, *expected_cidr,
            "case {case:?}: the store keys {sent} under {expected_cidr}, got {canonical}"
        );
        seed_asn(&engine, asn, PROVISIONED);

        let (status, body) = associate_pair(&router, asn, sent).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "asn").as_deref(),
            Some(*asn),
            "case {case:?}: expected the association to name the ASN {asn}, got body {body}"
        );
        assert_eq!(
            element(&body, "cidr").as_deref(),
            Some(*expected_cidr),
            "case {case:?}: expected the association to name {expected_cidr}, got body {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(PENDING_ASSOCIATION),
            "case {case:?}: expected the association in {PENDING_ASSOCIATION}, got body {body}"
        );

        // On the range from that instant on, and reported by the reader the operation's
        // documentation points at.
        let stored = stored_associations(&engine, &canonical);
        assert_eq!(
            stored,
            vec![((*asn).to_string(), PENDING_ASSOCIATION.to_string())],
            "case {case:?}: expected the range to hold exactly that association, got {stored:?}"
        );
        let described = described_associations(&router).await;
        assert_eq!(
            described,
            vec![((*asn).to_string(), PENDING_ASSOCIATION.to_string())],
            "case {case:?}: expected DescribeByoipCidrs to report it, got {described:?}"
        );
    }
}

/// "After the association succeeds, the ASN is eligible for advertisement": the move out of
/// `pending-association` is the CLOCK's, due at its instant whether or not anybody called,
/// and it is applied once — nothing stays owed afterwards.
#[tokio::test]
async fn the_clock_puts_a_pending_association_in_force() {
    // `(case, the Asn, the range)` — two pairs in one store, so the move is shown to be per
    // association rather than one global flag.
    let rows: &[(&str, &str, &str)] = &[
        ("an IPv4 range", "64496", "192.0.2.0/24"),
        ("an IPv6 range", "65540", "2001:db8::/32"),
    ];

    let (router, engine) = emulator();
    for (case, asn, cidr) in rows {
        seed_range(&engine, cidr, true);
        seed_asn(&engine, asn, PROVISIONED);
        let (status, body) = associate_pair(&router, asn, cidr).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            stored_association(&engine, cidr, asn).as_deref(),
            Some(PENDING_ASSOCIATION),
            "case {case:?}: expected {PENDING_ASSOCIATION} straight after the call, got {:?}",
            stored_association(&engine, cidr, asn)
        );
    }

    past_the_transition().await;
    // A call is what applies a due move, and it applies every one that has fallen due.
    let described = described_associations(&router).await;
    for (case, asn, cidr) in rows {
        assert_eq!(
            stored_association(&engine, cidr, asn).as_deref(),
            Some(ASSOCIATED),
            "case {case:?}: expected {ASSOCIATED} once the move fell due, got {:?}",
            stored_association(&engine, cidr, asn)
        );
        assert!(
            described.contains(&((*asn).to_string(), ASSOCIATED.to_string())),
            "case {case:?}: expected DescribeByoipCidrs to report {asn} as {ASSOCIATED}, \
             got {described:?}"
        );
    }
    let owed = engine
        .state::<Ec2State>()
        .lock()
        .asn_association_moves
        .len();
    assert_eq!(
        owed, 0,
        "a move the clock has applied is not owed again, got {owed} still owed"
    );
}

// ---------------------------------------------------------------------------
// A range IPAM does not manage
// ---------------------------------------------------------------------------

/// The BYOASN path is addressed by a range IPAM manages (`ByoipCidr$IpamPoolId`), so a range
/// the account never brought and a range it brought but never moved to IPAM are equally
/// absent to it. Nothing the store holds changes.
#[tokio::test]
async fn a_range_ipam_does_not_manage_is_refused_as_not_found() {
    // `(case, the Cidr under test)`, against a store that holds 192.0.2.0/24 in IPAM and
    // 203.0.113.0/24 outside it.
    let rows: &[(&str, &str)] = &[
        ("a range the account never brought", "198.51.100.0/24"),
        (
            "a range brought to AWS but never moved to IPAM",
            "203.0.113.0/24",
        ),
        ("a /25 inside the range IPAM manages", "192.0.2.0/25"),
        ("a /23 containing the range IPAM manages", "192.0.2.0/23"),
        ("the neighbouring /24", "192.0.3.0/24"),
        ("an IPv6 range the account never brought", "2001:db8::/32"),
        ("an IPv6 /48 inside it", "2001:db8:1::/48"),
    ];

    let (router, engine) = emulator();
    let in_ipam = seed_range(&engine, "192.0.2.0/24", true);
    let outside_ipam = seed_range(&engine, "203.0.113.0/24", false);
    seed_asn(&engine, "64496", PROVISIONED);

    for (case, cidr) in rows {
        let (status, body) = associate_pair(&router, "64496", cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {cidr}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, CIDR_NOT_FOUND,
            "case {case:?}: expected {CIDR_NOT_FOUND} for {cidr}, got {code:?} with body {body}"
        );
        // The message is built from the request: it names the range the caller asked about.
        let message = element(&body, "Message").map(|text| unescaped(&text));
        assert_eq!(
            message.as_deref(),
            Some(format!("The CIDR ({cidr}) could not be found in IPAM.").as_str()),
            "case {case:?}: expected the message to name {cidr}, got {message:?}"
        );
        for range in [&in_ipam, &outside_ipam] {
            let held = stored_associations(&engine, range);
            assert!(
                held.is_empty(),
                "case {case:?}: a refusal must leave {range} alone, got {held:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// An ASN the account has not brought to IPAM, or has not finished bringing
// ---------------------------------------------------------------------------

/// Only an ASN the account has brought to IPAM is its own to associate: every other ASN is
/// refused as not found, whatever the range is.
#[tokio::test]
async fn an_asn_the_account_has_not_brought_to_ipam_is_refused_as_not_found() {
    // `(case, the Asn under test)`, against a store that holds the ASN 64496 only.
    let rows: &[(&str, &str)] = &[
        ("another 2-byte ASN", "64497"),
        ("a 4-byte ASN", "65536"),
        ("the largest ASN there is", "4294967294"),
        ("the smallest ASN there is", "1"),
    ];

    let (router, engine) = emulator();
    let cidr = seed_range(&engine, "192.0.2.0/24", true);
    seed_asn(&engine, "64496", PROVISIONED);

    for (case, asn) in rows {
        let (status, body) = associate_pair(&router, asn, &cidr).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for {asn}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, BYOASN_NOT_FOUND,
            "case {case:?}: expected {BYOASN_NOT_FOUND} for {asn}, got {code:?} with body {body}"
        );
        let message = element(&body, "Message").map(|text| unescaped(&text));
        assert_eq!(
            message.as_deref(),
            Some(format!("The ASN '{asn}' does not exist").as_str()),
            "case {case:?}: expected the message to name {asn}, got {message:?}"
        );
        let held = stored_associations(&engine, &cidr);
        assert!(
            held.is_empty(),
            "case {case:?}: a refusal must leave the range alone, got {held:?}"
        );
    }
}

/// "The provisioning state of the BYOASN" (`Byoasn$State`): an association can only be made
/// from the one state in which IPAM has finished bringing the ASN to the account. Every
/// other member of `AsnState` — an ASN part way through a move, one whose provision failed,
/// one given back — is refused as being in the wrong state, and nothing is written.
#[tokio::test]
async fn an_asn_whose_provision_has_not_finished_is_refused_as_in_the_wrong_state() {
    // Every `AsnState` the model declares is a row, so a state added to the catalog shows
    // up here instead of being silently accepted.
    assert!(
        ASN_STATE_VALUES.contains(&PROVISIONED),
        "the model's AsnState catalog carries {PROVISIONED}, got {ASN_STATE_VALUES:?}"
    );

    for state in ASN_STATE_VALUES {
        let (router, engine) = emulator();
        let cidr = seed_range(&engine, "192.0.2.0/24", true);
        seed_asn(&engine, "64496", state);

        let (status, body) = associate_pair(&router, "64496", &cidr).await;
        if *state == PROVISIONED {
            assert_eq!(
                status, 200,
                "case {state:?}: expected HTTP 200, got {status} with body {body}"
            );
            assert_eq!(
                stored_association(&engine, &cidr, "64496").as_deref(),
                Some(PENDING_ASSOCIATION),
                "case {state:?}: expected the association to be written, got {:?}",
                stored_association(&engine, &cidr, "64496")
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {state:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INCORRECT_STATE,
            "case {state:?}: expected {INCORRECT_STATE}, got {code:?} with body {body}"
        );
        let held = stored_associations(&engine, &cidr);
        assert!(
            held.is_empty(),
            "case {state:?}: a refusal must write nothing, got {held:?}"
        );
    }
}

/// The provision the clock is still finishing is the same fact: the ASN is refused while its
/// move is owed and associable once it has fallen due, with no second call needed to push it.
#[tokio::test]
async fn an_asn_becomes_associable_once_its_provision_falls_due() {
    let (router, engine) = emulator();
    let cidr = seed_range(&engine, "192.0.2.0/24", true);
    seed_asn_being_provisioned(&engine, "64496");

    let (status, body) = associate_pair(&router, "64496", &cidr).await;
    assert_eq!(
        status, 400,
        "an ASN still being provisioned: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INCORRECT_STATE,
        "an ASN still being provisioned: expected {INCORRECT_STATE}, got {code:?}"
    );

    past_the_transition().await;

    let (status, body) = associate_pair(&router, "64496", &cidr).await;
    assert_eq!(
        status, 200,
        "once the provision fell due: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(PENDING_ASSOCIATION),
        "once the provision fell due: expected {PENDING_ASSOCIATION}, got body {body}"
    );
}

// ---------------------------------------------------------------------------
// A pair that already holds an association
// ---------------------------------------------------------------------------

/// One range-and-ASN pair holds at most one association: a pair that already holds one is
/// refused as already associated and keeps the association it had, while a pair whose
/// association is not there — its disassociation finished, or it never took — is associated
/// again, replacing that entry rather than adding a second.
#[tokio::test]
async fn a_pair_that_already_holds_an_association_is_refused() {
    // Every `AsnAssociationState` the model declares is a row.
    for state in ASN_ASSOCIATION_STATE_VALUES {
        let (router, engine) = emulator();
        let cidr = seed_range_with_association(&engine, "192.0.2.0/24", "64496", state);
        seed_asn(&engine, "64496", PROVISIONED);

        let (status, body) = associate_pair(&router, "64496", &cidr).await;
        if UNHELD_ASSOCIATION_STATES.contains(state) {
            assert_eq!(
                status, 200,
                "case {state:?}: a pair holding no association is associable, \
                 got {status} with body {body}"
            );
            let held = stored_associations(&engine, &cidr);
            assert_eq!(
                held,
                vec![("64496".to_string(), PENDING_ASSOCIATION.to_string())],
                "case {state:?}: expected the one association to be replaced, got {held:?}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {state:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, ALREADY_ASSOCIATED,
            "case {state:?}: expected {ALREADY_ASSOCIATED}, got {code:?} with body {body}"
        );
        let held = stored_associations(&engine, &cidr);
        assert_eq!(
            held,
            vec![("64496".to_string(), (*state).to_string())],
            "case {state:?}: a refusal must leave the association it had, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// A request that is not an association request at all
// ---------------------------------------------------------------------------

/// One range holds a FINITE number of associations: both the range and the ASN come from the
/// request and an association enlarges a stored record, so the call past this emulator's own
/// per-range ceiling is refused and nothing is written. The ceiling is read from the service
/// rather than copied, so the two cannot drift.
#[tokio::test]
async fn an_association_past_the_per_range_ceiling_is_refused() {
    let (router, engine) = emulator();
    let cidr = seed_range(&engine, "192.0.2.0/24", true);
    // One more ASN than the range can hold associations for. The ASNs are a contiguous run
    // from the RFC 5398 4-byte documentation block upwards, so each row is a distinct ASN
    // and none is an ASN that belongs to anybody.
    let asns: Vec<String> = (0..=MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR)
        .map(|offset| (65_536 + offset).to_string())
        .collect();
    for asn in &asns {
        seed_asn(&engine, asn, PROVISIONED);
    }

    for (index, asn) in asns.iter().enumerate() {
        let (status, body) = associate_pair(&router, asn, &cidr).await;
        if index < MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR {
            assert_eq!(
                status, 200,
                "association {index} of {MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR}: \
                 expected HTTP 200, got {status} with body {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "the association past the ceiling: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, RESOURCE_LIMIT_EXCEEDED,
            "the association past the ceiling: expected {RESOURCE_LIMIT_EXCEEDED}, \
             got {code:?} with body {body}"
        );
    }

    let held = stored_associations(&engine, &cidr);
    assert_eq!(
        held.len(),
        MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR,
        "the refused association must write nothing, got {held:?}"
    );
    assert!(
        held.iter().all(|(asn, _)| asn != &asns[MAX_ASN_ASSOCIATIONS_PER_BYOIP_CIDR]),
        "the refused ASN must not be on the range, got {held:?}"
    );
}

/// "A public 2-byte or 4-byte ASN" (`Asn`) and "The BYOIP CIDR you want to associate with an
/// ASN" (`Cidr`): a value that is neither names nothing to associate, so it is refused
/// before anything is looked up — and a request carrying no such member at all is missing a
/// parameter rather than naming an unknown resource.
#[tokio::test]
async fn a_request_whose_asn_or_cidr_is_not_one_is_refused_before_any_lookup() {
    // `(case, the parameters, the expected code)`.
    let rows: Vec<(&str, Params, &str)> = vec![
        (
            "an Asn that is not a number",
            vec![p("Asn", "sixty-four-thousand"), p("Cidr", "192.0.2.0/24")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn of zero",
            vec![p("Asn", "0"), p("Cidr", "192.0.2.0/24")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn past the 4-byte width",
            vec![p("Asn", "4294967296"), p("Cidr", "192.0.2.0/24")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a negative Asn",
            vec![p("Asn", "-64496"), p("Cidr", "192.0.2.0/24")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn written with a leading zero",
            vec![p("Asn", "064496"), p("Cidr", "192.0.2.0/24")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn in dotted notation",
            vec![p("Asn", "1.10"), p("Cidr", "192.0.2.0/24")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an empty Asn",
            vec![p("Asn", ""), p("Cidr", "192.0.2.0/24")],
            MISSING_PARAMETER,
        ),
        (
            "no Asn parameter at all",
            vec![p("Cidr", "192.0.2.0/24")],
            MALFORMED_INPUT,
        ),
        (
            "a Cidr that is not an address range",
            vec![p("Asn", "64496"), p("Cidr", "not-an-address-range")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "a Cidr with no prefix length",
            vec![p("Asn", "64496"), p("Cidr", "192.0.2.0")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv4 prefix length past 32",
            vec![p("Asn", "64496"), p("Cidr", "192.0.2.0/33")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv6 prefix length past 128",
            vec![p("Asn", "64496"), p("Cidr", "2001:db8::/129")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an empty Cidr",
            vec![p("Asn", "64496"), p("Cidr", "")],
            MISSING_PARAMETER,
        ),
        (
            "no Cidr parameter at all",
            vec![p("Asn", "64496")],
            MALFORMED_INPUT,
        ),
        (
            // Both members are read before anything is resolved, in the order the operation
            // names them, so the Asn's own refusal is the one answered.
            "both the Asn and the Cidr unreadable",
            vec![p("Asn", "sixty-four"), p("Cidr", "not-an-address-range")],
            INVALID_PARAMETER_VALUE,
        ),
    ];

    let (router, engine) = emulator();
    let cidr = seed_range(&engine, "192.0.2.0/24", true);
    seed_asn(&engine, "64496", PROVISIONED);

    for (case, params, expected) in rows {
        let (status, body) = associate(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} with body {body}"
        );
        let held = stored_associations(&engine, &cidr);
        assert!(
            held.is_empty(),
            "case {case:?}: a refused request must write nothing, got {held:?}"
        );
    }
}

/// "Checks whether you have the required permissions for the action, without actually making
/// the request" (`DryRun`): the flag is about PERMISSION, so it is answered once the
/// request's own form has been read and before either resource is resolved — including for a
/// range and an ASN the account does not hold — and nothing is written.
#[tokio::test]
async fn a_dry_run_is_answered_from_the_permission_check_alone() {
    // `(case, the Asn, the Cidr)` — a pair that would be served, and pairs naming resources
    // that are not there at all.
    let rows: &[(&str, &str, &str)] = &[
        ("a pair that would be served", "64496", "192.0.2.0/24"),
        ("a range IPAM does not manage", "64496", "198.51.100.0/24"),
        ("an ASN the account has not brought", "65536", "192.0.2.0/24"),
    ];

    let (router, engine) = emulator();
    let cidr = seed_range(&engine, "192.0.2.0/24", true);
    seed_asn(&engine, "64496", PROVISIONED);

    for (case, asn, requested) in rows {
        let (status, body) = associate(
            &router,
            &[p("Asn", asn), p("Cidr", requested), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION}, got {code:?} with body {body}"
        );
        let held = stored_associations(&engine, &cidr);
        assert!(
            held.is_empty(),
            "case {case:?}: a dry run must write nothing, got {held:?}"
        );
    }
}
