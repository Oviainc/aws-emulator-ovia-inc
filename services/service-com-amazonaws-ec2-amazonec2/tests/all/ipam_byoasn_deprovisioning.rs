// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeprovisionIpamByoasn`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DeprovisionIpamByoasn`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: GIVING A RESOURCE BACK OUT OF THE CONTAINER THE REQUEST NAMES
//! resolves the CONTAINER first — one the account holds none of is refused with that
//! container's own `NotFound`, and something that is not an id of that kind at all with its
//! `Malformed` — then the member inside it, where a member that container does not hold is
//! refused with the member's own `NotFound` and one that is not sitting still is refused for
//! the state it is in; and a request that gets past all of that leaves the member in its
//! NON-TERMINAL `pending-deprovision` state and answers it there, the CLOCK — never a later
//! caller — deciding what the release ends as from what still depends on the member at the
//! due instant.
//!
//! Here the container is an IPAM and the member an Autonomous System Number the account
//! brought to it, and what still depends on it are the BYOIP CIDR associations it holds:
//! "This action can only be called after any BYOIP CIDR associations are removed from your
//! Amazon Web Services account with DisassociateIpamByoasn" (`DeprovisionIpamByoasn`), so a
//! release with one still in place ends in `failed-deprovision` carrying why, exactly as a
//! BYOIP range's release ends when addresses are still allocated out of it.
//!
//! Each table row is a distinct input along what that generalises over — the ASN's width,
//! which `AsnState` it sits in (the whole published catalog, split into the states a release
//! is admissible from and the states it is not), which `AsnAssociationState` an association
//! sits in (again the whole catalog), how many ranges hold one, which member of the request
//! is at fault and in which way — so no single recorded request is the specification.
//!
//! The ASNs used are from the blocks RFC 5398 sets aside for documentation (64496-64511 for
//! 2-byte ASNs, 65536-65551 for 4-byte ASNs) and the ranges from RFC 5737 (`192.0.2.0/24`,
//! `198.51.100.0/24`), so no row names an ASN or an address range that belongs to anybody.
//!
//! The IPAM every row names is created through `CreateIpam`, the operation that owns it. The
//! ASN is seeded into the store, the way `ipam_byoasn_associations` and `ipam_byoasns` seed
//! theirs: `ProvisionIpamByoasn` — the operation that brings an ASN to IPAM — is still a
//! scaffold, so the API has no path to an ASN in a chosen `AsnState` yet.

use std::collections::BTreeSet;
use std::time::Duration;

