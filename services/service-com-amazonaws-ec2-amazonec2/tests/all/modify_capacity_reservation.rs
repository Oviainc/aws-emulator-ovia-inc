// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyCapacityReservation` and the
//! `DescribeCapacityReservations` that reads the modification back, driven over the wire the
//! service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose
//! body carries `Action=ModifyCapacityReservation`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol"). ec2Query is selected by the `ec2` signing name in the
//! request's SigV4 credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. A modification is APPLIED TO THE STORED RESERVATION and visible in the next read of
//!    it: "Modifies a Capacity Reservation's capacity, instance eligibility, and the
//!    conditions under which it is to be released" (`ModifyCapacityReservation`), so a new
//!    instance count, end condition or instance eligibility is what
//!    `DescribeCapacityReservations` reports afterwards — and the capacity nothing has
//!    launched into moves with the total, which is the reservation's own invariant.
//! 2. "The allowed modifications depend on the state of the Capacity Reservation" (same
//!    doc): only an `active` reservation admits a change of its capacity, end condition or
//!    eligibility; a reservation EC2 has not delivered yet admits only the quoted start-date
//!    push-out, and a reservation in any other state admits nothing at all — refused with
//!    `IncorrectState` whether or not the request names a member to change.
//! 3. A value outside a documented rule is REFUSED with EC2's own code for it and nothing is
//!    written: the published bound on a single change ("The number of instances can't be
//!    increased or decreased by more than 1000 in a single request"), the end-date pairing
//!    ("You must provide an EndDate value if EndDateType is limited. Omit EndDate if
//!    EndDateType is unlimited"), and the quote a start-date push-out requires ("Requires a
//!    quote from CreateCapacityReservationDateChangeQuote").
//! 4. A `DryRun` answers `DryRunOperation` and leaves the reservation exactly as it was.
//!
//! 5. A COMMITMENT and the capacity instances are RUNNING IN bound what a modification may
//!    do: "active state but still within the commitment duration - You can't decrease the
//!    instance count or set an end date that is within the commitment duration", and "To
//!    modify the instance eligibility, the Capacity Reservation must be completely idle (zero
//!    usage)" (`ModifyCapacityReservationRequest$InstanceMatchCriteria`) — asserted over
//!    reservations seeded straight into the store, which is the only way to hold one `active`
//!    inside a commitment or with capacity in use (no implemented operation launches an
//!    instance into a Capacity Reservation).
//!
//! Every table row is a distinct input along what those rules generalise over — which member
//! is modified, in which direction, by how much (including the published bound on a change
//! and one past it), which end condition the reservation starts and ends with, which
//! eligibility it starts and ends with, and which state the reservation is in.

use std::time::{Duration, SystemTime};

use ec2::ops::types::CapacityAllocation;
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use ec2::state::{
    iso8601_utc, CapacityReservationCommitment, CapacityReservationSpec,
    CAPACITY_RESERVATION_STATE_ACTIVE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, element, error_code, indexed};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The Availability Zone every reservation in this suite reserves capacity in, and its id for
/// the rows seeded straight into the store.
const AVAILABILITY_ZONE: &str = "us-east-1a";
const AVAILABILITY_ZONE_ID: &str = "use1-az1";

/// The instance type and platform every reservation here reserves capacity for — members of
/// the modelled `InstanceType` and `CapacityReservationInstancePlatform` catalogs, and a
/// family a future-dated reservation may name ("instance types in the C, M, R, I, T, and G
/// instance families only", `CreateCapacityReservationRequest$InstanceType`).
const INSTANCE_TYPE: &str = "m5.large";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";

/// One day in seconds, the unit the future-dated lead time is published in: "You can request
/// a future-dated Capacity Reservation between 5 and 120 days in advance"
/// (`CreateCapacityReservationRequest$StartDate`).
const DAY_SECONDS: i64 = 24 * 60 * 60;

/// A WELL-FORMED Capacity Reservation id the account holds no reservation for, so the service
/// looks it up rather than refusing it as malformed before the lookup.
const UNKNOWN_CAPACITY_RESERVATION_ID: &str = "cr-0ffffffffffffffff";

/// A value outside the Capacity Reservation id form EC2 issues, so there is nothing to look
/// it up as.
const MALFORMED_CAPACITY_RESERVATION_ID: &str = "capacity-reservation-7";

/// Form fields from `(name, value)` pairs.
fn fields(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs
        .iter()
        .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
        .collect()
}

