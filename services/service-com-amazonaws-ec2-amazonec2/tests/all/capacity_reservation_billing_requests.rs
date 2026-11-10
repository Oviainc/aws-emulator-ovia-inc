// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeCapacityReservationBillingRequests`, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! whose body carries `Action=DescribeCapacityReservationBillingRequests`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing
//! name in the request's SigV4 credential scope, so every call here carries one.
//!
//! Two rules are under test:
//!
//! 1. The operation REPORTS THE STORE: the set it answers with is the billing assignment
//!    requests the store holds, narrowed by the `CapacityReservationIds` and `Filters` the
//!    request names and cut into `MaxResults`-sized pages, and it writes nothing.
//! 2. `Role` picks WHICH SIDE of the assignment the caller is asking about: `odcr-owner`
//!    reports the requests the calling account initiated, `unused-reservation-billing-owner`
//!    the requests sent to it — and the filter that selects on the side the role already
//!    pinned is refused rather than answered.
//!
//! Every table row is a distinct input along what those rules generalise over — which role,
//! which side the calling account is on, which status, which documented filter, how many
//! values, how many filters, which ids, which page size — so no single recorded request is
//! the specification.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ec2::ops::types::{
    CALLER_ROLE_VALUES, CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES,
    CapacityReservationInfo,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    CALLER_ROLE_ODCR_OWNER, CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
    CapacityReservationBillingRequestSpec, iso8601_utc,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for. A billing assignment is between two accounts,
/// so the Region only selects the ec2Query envelope.
const REGION: &str = "us-east-1";

/// An account other than the emulator's own, as the Capacity Reservation owner that sent a
/// request to the calling account, and as the consumer account one of the calling account's
/// own requests was sent to.
const OTHER_OWNER: &str = "210987654321";

/// A second account other than the emulator's own, so "the other side" is never one fixed
/// id: `AccountID` is modelled `@length(12, 12)` with the pattern `^[0-9]{12}$`
/// (`smithy_query(query="shape",shape="AccountID")`), which both of these satisfy.
const OTHER_CONSUMER: &str = "345678901234";

/// The account every call in this suite is made by — the emulator's own, which is the
/// account each side of a stored request is compared against.
fn this_account() -> String {
    engine::emulator_account_id().to_string()
}

/// Which account one side of a seeded request names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Party {
    /// The account making the calls.
    This,
    /// Another account.
    OtherOwner,
    /// A third account.
    OtherConsumer,
}

impl Party {
    fn id(self) -> String {
        match self {
            Self::This => this_account(),
            Self::OtherOwner => OTHER_OWNER.to_string(),
            Self::OtherConsumer => OTHER_CONSUMER.to_string(),
        }
    }
}

/// One `DescribeCapacityReservationBillingRequests` call made as `role` and carrying
/// `params`, as its HTTP status and XML body.
async fn describe_as(
    router: &MultiServiceRouter,
    role: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut body = vec![("Role".to_string(), role.to_string())];
    body.extend_from_slice(params);
    super::ec2_query::call(
        router,
        REGION,
        "DescribeCapacityReservationBillingRequests",
        &body,
    )
    .await
}

/// The Capacity Reservation ids a successful response reports, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    role: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe_as(router, role, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "capacityReservationId")
}

/// What one seeded billing request is created with, and which side of it the calling account
/// is on.
struct RequestFixture {
    /// The Capacity Reservation owner that initiated the request.
    requested_by: Party,
    /// The consumer account the request was sent to.
    unused_reservation_billing_owner: Party,
    /// The request's status, a member of `CapacityReservationBillingRequestStatus`.
    status: &'static str,
    /// The status message, or `None` for a request that carries none.
    status_message: Option<&'static str>,
    /// The reservation's instance type, zone, zone id and tenancy, or `None` for a request
    /// that carries no `CapacityReservationInfo` at all.
    info: Option<(&'static str, &'static str, &'static str, &'static str)>,
}

impl RequestFixture {
    /// The `Role` that reports this request, or `None` when the calling account is on
    /// neither side of it.
    fn reporting_role(&self) -> Option<&'static str> {
        if self.requested_by == Party::This {
            return Some(CALLER_ROLE_ODCR_OWNER);
        }
        if self.unused_reservation_billing_owner == Party::This {
            return Some(CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER);
        }
        None
    }

    /// This fixture's `CapacityReservationInfo`, as the store holds it.
    fn capacity_reservation_info(&self) -> Option<CapacityReservationInfo> {
        self.info
            .map(|(instance_type, zone, zone_id, tenancy)| CapacityReservationInfo {
                instance_type: Some(instance_type.to_string()),
                availability_zone: Some(zone.to_string()),
                availability_zone_id: Some(zone_id.to_string()),
                tenancy: Some(tenancy.to_string()),
            })
    }
}

