// SPDX-License-Identifier: CC0-1.0

//! `DescribeCapacityBlockOfferings` end-to-end, over the wire the service actually speaks:
//! an `application/x-www-form-urlencoded` ec2Query POST carrying
//! `Action=DescribeCapacityBlockOfferings`, dispatched through the same
//! `MultiServiceRouter` the emulator serves with and signed for a Region, which is where
//! the service reads the Region it is being asked about.
//!
//! The rules under test:
//!
//! 1. The answer is COMPUTED FROM THE SEARCH: every Availability Zone of the Region the
//!    request was signed for offers the window the search describes — the instance or
//!    UltraServer type and counts it names, held for exactly `CapacityDurationHours`,
//!    starting at the soonest whole hour at or after `StartDateRange` (never in the past)
//!    and ending no later than `EndDateRange`. A duration no Capacity Block is sold for, an
//!    instance type the model does not declare, or a count that reserves nothing is refused;
//!    a window that cannot close inside `EndDateRange` is simply offered nowhere.
//! 2. An offering is DERIVED, NOT STORED: its id is a function of its own terms, so the same
//!    search answers the same ids, the search writes nothing, and the id decodes back to the
//!    zone, counts and window it was reported with.
//! 3. `MaxResults`/`NextToken` page that set, `DryRun` performs nothing, and a token this
//!    service did not mint is refused.
//!
//! Every table row is a distinct input along what those rules generalise over — the Region,
//! the instance type, the counts, the duration, the date bounds, the page size — so no
//! single recorded search is the specification.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use ec2::state::{
    availability_zones_in_region, capacity_block_upfront_fee, decode_capacity_block_offering_id,
    offered_instance_types, CapacityBlockOfferingTerms, CAPACITY_BLOCK_OFFERING_CURRENCY_CODE,
    CAPACITY_BLOCK_OFFERING_TENANCY, ZONE_TYPE_AVAILABILITY_ZONE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, unescaped};

/// The seconds in one hour, the unit `CapacityDurationHours` is given in.
const SECONDS_PER_HOUR: i64 = 3_600;

/// One `DescribeCapacityBlockOfferings` call signed for `region` and carrying `params`, as
/// its HTTP status and XML body.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeCapacityBlockOfferings", params).await
}

/// One offering of a response. Every member is read as text, so a member the offering does
/// not carry reads as `None` rather than as a default.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Offering {
    offering_id: String,
    instance_type: Option<String>,
    availability_zone: String,
    instance_count: Option<String>,
    start_date: String,
    end_date: String,
    duration_hours: Option<String>,
    duration_minutes: Option<String>,
    upfront_fee: Option<String>,
    currency_code: Option<String>,
    tenancy: Option<String>,
    ultraserver_type: Option<String>,
    ultraserver_count: Option<String>,
    zone_type: Option<String>,
}

impl Offering {
    /// The offering's window as seconds since the Unix epoch.
    fn window(&self, case: &str) -> (i64, i64) {
        (
            epoch_seconds(case, &self.start_date),
            epoch_seconds(case, &self.end_date),
        )
    }
}

/// Every offering in an ec2Query `DescribeCapacityBlockOfferings` body, in the order the
/// response carried them. Each offering is one `<item>` of the `<capacityBlockOfferingSet>`
/// list, whose first element carries an `xmlns` attribute — so the opening tag is read up to
/// its `>`, exactly as the `DescribeInstanceTypeOfferings` suite reads its own list.
fn offerings(case: &str, body: &str) -> Vec<Offering> {
    let list = element(body, "capacityBlockOfferingSet").unwrap_or_default();
    list.split("<item")
        .skip(1)
        .map(|rest| {
            let after_tag = rest.split_once('>').map(|(_, rest)| rest).unwrap_or(rest);
            let item = after_tag.split("</item>").next().unwrap_or_default();
            let text = |name: &str| element(item, name).map(|value| unescaped(&value));
            let required = |name: &str| {
                text(name).unwrap_or_else(|| {
                    panic!("case {case:?}: no <{name}> in the offering {item:?}")
                })
            };
            Offering {
                offering_id: required("capacityBlockOfferingId"),
                instance_type: text("instanceType"),
                availability_zone: required("availabilityZone"),
                instance_count: text("instanceCount"),
                start_date: required("startDate"),
                end_date: required("endDate"),
                duration_hours: text("capacityBlockDurationHours"),
                duration_minutes: text("capacityBlockDurationMinutes"),
                upfront_fee: text("upfrontFee"),
                currency_code: text("currencyCode"),
                tenancy: text("tenancy"),
                ultraserver_type: text("ultraserverType"),
                ultraserver_count: text("ultraserverCount"),
                zone_type: text("zoneType"),
            }
        })
        .collect()
}