/// `now` shifted by `seconds`, as the ISO 8601 UTC timestamp the wire carries a timestamp
/// as.
fn timestamp_in(seconds: i64) -> String {
    let now = SystemTime::now();
    let shift = Duration::from_secs(seconds.unsigned_abs());
    iso8601_utc(if seconds < 0 { now - shift } else { now + shift })
}

/// One `CreateCapacityReservation` call for `instance_count` instances with `extra` beside
/// the members every row sends, answering the reservation id it minted.
async fn create_reservation(
    router: &MultiServiceRouter,
    case: &str,
    instance_count: i32,
    extra: &[(String, String)],
) -> String {
    let mut params = fields(&[
        ("InstanceType", INSTANCE_TYPE),
        ("InstancePlatform", INSTANCE_PLATFORM),
    ]);
    params.push(("InstanceCount".to_string(), instance_count.to_string()));
    params.extend(extra.iter().cloned());
    let (status, body) =
        super::ec2_query::call(router, REGION, "CreateCapacityReservation", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reserving {instance_count} instances, got \
         {status} with body {body}"
    );
    element(&body, "capacityReservationId")
        .unwrap_or_else(|| panic!("case {case:?}: no reservation id in {body}"))
}

/// The fields a create of capacity for immediate use in [`AVAILABILITY_ZONE`] sends beyond
/// the type, platform and count.
fn immediate(extra: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = fields(&[("AvailabilityZone", AVAILABILITY_ZONE)]);
    params.extend(fields(extra));
    params
}

/// The fields a FUTURE-DATED create sends: a start date inside the published lead-time
/// window, the commitment and delivery preference such a request requires, and the
/// `targeted` eligibility it must carry.
fn future_dated() -> Vec<(String, String)> {
    let mut params = immediate(&[
        ("InstanceMatchCriteria", "targeted"),
        ("DeliveryPreference", "incremental"),
        ("CommitmentDuration", "604800"),
    ]);
    params.push(("StartDate".to_string(), timestamp_in(10 * DAY_SECONDS)));
    params
}

/// One `ModifyCapacityReservation` call for `capacity_reservation_id` carrying `extra`.
async fn modify(
    router: &MultiServiceRouter,
    capacity_reservation_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = fields(&[("CapacityReservationId", capacity_reservation_id)]);
    params.extend(extra.iter().cloned());
    super::ec2_query::call(router, REGION, "ModifyCapacityReservation", &params).await
}

/// The one reservation `capacity_reservation_id` names, as `DescribeCapacityReservations`
/// reports it — asserted to be readable.
async fn described(
    router: &MultiServiceRouter,
    case: &str,
    capacity_reservation_id: &str,
) -> String {
    let params = indexed("CapacityReservationId", &[capacity_reservation_id]);
    let (status, body) =
        super::ec2_query::call(router, REGION, "DescribeCapacityReservations", &params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 describing {capacity_reservation_id}, got \
         {status} with body {body}"
    );
    body
}

/// What one row of the success table asks for, and what the reservation must report once it
/// has been applied.
struct ModifyCase {
    /// What the row covers, named in every assertion.
    case: &'static str,
    /// The create fields beyond the type, platform and count.
    create_extra: Vec<(String, String)>,
    /// The capacity the reservation is created with.
    created_count: i32,
    /// The modification's own form fields.
    modification: Vec<(String, String)>,
    /// "The total number of instances for which the Capacity Reservation reserves capacity"
    /// afterwards, which is also its available count: nothing launches into a reservation in
    /// this emulator, so every reserved instance can still be launched into it.
    expected_count: i32,
    /// The `EndDateType` the reservation must report afterwards.
    expected_end_date_type: &'static str,
    /// Whether it must report an end date at all, which is a fact of that end type.
    expects_end_date: bool,
    /// The `InstanceMatchCriteria` it must report afterwards.
    expected_match_criteria: &'static str,
}