/// The requests every table below is written against.
///
/// The rows differ along everything the two rules generalise over: which side the calling
/// account is on (the owner that initiated the request, the consumer it was sent to, and
/// neither), which other account is on the far side, the status, whether a status message is
/// carried, and whether the reservation's details are. Between them the rows cover every
/// member of `CapacityReservationBillingRequestStatus`
/// (`crate::ops::types::CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES`).
fn seeded_fixtures() -> Vec<RequestFixture> {
    vec![
        // 0: the calling account asked a consumer account to take the billing.
        RequestFixture {
            requested_by: Party::This,
            unused_reservation_billing_owner: Party::OtherOwner,
            status: "pending",
            status_message: None,
            info: Some(("m5.large", "us-east-1a", "use1-az1", "default")),
        },
        // 1: and one that was accepted, by the other consumer account.
        RequestFixture {
            requested_by: Party::This,
            unused_reservation_billing_owner: Party::OtherConsumer,
            status: "accepted",
            status_message: Some("The request was accepted"),
            info: Some(("c5.xlarge", "us-east-1b", "use1-az2", "dedicated")),
        },
        // 2: a request sent TO the calling account, still waiting on it.
        RequestFixture {
            requested_by: Party::OtherOwner,
            unused_reservation_billing_owner: Party::This,
            status: "pending",
            status_message: None,
            info: Some(("r5.2xlarge", "us-west-2a", "usw2-az1", "default")),
        },
        // 3: one the calling account rejected, carrying no reservation details.
        RequestFixture {
            requested_by: Party::OtherConsumer,
            unused_reservation_billing_owner: Party::This,
            status: "rejected",
            status_message: Some("The consumer account rejected the request"),
            info: None,
        },
        // 4: a request between two OTHER accounts: neither role reports it.
        RequestFixture {
            requested_by: Party::OtherOwner,
            unused_reservation_billing_owner: Party::OtherConsumer,
            status: "pending",
            status_message: None,
            info: Some(("m5.large", "us-east-1a", "use1-az1", "default")),
        },
        // 5: one the calling account withdrew.
        RequestFixture {
            requested_by: Party::This,
            unused_reservation_billing_owner: Party::OtherOwner,
            status: "cancelled",
            status_message: Some("The request was cancelled by the owner"),
            info: Some(("m5.large", "eu-west-1a", "euw1-az1", "default")),
        },
        // 6: an assignment to the calling account that was later revoked.
        RequestFixture {
            requested_by: Party::OtherOwner,
            unused_reservation_billing_owner: Party::This,
            status: "revoked",
            status_message: Some("The billing assignment was revoked"),
            info: Some(("c5.xlarge", "us-east-1b", "use1-az2", "dedicated")),
        },
        // 7: one of the calling account's requests that nobody answered in time.
        RequestFixture {
            requested_by: Party::This,
            unused_reservation_billing_owner: Party::OtherConsumer,
            status: "expired",
            status_message: None,
            info: Some(("t3.micro", "ap-southeast-1a", "apse1-az1", "default")),
        },
    ]
}

/// What a seeded store holds, for the tables to name rows by.
struct Seeded {
    /// Every seeded request's `CapacityReservationId`, in fixture order — which is also
    /// store order, the ids being minted in that order.
    ids: Vec<String>,
    /// When each request was last updated, in the same order.
    last_update_times: Vec<SystemTime>,
    /// The rows, in the same order as `ids`.
    fixtures: Vec<RequestFixture>,
}

impl Seeded {
    /// The ids of the rows at `indices`, in store order.
    fn ids_at(&self, indices: &[usize]) -> Vec<String> {
        let mut named: Vec<String> = indices.iter().map(|index| self.ids[*index].clone()).collect();
        named.sort();
        named
    }

    /// The rows `role` reports, in store order.
    fn indices_for(&self, role: &str) -> Vec<usize> {
        self.fixtures
            .iter()
            .enumerate()
            .filter(|(_, fixture)| fixture.reporting_role() == Some(role))
            .map(|(index, _)| index)
            .collect()
    }
}

/// Write the seeded requests into the engine's store, the way an operation assigning the
/// billing of a reservation's unused capacity would, and report what it wrote.
fn seed(engine: &engine::InMemoryEngine) -> Seeded {
    let fixtures = seeded_fixtures();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let mut ids = Vec::new();
    let mut last_update_times = Vec::new();
    for (index, fixture) in fixtures.iter().enumerate() {
        let capacity_reservation_id = guard.next_id("cr");
        // A fixed instant, so the reported timestamp is the one that was stored rather than
        // whatever the clock said during the test.
        let last_update_time = UNIX_EPOCH + Duration::from_secs(1_700_000_000 + index as u64);
        let stored = guard.insert_capacity_reservation_billing_request(
            CapacityReservationBillingRequestSpec {
                capacity_reservation_id: capacity_reservation_id.clone(),
                requested_by: fixture.requested_by.id(),
                unused_reservation_billing_owner_id: fixture
                    .unused_reservation_billing_owner
                    .id(),
                last_update_time,
                status: fixture.status,
                status_message: fixture.status_message.map(str::to_string),
                capacity_reservation_info: fixture.capacity_reservation_info(),
            },
        );
        assert!(
            stored,
            "row {index}: the store must hold the seeded billing request"
        );
        ids.push(capacity_reservation_id);
        last_update_times.push(last_update_time);
    }
    drop(guard);
    Seeded {
        ids,
        last_update_times,
        fixtures,
    }
}

/// One `CapacityReservationId.N` list.
fn id_params(ids: &[String]) -> Vec<(String, String)> {
    let borrowed: Vec<&str> = ids.iter().map(String::as_str).collect();
    indexed("CapacityReservationId", &borrowed)
}

/// Both members of the modelled `CallerRole` catalog.
const BOTH_ROLES: [&str; 2] = [
    CALLER_ROLE_ODCR_OWNER,
    CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
];