use ec2::ops::types::{ASN_ASSOCIATION_STATE_VALUES, ASN_STATE_VALUES};
use ec2::state::{
    BYOIP_CIDR_STATE_PROVISIONED, ByoasnRecord, ByoipCidrRecord,
    ComAmazonawsEc2Amazonec2State as Ec2State, TRANSITION_DELAY, canonical_byoip_cidr,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{Param, child_text, document, param as p, set_items};

/// The Region every call here is signed for, which is therefore each IPAM's home Region. An
/// ASN is named by itself and this store holds the account's, not a Region's, so the Region
/// only selects the ec2Query envelope and the IPAM's home Region.
const REGION: &str = "us-west-2";

/// The IPAM pool a seeded range is managed by — `ByoipCidr$IpamPoolId` is what says IPAM
/// manages a range, and `MoveByoipCidrToIpam` is the call that sets it.
const POOL_ID: &str = "ipam-pool-00000000000000001";

// The `AsnState` words this suite asserts. Spelled out here rather than imported, so the
// service cannot make this suite pass by agreeing with itself.

/// The `AsnState` of an ASN IPAM has finished bringing to the account.
const PROVISIONED: &str = "provisioned";

/// The `AsnState` of an ASN whose provision did not complete.
const FAILED_PROVISION: &str = "failed-provision";

/// The `AsnState` of an ASN whose release is under way.
const PENDING_DEPROVISION: &str = "pending-deprovision";

/// The `AsnState` of an ASN the account has finished giving back.
const DEPROVISIONED: &str = "deprovisioned";

/// The `AsnState` of an ASN whose release did not complete.
const FAILED_DEPROVISION: &str = "failed-deprovision";

/// The `AsnState`s a release is admissible from: the ASN is the account's and sitting still.
const RELEASABLE_STATES: &[&str] = &[PROVISIONED, FAILED_PROVISION, FAILED_DEPROVISION];

/// The `AsnState`s a release is NOT admissible from: the ASN is part way through a move the
/// clock has not finished, or it has already been given back. Together with
/// [`RELEASABLE_STATES`] this is the whole published catalog
/// (`smithy_query(query="shape",shape="AsnState")`), which
/// [`the_state_catalog_is_split_between_the_two_tables`] pins.
const UNRELEASABLE_STATES: &[&str] = &["pending-provision", PENDING_DEPROVISION, DEPROVISIONED];

/// The `AsnAssociationState`s in which a range-and-ASN pair holds NO association, so such a
/// pair holds no release up (`smithy_query(query="shape",shape="AsnAssociationState")`).
const UNHELD_ASSOCIATION_STATES: &[&str] = &["disassociated", "failed-association"];

/// EC2's code for an IPAM id the account holds no IPAM for.
const IPAM_ID_NOT_FOUND: &str = "InvalidIpamId.NotFound";

/// EC2's code for something that is not an IPAM id at all.
const IPAM_ID_MALFORMED: &str = "InvalidIpamId.Malformed";

/// EC2's code for an ASN the IPAM has not brought to the account.
const BYOASN_NOT_FOUND: &str = "InvalidByoasn.NotFound";

/// EC2's code for a resource that exists but is not in a state admitting the request.
const INCORRECT_STATE: &str = "IncorrectState";

/// EC2's code for a parameter value the service cannot read.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// EC2's code for a parameter that reached the handler empty.
const MISSING_PARAMETER: &str = "MissingParameter";

/// The code the boundary validator refuses a request missing a modelled `@required` member
/// with, before any handler logic runs.
const MALFORMED_INPUT: &str = "MalformedInput";

/// EC2's code for a `DryRun` request that would otherwise have been served, and the status
/// this emulator answers it with.
const DRY_RUN_OPERATION: &str = "DryRunOperation";
const DRY_RUN_STATUS: u16 = 412;

/// Longer than the span the clock owes an ASN whose release is under way, so a read after
/// this much has certainly passed the due instant.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(250)
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// One `DeprovisionIpamByoasn` call carrying `params`, as its HTTP status and XML body.
async fn deprovision(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, REGION, "DeprovisionIpamByoasn", params).await
}

/// One `DeprovisionIpamByoasn` call naming `ipam_id` and `asn`.
async fn deprovision_asn(
    router: &MultiServiceRouter,
    ipam_id: &str,
    asn: &str,
) -> (u16, String) {
    deprovision(router, &[p("IpamId", ipam_id), p("Asn", asn)]).await
}

/// Create one IPAM through the operation that owns it, and answer the `IpamId` a request
/// names it by.
async fn create_ipam(case: &str, router: &MultiServiceRouter) -> String {
    let (status, body) = call(router, REGION, "CreateIpam", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateIpam, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "ipam")
        .and_then(|ipam| child_text(ipam, "ipamId"))
        .unwrap_or_else(|| panic!("case {case:?}: no IPAM id in {body}"))
}

/// One ASN as a response reports it: every member `Byoasn` carries, so a member the store
/// holds and the answer drops cannot pass unnoticed.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reported {
    asn: Option<String>,
    ipam_id: Option<String>,
    state: Option<String>,
    status_message: Option<String>,
}

/// The ASN a `DeprovisionIpamByoasn` answer reports.
fn reported(case: &str, body: &str) -> Reported {
    let parsed = document(case, body);
    let byoasn = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == "byoasn")
        .unwrap_or_else(|| panic!("case {case:?}: no <byoasn> in {body}"));
    Reported {
        asn: child_text(byoasn, "asn"),
        ipam_id: child_text(byoasn, "ipamId"),
        state: child_text(byoasn, "state"),
        status_message: child_text(byoasn, "statusMessage"),
    }
}

/// Seed one ASN the account has brought to the IPAM `ipam_id`, sitting in `state` with
/// nothing owed to it by the clock.
fn seed_asn(engine: &engine::InMemoryEngine, asn: &str, ipam_id: &str, state: &str) {
    let stored = engine
        .state::<Ec2State>()
        .lock()
        .insert_byoasn(ByoasnRecord::in_state(asn, ipam_id, state));
    assert!(stored, "the store has room for the seeded ASN {asn}");
}

