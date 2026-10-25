// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateCapacityReservation` and the
//! `DescribeCapacityReservations` that reads it back, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=CreateCapacityReservation`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! Four rules are under test:
//!
//! 1. A create RESERVES CAPACITY AND PERSISTS IT: every member the response reports is
//!    derived from the request and the Region it was signed for, and the same reservation
//!    comes back out of `DescribeCapacityReservations`.
//! 2. A reservation's `State` is READ OFF THE CLOCKS: a future-dated request is assessed
//!    and then `scheduled` (the move the create's own clock owes it), it is delivered
//!    (`active`) when its `StartDate` arrives, capacity for immediate use is `active` from
//!    the call, and any reservation is `expired` once its `EndDate` has passed.
//! 3. A request outside a documented rule is REFUSED with EC2's own code for it, and
//!    nothing is stored.
//! 4. A repeat of one create under the same `ClientToken` reports the reservation it
//!    minted instead of reserving capacity twice.
//!
//! Every table row is a distinct input along what those rules generalise over — the
//! platform, the instance type and family, the instance count (including both published
//! bounds and one past each), which Availability Zone member names the zone, the tenancy,
//! the match criteria, the end type, how far ahead a future-dated reservation starts, the
//! tags, which filter, which page size — so no single recorded request is the
//! specification.

use std::time::{Duration, SystemTime};

use ec2::state::{iso8601_utc, TRANSITION_DELAY};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, filters, indexed, unescaped};

/// The Region every call here is signed for unless the case says otherwise. A Capacity
/// Reservation reserves capacity in one Availability Zone, so the Region is what decides
/// which zones a create may name and which reservations a describe reports.
const REGION: &str = "us-east-1";

/// Long enough past [`TRANSITION_DELAY`] that the provisioning a create started is
/// certainly due, with margin for a loaded test machine.
fn past_the_transition() -> Duration {
    TRANSITION_DELAY + Duration::from_millis(400)
}

/// Form fields from `(name, value)` pairs.
fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// One `CreateCapacityReservation` call carrying `params`, as its HTTP status and XML body.
async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, "CreateCapacityReservation", params).await
}

/// One `DescribeCapacityReservations` call in `region`, as its HTTP status and XML body.
async fn describe_in(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeCapacityReservations", params).await
}

/// One `DescribeCapacityReservations` call in [`REGION`].
async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    describe_in(router, REGION, params).await
}

/// The Capacity Reservation ids a successful describe reports, in the order it reported
/// them.
async fn described_ids(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<String> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    elements(&body, "capacityReservationId")
}

/// The text of `name` in `body`, or the empty string when the service wrote no such
/// element — which an assertion then reports as the difference it is.
fn member(body: &str, name: &str) -> String {
    unescaped(&element(body, name).unwrap_or_default())
}