// ---------------------------------------------------------------------------
// The store is what is reported
// ---------------------------------------------------------------------------

/// Nothing in this emulator has assigned any billing yet for a fresh account, so neither
/// role reports anything — and asking is a READ: two calls answer the same and the store is
/// untouched.
#[tokio::test]
async fn a_fresh_account_reports_no_billing_requests_and_describing_changes_nothing() {
    let (router, engine) = super::ec2_query::emulator();

    for role in BOTH_ROLES {
        let first = described_ids(&router, role, role, &[]).await;
        assert!(
            first.is_empty(),
            "case {role:?}: expected no billing requests for a fresh account, got {first:?}"
        );
        let second = described_ids(&router, role, role, &[]).await;
        assert_eq!(
            second, first,
            "case {role:?}: a read must not change what a later caller sees: {first:?} then \
             {second:?}"
        );
    }
    assert!(
        engine
            .state::<Ec2State>()
            .lock()
            .capacity_reservation_billing_requests
            .is_empty(),
        "describing billing requests must not write to the store"
    );
}

/// Every member a stored request holds is reported, under the role that reports it, and a
/// member it does not carry is reported by no element at all.
#[tokio::test]
async fn every_member_of_a_stored_request_is_reported() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for (index, fixture) in seeded.fixtures.iter().enumerate() {
        let Some(role) = fixture.reporting_role() else {
            continue;
        };
        let case = format!("row {index} as {role}");
        let params = id_params(&seeded.ids_at(&[index]));
        let (status, body) = describe_as(&router, role, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let mut expected_members = vec![
            ("capacityReservationId", seeded.ids[index].clone()),
            ("requestedBy", fixture.requested_by.id()),
            (
                "unusedReservationBillingOwnerId",
                fixture.unused_reservation_billing_owner.id(),
            ),
            ("status", fixture.status.to_string()),
            (
                "lastUpdateTime",
                iso8601_utc(seeded.last_update_times[index]),
            ),
        ];
        if let Some(message) = fixture.status_message {
            expected_members.push(("statusMessage", message.to_string()));
        }
        if let Some((instance_type, zone, zone_id, tenancy)) = fixture.info {
            expected_members.push(("instanceType", instance_type.to_string()));
            expected_members.push(("availabilityZone", zone.to_string()));
            expected_members.push(("availabilityZoneId", zone_id.to_string()));
            expected_members.push(("tenancy", tenancy.to_string()));
        }
        for (member, expected) in expected_members {
            let reported = element(&body, member).map(|text| unescaped(&text));
            assert_eq!(
                reported.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{member}>{expected}</{member}>, got {body}"
            );
        }
        // A member the request does not carry is absent, not empty.
        if fixture.status_message.is_none() {
            assert!(
                element(&body, "statusMessage").is_none(),
                "case {case:?}: a request with no status message must report none, got {body}"
            );
        }
        if fixture.info.is_none() {
            assert!(
                element(&body, "capacityReservationInfo").is_none(),
                "case {case:?}: a request with no reservation details must report none, got \
                 {body}"
            );
        }
    }
}