/// The modifications rule 1 is asserted over.
///
/// The rows differ along everything the rule generalises over: which member is modified
/// (capacity, end condition, eligibility, several at once, none at all), the direction and
/// size of a capacity change — including the published bound on one — whether the end
/// condition is named with its type or by the end date alone, whether it is set or released,
/// and which eligibility the reservation starts and ends with.
fn modify_cases() -> Vec<ModifyCase> {
    vec![
        ModifyCase {
            case: "more capacity",
            create_extra: immediate(&[]),
            created_count: 4,
            modification: fields(&[("InstanceCount", "9")]),
            expected_count: 9,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "open",
        },
        ModifyCase {
            case: "less capacity, down to the published floor",
            create_extra: immediate(&[]),
            created_count: 10,
            modification: fields(&[("InstanceCount", "1")]),
            expected_count: 1,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "open",
        },
        ModifyCase {
            // "The number of instances can't be increased or decreased by more than 1000 in
            // a single request" (`ModifyCapacityReservationRequest$InstanceCount`): a change
            // OF 1000 is inside that bound, and the total it reaches is past the ceiling a
            // single create is held to, which the bound on a change is not.
            case: "a change of exactly the published maximum",
            create_extra: immediate(&[]),
            created_count: 1,
            modification: fields(&[("InstanceCount", "1001")]),
            expected_count: 1001,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "open",
        },
        ModifyCase {
            case: "the same capacity it already reserves",
            create_extra: immediate(&[]),
            created_count: 7,
            modification: fields(&[("InstanceCount", "7")]),
            expected_count: 7,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "open",
        },
        ModifyCase {
            // An end date with no end type: "limited - The Capacity Reservation expires
            // automatically at a specified date and time" (`$EndDateType`) is the end type
            // that end date belongs to.
            case: "an end date on a reservation released manually",
            create_extra: immediate(&[]),
            created_count: 2,
            modification: vec![("EndDate".to_string(), timestamp_in(2 * DAY_SECONDS))],
            expected_count: 2,
            expected_end_date_type: "limited",
            expects_end_date: true,
            expected_match_criteria: "open",
        },
        ModifyCase {
            case: "an end date with the limited end type named",
            create_extra: immediate(&[]),
            created_count: 3,
            modification: {
                let mut modification = fields(&[("EndDateType", "limited")]);
                modification.push(("EndDate".to_string(), timestamp_in(5 * DAY_SECONDS)));
                modification
            },
            expected_count: 3,
            expected_end_date_type: "limited",
            expects_end_date: true,
            expected_match_criteria: "open",
        },
        ModifyCase {
            // "unlimited - The Capacity Reservation remains active until you explicitly
            // cancel it" (`$EndDateType`), so the end date it expired at is gone.
            case: "the end date released",
            create_extra: {
                let mut extra = immediate(&[("EndDateType", "limited")]);
                extra.push(("EndDate".to_string(), timestamp_in(3 * DAY_SECONDS)));
                extra
            },
            created_count: 5,
            modification: fields(&[("EndDateType", "unlimited")]),
            expected_count: 5,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "open",
        },
        ModifyCase {
            case: "instance eligibility narrowed to targeted",
            create_extra: immediate(&[]),
            created_count: 6,
            modification: fields(&[("InstanceMatchCriteria", "targeted")]),
            expected_count: 6,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "targeted",
        },
        ModifyCase {
            case: "instance eligibility opened",
            create_extra: immediate(&[("InstanceMatchCriteria", "targeted")]),
            created_count: 2,
            modification: fields(&[("InstanceMatchCriteria", "open")]),
            expected_count: 2,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "open",
        },
        ModifyCase {
            case: "the capacity, the end condition and the eligibility at once",
            create_extra: immediate(&[]),
            created_count: 8,
            modification: {
                let mut modification = fields(&[
                    ("InstanceCount", "12"),
                    ("EndDateType", "limited"),
                    ("InstanceMatchCriteria", "targeted"),
                ]);
                modification.push(("EndDate".to_string(), timestamp_in(7 * DAY_SECONDS)));
                modification
            },
            expected_count: 12,
            expected_end_date_type: "limited",
            expects_end_date: true,
            expected_match_criteria: "targeted",
        },
        ModifyCase {
            // "Reserved. Capacity Reservations you have created are accepted by default"
            // (`$Accept`) and "Reserved for future use" (`$AdditionalInfo`): a request
            // naming only those asks for no modification, so the reservation stands as it
            // was.
            case: "the reserved members only",
            create_extra: immediate(&[("InstanceMatchCriteria", "targeted")]),
            created_count: 3,
            modification: fields(&[("Accept", "true"), ("AdditionalInfo", "none")]),
            expected_count: 3,
            expected_end_date_type: "unlimited",
            expects_end_date: false,
            expected_match_criteria: "targeted",
        },
    ]
}