/// A request that must succeed, as its own response body.
async fn create_ok(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = create(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    body
}

/// `now` shifted by `seconds`, as the ISO 8601 UTC timestamp the wire carries a timestamp
/// as.
fn timestamp_in(seconds: i64) -> String {
    let now = SystemTime::now();
    let shift = Duration::from_secs(seconds.unsigned_abs());
    iso8601_utc(if seconds < 0 { now - shift } else { now + shift })
}

/// One day in seconds, the unit the future-dated lead time is published in: "You can
/// request a future-dated Capacity Reservation between 5 and 120 days in advance"
/// (`CreateCapacityReservationRequest$StartDate`).
const DAY_SECONDS: i64 = 24 * 60 * 60;

/// One row of the clock table: what it covers, the create it sends, the state that create
/// answers, and the state the clock then owes the reservation.
type ClockRow = (
    &'static str,
    Vec<(String, String)>,
    &'static str,
    &'static str,
);

/// One row of the filter table: what it covers, the `Filters` entries it sends, and the
/// reservation ids those must select.
type FilterRow = (
    &'static str,
    Vec<(&'static str, &'static [&'static str])>,
    Vec<String>,
);

/// One row of the describe-refusal table: what it covers, the form fields it sends, and the
/// error code and HTTP status it must be answered with.
type DescribeRefusalRow = (&'static str, Vec<(String, String)>, &'static str, u16);

/// The slack a case leaves on a published lead-time bound, so the gap between this suite
/// reading the clock and the handler reading it cannot decide the case.
const CLOCK_SLACK_SECONDS: i64 = 60 * 60;

/// What one create in the success table asks for, and what rule 1 says the answer must
/// carry.
struct CreateCase {
    /// What the row is covering, named in every assertion.
    case: &'static str,
    /// The form fields beyond the three required members below.
    extra: Vec<(String, String)>,
    instance_type: &'static str,
    instance_platform: &'static str,
    instance_count: i32,
    /// The Availability Zone the reservation must report, whichever member named it.
    expected_zone: &'static str,
    /// That zone's id, which the reservation must report too.
    expected_zone_id: &'static str,
    /// The `CapacityReservationState` the create must answer with.
    expected_state: &'static str,
    /// `default` unless the row asked for single-tenant hardware.
    expected_tenancy: &'static str,
    /// `open` unless the row asked for `targeted`.
    expected_match_criteria: &'static str,
    /// `unlimited` unless the row carries an end date.
    expected_end_date_type: &'static str,
    /// Whether the reservation must report a commitment, which only a future-dated one has.
    expected_commitment: bool,
}

/// The creates rule 1 is asserted over.
///
/// The rows differ along everything the rule generalises over: which member names the zone
/// and which zone of the Region it is, the instance type and family, the platform (four
/// members of `CapacityReservationInstancePlatform`, none of them the one the issue cited),
/// the instance count at both published bounds, the tenancy, the match criteria, whether
/// EBS optimization was asked for, the end type, the tags, and whether the reservation is
/// for immediate use or future-dated.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            case: "minimal, every default taken",
            extra: fields(&[("AvailabilityZone", "us-east-1a")]),
            instance_type: "m5.large",
            instance_platform: "Linux/UNIX",
            instance_count: 1,
            expected_zone: "us-east-1a",
            expected_zone_id: "use1-az1",
            expected_state: "active",
            expected_tenancy: "default",
            expected_match_criteria: "open",
            expected_end_date_type: "unlimited",
            expected_commitment: false,
        },
        CreateCase {
            case: "zone named by id, dedicated and targeted at the count ceiling",
            extra: fields(&[
                ("AvailabilityZoneId", "use1-az2"),
                ("Tenancy", "dedicated"),
                ("InstanceMatchCriteria", "targeted"),
                ("EbsOptimized", "true"),
            ]),
            instance_type: "c5.xlarge",
            instance_platform: "Windows",
            instance_count: 1000,
            expected_zone: "us-east-1b",
            expected_zone_id: "use1-az2",
            expected_state: "active",
            expected_tenancy: "dedicated",
            expected_match_criteria: "targeted",
            expected_end_date_type: "unlimited",
            expected_commitment: false,
        },
        CreateCase {
            case: "limited end type with the end date it requires",
            extra: {
                let mut extra = fields(&[
                    ("AvailabilityZone", "us-east-1c"),
                    ("EndDateType", "limited"),
                ]);
                extra.push(("EndDate".to_string(), timestamp_in(2 * DAY_SECONDS)));
                extra
            },
            instance_type: "r6g.medium",
            instance_platform: "Red Hat Enterprise Linux",
            instance_count: 4,
            expected_zone: "us-east-1c",
            expected_zone_id: "use1-az3",
            expected_state: "active",
            expected_tenancy: "default",
            expected_match_criteria: "open",
            expected_end_date_type: "limited",
            expected_commitment: false,
        },
        CreateCase {
            case: "an end date three days out, under a platform and family of its own",
            extra: {
                let mut extra = fields(&[
                    ("AvailabilityZone", "us-east-1a"),
                    ("EndDateType", "limited"),
                ]);
                extra.push(("EndDate".to_string(), timestamp_in(3 * DAY_SECONDS)));
                extra
            },
            instance_type: "t3.micro",
            instance_platform: "SUSE Linux",
            instance_count: 7,
            expected_zone: "us-east-1a",
            expected_zone_id: "use1-az1",
            expected_state: "active",
            expected_tenancy: "default",
            expected_match_criteria: "open",
            expected_end_date_type: "limited",
            expected_commitment: false,
        },
        CreateCase {
            case: "tagged",
            extra: fields(&[
                ("AvailabilityZone", "us-east-1b"),
                ("TagSpecifications.1.ResourceType", "capacity-reservation"),
                ("TagSpecifications.1.Tag.1.Key", "team"),
                ("TagSpecifications.1.Tag.1.Value", "batch"),
                ("TagSpecifications.1.Tag.2.Key", "env"),
                ("TagSpecifications.1.Tag.2.Value", "staging"),
            ]),
            instance_type: "i3.large",
            instance_platform: "Ubuntu Pro",
            instance_count: 3,
            expected_zone: "us-east-1b",
            expected_zone_id: "use1-az2",
            expected_state: "active",
            expected_tenancy: "default",
            expected_match_criteria: "open",
            expected_end_date_type: "unlimited",
            expected_commitment: false,
        },
        CreateCase {
            case: "future-dated at the near lead-time bound",
            extra: {
                let mut extra = fields(&[
                    ("AvailabilityZone", "us-east-1a"),
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "2592000"),
                ]);
                extra.push((
                    "StartDate".to_string(),
                    timestamp_in(5 * DAY_SECONDS + CLOCK_SLACK_SECONDS),
                ));
                extra
            },
            instance_type: "m5.24xlarge",
            instance_platform: "Linux/UNIX",
            instance_count: 32,
            expected_zone: "us-east-1a",
            expected_zone_id: "use1-az1",
            expected_state: "assessing",
            expected_tenancy: "default",
            expected_match_criteria: "targeted",
            expected_end_date_type: "unlimited",
            expected_commitment: true,
        },
        CreateCase {
            case: "future-dated at the far lead-time bound",
            extra: {
                let mut extra = fields(&[
                    ("AvailabilityZoneId", "use1-az3"),
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "604800"),
                ]);
                extra.push((
                    "StartDate".to_string(),
                    timestamp_in(120 * DAY_SECONDS - CLOCK_SLACK_SECONDS),
                ));
                extra
            },
            instance_type: "g5.xlarge",
            instance_platform: "Linux/UNIX",
            instance_count: 8,
            expected_zone: "us-east-1c",
            expected_zone_id: "use1-az3",
            expected_state: "assessing",
            expected_tenancy: "default",
            expected_match_criteria: "targeted",
            expected_end_date_type: "unlimited",
            expected_commitment: true,
        },
    ]
}

/// The form fields one [`CreateCase`] sends.
fn create_params(row: &CreateCase) -> Vec<(String, String)> {
    let mut params = fields(&[
        ("InstanceType", row.instance_type),
        ("InstancePlatform", row.instance_platform),
    ]);
    params.push((
        "InstanceCount".to_string(),
        row.instance_count.to_string(),
    ));
    params.extend(row.extra.iter().cloned());
    params
}