/// A timestamp the wire carries, as seconds since the Unix epoch.
fn epoch_seconds(case: &str, rendered: &str) -> i64 {
    DateTime::from_str(rendered, Format::DateTime)
        .unwrap_or_else(|error| panic!("case {case:?}: {rendered:?} is not a timestamp: {error}"))
        .secs()
}

/// Now, in whole seconds since the Unix epoch.
fn now_seconds() -> i64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX)
}

/// The first whole hour at or after `at` — the alignment every offering's window opens on.
fn whole_hour_at_or_after(at: i64) -> i64 {
    let past_the_hour = at.rem_euclid(SECONDS_PER_HOUR);
    if past_the_hour == 0 {
        at
    } else {
        at + SECONDS_PER_HOUR - past_the_hour
    }
}

/// A timestamp as an ec2Query form value.
fn timestamp(at_epoch_seconds: i64) -> String {
    DateTime::from_secs(at_epoch_seconds)
        .fmt(Format::DateTime)
        .expect("a renderable timestamp")
}

/// One form field.
fn field(name: &str, value: impl std::fmt::Display) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The Availability Zone names of `region`, in the order a response reports them.
fn zone_names(region: &str) -> Vec<String> {
    let mut names: Vec<String> = availability_zones_in_region(region)
        .into_iter()
        .map(|zone| zone.zone_name)
        .collect();
    names.sort();
    names
}

/// A successful search's offerings.
async fn search(
    router: &MultiServiceRouter,
    region: &str,
    case: &str,
    params: &[(String, String)],
) -> Vec<Offering> {
    let (status, body) = describe(router, region, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200, got {status} with body {body}"
    );
    offerings(case, &body)
}

// ---------------------------------------------------------------------------
// The answer is computed from the search
// ---------------------------------------------------------------------------

/// One row of the search table: the case, the Region the call is signed for, the instance
/// type searched for, the instance count it names (or `None`), and the reservation duration.
struct SearchCase {
    case: &'static str,
    region: &'static str,
    instance_type: &'static str,
    instance_count: Option<i32>,
    duration_hours: i32,
}

/// The searches every offering-shape assertion below is made over.
///
/// The rows differ along everything rule 1 generalises over: the Region (and so the zone set
/// and the zone ids), the instance type — six entries of the modelled `InstanceType` catalog
/// (`smithy_query(query="shape",shape="InstanceType")`), none of them named by the issue this
/// suite closes — the instance count, including the published maximum of 64 and a search that
/// names none, and the duration, covering both published increments and both of their bounds
/// (24 h = 1 day, 336 h = 14 days, 504 h = 3 weeks, 4368 h = 182 days).
fn search_cases() -> Vec<SearchCase> {
    vec![
        SearchCase {
            case: "one instance for one day",
            region: "us-east-1",
            instance_type: "p5.48xlarge",
            instance_count: Some(1),
            duration_hours: 24,
        },
        SearchCase {
            case: "a full block for two days",
            region: "eu-west-2",
            instance_type: "trn1.32xlarge",
            instance_count: Some(64),
            duration_hours: 48,
        },
        SearchCase {
            case: "fourteen days, the last daily increment",
            region: "us-west-2",
            instance_type: "p4d.24xlarge",
            instance_count: Some(8),
            duration_hours: 336,
        },
        SearchCase {
            case: "three weeks",
            region: "ap-southeast-1",
            instance_type: "m5.large",
            instance_count: Some(4),
            duration_hours: 504,
        },
        SearchCase {
            case: "no instance count named",
            region: "eu-central-1",
            instance_type: "c6g.medium",
            instance_count: None,
            duration_hours: 24,
        },
        SearchCase {
            case: "182 days, the longest reservation sold",
            region: "us-east-2",
            instance_type: "t2.nano",
            instance_count: Some(2),
            duration_hours: 4_368,
        },
    ]
}