/// Rule 1: every modification is applied to the stored reservation, and the next read of it
/// reports what was applied.
#[tokio::test]
async fn a_modification_is_applied_and_read_back() {
    for row in modify_cases() {
        let (router, _engine) = super::ec2_query::emulator();
        let case = row.case;
        let id = create_reservation(&router, case, row.created_count, &row.create_extra).await;

        let (status, body) = modify(&router, &id, &row.modification).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected the modification to answer true, got {body}"
        );
        // "The status of the requested modification" reports an adjustment that is not in
        // effect yet, and this one is applied by the time the call answers.
        assert_eq!(
            element(&body, "adjustmentStatus"),
            None,
            "case {case:?}: expected no adjustment status on an applied modification, \
             got {body}"
        );

        let body = described(&router, case, &id).await;
        let expected: Vec<(&str, String)> = vec![
            ("totalInstanceCount", row.expected_count.to_string()),
            // Nothing launches into a reservation in this emulator, so the capacity that
            // can still be launched into it moves with the total.
            ("availableInstanceCount", row.expected_count.to_string()),
            ("endDateType", row.expected_end_date_type.to_string()),
            (
                "instanceMatchCriteria",
                row.expected_match_criteria.to_string(),
            ),
            // The members a modification may not change: "You can't modify a Capacity
            // Reservation's instance type, EBS optimization, platform, instance store
            // settings, Availability Zone, or tenancy" (`ModifyCapacityReservation`).
            ("instanceType", INSTANCE_TYPE.to_string()),
            ("instancePlatform", INSTANCE_PLATFORM.to_string()),
            ("availabilityZone", AVAILABILITY_ZONE.to_string()),
            ("tenancy", "default".to_string()),
        ];
        for (name, want) in &expected {
            let got = element(&body, name);
            assert_eq!(
                got.as_deref(),
                Some(want.as_str()),
                "case {case:?}: expected {name} {want:?}, got {got:?} in {body}"
            );
        }

        let end_date = element(&body, "endDate");
        assert_eq!(
            end_date.is_some(),
            row.expects_end_date,
            "case {case:?}: expected an end date present={}, got {end_date:?}",
            row.expects_end_date
        );
        // The end date the request named, as the reservation now reports it: the same
        // instant, read back through the describe.
        if let Some(wanted) = row
            .modification
            .iter()
            .find(|(name, _)| name == "EndDate")
            .map(|(_, value)| value.clone())
        {
            let reported = end_date.as_deref().and_then(as_timestamp);
            assert_eq!(
                reported,
                as_timestamp(&wanted),
                "case {case:?}: expected the end date {wanted:?} to be reported, got \
                 {end_date:?}"
            );
        }
    }
}

/// Which reservation a refusal row names.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The `active` reservation the row created.
    Active,
    /// A well-formed id the account holds no reservation for.
    Unknown,
    /// A value outside the Capacity Reservation id form.
    Malformed,
    /// The row's reservation, cancelled before the modification — a state that admits no
    /// modification at all.
    Cancelled,
    /// A FUTURE-DATED reservation, which EC2 is still assessing or has scheduled for its
    /// start date: "You can modify the tags only".
    FutureDated,
}

/// One row of the refusal table: what it covers, which reservation it names, the
/// modification's form fields, and the error code and HTTP status it must be answered with.
struct RefusalCase {
    case: &'static str,
    target: Target,
    modification: Vec<(String, String)>,
    expected_code: &'static str,
    expected_status: u16,
}