/// Rule 1: every member of the answer is derived from the request and the Region, and the
/// same reservation comes back out of the describe.
#[tokio::test]
async fn create_reserves_capacity_and_the_describe_reports_it() {
    for row in create_cases() {
        let (router, _engine) = super::ec2_query::emulator();
        let case = row.case;
        let body = create_ok(&router, case, &create_params(&row)).await;

        let id = member(&body, "capacityReservationId");
        assert!(
            id.starts_with("cr-"),
            "case {case:?}: expected a cr- Capacity Reservation id, got {id:?}"
        );
        let arn = member(&body, "capacityReservationArn");
        assert!(
            arn.contains(&format!(":ec2:{REGION}:")) && arn.ends_with(&format!("/{id}")),
            "case {case:?}: expected an ec2 ARN in {REGION} naming {id}, got {arn:?}"
        );
        let owner = member(&body, "ownerId");
        assert!(
            owner.len() == 12 && owner.chars().all(|c| c.is_ascii_digit()),
            "case {case:?}: expected a 12-digit owner account id, got {owner:?}"
        );

        let expected: Vec<(&str, String)> = vec![
            ("availabilityZone", row.expected_zone.to_string()),
            ("availabilityZoneId", row.expected_zone_id.to_string()),
            ("instanceType", row.instance_type.to_string()),
            ("instancePlatform", row.instance_platform.to_string()),
            ("tenancy", row.expected_tenancy.to_string()),
            ("totalInstanceCount", row.instance_count.to_string()),
            // Nothing is running in a reservation this call just created, so every
            // instance it reserves can still be launched into it.
            ("availableInstanceCount", row.instance_count.to_string()),
            ("state", row.expected_state.to_string()),
            ("endDateType", row.expected_end_date_type.to_string()),
            (
                "instanceMatchCriteria",
                row.expected_match_criteria.to_string(),
            ),
            // "The type of Capacity Reservation": one a caller created itself, not the
            // capacity of a Capacity Block.
            ("reservationType", "default".to_string()),
        ];
        for (name, want) in &expected {
            let got = member(&body, name);
            assert_eq!(
                &got, want,
                "case {case:?}: expected {name} {want:?}, got {got:?}"
            );
        }

        let commitment = member(&body, "commitmentDuration");
        if row.expected_commitment {
            assert!(
                !commitment.is_empty(),
                "case {case:?}: expected a commitment duration on a future-dated \
                 reservation, got none"
            );
            let committed = member(&body, "committedInstanceCount");
            assert_eq!(
                committed,
                row.instance_count.to_string(),
                "case {case:?}: expected the committed count to be the reserved count \
                 {}, got {committed:?}",
                row.instance_count
            );
            assert_eq!(
                member(&body, "originalStartDate"),
                member(&body, "startDate"),
                "case {case:?}: expected the originally requested start date to be the \
                 reservation's start date"
            );
            // "The date and time at which the commitment duration expires"
            // (`CapacityReservationCommitmentInfo$CommitmentEndDate`): the start date plus
            // the duration committed to, which is the value the handler derives.
            let committed_seconds: i64 = commitment
                .parse()
                .unwrap_or_else(|_| panic!("case {case:?}: expected a number, got {commitment:?}"));
            let parsed = |name: &str, text: &str| -> ::aws_smithy_types::DateTime {
                ::aws_smithy_types::DateTime::from_str(
                    text,
                    ::aws_smithy_types::date_time::Format::DateTime,
                )
                .unwrap_or_else(|_| {
                    panic!("case {case:?}: expected a timestamp in {name}, got {text:?}")
                })
            };
            let start = member(&body, "startDate");
            let end = member(&body, "commitmentEndDate");
            let started = parsed("startDate", &start);
            let ends = parsed("commitmentEndDate", &end);
            assert_eq!(
                (ends.secs() - started.secs(), ends.subsec_nanos()),
                (committed_seconds, started.subsec_nanos()),
                "case {case:?}: expected the commitment to end {committed_seconds}s after \
                 the start date {start:?}, got {end:?}"
            );
        } else {
            assert!(
                commitment.is_empty(),
                "case {case:?}: expected no commitment on a reservation created for \
                 immediate use, got {commitment:?}"
            );
        }

        // Rule 1's second half: the reservation the create answered is the one the store
        // holds, read back through the describe.
        let (status, read) = describe(&router, &indexed("CapacityReservationId", &[&id])).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the describe, got {status} with {read}"
        );
        for (name, want) in &expected {
            // Every member but the state, which the clock moves on its own and
            // `the_clock_moves_a_reservation_out_of_the_state_the_create_left_it_in`
            // pins; what this case asserts is that the store kept what the create
            // derived.
            if *name == "state" {
                continue;
            }
            let got = member(&read, name);
            assert_eq!(
                &got, want,
                "case {case:?}: expected the describe to report {name} {want:?}, got {got:?}"
            );
        }
        assert_eq!(
            member(&read, "capacityReservationArn"),
            arn,
            "case {case:?}: expected the describe to report the ARN the create answered"
        );
    }
}