/// A request written after a call is reported by the next one, so the answer is computed
/// from the store rather than fixed.
#[tokio::test]
async fn a_request_stored_after_a_call_is_reported_by_the_next_one() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    let before = described_ids(&router, "before", CALLER_ROLE_ODCR_OWNER, &[]).await;
    assert_eq!(
        before,
        seeded.ids_at(&seeded.indices_for(CALLER_ROLE_ODCR_OWNER)),
        "before: expected the requests this account initiated, got {before:?}"
    );

    let late_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let capacity_reservation_id = guard.next_id("cr");
        let stored = guard.insert_capacity_reservation_billing_request(
            CapacityReservationBillingRequestSpec {
                capacity_reservation_id: capacity_reservation_id.clone(),
                requested_by: this_account(),
                unused_reservation_billing_owner_id: OTHER_CONSUMER.to_string(),
                last_update_time: UNIX_EPOCH + Duration::from_secs(1_700_000_100),
                status: ec2::state::CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_PENDING,
                status_message: None,
                capacity_reservation_info: None,
            },
        );
        assert!(
            stored,
            "the later write must be held: the store is far below its ceiling"
        );
        capacity_reservation_id
    };

    let after = described_ids(&router, "after a later write", CALLER_ROLE_ODCR_OWNER, &[]).await;
    let mut expected = before.clone();
    expected.push(late_id.clone());
    expected.sort();
    assert_eq!(
        after, expected,
        "after a later write: expected {late_id:?} to be reported too, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Role
// ---------------------------------------------------------------------------

/// `Role` selects the side of the assignment the caller is asking about: as the Capacity
/// Reservation owner it reports the requests this account initiated, as the consumer account
/// the ones sent to it — and a request between two other accounts is reported under neither.
#[tokio::test]
async fn the_role_selects_the_side_of_the_assignment_the_caller_is_asking_about() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for role in BOTH_ROLES {
        let reported = described_ids(&router, role, role, &[]).await;
        let expected = seeded.ids_at(&seeded.indices_for(role));
        assert_eq!(
            reported, expected,
            "case {role:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Both sides together are every request EXCEPT the one between two other accounts, and
    // no request is reported under both roles.
    let owner_side = described_ids(
        &router,
        "owner side",
        CALLER_ROLE_ODCR_OWNER,
        &[],
    )
    .await;
    let consumer_side = described_ids(
        &router,
        "consumer side",
        CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
        &[],
    )
    .await;
    let mut both: Vec<String> = owner_side.iter().chain(consumer_side.iter()).cloned().collect();
    both.sort();
    let foreign: Vec<usize> = (0..seeded.ids.len())
        .filter(|index| seeded.fixtures[*index].reporting_role().is_none())
        .collect();
    let mut expected: Vec<String> = (0..seeded.ids.len())
        .filter(|index| !foreign.contains(index))
        .map(|index| seeded.ids[index].clone())
        .collect();
    expected.sort();
    assert_eq!(
        both, expected,
        "the two roles together must report every request this account is a party to, got \
         {both:?}"
    );
    for index in foreign {
        let id = &seeded.ids[index];
        assert!(
            !both.contains(id),
            "row {index}: a request between two other accounts must be reported under neither \
             role, got {both:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CapacityReservationIds
// ---------------------------------------------------------------------------

/// `CapacityReservationIds` narrows the answer to the reservations it names, in store order
/// whatever order they were named in; an id this store holds no request for selects nothing,
/// which is also the answer for a reservation whose billing nobody has asked to assign.
#[tokio::test]
async fn capacity_reservation_ids_narrow_the_answer() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);
    let owner_side = seeded.indices_for(CALLER_ROLE_ODCR_OWNER);

    // Rows: one id, two, every id this role reports, the same two named in reverse, and a
    // row the OTHER role reports — which this role does not, however it is named.
    let cases: Vec<(&str, Vec<usize>, Vec<usize>)> = vec![
        ("one id", vec![owner_side[0]], vec![owner_side[0]]),
        (
            "two ids",
            vec![owner_side[1], owner_side[3]],
            vec![owner_side[1], owner_side[3]],
        ),
        ("every id of the role", owner_side.clone(), owner_side.clone()),
        (
            "two ids, reversed",
            vec![owner_side[3], owner_side[1]],
            vec![owner_side[1], owner_side[3]],
        ),
        (
            "an id the other role reports",
            seeded.indices_for(CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER),
            vec![],
        ),
    ];
    for (case, named, expected_indices) in cases {
        let params = id_params(&seeded.ids_at(&named));
        let reported = described_ids(&router, case, CALLER_ROLE_ODCR_OWNER, &params).await;
        let expected = seeded.ids_at(&expected_indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} in store order, got {reported:?}"
        );
    }

    // An id of the right shape that names nothing, one of another resource's family, and
    // free text: each selects nothing rather than widening the answer.
    for unknown in ["cr-00000000000000fff", "vol-00000000000000001", "not-an-id"] {
        let params = id_params(&[unknown.to_string()]);
        let reported = described_ids(&router, unknown, CALLER_ROLE_ODCR_OWNER, &params).await;
        assert!(
            reported.is_empty(),
            "case {unknown:?}: expected no requests, got {reported:?}"
        );
    }

    // An id that names nothing beside one that does leaves the one that does.
    let mut mixed = seeded.ids_at(&[owner_side[0]]);
    mixed.push("cr-00000000000000fff".to_string());
    let reported = described_ids(
        &router,
        "a known id beside an unknown one",
        CALLER_ROLE_ODCR_OWNER,
        &id_params(&mixed),
    )
    .await;
    assert_eq!(
        reported,
        seeded.ids_at(&[owner_side[0]]),
        "a known id beside an unknown one: expected the known one, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// Filters
// ---------------------------------------------------------------------------

/// One row of the filter table: the case's name, the `Role` the call is made as, the name
/// and values of the one filter it carries, and the rows that must come back.
type FilterCase = (
    &'static str,
    &'static str,
    &'static str,
    Vec<String>,
    Vec<usize>,
);

/// Each documented filter selects on its own attribute, values within one filter are OR'd,
/// several filters are AND'd, and the comparison is exact, case-sensitive and understands
/// EC2's `*` wildcard — always within the set the `Role` already selected.
#[tokio::test]
async fn each_documented_filter_selects_the_requests_whose_attribute_equals_a_value() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // (case, role, filter name, values, the rows that must come back). The rows cover each
    // of the three filter names `Filters` documents, on a status the issue never cited.
    let cases: Vec<FilterCase> = vec![
        (
            "a pending request of the owner side",
            CALLER_ROLE_ODCR_OWNER,
            "status",
            vec!["pending".into()],
            vec![0],
        ),
        (
            "two statuses are OR'd",
            CALLER_ROLE_ODCR_OWNER,
            "status",
            vec!["accepted".into(), "expired".into()],
            vec![1, 7],
        ),
        (
            "a status only the other side holds",
            CALLER_ROLE_ODCR_OWNER,
            "status",
            vec!["rejected".into()],
            vec![],
        ),
        (
            "a status spelled in another case",
            CALLER_ROLE_ODCR_OWNER,
            "status",
            vec!["PENDING".into()],
            vec![],
        ),
        (
            "EC2's wildcard over statuses",
            CALLER_ROLE_ODCR_OWNER,
            "status",
            vec!["*ed".into()],
            vec![1, 5, 7],
        ),
        (
            "a filter carrying no value at all",
            CALLER_ROLE_ODCR_OWNER,
            "status",
            vec![],
            vec![],
        ),
        (
            "the consumer account one side was sent to",
            CALLER_ROLE_ODCR_OWNER,
            "unused-reservation-billing-owner",
            vec![OTHER_OWNER.into()],
            vec![0, 5],
        ),
        (
            "the other consumer account",
            CALLER_ROLE_ODCR_OWNER,
            "unused-reservation-billing-owner",
            vec![OTHER_CONSUMER.into()],
            vec![1, 7],
        ),
        (
            "a consumer account no request of this side was sent to",
            CALLER_ROLE_ODCR_OWNER,
            "unused-reservation-billing-owner",
            vec![this_account()],
            vec![],
        ),
        (
            "the owner that initiated the requests sent to this account",
            CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
            "requested-by",
            vec![OTHER_OWNER.into()],
            vec![2, 6],
        ),
        (
            "the other owner",
            CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
            "requested-by",
            vec![OTHER_CONSUMER.into()],
            vec![3],
        ),
        (
            "two owners are OR'd",
            CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
            "requested-by",
            vec![OTHER_OWNER.into(), OTHER_CONSUMER.into()],
            vec![2, 3, 6],
        ),
        (
            "EC2's wildcard over an account id",
            CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
            "requested-by",
            vec!["2109*".into()],
            vec![2, 6],
        ),
        (
            "a pending request of the consumer side",
            CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
            "status",
            vec!["pending".into()],
            vec![2],
        ),
        (
            "a status no request of this side holds",
            CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
            "status",
            vec!["cancelled".into()],
            vec![],
        ),
    ];
    for (case, role, name, values, indices) in cases {
        let borrowed: Vec<&str> = values.iter().map(String::as_str).collect();
        let params = filters(&[(name, &borrowed)]);
        let reported = described_ids(&router, case, role, &params).await;
        let expected = seeded.ids_at(&indices);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // Several filters are AND'd: only the requests that satisfy every one of them.
    let anded = filters(&[
        ("status", &["pending", "accepted"]),
        ("unused-reservation-billing-owner", &[OTHER_CONSUMER]),
    ]);
    let reported = described_ids(&router, "two filters", CALLER_ROLE_ODCR_OWNER, &anded).await;
    assert_eq!(
        reported,
        seeded.ids_at(&[1]),
        "two filters: expected only the accepted request sent to the other consumer account, \
         got {reported:?}"
    );

    // The seeded rows cover every member of the modelled status catalog, so the table above
    // cannot silently stop exercising one of them.
    for status in CAPACITY_RESERVATION_BILLING_REQUEST_STATUS_VALUES {
        assert!(
            seeded
                .fixtures
                .iter()
                .any(|fixture| fixture.status == *status),
            "the seeded rows must cover the status {status:?}"
        );
    }
}

/// A filter name this operation does not document is refused, rather than read as selecting
/// nothing.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // Rows: a plural misspelling, the plural of the id filter this operation DOES serve, the
    // tag filters a taggable resource has and this record does not, a name differing only in
    // case, and an empty name. `capacity-reservation-id` is SERVED — it selects on the
    // reported record's own id — so it is covered by
    // `describe_filters_on_reported_members` instead.
    for name in [
        "statuses",
        "capacity-reservation-ids",
        "tag-key",
        "tag:Name",
        "Status",
        "",
    ] {
        let params = filters(&[(name, &["anything"])]);
        let (status, body) = describe_as(&router, CALLER_ROLE_ODCR_OWNER, &params).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {name:?}: expected InvalidParameterValue, got {body}"
        );
        let message = element(&body, "Message").map(|text| unescaped(&text));
        assert_eq!(
            message.as_deref(),
            Some(format!("The filter '{name}' is invalid").as_str()),
            "case {name:?}: the refusal must name the filter, got {body}"
        );
    }
}

/// The filter that selects on the side the `Role` has already pinned to the calling account
/// is refused with `InvalidParameterCombination`; the same filter under the OTHER role is an
/// ordinary selector.
#[tokio::test]
async fn the_filter_the_role_already_pins_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // (role, the filter it does not support, the filter it does).
    let cases: [(&str, &str, &str); 2] = [
        (
            CALLER_ROLE_ODCR_OWNER,
            "requested-by",
            "unused-reservation-billing-owner",
        ),
        (
            CALLER_ROLE_UNUSED_RESERVATION_BILLING_OWNER,
            "unused-reservation-billing-owner",
            "requested-by",
        ),
    ];
    for (role, unsupported, supported) in cases {
        let case = format!("{role} with {unsupported}");
        let params = filters(&[(unsupported, &[this_account().as_str()])]);
        let (status, body) = describe_as(&router, role, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterCombination",
            "case {case:?}: expected InvalidParameterCombination, got {body}"
        );

        // It is refused whichever other filters the request carries, and before anything is
        // reported.
        let beside = filters(&[("status", &["pending"]), (unsupported, &[OTHER_OWNER])]);
        let (status, body) = describe_as(&router, role, &beside).await;
        assert_eq!(
            status, 400,
            "case {case:?} beside another filter: expected HTTP 400, got {status} with body \
             {body}"
        );
        assert!(
            elements(&body, "capacityReservationId").is_empty(),
            "case {case:?}: a refused request may report nothing, got {body}"
        );

        // The one the role does NOT pin is answered normally.
        let allowed = filters(&[(supported, &[OTHER_OWNER])]);
        let (status, body) = describe_as(&router, role, &allowed).await;
        assert_eq!(
            status, 200,
            "case {role:?} with {supported:?}: expected HTTP 200, got {status} with body {body}"
        );
    }

    // And the supported pairing really does select: the owner side's requests sent to the
    // first other account.
    let reported = described_ids(
        &router,
        "the supported pairing selects",
        CALLER_ROLE_ODCR_OWNER,
        &filters(&[("unused-reservation-billing-owner", &[OTHER_OWNER])]),
    )
    .await;
    assert_eq!(
        reported,
        seeded.ids_at(&[0, 5]),
        "the supported pairing selects: expected the two requests sent to {OTHER_OWNER}, got \
         {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The bounds a request's own size is held to
// ---------------------------------------------------------------------------

/// The `Filter.N.Name` / `Filter.N.Value.M` form fields for a filter list of any size.
fn sized_filter_params(entries: &[(&str, Vec<String>)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (name, values)) in entries.iter().enumerate() {
        params.push((format!("Filter.{}.Name", index + 1), (*name).to_string()));
        for (position, value) in values.iter().enumerate() {
            params.push((
                format!("Filter.{}.Value.{}", index + 1, position + 1),
                value.clone(),
            ));
        }
    }
    params
}

/// One row of the bounds table: the case's name, the form fields it sends, and the HTTP
/// status it must get.
type BoundCase = (String, Vec<(String, String)>, u16);

/// The model bounds neither the number of ids, nor the number of filters, nor the number or
/// the size of a filter's values, so the handler is where each is bounded: a request at a
/// bound is answered, one past it is refused with `InvalidParameterValue` rather than being
/// served at whatever cost it asks for.
#[tokio::test]
async fn a_request_is_held_to_the_bounds_its_own_size_is_capped_at() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The caps this handler imposes, read off `crate::state`: ids per request, filters per
    // request, values per filter, bytes per value.
    let max_ids = ec2::state::MAX_RESOURCE_IDS;
    let max_filters = ec2::state::MAX_FILTERS;
    let max_values = ec2::state::MAX_FILTER_VALUES;
    let max_value_bytes = ec2::state::MAX_FILTER_VALUE_BYTES;

    let id = seeded.ids[0].clone();
    let at_cap_ids: Vec<String> = (0..max_ids).map(|_| id.clone()).collect();
    let past_cap_ids: Vec<String> = (0..max_ids + 1).map(|_| id.clone()).collect();
    let at_cap_filters: Vec<(&str, Vec<String>)> = (0..max_filters)
        .map(|_| ("status", vec!["pending".to_string()]))
        .collect();
    let past_cap_filters: Vec<(&str, Vec<String>)> = (0..max_filters + 1)
        .map(|_| ("status", vec!["pending".to_string()]))
        .collect();
    let value_of = |bytes: usize| "a".repeat(bytes);

    // Each bound is exercised AT the cap and one past it; at the cap the request is a
    // perfectly ordinary call.
    let cases: Vec<BoundCase> = vec![
        (format!("{max_ids} ids"), id_params(&at_cap_ids), 200),
        (
            format!("{} ids", max_ids + 1),
            id_params(&past_cap_ids),
            400,
        ),
        (
            format!("{max_filters} filters"),
            sized_filter_params(&at_cap_filters),
            200,
        ),
        (
            format!("{} filters", max_filters + 1),
            sized_filter_params(&past_cap_filters),
            400,
        ),
        (
            format!("{max_values} values"),
            sized_filter_params(&[(
                "status",
                (0..max_values).map(|_| "pending".to_string()).collect(),
            )]),
            200,
        ),
        (
            format!("{} values", max_values + 1),
            sized_filter_params(&[(
                "status",
                (0..max_values + 1).map(|_| "pending".to_string()).collect(),
            )]),
            400,
        ),
        (
            format!("a {max_value_bytes}-byte value"),
            sized_filter_params(&[("status", vec![value_of(max_value_bytes)])]),
            200,
        ),
        (
            format!("a {}-byte value", max_value_bytes + 1),
            sized_filter_params(&[("status", vec![value_of(max_value_bytes + 1)])]),
            400,
        ),
    ];
    for (case, params, expected_status) in cases {
        let (status, body) = describe_as(&router, CALLER_ROLE_ODCR_OWNER, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// `MaxResults` bounds the page and the token resumes from where it ended, so walking the
/// pages reports every request of the role exactly once in store order; a request with no
/// `MaxResults` is answered in one page.
#[tokio::test]
async fn max_results_pages_the_store_and_the_token_resumes_it() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // `DescribeCapacityReservationBillingRequestsRequestMaxResults` is modelled
    // `range 1..1000`, so the rows are its lower bound, sizes that do and do not divide the
    // set, and the published upper bound — each walked under both roles.
    for role in BOTH_ROLES {
        let expected = seeded.ids_at(&seeded.indices_for(role));
        for page_size in [1, 2, 3, 1000] {
            let case = format!("{role} with MaxResults={page_size}");
            let mut params = vec![("MaxResults".to_string(), page_size.to_string())];
            let mut walked: Vec<String> = Vec::new();
            loop {
                let (status, body) = describe_as(&router, role, &params).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                let page = elements(&body, "capacityReservationId");
                assert!(
                    page.len() <= page_size,
                    "case {case:?}: a page holds at most MaxResults requests, got {page:?}"
                );
                walked.extend(page);
                match element(&body, "nextToken") {
                    None => break,
                    Some(token) => {
                        params = vec![
                            ("MaxResults".to_string(), page_size.to_string()),
                            ("NextToken".to_string(), token),
                        ];
                    }
                }
            }
            assert_eq!(
                walked, expected,
                "case {case:?}: the walk must report every request exactly once in store \
                 order, got {walked:?}"
            );
        }

        let unpaged = described_ids(&router, &format!("{role}, no MaxResults"), role, &[]).await;
        assert_eq!(
            unpaged, expected,
            "case {role:?} with no MaxResults: expected every request in one answer, got \
             {unpaged:?}"
        );
    }
}

/// A page is a cut of the FILTERED set, not of the whole store.
#[tokio::test]
async fn a_page_is_a_cut_of_the_filtered_set() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // The owner side holds three requests whose status ends in "ed" (rows 1, 5 and 7).
    let mut params = filters(&[("status", &["*ed"])]);
    params.push(("MaxResults".to_string(), "2".to_string()));
    let (status, body) = describe_as(&router, CALLER_ROLE_ODCR_OWNER, &params).await;
    assert_eq!(
        status, 200,
        "filtered first page: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "capacityReservationId"),
        seeded.ids_at(&[1, 5]),
        "filtered first page: expected the first two matching requests, got {body}"
    );
    let token = element(&body, "nextToken").expect("a third matching request follows the page");

    let mut next = filters(&[("status", &["*ed"])]);
    next.push(("MaxResults".to_string(), "2".to_string()));
    next.push(("NextToken".to_string(), token));
    let reported = described_ids(
        &router,
        "filtered second page",
        CALLER_ROLE_ODCR_OWNER,
        &next,
    )
    .await;
    assert_eq!(
        reported,
        seeded.ids_at(&[7]),
        "filtered second page: expected the remaining matching request, got {reported:?}"
    );
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted.
#[tokio::test]
async fn max_results_is_accepted_at_its_bounds_and_refused_past_them() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // The published bounds (`range 1..1000`) and one past each. A value outside them is
    // refused by the modelled `@range` before the handler runs, so the handler never sees it;
    // which code the BOUNDARY answers is one rule for the whole service rather than this
    // operation's to choose, and this row pins what it answers.
    let cases = [(0, 400), (1, 200), (1000, 200), (1001, 400)];
    for (max_results, expected_status) in cases {
        let params = vec![("MaxResults".to_string(), max_results.to_string())];
        let (status, body) = describe_as(&router, CALLER_ROLE_ODCR_OWNER, &params).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} with \
             body {body}"
        );
        if expected_status == 400 {
            // Which code the BOUNDARY answers is one rule for the whole service, keyed on
            // what the value is: a POSITIVE page size below the smallest the operation
            // serves carries EC2's own `InvalidMaxResults`, a value that is no page size at
            // all — this operation's `@range` starting at 1, that is what a page size below
            // it is — carries EC2's `InvalidParameter`, and one above the largest keeps the
            // boundary validator's generic code.
            let expected_code = if max_results < 1 {
                "InvalidParameter"
            } else {
                "ValidationError"
            };
            assert_eq!(
                error_code(&body),
                expected_code,
                "case MaxResults={max_results}: expected {expected_code}, got {body}"
            );
            assert!(
                elements(&body, "capacityReservationId").is_empty(),
                "case MaxResults={max_results}: a refused request may report nothing, got {body}"
            );
        }
    }
}

/// A `NextToken` this service did not issue is refused with `InvalidPaginationToken`, rather
/// than read as a cursor.
#[tokio::test]
async fn a_next_token_this_service_did_not_issue_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    // Rows: free text, punctuation, a hex string this service never minted, the bare
    // resource id a caller might guess the token is, that id hex-encoded, and a token
    // carrying this service's marker with a checksum that does not match.
    let forged_marker = ::hex::encode(format!("ec2-1:0000000000000000:{}", seeded.ids[0]));
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        "6161".to_string(),
        seeded.ids[0].clone(),
        ::hex::encode(&seeded.ids[0]),
        forged_marker,
    ];
    for token in cases {
        let params = vec![("NextToken".to_string(), token.clone())];
        let (status, body) = describe_as(&router, CALLER_ROLE_ODCR_OWNER, &params).await;
        assert_eq!(
            status, 400,
            "case {token:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {token:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Role as a modelled catalog, and DryRun
// ---------------------------------------------------------------------------

/// The error code the BOUNDARY answers for a `@required` member the request left out, as
/// every operation of this service answers it today. Which of EC2's own codes a boundary
/// refusal carries, and how it is worded, is one rule for the whole service and is recorded
/// as its own issue
/// (`issues/.../20261023T102725Z-ec2-answers-parameter-rejections-with-generic-validationerror-and.md`),
/// so this row pins what the boundary answers rather than restating that rule for one
/// operation. A value outside the `CallerRole` catalog is different: EC2's own words for THAT
/// refusal are recorded, so the rows below state them.
const BOUNDARY_MISSING_MEMBER_CODE: &str = "MalformedInput";

/// One row of the `Role` refusal table: the case's name, the form fields it sends, and the
/// error code and message it must be refused with (`None` for a refusal this suite pins the
/// code of only).
type RoleCase = (
    &'static str,
    Vec<(String, String)>,
    &'static str,
    Option<String>,
);

/// `Role` is a `@required` member of the modelled `CallerRole` catalog, so a call that names
/// no role, or a word the catalog does not hold, is refused with HTTP 400 — never answered
/// with a list, and never with the 501 that would claim the operation is unimplemented. A
/// value outside the catalog carries EC2's own code for a value it cannot accept and EC2's
/// own sentence for that parameter type: the submitted value echoed back, and the values the
/// parameter accepts.
#[tokio::test]
async fn a_role_outside_the_modelled_catalog_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    seed(&engine);

    // EC2's sentence for a `Role` outside the catalog. Only the echoed value is built from
    // the request; the accepted values come from the model's own catalog, in the order it
    // lists them (`smithy_query(query="shape",shape="CallerRole")`), offered the way a
    // sentence offers a list — `a`, `a or b`, `a, b or c` — so a member the model adds later
    // is covered by these rows rather than breaking them.
    fn offered(values: &[&str]) -> String {
        match values.split_last() {
            None => String::new(),
            Some((last, [])) => (*last).to_string(),
            Some((last, leading)) => format!("{} or {last}", leading.join(", ")),
        }
    }
    let catalog = offered(CALLER_ROLE_VALUES);
    assert_eq!(
        catalog, "odcr-owner or unused-reservation-billing-owner",
        "case {CALLER_ROLE_VALUES:?}: the two-member catalog is offered as 'a or b', got \
         {catalog:?}"
    );
    let refusal = |value: &str| {
        Some(format!(
            "{value} is not a valid value for RoleType. Specify either {catalog}."
        ))
    };

    // Rows: no `Role` at all — the member the request left out, which the boundary refuses
    // before any wording of this service's applies — then the empty string, a word the
    // catalog does not hold, a member in the wrong case and one with a trailing space, each
    // a value the catalog does not hold and so EC2's `InvalidParameterValue`.
    let cases: [RoleCase; 5] = [
        (
            "no Role at all",
            vec![],
            BOUNDARY_MISSING_MEMBER_CODE,
            None,
        ),
        (
            "the empty string",
            vec![("Role".to_string(), String::new())],
            "InvalidParameterValue",
            refusal(""),
        ),
        (
            "a word the catalog does not hold",
            vec![("Role".to_string(), "billing-owner".to_string())],
            "InvalidParameterValue",
            refusal("billing-owner"),
        ),
        (
            "a member in the wrong case",
            vec![("Role".to_string(), "ODCR-Owner".to_string())],
            "InvalidParameterValue",
            refusal("ODCR-Owner"),
        ),
        (
            "a member with a trailing space",
            vec![("Role".to_string(), "odcr-owner ".to_string())],
            "InvalidParameterValue",
            refusal("odcr-owner "),
        ),
    ];
    for (case, params, expected_code, expected_message) in cases {
        let (status, body) = super::ec2_query::call(
            &router,
            REGION,
            "DescribeCapacityReservationBillingRequests",
            &params,
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let message = element(&body, "Message").map(|text| unescaped(&text));
        if let Some(expected) = expected_message.as_deref() {
            assert_eq!(
                message.as_deref(),
                Some(expected),
                "case {case:?}: expected the message {expected:?}, got {body}"
            );
        } else {
            assert!(
                message
                    .as_deref()
                    .is_some_and(|text| text.contains("role")),
                "case {case:?}: the refusal must name the parameter to fix, got {message:?}"
            );
        }
        assert!(
            elements(&body, "capacityReservationId").is_empty(),
            "case {case:?}: a refused request may report nothing, got {body}"
        );
    }
}

/// `DryRun` reports the permitted outcome — `DryRunOperation`, HTTP 412 — and describes
/// nothing; `DryRun=false` is an ordinary call. A request that would have been refused is
/// refused with THAT reason instead.
#[tokio::test]
async fn dry_run_reports_the_permitted_outcome_without_describing() {
    let (router, engine) = super::ec2_query::emulator();
    let seeded = seed(&engine);

    for role in BOTH_ROLES {
        let (status, body) = describe_as(
            &router,
            role,
            &[("DryRun".to_string(), "true".to_string())],
        )
        .await;
        assert_eq!(
            status, 412,
            "case {role:?} with DryRun=true: expected HTTP 412, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {role:?} with DryRun=true: expected DryRunOperation, got {body}"
        );
        assert!(
            elements(&body, "capacityReservationId").is_empty(),
            "case {role:?} with DryRun=true: nothing may be described, got {body}"
        );
    }

    let reported = described_ids(
        &router,
        "DryRun=false",
        CALLER_ROLE_ODCR_OWNER,
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(
        reported,
        seeded.ids_at(&seeded.indices_for(CALLER_ROLE_ODCR_OWNER)),
        "DryRun=false: expected an ordinary answer, got {reported:?}"
    );

    // An undocumented filter beats the flag: the request would NOT have succeeded.
    let mut params = filters(&[("statuses", &["pending"])]);
    params.push(("DryRun".to_string(), "true".to_string()));
    let (status, body) = describe_as(&router, CALLER_ROLE_ODCR_OWNER, &params).await;
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "DryRun with an undocumented filter: expected the refusal that applies, got {status} \
         {body}"
    );
}