/// The refusals rules 2 and 3 are asserted over.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "an id the account holds no reservation for",
            target: Target::Unknown,
            modification: fields(&[("InstanceCount", "2")]),
            expected_code: "InvalidCapacityReservationId.NotFound",
            expected_status: 400,
        },
        RefusalCase {
            case: "a value outside the Capacity Reservation id form",
            target: Target::Malformed,
            modification: fields(&[("InstanceCount", "2")]),
            expected_code: "InvalidCapacityReservationId.Malformed",
            expected_status: 400,
        },
        RefusalCase {
            case: "capacity for no instances",
            target: Target::Active,
            modification: fields(&[("InstanceCount", "0")]),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "a negative capacity",
            target: Target::Active,
            modification: fields(&[("InstanceCount", "-1")]),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            // One past the published bound on a single change: 1005 - 4 is 1001.
            case: "one more than the published maximum change",
            target: Target::Active,
            modification: fields(&[("InstanceCount", "1005")]),
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "an end date with the unlimited end type",
            target: Target::Active,
            modification: {
                let mut modification = fields(&[("EndDateType", "unlimited")]);
                modification.push(("EndDate".to_string(), timestamp_in(2 * DAY_SECONDS)));
                modification
            },
            expected_code: "InvalidParameterCombination",
            expected_status: 400,
        },
        RefusalCase {
            case: "the limited end type with no end date",
            target: Target::Active,
            modification: fields(&[("EndDateType", "limited")]),
            expected_code: "MissingParameter",
            expected_status: 400,
        },
        RefusalCase {
            case: "an end date that has already passed",
            target: Target::Active,
            modification: vec![("EndDate".to_string(), timestamp_in(-DAY_SECONDS))],
            expected_code: "InvalidParameterValue",
            expected_status: 400,
        },
        RefusalCase {
            case: "a modification of a cancelled reservation",
            target: Target::Cancelled,
            modification: fields(&[("InstanceCount", "2")]),
            expected_code: "IncorrectState",
            expected_status: 400,
        },
        RefusalCase {
            case: "the capacity of a reservation that is not delivered yet",
            target: Target::FutureDated,
            modification: fields(&[("InstanceCount", "2")]),
            expected_code: "IncorrectState",
            expected_status: 400,
        },
        RefusalCase {
            case: "the eligibility of a reservation that is not delivered yet",
            target: Target::FutureDated,
            modification: fields(&[("InstanceMatchCriteria", "open")]),
            expected_code: "IncorrectState",
            expected_status: 400,
        },
        RefusalCase {
            case: "a start date on a reservation whose capacity is delivered",
            target: Target::Active,
            modification: vec![("StartDate".to_string(), timestamp_in(6 * DAY_SECONDS))],
            expected_code: "IncorrectState",
            expected_status: 400,
        },
        RefusalCase {
            case: "a start date push-out with no quote",
            target: Target::FutureDated,
            modification: vec![("StartDate".to_string(), timestamp_in(20 * DAY_SECONDS))],
            expected_code: "MissingParameter",
            expected_status: 400,
        },
        RefusalCase {
            case: "a quote the account does not hold",
            target: Target::FutureDated,
            modification: {
                let mut modification = fields(&[
                    ("QuoteId", "crmq-0123456789abcdef0"),
                    ("AcceptModificationTerms", "true"),
                ]);
                modification.push(("StartDate".to_string(), timestamp_in(20 * DAY_SECONDS)));
                modification
            },
            expected_code: "InvalidCapacityReservationModificationQuoteId.NotFound",
            expected_status: 400,
        },
        RefusalCase {
            // A value outside the quote id form this service issues, refused before any
            // lookup the way every other id of this service is.
            case: "a value that is not a modification quote id",
            target: Target::FutureDated,
            modification: {
                let mut modification = fields(&[
                    ("QuoteId", "modification-quote-7"),
                    ("AcceptModificationTerms", "true"),
                ]);
                modification.push(("StartDate".to_string(), timestamp_in(20 * DAY_SECONDS)));
                modification
            },
            expected_code: "InvalidCapacityReservationModificationQuoteId.Malformed",
            expected_status: 400,
        },
        RefusalCase {
            // "You can't modify the Capacity Reservation in any way" (this operation's own
            // doc) is about the REQUEST, so a modify of a reservation in such a state is
            // refused even when it names no member to change.
            case: "a modify of a cancelled reservation naming no member",
            target: Target::Cancelled,
            modification: Vec::new(),
            expected_code: "IncorrectState",
            expected_status: 400,
        },
    ]
}