/// Rule 1 for the tags: what a create tags the reservation with is what the describe
/// reports.
#[tokio::test]
async fn tags_round_trip_through_the_describe() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "two tags on the reservation";
    let params = fields(&[
        ("InstanceType", "i3.large"),
        ("InstancePlatform", "Ubuntu Pro"),
        ("InstanceCount", "3"),
        ("AvailabilityZone", "us-east-1b"),
        ("TagSpecifications.1.ResourceType", "capacity-reservation"),
        ("TagSpecifications.1.Tag.1.Key", "team"),
        ("TagSpecifications.1.Tag.1.Value", "batch"),
        ("TagSpecifications.1.Tag.2.Key", "env"),
        ("TagSpecifications.1.Tag.2.Value", "staging"),
    ]);
    let body = create_ok(&router, case, &params).await;
    let id = member(&body, "capacityReservationId");

    let (status, read) = describe(&router, &indexed("CapacityReservationId", &[&id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {read}"
    );
    assert_eq!(
        elements(&read, "key"),
        vec!["team".to_string(), "env".to_string()],
        "case {case:?}: expected the tag keys the create carried"
    );
    assert_eq!(
        elements(&read, "value"),
        vec!["batch".to_string(), "staging".to_string()],
        "case {case:?}: expected the tag values the create carried"
    );
}

/// Rule 2: the state is READ OFF THE CLOCKS — a future-dated reservation leaves the create
/// `assessing` and the clock owes it `scheduled` on its own, one whose start date has
/// arrived is delivered (`active`), and capacity for immediate use is `active` from the
/// call, with no read changing what a later caller sees.
#[tokio::test]
async fn the_clock_moves_a_reservation_out_of_the_state_the_create_left_it_in() {
    let (router, engine) = super::ec2_query::emulator();

    // (case, the create's extra fields, the state the create answers, the state the clock
    // owes it). The rows differ in which pair of states the reservation walks: a
    // future-dated request is assessed and then scheduled, while capacity for immediate use
    // is usable at once and the clock owes it nothing.
    let immediate = fields(&[
        ("InstanceType", "m5.large"),
        ("InstancePlatform", "Linux/UNIX"),
        ("InstanceCount", "2"),
        ("AvailabilityZone", "us-east-1a"),
    ]);
    let mut future_dated = fields(&[
        ("InstanceType", "c5.large"),
        ("InstancePlatform", "Linux/UNIX"),
        ("InstanceCount", "32"),
        ("AvailabilityZone", "us-east-1b"),
        ("InstanceMatchCriteria", "targeted"),
        ("DeliveryPreference", "incremental"),
        ("CommitmentDuration", "2592000"),
    ]);
    future_dated.push((
        "StartDate".to_string(),
        timestamp_in(30 * DAY_SECONDS),
    ));

    let rows: Vec<ClockRow> = vec![
        (
            "created for immediate use: active at once, and it stays active",
            immediate,
            "active",
            "active",
        ),
        (
            "future-dated: assessing then scheduled, its start date still ahead",
            future_dated,
            "assessing",
            "scheduled",
        ),
    ];

    let mut future_dated_id = String::new();
    for (case, params, created_state, settled_state) in rows {
        let body = create_ok(&router, case, &params).await;
        let id = member(&body, "capacityReservationId");
        let state = member(&body, "state");
        assert_eq!(
            state, created_state,
            "case {case:?}: expected the create to answer {created_state:?}, got {state:?}"
        );
        // Before the move falls due the describe reports the same state, so a caller acting
        // straight after the create still sees the transient one.
        let (_, early) = describe(&router, &indexed("CapacityReservationId", &[&id])).await;
        let early_state = member(&early, "state");
        assert_eq!(
            early_state, created_state,
            "case {case:?}: expected {created_state:?} before the move falls due, \
             got {early_state:?}"
        );

        tokio::time::sleep(past_the_transition()).await;
        let (_, late) = describe(&router, &indexed("CapacityReservationId", &[&id])).await;
        let late_state = member(&late, "state");
        assert_eq!(
            late_state, settled_state,
            "case {case:?}: expected {settled_state:?} once the move was due, \
             got {late_state:?}"
        );
        // A read does not consume the move: a second read agrees with the first.
        let (_, again) = describe(&router, &indexed("CapacityReservationId", &[&id])).await;
        let again_state = member(&again, "state");
        assert_eq!(
            again_state, settled_state,
            "case {case:?}: expected a second read to agree, got {again_state:?}"
        );
        if settled_state == "scheduled" {
            future_dated_id = id;
        }
    }

    // The other end of a future-dated reservation's walk: "scheduled for delivery on the
    // requested start date" (`CapacityReservation$State`), so once the WALL clock reaches
    // that date the capacity is delivered and the reservation is `active`. The start date
    // is at least five days out, which no test can wait for, so the stored record's own
    // start date is moved back to the past — the one value the rule reads — and the
    // reservation is read back over the wire.
    let case = "a scheduled reservation whose start date has arrived";
    {
        let store = engine.state::<ec2::state::ComAmazonawsEc2Amazonec2State>();
        let mut guard = store.lock();
        let record = guard
            .capacity_reservations
            .get_mut(&future_dated_id)
            .unwrap_or_else(|| panic!("case {case:?}: expected {future_dated_id} in the store"));
        record.view.start_date = Some(ec2::state::system_time_to_date_time(
            SystemTime::now() - Duration::from_secs(60),
        ));
    }
    let (_, delivered) = describe(
        &router,
        &indexed("CapacityReservationId", &[&future_dated_id]),
    )
    .await;
    let delivered_state = member(&delivered, "state");
    assert_eq!(
        delivered_state, "active",
        "case {case:?}: expected active once the start date had arrived, \
         got {delivered_state:?}"
    );
}

/// Rule 2 at the other end of the window: "The Capacity Reservation's state changes to
/// expired when it reaches its end date and time".
#[tokio::test]
async fn a_reservation_expires_when_its_end_date_passes() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "limited reservation whose end date passes";
    let mut params = fields(&[
        ("InstanceType", "m5.large"),
        ("InstancePlatform", "Linux/UNIX"),
        ("InstanceCount", "1"),
        ("AvailabilityZone", "us-east-1a"),
        ("EndDateType", "limited"),
    ]);
    // Far enough ahead that the create cannot arrive after it on a loaded machine (the
    // handler refuses an end date that is not in the future), and near enough that the
    // sleep below outlasts it.
    params.push(("EndDate".to_string(), timestamp_in(3)));

    let body = create_ok(&router, case, &params).await;
    let id = member(&body, "capacityReservationId");
    assert_eq!(
        member(&body, "state"),
        "active",
        "case {case:?}: expected the create to answer active"
    );

    tokio::time::sleep(Duration::from_millis(3_500)).await;
    let (status, read) = describe(&router, &indexed("CapacityReservationId", &[&id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {read}"
    );
    let state = member(&read, "state");
    assert_eq!(
        state, "expired",
        "case {case:?}: expected expired once the end date had passed, got {state:?}"
    );
}

/// One refused create: what it asks for, and the code and status EC2 answers it with.
struct RefusalCase {
    case: &'static str,
    params: Vec<(String, String)>,
    expected_code: &'static str,
    expected_status: u16,
}

/// The creates rule 3 is asserted over.
///
/// One row per documented rule a request can break, each naming the member at fault: the
/// zone members' combination, the catalog each enumerated value comes from, the published
/// instance-count range (one past each bound), the end-date pairing, the future-dated
/// requirements and lead-time bounds (one past each), the taggable resource type, and the
/// two request shapes this emulator holds no resource for.
fn refusal_cases() -> Vec<RefusalCase> {
    let base = |extra: &[(&str, &str)]| -> Vec<(String, String)> {
        let mut params = fields(&[
            ("InstanceType", "m5.large"),
            ("InstancePlatform", "Linux/UNIX"),
            ("InstanceCount", "2"),
        ]);
        params.extend(fields(extra));
        params
    };
    let future_dated = |extra: &[(&str, &str)], start_in: i64| -> Vec<(String, String)> {
        let mut params = base(&[("AvailabilityZone", "us-east-1a")]);
        params.extend(fields(extra));
        params.push(("StartDate".to_string(), timestamp_in(start_in)));
        params
    };

    vec![
        RefusalCase {
            case: "neither zone member",
            params: base(&[]),
            expected_code: "MissingParameter",
            expected_status: 400,
        },
        RefusalCase {
            case: "both zone members",
            params: base(&[
                ("AvailabilityZone", "us-east-1a"),
                ("AvailabilityZoneId", "use1-az1"),
            ]),
            expected_code: "InvalidParameterCombination",
            expected_status: 400,
        },
        RefusalCase {
            case: "a zone name the Region does not have",
            params: base(&[("AvailabilityZone", "us-east-1z")]),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "a zone id the Region does not have",
            params: base(&[("AvailabilityZoneId", "usw2-az1")]),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "an instance type outside the modelled catalog",
            params: {
                let mut params = base(&[("AvailabilityZone", "us-east-1a")]);
                params.retain(|(name, _)| name != "InstanceType");
                params.push(("InstanceType".to_string(), "m5.notatype".to_string()));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "one instance below the published count floor",
            params: {
                let mut params = base(&[("AvailabilityZone", "us-east-1a")]);
                params.retain(|(name, _)| name != "InstanceCount");
                params.push(("InstanceCount".to_string(), "0".to_string()));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "one instance above the published count ceiling",
            params: {
                let mut params = base(&[("AvailabilityZone", "us-east-1a")]);
                params.retain(|(name, _)| name != "InstanceCount");
                params.push(("InstanceCount".to_string(), "1001".to_string()));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "limited end type with no end date",
            params: base(&[
                ("AvailabilityZone", "us-east-1a"),
                ("EndDateType", "limited"),
            ]),
            expected_code: "MissingParameter",
            expected_status: 400,
        },
        RefusalCase {
            case: "an end date on an unlimited reservation",
            params: {
                let mut params = base(&[
                    ("AvailabilityZone", "us-east-1a"),
                    ("EndDateType", "unlimited"),
                ]);
                params.push(("EndDate".to_string(), timestamp_in(DAY_SECONDS)));
                params
            },
            expected_code: "InvalidParameterCombination",
            expected_status: 400,
        },
        RefusalCase {
            case: "an end date with no end type, which leaves the reservation unlimited",
            params: {
                let mut params = base(&[("AvailabilityZone", "us-east-1a")]);
                params.push(("EndDate".to_string(), timestamp_in(DAY_SECONDS)));
                params
            },
            expected_code: "InvalidParameterCombination",
            expected_status: 400,
        },
        RefusalCase {
            case: "an end date that has already passed",
            params: {
                let mut params = base(&[
                    ("AvailabilityZone", "us-east-1a"),
                    ("EndDateType", "limited"),
                ]);
                params.push(("EndDate".to_string(), timestamp_in(-DAY_SECONDS)));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "a commitment duration with no start date",
            params: base(&[
                ("AvailabilityZone", "us-east-1a"),
                ("CommitmentDuration", "2592000"),
            ]),
            expected_code: "InvalidParameterCombination",
            expected_status: 400,
        },
        RefusalCase {
            case: "a delivery preference with no start date",
            params: base(&[
                ("AvailabilityZone", "us-east-1a"),
                ("DeliveryPreference", "incremental"),
            ]),
            expected_code: "InvalidParameterCombination",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated with no commitment duration",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "MissingParameter",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated with no delivery preference",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("CommitmentDuration", "2592000"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "MissingParameter",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated asking for the unsupported delivery preference",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "fixed"),
                    ("CommitmentDuration", "2592000"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated with open match criteria",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "open"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "2592000"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated starting sooner than the published lead time",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "2592000"),
                ],
                5 * DAY_SECONDS - CLOCK_SLACK_SECONDS,
            ),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated starting later than the published lead time",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "2592000"),
                ],
                120 * DAY_SECONDS + CLOCK_SLACK_SECONDS,
            ),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated in an instance family it is not offered for",
            params: {
                let mut params = future_dated(
                    &[
                        ("InstanceMatchCriteria", "targeted"),
                        ("DeliveryPreference", "incremental"),
                        ("CommitmentDuration", "2592000"),
                    ],
                    10 * DAY_SECONDS,
                );
                params.retain(|(name, _)| name != "InstanceType");
                params.push(("InstanceType".to_string(), "a1.4xlarge".to_string()));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        // A family is the LETTERS a type starts with, not its first character: `inf` and
        // `trn` are families of their own, so neither is the `i` or the `t` it begins with.
        RefusalCase {
            case: "future-dated in a family that merely starts with an offered letter",
            params: {
                let mut params = future_dated(
                    &[
                        ("InstanceMatchCriteria", "targeted"),
                        ("DeliveryPreference", "incremental"),
                        ("CommitmentDuration", "2592000"),
                    ],
                    10 * DAY_SECONDS,
                );
                params.retain(|(name, _)| name != "InstanceType");
                params.push(("InstanceType".to_string(), "inf1.xlarge".to_string()));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated in another family starting with an offered letter",
            params: {
                let mut params = future_dated(
                    &[
                        ("InstanceMatchCriteria", "targeted"),
                        ("DeliveryPreference", "incremental"),
                        ("CommitmentDuration", "2592000"),
                    ],
                    10 * DAY_SECONDS,
                );
                params.retain(|(name, _)| name != "InstanceType");
                params.push(("InstanceType".to_string(), "trn1.2xlarge".to_string()));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "future-dated with an end date inside the commitment",
            params: {
                let mut params = future_dated(
                    &[
                        ("InstanceMatchCriteria", "targeted"),
                        ("DeliveryPreference", "incremental"),
                        ("CommitmentDuration", "2592000"),
                        ("EndDateType", "limited"),
                    ],
                    10 * DAY_SECONDS,
                );
                params.push(("EndDate".to_string(), timestamp_in(20 * DAY_SECONDS)));
                params
            },
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        // The four commitment-duration rows below are each refused with the modelled
        // `@range 1..200000000` on `CapacityReservationCommitmentDuration`, which the
        // boundary validator applies before the handler runs (`ValidationError`). The
        // handler enforces the same bound itself, so no path can reach the arithmetic that
        // derives the commitment's end date with an unbounded or negative number; what a
        // caller on the wire sees is this modelled refusal.
        RefusalCase {
            case: "a commitment of no seconds, one below the published floor",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "0"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "ValidationError",
            expected_status: 400,
        },
        RefusalCase {
            case: "a commitment that runs backwards",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "-2592000"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "ValidationError",
            expected_status: 400,
        },
        RefusalCase {
            case: "a commitment one second above the published ceiling",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "200000001"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "ValidationError",
            expected_status: 400,
        },
        RefusalCase {
            case: "a commitment of every second an i64 can hold",
            params: future_dated(
                &[
                    ("InstanceMatchCriteria", "targeted"),
                    ("DeliveryPreference", "incremental"),
                    ("CommitmentDuration", "9223372036854775807"),
                ],
                10 * DAY_SECONDS,
            ),
            expected_code: "ValidationError",
            expected_status: 400,
        },
        RefusalCase {
            case: "a tag specification naming another resource type",
            params: base(&[
                ("AvailabilityZone", "us-east-1a"),
                ("TagSpecifications.1.ResourceType", "instance"),
                ("TagSpecifications.1.Tag.1.Key", "team"),
                ("TagSpecifications.1.Tag.1.Value", "batch"),
            ]),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "a dry run of a request that would have succeeded",
            params: base(&[("AvailabilityZone", "us-east-1a"), ("DryRun", "true")]),
            expected_code: "DryRunOperation",
            expected_status: 412,
        },
        RefusalCase {
            case: "on an Outpost, which this emulator holds none of",
            params: base(&[
                ("AvailabilityZone", "us-east-1a"),
                (
                    "OutpostArn",
                    "arn:aws:outposts:us-east-1:123456789012:outpost/op-0123456789abcdef0",
                ),
            ]),
            expected_code: "NotImplemented",
            expected_status: 501,
        },
        RefusalCase {
            case: "in a placement group, which the model's own pattern admits no ARN for",
            params: base(&[
                ("AvailabilityZone", "us-east-1a"),
                (
                    "PlacementGroupArn",
                    "arn:aws:ec2:us-east-1:123456789012:placement-group/pg-1",
                ),
            ]),
            // `PlacementGroupArn`'s modelled pattern is
            // `^arn:aws([a-z-]+)?:ec2:[a-z\d-]+:\d{12}:placement-group/^.{1,255}$`
            // (`smithy_query(query="shape",shape="PlacementGroupArn")`) — the second `^` is
            // an anchor, so no value satisfies it and the boundary validator refuses every
            // one before the handler runs. The handler's own answer for a placement group
            // (this emulator holds none) is therefore unreachable from the wire, and what a
            // caller sees is this modelled refusal.
            expected_code: "ValidationError",
            expected_status: 400,
        },
    ]
}

/// Rule 3: each refused create answers EC2's own code for the rule it broke, and leaves the
/// store empty.
#[tokio::test]
async fn a_request_outside_a_documented_rule_is_refused_and_stores_nothing() {
    for row in refusal_cases() {
        let (router, _engine) = super::ec2_query::emulator();
        let case = row.case;
        let (status, body) = create(&router, &row.params).await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.expected_status
        );
        let code = error_code(&body);
        assert_eq!(
            code, row.expected_code,
            "case {case:?}: expected the error code {:?}, got {code:?} with body {body}",
            row.expected_code
        );
        let held = described_ids(&router, case, &[]).await;
        assert!(
            held.is_empty(),
            "case {case:?}: expected a refused create to store nothing, found {held:?}"
        );
    }
}

/// Rule 4: one `ClientToken` names the one reservation it minted.
#[tokio::test]
async fn a_replayed_client_token_reports_the_reservation_it_minted() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the same create sent twice under one token";
    let params = fields(&[
        ("InstanceType", "m5.large"),
        ("InstancePlatform", "Linux/UNIX"),
        ("InstanceCount", "2"),
        ("AvailabilityZone", "us-east-1a"),
        ("ClientToken", "a-token"),
    ]);

    let first = create_ok(&router, case, &params).await;
    let second = create_ok(&router, case, &params).await;
    let first_id = member(&first, "capacityReservationId");
    let second_id = member(&second, "capacityReservationId");
    assert_eq!(
        first_id, second_id,
        "case {case:?}: expected the replay to report {first_id:?}, got {second_id:?}"
    );
    let held = described_ids(&router, case, &[]).await;
    assert_eq!(
        held,
        vec![first_id.clone()],
        "case {case:?}: expected one stored reservation after the replay"
    );

    // A different token is a different request, so it reserves capacity of its own.
    let mut other = params.clone();
    other.retain(|(name, _)| name != "ClientToken");
    other.push(("ClientToken".to_string(), "another-token".to_string()));
    let third = create_ok(&router, "a second token", &other).await;
    let third_id = member(&third, "capacityReservationId");
    assert_ne!(
        third_id, first_id,
        "case {case:?}: expected a second token to mint a second reservation"
    );
    let held = described_ids(&router, "a second token", &[]).await;
    assert_eq!(
        held.len(),
        2,
        "case {case:?}: expected two stored reservations, found {held:?}"
    );

    // A token names ONE request: replaying it with different parameters is an error, not an
    // answer about the reservation the first request minted.
    let mut mismatched = params.clone();
    mismatched.retain(|(name, _)| name != "InstanceCount");
    mismatched.push(("InstanceCount".to_string(), "9".to_string()));
    let case = "the same token with a different instance count";
    let (status, body) = create(&router, &mismatched).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "IdempotentParameterMismatch",
        "case {case:?}: expected IdempotentParameterMismatch, got {code:?}"
    );
    let held = described_ids(&router, case, &[]).await;
    assert_eq!(
        held.len(),
        2,
        "case {case:?}: expected the refused replay to store nothing, found {held:?}"
    );

    // An idempotency key is scoped to the Region the request was signed for, so the same
    // token signed for another Region is a different request.
    let elsewhere = fields(&[
        ("InstanceType", "m5.large"),
        ("InstancePlatform", "Linux/UNIX"),
        ("InstanceCount", "2"),
        ("AvailabilityZone", "us-west-2a"),
        ("ClientToken", "a-token"),
    ]);
    let case = "the same token in another Region";
    let (status, body) =
        super::ec2_query::call(&router, "us-west-2", "CreateCapacityReservation", &elsewhere).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    let id_elsewhere = member(&body, "capacityReservationId");
    assert_ne!(
        id_elsewhere, first_id,
        "case {case:?}: expected a reservation of its own, got the one us-east-1 minted"
    );
}

/// The reservations the describe table below is written against: one per attribute a
/// documented filter selects on.
async fn seed_three(router: &MultiServiceRouter) -> Vec<String> {
    let rows: Vec<Vec<(String, String)>> = vec![
        fields(&[
            ("InstanceType", "m5.large"),
            ("InstancePlatform", "Linux/UNIX"),
            ("InstanceCount", "2"),
            ("AvailabilityZone", "us-east-1a"),
        ]),
        fields(&[
            ("InstanceType", "c5.xlarge"),
            ("InstancePlatform", "Windows"),
            ("InstanceCount", "4"),
            ("AvailabilityZoneId", "use1-az2"),
            ("Tenancy", "dedicated"),
            ("InstanceMatchCriteria", "targeted"),
        ]),
        fields(&[
            ("InstanceType", "r6g.medium"),
            ("InstancePlatform", "SUSE Linux"),
            ("InstanceCount", "8"),
            ("AvailabilityZone", "us-east-1c"),
        ]),
    ];
    let mut ids = Vec::new();
    for params in rows {
        let body = create_ok(router, "seed", &params).await;
        ids.push(member(&body, "capacityReservationId"));
    }
    ids
}

/// Rule 1's narrowing half: the describe reports the store narrowed by the ids and the
/// documented filters the request names.
#[tokio::test]
async fn the_describe_narrows_by_ids_and_by_every_documented_filter() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_three(&router).await;
    let (first, second, third) = (ids[0].clone(), ids[1].clone(), ids[2].clone());

    let all = described_ids(&router, "no narrowing", &[]).await;
    assert_eq!(
        all,
        vec![first.clone(), second.clone(), third.clone()],
        "expected every reservation in id order"
    );

    // (case, filter entries, the ids the filter must select). The rows walk every name in
    // the operation's documented filter list, with one row carrying two values and one
    // carrying two filters.
    let rows: Vec<FilterRow> = vec![
        (
            "instance-type",
            vec![("instance-type", &["c5.xlarge"][..])],
            vec![second.clone()],
        ),
        (
            "instance-platform",
            vec![("instance-platform", &["SUSE Linux"][..])],
            vec![third.clone()],
        ),
        (
            "availability-zone",
            vec![("availability-zone", &["us-east-1a"][..])],
            vec![first.clone()],
        ),
        (
            "tenancy",
            vec![("tenancy", &["dedicated"][..])],
            vec![second.clone()],
        ),
        (
            "instance-match-criteria",
            vec![("instance-match-criteria", &["open"][..])],
            vec![first.clone(), third.clone()],
        ),
        (
            "end-date-type",
            vec![("end-date-type", &["unlimited"][..])],
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "state, which is the derived one the caller is shown",
            vec![("state", &["active"][..])],
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "owner-id, wildcarded",
            vec![("owner-id", &["*"][..])],
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "outpost-arn, which none of them carries",
            vec![("outpost-arn", &["*"][..])],
            Vec::new(),
        ),
        (
            "placement-group-arn, which none of them carries",
            vec![("placement-group-arn", &["*"][..])],
            Vec::new(),
        ),
        (
            "end-date, which none of them carries",
            vec![("end-date", &["*"][..])],
            Vec::new(),
        ),
        (
            "start-date, wildcarded",
            vec![("start-date", &["*"][..])],
            vec![first.clone(), second.clone(), third.clone()],
        ),
        (
            "two values of one filter are joined with OR",
            vec![("instance-type", &["m5.large", "r6g.medium"][..])],
            vec![first.clone(), third.clone()],
        ),
        (
            "two filters are joined with AND",
            vec![
                ("instance-type", &["c5.xlarge"][..]),
                ("tenancy", &["default"][..]),
            ],
            Vec::new(),
        ),
        (
            "a wildcard inside a value",
            vec![("instance-type", &["?6g.*"][..])],
            vec![third.clone()],
        ),
    ];

    for (case, entries, want) in rows {
        let got = described_ids(&router, case, &filters(&entries)).await;
        assert_eq!(
            got, want,
            "case {case:?}: expected {want:?}, got {got:?}"
        );
    }

    // The ids the request names, which narrow the same way.
    let got = described_ids(
        &router,
        "two of the three ids",
        &indexed("CapacityReservationId", &[&third, &first]),
    )
    .await;
    assert_eq!(
        got,
        vec![first.clone(), third.clone()],
        "expected the two named reservations in id order"
    );
}

/// Rule 1 and the describe's refusals: an undocumented filter, an unknown id and a token
/// this service did not issue are each refused rather than read as "no results".
#[tokio::test]
async fn the_describe_refuses_what_it_cannot_answer() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_three(&router).await;

    let rows: Vec<DescribeRefusalRow> = vec![
        (
            "a filter name the operation does not document",
            filters(&[("capacity-reservation-id", &[ids[0].as_str()][..])]),
            "InvalidParameterValue",
            400,
        ),
        (
            "a filter name in the wrong case",
            filters(&[("Instance-Type", &["m5.large"][..])]),
            "InvalidParameterValue",
            400,
        ),
        (
            "an id no reservation of the account has",
            indexed("CapacityReservationId", &["cr-00000000000000000"]),
            "InvalidCapacityReservationId.NotFound",
            400,
        ),
        (
            "a NextToken this service did not issue",
            fields(&[("NextToken", "not-a-token")]),
            "InvalidPaginationToken",
            400,
        ),
        (
            "a dry run of a describe that would have succeeded",
            fields(&[("DryRun", "true")]),
            "DryRunOperation",
            412,
        ),
    ];

    for (case, params, expected_code, expected_status) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected the error code {expected_code:?}, got {code:?}"
        );
    }
}

/// Rule 1's paging half: `MaxResults` cuts the answer into pages and `NextToken` walks them
/// without repeating or dropping a reservation.
#[tokio::test]
async fn the_describe_pages_the_store() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_three(&router).await;

    // One row per page size: smaller than the store, exactly it, larger than it.
    for page_size in [1_usize, 2, 3, 4] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = fields(&[("MaxResults", page_size.to_string().as_str())]);
            if let Some(token) = token.as_deref() {
                params.push(("NextToken".to_string(), token.to_string()));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = elements(&body, "capacityReservationId");
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} per page, got {page:?}"
            );
            walked.extend(page);
            token = element(&body, "nextToken");
            pages += 1;
            assert!(
                pages <= ids.len() + 1,
                "case {case:?}: expected the walk to finish, it has made {pages} pages"
            );
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, ids,
            "case {case:?}: expected the walk to report every reservation once, in id order"
        );
    }
}