/// Seed one BYOIP range IPAM manages that holds an association for `asn` in
/// `association_state`, and answer its canonical CIDR.
fn seed_range_with_association(
    engine: &engine::InMemoryEngine,
    cidr: &str,
    asn: &str,
    association_state: &str,
) -> String {
    let canonical =
        canonical_byoip_cidr(cidr).unwrap_or_else(|_| panic!("{cidr} is an address range"));
    let mut record = ByoipCidrRecord::in_state(cidr, BYOIP_CIDR_STATE_PROVISIONED);
    record.byoip_cidr.ipam_pool_id = Some(POOL_ID.to_string());
    record.byoip_cidr.asn_associations = Some(vec![ec2::ops::types::AsnAssociation {
        asn: Some(asn.to_string()),
        cidr: Some(canonical.clone()),
        state: Some(association_state.to_string()),
        status_message: None,
    }]);
    let stored = engine.state::<Ec2State>().lock().insert_byoip_cidr(record);
    assert!(stored, "the store has room for the seeded range {cidr}");
    canonical
}

/// The `AsnState` the STORE holds for `asn`, read straight from it so it reports what a
/// later caller would be told without a call of its own materialising anything.
fn stored_state(engine: &engine::InMemoryEngine, asn: &str) -> Option<String> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .byoasns
        .get(asn)
        .and_then(|record| record.byoasn.state.clone())
}

/// The ASN `DescribeIpamByoasn` REPORTS for `asn` — the reader of the store this operation
/// writes.
async fn described(router: &MultiServiceRouter, case: &str, asn: &str) -> Option<Reported> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeIpamByoasn",
        &[p("MaxResults", "100")],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeIpamByoasn, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    set_items(parsed.root_element(), "byoasnSet")
        .into_iter()
        .map(|item| Reported {
            asn: child_text(item, "asn"),
            ipam_id: child_text(item, "ipamId"),
            state: child_text(item, "state"),
            status_message: child_text(item, "statusMessage"),
        })
        .find(|entry| entry.asn.as_deref() == Some(asn))
}

/// The two state tables above are the whole published catalog, split in two: a state the
/// model adds later falls into neither and is caught here rather than going untested.
#[tokio::test]
async fn the_state_catalog_is_split_between_the_two_tables() {
    let published: BTreeSet<&str> = ASN_STATE_VALUES.iter().copied().collect();
    let covered: BTreeSet<&str> = RELEASABLE_STATES
        .iter()
        .chain(UNRELEASABLE_STATES.iter())
        .copied()
        .collect();
    assert_eq!(
        covered, published,
        "case \"the AsnState catalog\": every published state belongs to exactly one table, \
         covered {covered:?} against published {published:?}"
    );
}

/// An ASN sitting still in the IPAM the request names is given back: answered in the
/// non-terminal `pending-deprovision`, read back there by the describe, and moved to
/// `deprovisioned` by the clock rather than by a caller.
#[tokio::test]
async fn an_asn_sitting_still_is_given_back_and_reads_back() {
    // The states come from the table above, which the catalog test pins against the
    // published `AsnState` values — so a release is driven from every state it admits, not
    // only from the one the issue cited.
    let widths = [
        ("a 2-byte ASN", "64496"),
        ("a 4-byte ASN", "65536"),
    ];
    for state in RELEASABLE_STATES {
        for (width, asn) in widths {
            let case = format!("{width} sitting in {state}");
            let (router, engine) = emulator();
            let ipam_id = create_ipam(&case, &router).await;
            seed_asn(&engine, asn, &ipam_id, state);

            let (status, body) = deprovision_asn(&router, &ipam_id, asn).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let answered = reported(&case, &body);
            assert_eq!(
                answered,
                Reported {
                    asn: Some(asn.to_string()),
                    ipam_id: Some(ipam_id.clone()),
                    state: Some(PENDING_DEPROVISION.to_string()),
                    status_message: None,
                },
                "case {case:?}: expected the ASN answered in {PENDING_DEPROVISION} with no \
                 status message, got {answered:?}"
            );

            // The span is never zero: a caller reading straight back still sees the release
            // under way, in the store and through the describe alike.
            assert_eq!(
                stored_state(&engine, asn),
                Some(PENDING_DEPROVISION.to_string()),
                "case {case:?}: the release is not due yet, got {:?}",
                stored_state(&engine, asn)
            );
            let read_back = described(&router, &case, asn).await;
            assert_eq!(
                read_back.as_ref().and_then(|entry| entry.state.clone()),
                Some(PENDING_DEPROVISION.to_string()),
                "case {case:?}: the describe reports the release under way, got {read_back:?}"
            );

            tokio::time::sleep(past_the_transition()).await;

            let finished = described(&router, &case, asn).await;
            assert_eq!(
                finished,
                Some(Reported {
                    asn: Some(asn.to_string()),
                    ipam_id: Some(ipam_id.clone()),
                    state: Some(DEPROVISIONED.to_string()),
                    status_message: None,
                }),
                "case {case:?}: expected the ASN {DEPROVISIONED} once the clock owed it, got \
                 {finished:?}"
            );
        }
    }
}