/// Rules 2 and 3: a refusal carries EC2's own code for what the request got wrong, and the
/// reservation is left exactly as it was.
#[tokio::test]
async fn a_refused_modification_changes_nothing() {
    for row in refusal_cases() {
        let (router, _engine) = super::ec2_query::emulator();
        let case = row.case;
        // The reservation every row holds, whatever id the modification names: the row's
        // own when it names one, so the "nothing was written" read below always has a
        // reservation to read.
        let created_count = 4;
        let create_extra = match row.target {
            Target::FutureDated => future_dated(),
            _ => immediate(&[]),
        };
        let id = create_reservation(&router, case, created_count, &create_extra).await;
        if matches!(row.target, Target::Cancelled) {
            let params = fields(&[("CapacityReservationId", id.as_str())]);
            let (status, body) =
                super::ec2_query::call(&router, REGION, "CancelCapacityReservation", &params)
                    .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 cancelling {id}, got {status} with \
                 body {body}"
            );
        }
        let before = described(&router, case, &id).await;

        let named = match row.target {
            Target::Unknown => UNKNOWN_CAPACITY_RESERVATION_ID.to_string(),
            Target::Malformed => MALFORMED_CAPACITY_RESERVATION_ID.to_string(),
            _ => id.clone(),
        };
        let (status, body) = modify(&router, &named, &row.modification).await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected HTTP {}, got {status} with body {body}",
            row.expected_status
        );
        let code = error_code(&body);
        assert_eq!(
            code, row.expected_code,
            "case {case:?}: expected {}, got {code:?} in {body}",
            row.expected_code
        );

        // Nothing was written: every member a modification could have changed still reads
        // as it did before the refused call.
        let after = described(&router, case, &id).await;
        for name in [
            "totalInstanceCount",
            "availableInstanceCount",
            "endDateType",
            "instanceMatchCriteria",
            "startDate",
        ] {
            assert_eq!(
                element(&after, name),
                element(&before, name),
                "case {case:?}: expected {name} to be untouched by the refused \
                 modification, got {after}"
            );
        }
        assert_eq!(
            element(&after, "endDate"),
            element(&before, "endDate"),
            "case {case:?}: expected the end date to be untouched by the refused \
             modification, got {after}"
        );
    }
}

/// Rule 4: a dry run answers the outcome and writes nothing.
#[tokio::test]
async fn a_dry_run_modification_writes_nothing() {
    // The rows differ in what the dry run would have changed.
    let cases: [(&str, Vec<(String, String)>); 3] = [
        ("the capacity", fields(&[("InstanceCount", "11")])),
        (
            "the end condition",
            vec![("EndDate".to_string(), timestamp_in(4 * DAY_SECONDS))],
        ),
        (
            "the eligibility",
            fields(&[("InstanceMatchCriteria", "targeted")]),
        ),
    ];
    for (case, modification) in cases {
        let (router, _engine) = super::ec2_query::emulator();
        let id = create_reservation(&router, case, 4, &immediate(&[])).await;
        let before = described(&router, case, &id).await;

        let mut params = modification.clone();
        params.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = modify(&router, &id, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: a dry run that would have succeeded: expected HTTP 412, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );

        let after = described(&router, case, &id).await;
        for name in [
            "totalInstanceCount",
            "availableInstanceCount",
            "endDateType",
            "instanceMatchCriteria",
        ] {
            assert_eq!(
                element(&after, name),
                element(&before, name),
                "case {case:?}: expected {name} to be untouched by the dry run, got {after}"
            );
        }
        assert_eq!(
            element(&after, "endDate"),
            None,
            "case {case:?}: expected no end date after the dry run, got {after}"
        );

        // The same modification without the dry run is applied, which is what makes the
        // dry run's refusal a report rather than a failure.
        let (status, body) = modify(&router, &id, &modification).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 applying the modification, got {status} \
             with body {body}"
        );
        assert_eq!(
            element(&body, "return").as_deref(),
            Some("true"),
            "case {case:?}: expected the modification to answer true, got {body}"
        );
        let after = described(&router, case, &id).await;
        assert_ne!(
            (
                element(&after, "totalInstanceCount"),
                element(&after, "endDate"),
                element(&after, "instanceMatchCriteria"),
            ),
            (
                element(&before, "totalInstanceCount"),
                element(&before, "endDate"),
                element(&before, "instanceMatchCriteria"),
            ),
            "case {case:?}: expected the applied modification to change the \
             reservation, got {after}"
        );
    }
}

/// What one seeded reservation holds: the shapes no implemented operation of this crate can
/// produce, written straight into the store the way the sibling Capacity Reservation suites
/// seed theirs.
struct Seeded {
    /// The capacity the reservation reserves.
    instance_count: i32,
    /// How much longer the commitment it is inside still runs, or `None` for a reservation
    /// that committed to nothing. A reservation inside a commitment is `active` only once it
    /// has been delivered, which is at least five days after the create that requested it.
    commitment_for: Option<Duration>,
    /// The capacity instances are running in, recorded as the `used` capacity allocation the
    /// model carries it as. Nothing in this emulator launches an instance into a Capacity
    /// Reservation, so this is the only way a reservation has usage at all.
    in_use: i32,
    /// Whether the reservation holds a Capacity Block's capacity.
    capacity_block: bool,
}