/// Every Availability Zone of the signed-for Region offers the searched window, with the
/// counts, duration, price and zone facts the search and the zone imply.
#[tokio::test]
async fn every_zone_of_the_signed_region_offers_the_searched_window() {
    let (router, engine) = super::ec2_query::emulator();

    for row in search_cases() {
        let case = row.case;
        let mut params = vec![
            field("InstanceType", row.instance_type),
            field("CapacityDurationHours", row.duration_hours),
        ];
        if let Some(instance_count) = row.instance_count {
            params.push(field("InstanceCount", instance_count));
        }

        let before = now_seconds();
        let reported = search(&router, row.region, case, &params).await;
        let after = now_seconds();

        let zones: Vec<String> = reported
            .iter()
            .map(|offering| offering.availability_zone.clone())
            .collect();
        assert_eq!(
            zones,
            zone_names(row.region),
            "case {case:?}: expected one offering per Availability Zone of {}, got {zones:?}",
            row.region
        );

        // What the search reserves: the count it named, or the one unit the smallest
        // Capacity Block holds when it named none.
        let expected_count = row.instance_count.unwrap_or(1);
        let expected_fee = capacity_block_upfront_fee(
            u64::try_from(expected_count).expect("a positive count"),
            u64::try_from(row.duration_hours).expect("a positive duration"),
        );
        for offering in &reported {
            let zone = offering.availability_zone.clone();
            let case = format!("{case} in {zone}");
            let members: [(&str, Option<String>, Option<String>); 8] = [
                (
                    "instanceType",
                    offering.instance_type.clone(),
                    Some(row.instance_type.to_string()),
                ),
                (
                    "instanceCount",
                    offering.instance_count.clone(),
                    Some(expected_count.to_string()),
                ),
                (
                    "capacityBlockDurationHours",
                    offering.duration_hours.clone(),
                    Some(row.duration_hours.to_string()),
                ),
                (
                    "capacityBlockDurationMinutes",
                    offering.duration_minutes.clone(),
                    Some("0".to_string()),
                ),
                (
                    "upfrontFee",
                    offering.upfront_fee.clone(),
                    Some(expected_fee.clone()),
                ),
                (
                    "currencyCode",
                    offering.currency_code.clone(),
                    Some(CAPACITY_BLOCK_OFFERING_CURRENCY_CODE.to_string()),
                ),
                (
                    "tenancy",
                    offering.tenancy.clone(),
                    Some(CAPACITY_BLOCK_OFFERING_TENANCY.to_string()),
                ),
                (
                    "zoneType",
                    offering.zone_type.clone(),
                    Some(ZONE_TYPE_AVAILABILITY_ZONE.to_string()),
                ),
            ];
            for (member, actual, expected) in members {
                assert_eq!(
                    actual, expected,
                    "case {case:?}: expected {member} {expected:?}, got {actual:?} in \
                     {offering:?}"
                );
            }
            // An offering of plain instances reserves no UltraServers, so it names neither
            // an UltraServer type nor a count.
            assert_eq!(
                (
                    offering.ultraserver_type.clone(),
                    offering.ultraserver_count.clone()
                ),
                (None, None),
                "case {case:?}: an instance offering carries no UltraServer members, got \
                 {offering:?}"
            );

            // The window: exactly the searched duration, opening on the first whole hour at
            // or after the moment of the call.
            let (start, end) = offering.window(&case);
            assert_eq!(
                end - start,
                i64::from(row.duration_hours) * SECONDS_PER_HOUR,
                "case {case:?}: expected a window of {} hours, got {start}..{end}",
                row.duration_hours
            );
            let earliest = whole_hour_at_or_after(before);
            let latest = whole_hour_at_or_after(after);
            assert!(
                (earliest..=latest).contains(&start),
                "case {case:?}: expected the window to open on the first whole hour in \
                 {earliest}..={latest}, got {start}"
            );
        }

        // Rule 2: a search is a read. It writes no Capacity Block, and nothing else of the
        // store, so a later caller sees exactly what it saw.
        assert!(
            engine
                .state::<ec2::state::ComAmazonawsEc2Amazonec2State>()
                .lock()
                .capacity_blocks
                .is_empty(),
            "case {case:?}: searching for offerings must not write to the store"
        );
    }
}