/// An IPAM id the account holds no IPAM for is refused with the IPAM's own `NotFound`,
/// naming the id the request sent — whatever the ASN turns out to be, and writing nothing.
#[tokio::test]
async fn an_ipam_the_account_does_not_hold_is_refused() {
    let rows = [
        ("an IPAM id of the long form nothing holds", "ipam-0123456789abcdef0"),
        ("an IPAM id of the short form nothing holds", "ipam-01234567"),
    ];
    let asn = "64498";

    for (case, unheld) in rows {
        let (router, engine) = emulator();
        // An IPAM the account DOES hold, holding the ASN: the refusal is about the id the
        // request named, not about the account holding no IPAM at all.
        let held = create_ipam(case, &router).await;
        seed_asn(&engine, asn, &held, PROVISIONED);

        let (status, body) = deprovision_asn(&router, unheld, asn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, IPAM_ID_NOT_FOUND,
            "case {case:?}: expected {IPAM_ID_NOT_FOUND}, got {code:?} with body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(unheld),
            "case {case:?}: the refusal names the id the request sent, got {message:?}"
        );
        assert_eq!(
            stored_state(&engine, asn),
            Some(PROVISIONED.to_string()),
            "case {case:?}: a refused request writes nothing, got {:?}",
            stored_state(&engine, asn)
        );
    }
}

/// An ASN the IPAM the request names has not brought to the account is refused with the
/// ASN's own `NotFound` — including one brought to a DIFFERENT IPAM of the same account,
/// which that IPAM still holds afterwards.
#[tokio::test]
async fn an_asn_the_named_ipam_does_not_hold_is_refused() {
    for case in [
        "an ASN the account has not brought to IPAM",
        "an ASN brought to another IPAM of the account",
    ] {
        let brought_elsewhere = case.contains("another IPAM");
        let (router, engine) = emulator();
        let first = create_ipam(case, &router).await;
        let second = create_ipam(case, &router).await;
        let asn = "64499";
        if brought_elsewhere {
            seed_asn(&engine, asn, &first, PROVISIONED);
        }

        let (status, body) = deprovision_asn(&router, &second, asn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, BYOASN_NOT_FOUND,
            "case {case:?}: expected {BYOASN_NOT_FOUND}, got {code:?} with body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(asn),
            "case {case:?}: the refusal names the ASN the request sent, got {message:?}"
        );
        let expected_state = brought_elsewhere.then(|| PROVISIONED.to_string());
        assert_eq!(
            stored_state(&engine, asn),
            expected_state,
            "case {case:?}: the IPAM that does hold the ASN keeps it, got {:?}",
            stored_state(&engine, asn)
        );
    }
}

/// An ASN that is not sitting still — part way through a move the clock has not finished, or
/// already given back — is refused for the state it is in, and stays in it.
#[tokio::test]
async fn an_asn_not_sitting_still_is_refused_for_its_state() {
    let asn = "64500";
    for state in UNRELEASABLE_STATES {
        let case = format!("an ASN sitting in {state}");
        let (router, engine) = emulator();
        let ipam_id = create_ipam(&case, &router).await;
        seed_asn(&engine, asn, &ipam_id, state);

        let (status, body) = deprovision_asn(&router, &ipam_id, asn).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, INCORRECT_STATE,
            "case {case:?}: expected {INCORRECT_STATE}, got {code:?} with body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(state) && message.contains(asn),
            "case {case:?}: the refusal names the ASN and the state it is in, got {message:?}"
        );
        assert_eq!(
            stored_state(&engine, asn),
            Some((*state).to_string()),
            "case {case:?}: a refused request moves nothing, got {:?}",
            stored_state(&engine, asn)
        );
    }
}