/// `seeded` written into the store as an `active` Capacity Reservation, answering its id.
fn seed(engine: &engine::InMemoryEngine, seeded: &Seeded) -> String {
    let now = SystemTime::now();
    let state = engine.state::<Ec2State>();
    let mut store = state.lock();
    let id = store
        .insert_capacity_reservation(CapacityReservationSpec {
            instance_type: INSTANCE_TYPE.to_string(),
            instance_platform: INSTANCE_PLATFORM.to_string(),
            availability_zone: AVAILABILITY_ZONE.to_string(),
            availability_zone_id: AVAILABILITY_ZONE_ID.to_string(),
            region: REGION.to_string(),
            tenancy: "default".to_string(),
            instance_count: seeded.instance_count,
            ebs_optimized: false,
            ephemeral_storage: false,
            end_date: None,
            instance_match_criteria: "open".to_string(),
            created_at: now,
            // Delivered capacity: no requested delivery date and no delivery method, so the
            // reservation is `active` from the moment it is seeded.
            start_date: None,
            commitment: seeded
                .commitment_for
                .map(|runs_for| CapacityReservationCommitment {
                    duration_seconds: i64::try_from(runs_for.as_secs())
                        .expect("a commitment of representable length"),
                    end_date: now + runs_for,
                }),
            delivery_preference: None,
            state: CAPACITY_RESERVATION_STATE_ACTIVE,
            tags: Vec::new(),
            interruption_info: None,
            zero_size_preference: None,
            pending: None,
        })
        .expect("the store accepts a seeded Capacity Reservation");
    if let Some(record) = store.capacity_reservations.get_mut(&id) {
        if seeded.in_use > 0 {
            record.view.capacity_allocations = Some(vec![CapacityAllocation {
                // "`used` indicates that the instance capacity is in use by instances that
                // are running in the Capacity Reservation"
                // (`CapacityAllocation$AllocationType`).
                allocation_type: Some("used".to_string()),
                count: Some(seeded.in_use),
                allocation_metadata: None,
            }]);
            record.view.available_instance_count =
                Some(seeded.instance_count - seeded.in_use);
        }
        if seeded.capacity_block {
            record.view.capacity_block_id = Some("cb-0123456789abcdef0".to_string());
        }
    }
    id
}

/// One row of the commitment-and-usage table: what it covers, the reservation it seeds, the
/// modification it asks for, and the code it must be refused with — or `None` for a
/// modification the rule admits.
struct BoundCase {
    case: &'static str,
    seeded: Seeded,
    modification: Vec<(String, String)>,
    expected_code: Option<&'static str>,
}