/// Rule 1's Region half: "The results describe only the Capacity Reservations in the Amazon
/// Web Services Region that you're currently using", so a reservation made in one Region is
/// not reported in another — and its id does not resolve there.
#[tokio::test]
async fn a_reservation_belongs_to_the_region_it_was_made_in() {
    let (router, _engine) = super::ec2_query::emulator();
    let ids = seed_three(&router).await;

    let other_region = "us-west-2";
    let (status, body) = describe_in(&router, other_region, &[]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from {other_region}, got {status} with body {body}"
    );
    let reported = elements(&body, "capacityReservationId");
    assert!(
        reported.is_empty(),
        "expected no reservations in {other_region}, got {reported:?}"
    );

    let (status, body) = describe_in(
        &router,
        other_region,
        &indexed("CapacityReservationId", &[ids[0].as_str()]),
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for an id from another Region, got {status} with body {body}"
    );
    let code = error_code(&body);
    assert_eq!(
        code, "InvalidCapacityReservationId.NotFound",
        "expected the not-found code for an id from another Region, got {code:?}"
    );

    // The same zone letters in another Region are that Region's zones, so a create there
    // is reported there.
    let params = fields(&[
        ("InstanceType", "m5.large"),
        ("InstancePlatform", "Linux/UNIX"),
        ("InstanceCount", "2"),
        ("AvailabilityZone", "us-west-2a"),
    ]);
    let (status, body) =
        super::ec2_query::call(&router, other_region, "CreateCapacityReservation", &params).await;
    assert_eq!(
        status, 200,
        "expected a create in {other_region} to succeed, got {status} with body {body}"
    );
    let made_there = member(&body, "capacityReservationId");
    let (_, body) = describe_in(&router, other_region, &[]).await;
    assert_eq!(
        elements(&body, "capacityReservationId"),
        vec![made_there],
        "expected {other_region} to report only the reservation made there"
    );
}