/// A release asked for twice is refused the second time: the first left the ASN
/// `pending-deprovision`, which is not a state a release is admissible from.
#[tokio::test]
async fn a_second_release_while_the_first_is_under_way_is_refused() {
    let case = "a release asked for twice";
    let (router, engine) = emulator();
    let ipam_id = create_ipam(case, &router).await;
    let asn = "64501";
    seed_asn(&engine, asn, &ipam_id, PROVISIONED);

    let (status, body) = deprovision_asn(&router, &ipam_id, asn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the first release is served, got {status} with body {body}"
    );

    let (status, body) = deprovision_asn(&router, &ipam_id, asn).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from the second release, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, INCORRECT_STATE,
        "case {case:?}: expected {INCORRECT_STATE}, got {code:?} with body {body}"
    );
    assert_eq!(
        stored_state(&engine, asn),
        Some(PENDING_DEPROVISION.to_string()),
        "case {case:?}: the refused second release moves nothing, got {:?}",
        stored_state(&engine, asn)
    );
}

/// An association the caller has not cleared decides how the release ENDS, at the due
/// instant: an ASN a range still holds an association for lands in `failed-deprovision`
/// carrying which ranges held it, and one whose associations are not there is given back.
#[tokio::test]
async fn associations_still_held_decide_how_a_release_ends() {
    // The association states come from the published catalog
    // (`smithy_query(query="shape",shape="AsnAssociationState")`, re-exported as
    // `ASN_ASSOCIATION_STATE_VALUES`), so every state the model admits is a row.
    for association_state in ASN_ASSOCIATION_STATE_VALUES {
        let holds = !UNHELD_ASSOCIATION_STATES.contains(association_state);
        let case = format!("an association sitting in {association_state}");
        let (router, engine) = emulator();
        let ipam_id = create_ipam(&case, &router).await;
        let asn = "65537";
        seed_asn(&engine, asn, &ipam_id, PROVISIONED);
        let cidr = seed_range_with_association(&engine, "192.0.2.0/24", asn, association_state);

        let (status, body) = deprovision_asn(&router, &ipam_id, asn).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let answered = reported(&case, &body);
        assert_eq!(
            answered.state,
            Some(PENDING_DEPROVISION.to_string()),
            "case {case:?}: the release starts under way whatever holds it, got {answered:?}"
        );

        tokio::time::sleep(past_the_transition()).await;

        let finished = described(&router, &case, asn).await;
        let expected = if holds { FAILED_DEPROVISION } else { DEPROVISIONED };
        assert_eq!(
            finished.as_ref().and_then(|entry| entry.state.clone()),
            Some(expected.to_string()),
            "case {case:?}: expected the release to end in {expected}, got {finished:?}"
        );
        let status_message = finished
            .as_ref()
            .and_then(|entry| entry.status_message.clone());
        if holds {
            let reason = status_message.unwrap_or_default();
            assert!(
                reason.contains(&cidr) && reason.contains('1'),
                "case {case:?}: the failed release says which ranges held it, got {reason:?}"
            );
        } else {
            assert_eq!(
                status_message, None,
                "case {case:?}: a release nothing held carries no status message, got \
                 {status_message:?}"
            );
        }
    }
}

/// Every range that still holds an association is named: the count and the ranges come from
/// the store, so two of them are both reported.
#[tokio::test]
async fn a_failed_release_names_every_range_that_held_it() {
    let case = "two ranges holding an association";
    let (router, engine) = emulator();
    let ipam_id = create_ipam(case, &router).await;
    let asn = "65538";
    seed_asn(&engine, asn, &ipam_id, PROVISIONED);
    let first = seed_range_with_association(&engine, "192.0.2.0/24", asn, "associated");
    let second = seed_range_with_association(&engine, "198.51.100.0/24", asn, "associated");

    let (status, body) = deprovision_asn(&router, &ipam_id, asn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );

    tokio::time::sleep(past_the_transition()).await;

    let finished = described(&router, case, asn).await;
    let reason = finished
        .as_ref()
        .and_then(|entry| entry.status_message.clone())
        .unwrap_or_default();
    assert!(
        reason.contains(&first) && reason.contains(&second) && reason.contains('2'),
        "case {case:?}: both ranges and their count are named, got {reason:?}"
    );
    assert_eq!(
        stored_state(&engine, asn),
        Some(FAILED_DEPROVISION.to_string()),
        "case {case:?}: the ASN is still the account's to release again, got {:?}",
        stored_state(&engine, asn)
    );
}