/// One row of the date-range table: the case, the `StartDateRange` and `EndDateRange`
/// offsets from now (in seconds, either of which the search may leave out), the reservation
/// duration in hours, and whether any window fits those bounds.
type DateRangeCase = (&'static str, Option<i64>, Option<i64>, i32, bool);

/// `StartDateRange` is the earliest start the search will take and `EndDateRange` the latest
/// end: a window is moved forward to the first whole hour at or after the one, never into the
/// past, and a duration that cannot close inside the other is offered nowhere.
#[tokio::test]
async fn the_requested_date_range_bounds_the_window() {
    let (router, _engine) = super::ec2_query::emulator();
    let now = now_seconds();
    let day = 24 * SECONDS_PER_HOUR;

    // The offsets span: no bound at all, a bound in the past, bounds minutes / days / weeks
    // ahead, a bound at the far end of what a timestamp can carry, an end bound that just
    // fits the duration and one that is an hour short of it.
    let cases: [DateRangeCase; 9] = [
        ("no date bounds", None, None, 24, true),
        ("a start bound in the past", Some(-7 * day), None, 24, true),
        ("a start bound minutes ahead", Some(600), None, 24, true),
        ("a start bound ten days ahead", Some(10 * day), None, 48, true),
        ("a start bound a quarter ahead", Some(90 * day), None, 168, true),
        (
            "a start bound fifty years ahead",
            Some(50 * 365 * day),
            None,
            336,
            true,
        ),
        (
            // The last instant an RFC 3339 timestamp can name: a window opening there cannot
            // CLOSE inside the representable range, so nothing is offered rather than an
            // offering carrying a window the wire cannot write.
            "a start bound at the last representable instant",
            Some(253_402_300_799 - now),
            None,
            24,
            false,
        ),
        (
            "an end bound that fits the duration",
            Some(day),
            Some(3 * day),
            24,
            true,
        ),
        (
            "an end bound an hour short of the duration",
            None,
            Some(23 * SECONDS_PER_HOUR),
            24,
            false,
        ),
    ];

    for (case, start_offset, end_offset, duration_hours, offered) in cases {
        let mut params = vec![
            field("InstanceType", "p5.48xlarge"),
            field("InstanceCount", 2),
            field("CapacityDurationHours", duration_hours),
        ];
        if let Some(offset) = start_offset {
            params.push(field("StartDateRange", timestamp(now + offset)));
        }
        if let Some(offset) = end_offset {
            params.push(field("EndDateRange", timestamp(now + offset)));
        }

        let before = now_seconds();
        let reported = search(&router, "us-east-1", case, &params).await;
        let after = now_seconds();

        if !offered {
            assert!(
                reported.is_empty(),
                "case {case:?}: no window fits the bounds, so nothing is offered; got \
                 {reported:?}"
            );
            continue;
        }
        assert!(
            !reported.is_empty(),
            "case {case:?}: expected the Region's zones to offer a window, got none"
        );
        for offering in &reported {
            let (start, end) = offering.window(case);
            // Never in the past, and never before the start the caller asked for.
            let floor = start_offset.map_or(before, |offset| (now + offset).max(before));
            let ceiling = start_offset.map_or(after, |offset| (now + offset).max(after));
            assert!(
                (whole_hour_at_or_after(floor)..=whole_hour_at_or_after(ceiling)).contains(&start),
                "case {case:?}: expected a window opening on the first whole hour at or \
                 after {floor}, got {start}"
            );
            assert_eq!(
                end - start,
                i64::from(duration_hours) * SECONDS_PER_HOUR,
                "case {case:?}: expected a window of {duration_hours} hours, got {start}..{end}"
            );
            if let Some(offset) = end_offset {
                assert!(
                    end <= now + offset,
                    "case {case:?}: expected the window to close by {}, got {end}",
                    now + offset
                );
            }
        }
    }
}

/// A search for EC2 UltraServers is answered with UltraServer offerings: the type and count
/// it named, priced on the UltraServers it reserves.
#[tokio::test]
async fn an_ultraserver_search_reports_ultraserver_offerings() {
    let (router, _engine) = super::ec2_query::emulator();

    // (case, UltraserverType, UltraserverCount, duration hours). The types are the EC2
    // UltraServer types AWS publishes for Capacity Blocks, which the model carries no enum
    // for; the counts include a search that names none.
    let cases: [(&str, &str, Option<i32>, i32); 3] = [
        ("one UltraServer for a day", "u-p6e-gb200x72", Some(1), 24),
        ("four UltraServers for a week", "u-trn2x16", Some(4), 168),
        ("no UltraServer count named", "u-p6e-gb200x72", None, 48),
    ];

    for (case, ultraserver_type, ultraserver_count, duration_hours) in cases {
        let mut params = vec![
            field("UltraserverType", ultraserver_type),
            field("CapacityDurationHours", duration_hours),
        ];
        if let Some(count) = ultraserver_count {
            params.push(field("UltraserverCount", count));
        }
        let reported = search(&router, "us-east-1", case, &params).await;

        let zones: Vec<String> = reported
            .iter()
            .map(|offering| offering.availability_zone.clone())
            .collect();
        assert_eq!(
            zones,
            zone_names("us-east-1"),
            "case {case:?}: expected one offering per Availability Zone, got {zones:?}"
        );
        let expected_count = ultraserver_count.unwrap_or(1);
        let expected_fee = capacity_block_upfront_fee(
            u64::try_from(expected_count).expect("a positive count"),
            u64::try_from(duration_hours).expect("a positive duration"),
        );
        for offering in &reported {
            assert_eq!(
                offering.ultraserver_type.as_deref(),
                Some(ultraserver_type),
                "case {case:?}: expected ultraserverType {ultraserver_type:?}, got {offering:?}"
            );
            assert_eq!(
                offering.ultraserver_count.as_deref(),
                Some(expected_count.to_string().as_str()),
                "case {case:?}: expected ultraserverCount {expected_count}, got {offering:?}"
            );
            assert_eq!(
                offering.upfront_fee.as_deref(),
                Some(expected_fee.as_str()),
                "case {case:?}: expected upfrontFee {expected_fee:?}, got {offering:?}"
            );
            // The search named no instance type, so the offering reserves UltraServers and
            // names no instance type of its own.
            assert_eq!(
                offering.instance_type, None,
                "case {case:?}: expected no instanceType, got {offering:?}"
            );
        }
    }

    // A value that is not shaped like an UltraServer type at all is refused rather than
    // echoed back on every offering and carried in each offering's id: free text, a type
    // name in the wrong case, one carrying the id payload's field separator, and one past
    // the published name lengths.
    for ultraserver_type in [
        "U-P6E-GB200X72",
        "u p6e",
        "u-p6e|gb200x72",
        "<script>",
        &"u".repeat(65),
    ] {
        let params = vec![
            field("UltraserverType", ultraserver_type),
            field("CapacityDurationHours", 24),
        ];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {ultraserver_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {ultraserver_type:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// A search that names no type at all is answered from the modelled instance-type catalog,
/// in the stable order a `NextToken` walk resumes in.
#[tokio::test]
async fn a_search_that_names_no_type_is_answered_from_the_modelled_catalog() {
    let (router, _engine) = super::ec2_query::emulator();
    let page_size = 6;
    let params = vec![
        field("CapacityDurationHours", 24),
        field("MaxResults", page_size),
    ];

    let reported = search(&router, "us-east-1", "no type named", &params).await;
    assert_eq!(
        reported.len(),
        page_size,
        "no type named: expected a full page of {page_size} offerings, got {}",
        reported.len()
    );
    for offering in &reported {
        let instance_type = offering.instance_type.clone().unwrap_or_default();
        assert!(
            offered_instance_types().contains(&instance_type.as_str()),
            "no type named: {instance_type:?} is not a modelled instance type, in {offering:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An offering is derived, not stored
// ---------------------------------------------------------------------------

/// The same search answers the same offerings, including their ids: an offering is derived
/// from its own terms, so nothing about it is minted per call.
#[tokio::test]
async fn the_same_search_answers_the_same_offerings() {
    let (router, _engine) = super::ec2_query::emulator();

    // Rows: an instance search, an UltraServer search, and a search bounded by a date range —
    // the three shapes of search this operation answers.
    let now = now_seconds();
    let cases: [(&str, Vec<(String, String)>); 3] = [
        (
            "an instance search",
            vec![
                field("InstanceType", "p5.48xlarge"),
                field("InstanceCount", 4),
                field("CapacityDurationHours", 48),
            ],
        ),
        (
            "an UltraServer search",
            vec![
                field("UltraserverType", "u-p6e-gb200x72"),
                field("UltraserverCount", 2),
                field("CapacityDurationHours", 24),
            ],
        ),
        (
            "a search bounded by a date range",
            vec![
                field("InstanceType", "trn2.48xlarge"),
                field("InstanceCount", 16),
                field("CapacityDurationHours", 168),
                field("StartDateRange", timestamp(now + 30 * 24 * SECONDS_PER_HOUR)),
                field("EndDateRange", timestamp(now + 90 * 24 * SECONDS_PER_HOUR)),
            ],
        ),
    ];

    for (case, params) in cases {
        let first = search(&router, "us-east-1", case, &params).await;
        let second = search(&router, "us-east-1", case, &params).await;
        assert_eq!(
            first, second,
            "case {case:?}: a read must answer the same thing twice, got {first:?} then \
             {second:?}"
        );
        let ids: BTreeSet<String> = first
            .iter()
            .map(|offering| offering.offering_id.clone())
            .collect();
        assert_eq!(
            ids.len(),
            first.len(),
            "case {case:?}: each offering carries its own id, got {ids:?}"
        );
    }
}

/// An offering's id carries the terms it was reported with — the zone, the counts and the
/// window — so a purchase can resolve it without the search having stored anything. An id
/// this service did not mint carries nothing.
#[tokio::test]
async fn an_offering_id_carries_the_terms_it_was_reported_with() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "eu-west-1";
    let params = vec![
        field("InstanceType", "p5e.48xlarge"),
        field("InstanceCount", 8),
        field("CapacityDurationHours", 72),
    ];
    let reported = search(&router, region, "an instance search", &params).await;
    assert!(
        !reported.is_empty(),
        "an instance search: expected offerings to read ids back out of"
    );

    let zones = availability_zones_in_region(region);
    for offering in &reported {
        let case = format!("the offering in {}", offering.availability_zone);
        let decoded = decode_capacity_block_offering_id(&offering.offering_id).unwrap_or_else(|| {
            panic!(
                "case {case:?}: {:?} does not decode to any terms",
                offering.offering_id
            )
        });
        let zone = zones
            .iter()
            .find(|zone| zone.zone_name == offering.availability_zone)
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {:?} is not a zone of {region}",
                    offering.availability_zone
                )
            });
        let (start, end) = offering.window(&case);
        let expected = CapacityBlockOfferingTerms {
            instance_type: Some("p5e.48xlarge".to_string()),
            instance_count: Some(8),
            ultraserver_type: None,
            ultraserver_count: None,
            availability_zone: zone.zone_name.clone(),
            availability_zone_id: zone.zone_id.clone(),
            start_date_secs: start,
            end_date_secs: end,
        };
        assert_eq!(
            decoded, expected,
            "case {case:?}: expected the id to carry {expected:?}, got {decoded:?}"
        );
    }

    // An id this service did not mint: another resource's id, an empty payload, text that is
    // not hex, a payload that is hex but not an offering, and a real id with one character of
    // its payload flipped — none of them decodes to terms.
    let minted = reported[0].offering_id.clone();
    let mut flipped: Vec<char> = minted.chars().collect();
    let last = flipped.len() - 1;
    flipped[last] = if flipped[last] == 'a' { 'b' } else { 'a' };
    let forged: Vec<String> = vec![
        "cb-00000000000000fff".to_string(),
        "cbo-".to_string(),
        "cbo-not-hex".to_string(),
        format!("cbo-{}", hex::encode(b"ec2-cbo-1:0:1|2|3|4|5|6|7|8")),
        flipped.into_iter().collect(),
    ];
    for id in forged {
        assert!(
            decode_capacity_block_offering_id(&id).is_none(),
            "case {id:?}: an id this service did not mint must decode to nothing"
        );
    }
}

// ---------------------------------------------------------------------------
// What a search may ask for
// ---------------------------------------------------------------------------

/// A duration is sold "in 1-day increments up 14 days, and in 7-day increments up to 182
/// days" (`DescribeCapacityBlockOfferingsRequest$CapacityDurationHours`): anything else is
/// refused, whatever else the search names.
#[tokio::test]
async fn a_duration_outside_the_documented_increments_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    // Each published increment, both published bounds, and one past each bound.
    let offered: [i32; 6] = [24, 48, 336, 504, 2_184, 4_368];
    for duration_hours in offered {
        let case = format!("{duration_hours} hours");
        let params = vec![
            field("InstanceType", "p5.48xlarge"),
            field("InstanceCount", 1),
            field("CapacityDurationHours", duration_hours),
        ];
        let reported = search(&router, "us-east-1", &case, &params).await;
        assert!(
            !reported.is_empty(),
            "case {case:?}: a duration inside the published increments is offered, got nothing"
        );
    }

    // Zero and a negative duration reserve nothing; 1 and 23 are inside a day; 25 and 47 are
    // not whole days; 360 (15 days) is a whole number of days past the daily bound; 4536
    // (27 weeks) is a whole number of weeks past the weekly bound; 169 is a day past a week.
    let refused: [i32; 9] = [0, -24, 1, 23, 25, 47, 169, 360, 4_536];
    for duration_hours in refused {
        let case = format!("{duration_hours} hours");
        let params = vec![
            field("InstanceType", "p5.48xlarge"),
            field("InstanceCount", 1),
            field("CapacityDurationHours", duration_hours),
        ];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

/// An instance type the model does not declare is not an instance type at all; one it does
/// declare is offered. A count that reserves nothing, or more instances than a Capacity Block
/// holds, is refused.
#[tokio::test]
async fn a_search_outside_the_published_catalogs_and_counts_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();

    // Catalog entries the issue does not cite, spanning accelerated, general purpose and
    // Graviton families, are offered; near-misses and free text are not instance types.
    for instance_type in ["p4d.24xlarge", "trn1.32xlarge", "m6g.medium", "x2iezn.metal"] {
        let params = vec![
            field("InstanceType", instance_type),
            field("CapacityDurationHours", 24),
        ];
        let reported = search(&router, "us-east-1", instance_type, &params).await;
        assert!(
            !reported.is_empty(),
            "case {instance_type:?}: a modelled instance type is offered, got nothing"
        );
    }
    for instance_type in ["p5.48xlarge-x", "P5.48XLARGE", "m5", "not-a-type"] {
        let params = vec![
            field("InstanceType", instance_type),
            field("CapacityDurationHours", 24),
        ];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {instance_type:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {instance_type:?}: expected InvalidParameterValue, got {body}"
        );
    }

    // "Each Capacity Block can have up to 64 instances": 64 is offered, 65 is not, and a
    // count of none or less than none reserves nothing. The same holds for UltraServers,
    // which the model publishes no maximum for.
    let counts: [(&str, (&str, i32), bool); 7] = [
        ("the published maximum", ("InstanceCount", 64), true),
        ("one instance", ("InstanceCount", 1), true),
        ("past the published maximum", ("InstanceCount", 65), false),
        ("far past it", ("InstanceCount", 1_000), false),
        ("no instances", ("InstanceCount", 0), false),
        ("fewer than none", ("InstanceCount", -1), false),
        ("no UltraServers", ("UltraserverCount", 0), false),
    ];
    for (case, (member, count), offered) in counts {
        let params = vec![
            field("InstanceType", "p5.48xlarge"),
            field(member, count),
            field("CapacityDurationHours", 24),
        ];
        let (status, body) = describe(&router, "us-east-1", &params).await;
        if offered {
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging, and DryRun
// ---------------------------------------------------------------------------

/// `MaxResults` and `NextToken` page one search's offerings: every page holds at most
/// `MaxResults` of them, the walk reports each exactly once, and the last page carries no
/// token. A token this service did not mint is refused.
#[tokio::test]
async fn max_results_and_next_token_page_one_search() {
    let (router, _engine) = super::ec2_query::emulator();
    let region = "us-east-1";

    // A search that covers the whole catalog in every zone, so there is more than one page to
    // walk whatever page size is asked for.
    let base = vec![field("CapacityDurationHours", 24), field("MaxResults", 100)];
    let full = search(&router, region, "one page of 100", &base).await;
    assert_eq!(
        full.len(),
        100,
        "one page of 100: expected a full page, got {}",
        full.len()
    );

    for page_size in [1usize, 7, 50] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        // Enough pages to cover the first `full` offerings at this page size, so the walk is
        // compared against the single-page answer above rather than against the whole catalog.
        let wanted = full.len();
        while seen.len() < wanted {
            let mut params = vec![
                field("CapacityDurationHours", 24),
                field("MaxResults", page_size),
            ];
            if let Some(token) = &token {
                params.push(field("NextToken", token));
            }
            let (status, body) = describe(&router, region, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 on page {}, got {status} with body {body}",
                pages + 1
            );
            let page = offerings(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} offerings, got {}",
                page.len()
            );
            seen.extend(page.iter().map(|offering| offering.offering_id.clone()));
            pages += 1;
            token = element(&body, "nextToken");
            assert!(
                token.is_some(),
                "case {case:?}: more offerings follow page {pages}, so it must carry a token"
            );
            assert!(
                pages <= 1_000,
                "case {case:?}: the walk must terminate, got {pages} pages"
            );
        }
        let expected: Vec<String> = full
            .iter()
            .map(|offering| offering.offering_id.clone())
            .collect();
        assert_eq!(
            &seen[..wanted],
            &expected[..],
            "case {case:?}: the walk must report the same offerings in the same order"
        );
    }

    // The last page of a search that fits in one page carries no token at all.
    let params = vec![
        field("InstanceType", "p5.48xlarge"),
        field("CapacityDurationHours", 24),
    ];
    let (status, body) = describe(&router, region, &params).await;
    assert_eq!(
        status, 200,
        "one zone set: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "nextToken"),
        None,
        "one zone set: a complete answer carries no token, got {body}"
    );

    // A token this service did not mint is refused rather than read as a cursor.
    for forged in ["abc", "0001", "not-a-token", ""] {
        let params = vec![
            field("InstanceType", "p5.48xlarge"),
            field("CapacityDurationHours", 24),
            field("NextToken", forged),
        ];
        let (status, body) = describe(&router, region, &params).await;
        assert_eq!(
            status, 400,
            "case {forged:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {forged:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // A token is bound to the search that minted it: presented back against a DIFFERENT
    // search it is refused, rather than resuming at a position inside a set it never
    // described. The rows change one search member each.
    let minting = vec![
        field("CapacityDurationHours", 24),
        field("InstanceCount", 4),
        field("MaxResults", 2),
    ];
    let (status, body) = describe(&router, region, &minting).await;
    assert_eq!(
        status, 200,
        "minting a token: expected HTTP 200, got {status} with body {body}"
    );
    let token = element(&body, "nextToken").expect("minting a token: a page must follow");
    let others: [(&str, Vec<(String, String)>); 4] = [
        (
            "another duration",
            vec![field("CapacityDurationHours", 48), field("InstanceCount", 4)],
        ),
        (
            "another instance count",
            vec![field("CapacityDurationHours", 24), field("InstanceCount", 8)],
        ),
        (
            "an instance type the first search did not name",
            vec![
                field("CapacityDurationHours", 24),
                field("InstanceCount", 4),
                field("InstanceType", "p5.48xlarge"),
            ],
        ),
        (
            "a start date bound",
            vec![
                field("CapacityDurationHours", 24),
                field("InstanceCount", 4),
                field("StartDateRange", timestamp(now_seconds() + 48 * SECONDS_PER_HOUR)),
            ],
        ),
    ];
    for (case, mut params) in others {
        params.push(field("NextToken", &token));
        let (status, body) = describe(&router, region, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for a token from another search, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }

    // The same search accepts its own token, so the check above refuses the changed search
    // rather than every token.
    let mut resumed = minting.clone();
    resumed.push(field("NextToken", &token));
    let (status, body) = describe(&router, region, &resumed).await;
    assert_eq!(
        status, 200,
        "resuming the same search: expected HTTP 200, got {status} with body {body}"
    );
}

/// `DryRun` performs nothing: a search that would have succeeded answers 412
/// `DryRunOperation`, and one that would have been refused is refused with its own reason.
#[tokio::test]
async fn dry_run_performs_nothing() {
    let (router, _engine) = super::ec2_query::emulator();

    let params = vec![
        field("InstanceType", "p5.48xlarge"),
        field("InstanceCount", 2),
        field("CapacityDurationHours", 24),
        field("DryRun", "true"),
    ];
    let (status, body) = describe(&router, "us-east-1", &params).await;
    assert_eq!(
        status, 412,
        "a dry run that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run that would have succeeded: expected DryRunOperation, got {body}"
    );

    // A dry run asks whether the request WOULD have succeeded, so a request that would have
    // been refused is refused with that reason instead.
    let params = vec![
        field("InstanceType", "p5.48xlarge"),
        field("CapacityDurationHours", 7),
        field("DryRun", "true"),
    ];
    let (status, body) = describe(&router, "us-east-1", &params).await;
    assert_eq!(
        status, 400,
        "a dry run that would have been refused: expected HTTP 400, got {status} with body \
         {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a dry run that would have been refused: expected InvalidParameterValue, got {body}"
    );
}