/// A week, the commitment every seeded row of the table below is inside.
const COMMITMENT: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The rows rule 5 is asserted over: both halves of "You can't decrease the instance count or
/// set an end date that is within the commitment duration", the modifications the same
/// sentence still admits, the zero-usage requirement on the instance eligibility, and the
/// Capacity Block that admits no modification at all.
fn bound_cases() -> Vec<BoundCase> {
    vec![
        BoundCase {
            case: "a decrease inside the commitment",
            seeded: Seeded {
                instance_count: 8,
                commitment_for: Some(COMMITMENT),
                in_use: 0,
                capacity_block: false,
            },
            modification: fields(&[("InstanceCount", "4")]),
            expected_code: Some("InvalidParameterValue"),
        },
        BoundCase {
            // "All other modifications are allowed" (`ModifyCapacityReservation`).
            case: "an increase inside the commitment",
            seeded: Seeded {
                instance_count: 8,
                commitment_for: Some(COMMITMENT),
                in_use: 0,
                capacity_block: false,
            },
            modification: fields(&[("InstanceCount", "12")]),
            expected_code: None,
        },
        BoundCase {
            case: "an end date inside the commitment",
            seeded: Seeded {
                instance_count: 4,
                commitment_for: Some(COMMITMENT),
                in_use: 0,
                capacity_block: false,
            },
            modification: vec![("EndDate".to_string(), timestamp_in(2 * DAY_SECONDS))],
            expected_code: Some("InvalidParameterValue"),
        },
        BoundCase {
            case: "an end date past the commitment",
            seeded: Seeded {
                instance_count: 4,
                commitment_for: Some(COMMITMENT),
                in_use: 0,
                capacity_block: false,
            },
            modification: vec![("EndDate".to_string(), timestamp_in(10 * DAY_SECONDS))],
            expected_code: None,
        },
        BoundCase {
            case: "the eligibility inside the commitment",
            seeded: Seeded {
                instance_count: 4,
                commitment_for: Some(COMMITMENT),
                in_use: 0,
                capacity_block: false,
            },
            modification: fields(&[("InstanceMatchCriteria", "targeted")]),
            expected_code: None,
        },
        BoundCase {
            case: "a capacity below the capacity in use",
            seeded: Seeded {
                instance_count: 8,
                commitment_for: None,
                in_use: 5,
                capacity_block: false,
            },
            modification: fields(&[("InstanceCount", "4")]),
            expected_code: Some("InvalidParameterValue"),
        },
        BoundCase {
            case: "a capacity above the capacity in use",
            seeded: Seeded {
                instance_count: 8,
                commitment_for: None,
                in_use: 5,
                capacity_block: false,
            },
            modification: fields(&[("InstanceCount", "10")]),
            expected_code: None,
        },
        BoundCase {
            case: "the eligibility while capacity is in use",
            seeded: Seeded {
                instance_count: 8,
                commitment_for: None,
                in_use: 2,
                capacity_block: false,
            },
            modification: fields(&[("InstanceMatchCriteria", "targeted")]),
            expected_code: Some("IncorrectState"),
        },
        BoundCase {
            case: "the capacity of a Capacity Block",
            seeded: Seeded {
                instance_count: 4,
                commitment_for: None,
                in_use: 0,
                capacity_block: true,
            },
            modification: fields(&[("InstanceCount", "6")]),
            expected_code: Some("IncorrectState"),
        },
        BoundCase {
            case: "the end condition of a Capacity Block",
            seeded: Seeded {
                instance_count: 4,
                commitment_for: None,
                in_use: 0,
                capacity_block: true,
            },
            modification: fields(&[("EndDateType", "unlimited")]),
            expected_code: Some("IncorrectState"),
        },
    ]
}

/// Rule 5: the commitment and the capacity instances are running in bound what a
/// modification may do, and a Capacity Block admits none.
#[tokio::test]
async fn the_commitment_and_the_capacity_in_use_bound_a_modification() {
    for row in bound_cases() {
        let (router, engine) = super::ec2_query::emulator();
        let case = row.case;
        let id = seed(&engine, &row.seeded);
        let before = described(&router, case, &id).await;

        let (status, body) = modify(&router, &id, &row.modification).await;
        let after = described(&router, case, &id).await;
        match row.expected_code {
            Some(expected) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with body {body}"
                );
                let code = error_code(&body);
                assert_eq!(
                    code, expected,
                    "case {case:?}: expected {expected}, got {code:?} in {body}"
                );
                for name in [
                    "totalInstanceCount",
                    "availableInstanceCount",
                    "endDateType",
                    "instanceMatchCriteria",
                ] {
                    assert_eq!(
                        element(&after, name),
                        element(&before, name),
                        "case {case:?}: expected {name} to be untouched by the refused \
                         modification, got {after}"
                    );
                }
                assert_eq!(
                    element(&after, "endDate"),
                    element(&before, "endDate"),
                    "case {case:?}: expected the end date to be untouched by the refused \
                     modification, got {after}"
                );
            }
            None => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected HTTP 200, got {status} with body {body}"
                );
                assert_eq!(
                    element(&body, "return").as_deref(),
                    Some("true"),
                    "case {case:?}: expected the modification to answer true, got {body}"
                );
                // The modification landed: the member the row asked for differs from what
                // the reservation carried before it.
                let member = match row.modification.first().map(|(name, _)| name.as_str()) {
                    Some("InstanceCount") => "totalInstanceCount",
                    Some("EndDate") => "endDate",
                    Some("InstanceMatchCriteria") => "instanceMatchCriteria",
                    other => panic!("case {case:?}: unexpected modification {other:?}"),
                };
                assert_ne!(
                    element(&after, member),
                    element(&before, member),
                    "case {case:?}: expected {member} to carry the modification, got {after}"
                );
                // The capacity instances are running in is still reserved for them: the
                // capacity nothing has launched into never covers it.
                let total: i32 = element(&after, "totalInstanceCount")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default();
                assert!(
                    total >= row.seeded.in_use,
                    "case {case:?}: expected at least the {} instances in use to stay \
                     reserved, got {total}",
                    row.seeded.in_use
                );
            }
        }
    }
}