/// A request whose `IpamId` or `Asn` is not one of those at all is refused before anything is
/// resolved, with the code that says what is wrong with it — and nothing moves.
#[tokio::test]
async fn an_unreadable_member_is_refused_before_anything_is_resolved() {
    let asn = "64502";
    let rows: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "an empty IpamId",
            vec![p("IpamId", ""), p("Asn", asn)],
            MISSING_PARAMETER,
        ),
        ("no IpamId parameter at all", vec![p("Asn", asn)], MALFORMED_INPUT),
        (
            "an IpamId that is not an id at all",
            vec![p("IpamId", "not-an-ipam"), p("Asn", asn)],
            IPAM_ID_MALFORMED,
        ),
        (
            "an IpamId whose suffix is not hexadecimal",
            vec![p("IpamId", "ipam-0000000000000000g"), p("Asn", asn)],
            IPAM_ID_MALFORMED,
        ),
        (
            "an IpamId of another resource kind",
            vec![p("IpamId", "vpc-00000000000000001"), p("Asn", asn)],
            IPAM_ID_MALFORMED,
        ),
        (
            "an IpamId naming an IPAM POOL",
            vec![p("IpamId", POOL_ID), p("Asn", asn)],
            IPAM_ID_MALFORMED,
        ),
        (
            "an empty Asn",
            vec![p("IpamId", "ipam-00000000000000001"), p("Asn", "")],
            MISSING_PARAMETER,
        ),
        (
            "no Asn parameter at all",
            vec![p("IpamId", "ipam-00000000000000001")],
            MALFORMED_INPUT,
        ),
        (
            "an Asn that is not a number",
            vec![p("IpamId", "ipam-00000000000000001"), p("Asn", "sixty-four")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn of zero, which names no autonomous system",
            vec![p("IpamId", "ipam-00000000000000001"), p("Asn", "0")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn past the 4-byte width",
            vec![p("IpamId", "ipam-00000000000000001"), p("Asn", "4294967296")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn written with a leading zero",
            vec![p("IpamId", "ipam-00000000000000001"), p("Asn", "064496")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            "an Asn in dotted notation",
            vec![p("IpamId", "ipam-00000000000000001"), p("Asn", "1.10")],
            INVALID_PARAMETER_VALUE,
        ),
        (
            // The IpamId is read first, so its own refusal is the one answered when both
            // members are unreadable.
            "both members unreadable",
            vec![p("IpamId", "not-an-ipam"), p("Asn", "sixty-four")],
            IPAM_ID_MALFORMED,
        ),
    ];

    let (router, engine) = emulator();
    let ipam_id = create_ipam("the holding IPAM", &router).await;
    seed_asn(&engine, asn, &ipam_id, PROVISIONED);

    for (case, params, expected) in rows {
        let (status, body) = deprovision(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} with body {body}"
        );
        assert_eq!(
            stored_state(&engine, asn),
            Some(PROVISIONED.to_string()),
            "case {case:?}: a refused request moves nothing, got {:?}",
            stored_state(&engine, asn)
        );
    }
}

/// A `DryRun` request reports the permission check instead of releasing anything — for an
/// ASN the account holds and for one it does not alike, since the flag asks about permission
/// rather than about the resources the request names.
#[tokio::test]
async fn a_dry_run_reports_the_permission_check_and_releases_nothing() {
    let held_asn = "64503";
    let (router, engine) = emulator();
    let ipam_id = create_ipam("a dry run", &router).await;
    seed_asn(&engine, held_asn, &ipam_id, PROVISIONED);

    let rows = [
        ("a dry run for an ASN the account holds", ipam_id.as_str(), held_asn),
        (
            "a dry run for an IPAM the account does not hold",
            "ipam-0123456789abcdef0",
            "64504",
        ),
    ];
    for (case, named_ipam, asn) in rows {
        let (status, body) = deprovision(
            &router,
            &[p("IpamId", named_ipam), p("Asn", asn), p("DryRun", "true")],
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
    }

    assert_eq!(
        stored_state(&engine, held_asn),
        Some(PROVISIONED.to_string()),
        "case \"a dry run\": a dry run releases nothing, got {:?}",
        stored_state(&engine, held_asn)
    );
}
