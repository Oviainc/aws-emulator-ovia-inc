// SPDX-License-Identifier: CC0-1.0

//! `DescribeAvailabilityZones` end-to-end: the zones a Region has, and the selectors
//! that narrow them.
//!
//! Every call goes over the wire the way a client makes it — an
//! `application/x-www-form-urlencoded` ec2Query POST dispatched through the same
//! `MultiServiceRouter` the emulator serves with, carrying the Region in the SigV4
//! `Credential=` scope of its `Authorization` header, which is where the service
//! reads the Region it is being asked about.
//!
//! The rules under test:
//!
//! 1. A Region reports its own Availability Zones: zone `n` is named
//!    `<region><letter>` from `a` and carries the zone id `<prefix>-az<n>`, where the
//!    prefix is the Region's leading component followed by each further component
//!    abbreviated to its direction initials. The spelling of both forms is the one the
//!    operation's `Filters` doc gives for its `zone-name` and `zone-id` filters
//!    (`smithy_query(query="doc",shape="DescribeAvailabilityZonesRequest",member="Filters")`:
//!    `us-east-1a`, `use1-az1`).
//! 2. `ZoneNames` / `ZoneIds` report the zones they name; a name or id that is not a
//!    zone of the Region is refused with `InvalidParameterValue`.
//! 3. `Filters` are ANDed with one another and ORed over each filter's values, with
//!    EC2's `*` and `?` wildcards and its backslash escape; a filter the operation
//!    does not document is refused with `InvalidParameterValue`.
//! 4. `DryRun` performs nothing and answers 412 `DryRunOperation`.
//! 5. The describe is a read: repeating it answers the same zones.
//!
//! Zone ORDER is deliberately not asserted — "the order of the elements in the
//! response ... might vary" — so every comparison here is over a sorted set.

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, filters, indexed, owned, unescaped};

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. This
/// operation reports facts of a Region and stores nothing, so the engine handle it hands
/// back is not needed here.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `DescribeAvailabilityZones` call signed for `region`, with `params` as extra
/// form fields. Answers the HTTP status and the response body.
async fn describe(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "DescribeAvailabilityZones", params).await
}

/// One zone of a `DescribeAvailabilityZones` response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Zone {
    name: String,
    id: String,
    region: String,
    group: String,
    border_group: String,
    zone_type: String,
    state: String,
    opt_in_status: String,
    /// Element names the response carried for this zone, for the absent-member checks.
    elements: Vec<String>,
}

/// Every zone in an ec2Query `DescribeAvailabilityZones` body, sorted. Each zone is
/// one `<item>` of the `<availabilityZoneInfo>` list — and the list's first element
/// carries an `xmlns` attribute, so the opening tag is read up to its `>`.
fn zones(case: &str, body: &str) -> Vec<Zone> {
    let list = element(body, "availabilityZoneInfo").unwrap_or_default();
    let mut zones: Vec<Zone> = list
        .split("<item")
        .skip(1)
        .map(|rest| {
            let after_tag = rest.split_once('>').map(|(_, rest)| rest).unwrap_or(rest);
            let item = after_tag.split("</item>").next().unwrap_or_default();
            let member = |name: &str| {
                element(item, name)
                    .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in zone {item:?}"))
            };
            Zone {
                name: member("zoneName"),
                id: member("zoneId"),
                region: member("regionName"),
                group: member("groupName"),
                border_group: member("networkBorderGroup"),
                zone_type: member("zoneType"),
                // `AvailabilityZone$State` is on the wire as `zoneState`.
                state: member("zoneState"),
                opt_in_status: member("optInStatus"),
                elements: item
                    .split('<')
                    .filter_map(|fragment| fragment.split('>').next())
                    .filter(|tag| !tag.is_empty() && !tag.starts_with('/'))
                    .map(ToString::to_string)
                    .collect(),
            }
        })
        .collect();
    zones.sort();
    zones
}

/// The zone names in a response, sorted.
fn zone_names(case: &str, body: &str) -> Vec<String> {
    zones(case, body).into_iter().map(|zone| zone.name).collect()
}

/// One row of a selector table: the case name, the `ZoneNames` and `ZoneIds` the
/// request carries, and the zone names expected back.
type SelectorCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a [&'a str]);

/// One row of a refusal table: the case name, the `ZoneNames` and `ZoneIds` the
/// request carries, and the value the refusal must name.
type RefusedSelectorCase<'a> = (&'a str, &'a [&'a str], &'a [&'a str], &'a str);

/// One row of a filter table: the case name, the `Filters` the request carries
/// (each a name and its values), and the zone names expected back.
type FilterCase<'a> = (&'a str, &'a [(&'a str, &'a [&'a str])], &'a [&'a str]);

// ---------------------------------------------------------------------------
// Rule 1: a Region reports its own zones
// ---------------------------------------------------------------------------

/// Each Region answers three Availability Zones of its own, named for the Region and
/// identified by the Region's zone-id prefix.
///
/// The rows differ in the SHAPE of the Region name the zones are derived from: a
/// plain direction, a compound direction (`northeast`), a `central` Region, and a
/// three-component Region (`us-gov-west-1`). The expected prefixes are the ones AWS
/// publishes for these Regions; `us-east-1` -> `use1` is the example in the
/// operation's own `zone-id` filter doc.
#[tokio::test]
async fn each_region_reports_its_own_availability_zones() {
    let router = emulator();
    let cases: [(&str, &str); 8] = [
        ("us-east-1", "use1"),
        ("us-west-2", "usw2"),
        ("eu-west-1", "euw1"),
        ("eu-central-1", "euc1"),
        ("ap-northeast-1", "apne1"),
        ("ap-southeast-2", "apse2"),
        ("us-gov-west-1", "usgw1"),
        ("cn-northwest-1", "cnnw1"),
    ];

    for (region, prefix) in cases {
        let (status, body) = describe(&router, region, &[]).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected 200, got {status} {body}"
        );
        let zones = zones(region, &body);
        assert_eq!(
            zones.len(),
            3,
            "case {region:?}: expected three zones, got {zones:?}"
        );
        for (index, zone) in zones.iter().enumerate() {
            let letter = char::from(b'a' + index as u8);
            let expected_name = format!("{region}{letter}");
            let expected_id = format!("{prefix}-az{}", index + 1);
            assert_eq!(
                zone.name, expected_name,
                "case {region:?}: expected zone {expected_name}, got {zone:?}"
            );
            assert_eq!(
                zone.id, expected_id,
                "case {region:?}: expected zone id {expected_id}, got {zone:?}"
            );
            assert_eq!(
                zone.region, region,
                "case {region:?}: a zone reports its own Region, got {zone:?}"
            );
            assert_eq!(
                zone.group,
                format!("{region}-zg-1"),
                "case {region:?}: expected the Region's zone group, got {zone:?}"
            );
            assert_eq!(
                zone.border_group, region,
                "case {region:?}: an Availability Zone's network border group is its Region, got {zone:?}"
            );
            assert_eq!(
                zone.zone_type, "availability-zone",
                "case {region:?}: expected an availability-zone, got {zone:?}"
            );
            assert_eq!(
                zone.state, "available",
                "case {region:?}: expected an available zone, got {zone:?}"
            );
            assert_eq!(
                zone.opt_in_status, "opt-in-not-required",
                "case {region:?}: an Availability Zone is always opt-in-not-required, got {zone:?}"
            );
            // Only a Local Zone or a Wavelength Zone has a parent zone, so these two
            // members must be absent rather than empty or invented.
            for absent in ["parentZoneName", "parentZoneId", "groupLongName"] {
                assert!(
                    !zone.elements.iter().any(|tag| tag == absent),
                    "case {region:?}: an Availability Zone carries no <{absent}>, got {zone:?}"
                );
            }
        }
    }
}

/// `AllAvailabilityZones` cannot withhold a zone whose opt-in status is
/// `opt-in-not-required`, so it reports the same zones either way.
#[tokio::test]
async fn all_availability_zones_reports_the_same_opt_in_not_required_zones() {
    let router = emulator();
    for case in ["true", "false"] {
        let (status, body) = describe(
            &router,
            "eu-west-1",
            &[("AllAvailabilityZones".to_string(), case.to_string())],
        )
        .await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            zone_names(case, &body),
            owned(&["eu-west-1a", "eu-west-1b", "eu-west-1c"]),
            "case {case:?}: expected every zone of the Region, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: ZoneNames / ZoneIds
// ---------------------------------------------------------------------------

/// A zone named by `ZoneNames` or identified by `ZoneIds` is reported; naming none
/// reports them all.
///
/// The rows differ in which selector names the zones, how many it names, and whether
/// both selectors are used at once.
#[tokio::test]
async fn zone_names_and_zone_ids_select_the_zones_they_name() {
    let router = emulator();
    let cases: [SelectorCase; 6] = [
        ("neither selector", &[], &[], &["us-east-1a", "us-east-1b", "us-east-1c"]),
        ("one name", &["us-east-1b"], &[], &["us-east-1b"]),
        (
            "two names",
            &["us-east-1c", "us-east-1a"],
            &[],
            &["us-east-1a", "us-east-1c"],
        ),
        ("one id", &[], &["use1-az2"], &["us-east-1b"]),
        (
            "two ids",
            &[],
            &["use1-az3", "use1-az1"],
            &["us-east-1a", "us-east-1c"],
        ),
        (
            "a name and an id",
            &["us-east-1a"],
            &["use1-az3"],
            &["us-east-1a", "us-east-1c"],
        ),
    ];

    for (case, names, ids, expected) in cases {
        let mut params = indexed("ZoneName", names);
        params.extend(indexed("ZoneId", ids));
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            zone_names(case, &body),
            owned(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// A `ZoneNames` or `ZoneIds` entry that is not a zone of the Region is refused with
/// `InvalidParameterValue`, naming the value at fault — not answered with an empty
/// list.
///
/// The rows differ in which selector carries the bad value and in how it is wrong: a
/// zone letter the Region does not have, a zone of a DIFFERENT Region, a zone-id
/// number the Region does not have, and a zone id in the wrong Region's prefix.
#[tokio::test]
async fn an_unknown_zone_selector_is_refused() {
    let router = emulator();
    let cases: [RefusedSelectorCase; 5] = [
        ("unknown letter", &["us-east-1z"], &[], "us-east-1z"),
        ("another Region's zone", &["us-west-2a"], &[], "us-west-2a"),
        (
            "a known zone beside an unknown one",
            &["us-east-1a", "us-east-1q"],
            &[],
            "us-east-1q",
        ),
        ("unknown zone-id number", &[], &["use1-az9"], "use1-az9"),
        ("another Region's zone id", &[], &["usw2-az1"], "usw2-az1"),
    ];

    for (case, names, ids, offending) in cases {
        let mut params = indexed("ZoneName", names);
        params.extend(indexed("ZoneId", ids));
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(offending),
            "case {case:?}: the message must name {offending:?}, got {message:?}"
        );
        assert!(
            element(&body, "availabilityZoneInfo").is_none(),
            "case {case:?}: a refused request reports no zones, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: Filters
// ---------------------------------------------------------------------------

/// Filters are ANDed with one another, each filter's values are ORed, and a value may
/// carry EC2's `*` / `?` wildcards or escape them with a backslash.
///
/// The rows walk every filter the operation documents, each filter's value forms
/// (exact, multi-value, wildcarded, escaped), a filter naming a member an
/// Availability Zone does not carry, and two filters at once — matching and
/// contradictory.
#[tokio::test]
async fn filters_are_anded_and_their_values_ored() {
    let router = emulator();
    let all = &["us-west-2a", "us-west-2b", "us-west-2c"][..];
    let cases: [FilterCase; 19] = [
        ("zone-name exact", &[("zone-name", &["us-west-2b"])], &["us-west-2b"]),
        (
            "zone-name two values are ORed",
            &[("zone-name", &["us-west-2a", "us-west-2c"])],
            &["us-west-2a", "us-west-2c"],
        ),
        ("zone-name trailing *", &[("zone-name", &["us-west-2*"])], all),
        ("zone-name leading *", &[("zone-name", &["*2c"])], &["us-west-2c"]),
        ("zone-name ? is one character", &[("zone-name", &["us-west-2?"])], all),
        (
            "zone-name ? needs a character",
            &[("zone-name", &["us-west-2a?"])],
            &[],
        ),
        (
            "an escaped * is a literal *",
            &[("zone-name", &["us-west-2\\*"])],
            &[],
        ),
        ("zone-name no match", &[("zone-name", &["eu-west-1a"])], &[]),
        ("zone-id exact", &[("zone-id", &["usw2-az1"])], &["us-west-2a"]),
        ("zone-id wildcard", &[("zone-id", &["usw2-az*"])], all),
        ("state available", &[("state", &["available"])], all),
        ("state unavailable", &[("state", &["unavailable"])], &[]),
        ("zone-type", &[("zone-type", &["availability-zone"])], all),
        ("zone-type local-zone", &[("zone-type", &["local-zone"])], &[]),
        ("region-name", &[("region-name", &["us-west-2"])], all),
        ("another region-name", &[("region-name", &["us-east-1"])], &[]),
        (
            "opt-in-status",
            &[("opt-in-status", &["opt-in-not-required"])],
            all,
        ),
        (
            "two filters are ANDed",
            &[
                ("zone-type", &["availability-zone"]),
                ("zone-name", &["us-west-2c"]),
            ],
            &["us-west-2c"],
        ),
        (
            "contradictory filters match nothing",
            &[("zone-name", &["us-west-2a"]), ("zone-id", &["usw2-az2"])],
            &[],
        ),
    ];

    for (case, entries, expected) in cases {
        let (status, body) = describe(&router, "us-west-2", &filters(entries)).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {status} {body}");
        assert_eq!(
            zone_names(case, &body),
            owned(expected),
            "case {case:?}: expected {expected:?}, got {body}"
        );
    }
}

/// A filter naming a member an Availability Zone does not carry compares against no
/// values, so it selects nothing — while the same filter name is still a VALID
/// filter, so the request is answered rather than refused.
#[tokio::test]
async fn a_filter_on_a_member_an_availability_zone_lacks_matches_nothing() {
    let router = emulator();
    let cases: [(&str, &[&str]); 5] = [
        ("parent-zone-id", &["usw2-az1"]),
        ("parent-zone-name", &["us-west-2a"]),
        ("group-long-name", &["US West (Oregon) 1"]),
        ("message", &["*"]),
        ("message", &["anything"]),
    ];

    for (name, values) in cases {
        let (status, body) = describe(&router, "us-west-2", &filters(&[(name, values)])).await;
        assert_eq!(status, 200, "case {name:?}: expected 200, got {status} {body}");
        assert_eq!(
            zone_names(name, &body),
            Vec::<String>::new(),
            "case {name:?}: an Availability Zone has no {name}, so nothing matches: {body}"
        );
    }

    // …and the zone group's short name, which a zone DOES carry, still matches.
    let (status, body) = describe(
        &router,
        "us-west-2",
        &filters(&[("group-name", &["us-west-2-zg-1"])]),
    )
    .await;
    assert_eq!(status, 200, "case \"group-name\": expected 200, got {status} {body}");
    assert_eq!(
        zone_names("group-name", &body),
        owned(&["us-west-2a", "us-west-2b", "us-west-2c"]),
        "case \"group-name\": expected the Region's zone group to match every zone, got {body}"
    );
}

/// Every filter the operation's `Filters` doc lists is accepted; the refusal below is
/// only for a name outside that list.
///
/// The list is the one in
/// `smithy_query(query="doc",shape="DescribeAvailabilityZonesRequest",member="Filters")`,
/// written out here so the handler's own list cannot drift from the model's doc.
#[tokio::test]
async fn every_documented_filter_name_is_accepted() {
    let router = emulator();
    let documented = [
        "group-long-name",
        "group-name",
        "message",
        "opt-in-status",
        "parent-zone-id",
        "parent-zone-name",
        "region-name",
        "state",
        "zone-id",
        "zone-name",
        "zone-type",
    ];

    for name in documented {
        let (status, body) = describe(
            &router,
            "us-east-1",
            &filters(&[(name, &["us-east-1a", "available", "us-east-1"])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: a documented filter must be accepted, got {status} {body}"
        );
    }
}

/// A filter name this operation does not document is refused with
/// `InvalidParameterValue` naming it, rather than silently ignored.
///
/// The rows differ in how the name is wrong: a near-miss of a real filter, a
/// member-cased name, a name from another describe, and no name at all.
#[tokio::test]
async fn an_unknown_filter_name_is_refused() {
    let router = emulator();
    let cases: [(&str, &str); 4] = [
        ("plural near-miss", "zone-names"),
        ("member casing", "ZoneName"),
        ("another describe's filter", "instance-type"),
        ("empty name", ""),
    ];

    for (case, name) in cases {
        let (status, body) = describe(
            &router,
            "us-east-1",
            &filters(&[(name, &["us-east-1a"])]),
        )
        .await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {status} {body}");
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("InvalidParameterValue"),
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(&format!("'{name}'")),
            "case {case:?}: the message must name the filter {name:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: DryRun
// ---------------------------------------------------------------------------

/// A `DryRun` request performs nothing and is answered 412 `DryRunOperation`, and the
/// same call without it still answers the Region's zones.
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let router = emulator();
    for case in ["DryRun alone", "DryRun beside a filter"] {
        let mut params = vec![("DryRun".to_string(), "true".to_string())];
        if case.contains("filter") {
            params.extend(filters(&[("zone-name", &["us-east-1a"])]));
        }
        let (status, body) = describe(&router, "us-east-1", &params).await;
        assert_eq!(status, 412, "case {case:?}: expected 412, got {status} {body}");
        assert_eq!(
            element(&body, "Code").as_deref(),
            Some("DryRunOperation"),
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "availabilityZoneInfo").is_none(),
            "case {case:?}: a dry run reports no zones, got {body}"
        );
    }

    // `DryRun=false` is not a dry run: the request is performed.
    let (status, body) = describe(
        &router,
        "us-east-1",
        &[("DryRun".to_string(), "false".to_string())],
    )
    .await;
    assert_eq!(status, 200, "case \"DryRun=false\": expected 200, got {status} {body}");
    assert_eq!(
        zone_names("DryRun=false", &body),
        owned(&["us-east-1a", "us-east-1b", "us-east-1c"]),
        "case \"DryRun=false\": expected every zone of the Region, got {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: the describe is a read
// ---------------------------------------------------------------------------

/// Describing zones changes nothing a later caller sees: repeated calls — including
/// ones that select, filter or dry-run in between — answer the same zones.
#[tokio::test]
async fn describing_zones_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let expected = owned(&["us-east-1a", "us-east-1b", "us-east-1c"]);

    let (_, first) = describe(&router, "us-east-1", &[]).await;
    assert_eq!(
        zone_names("first", &first),
        expected,
        "case \"first\": expected every zone of the Region, got {first}"
    );

    // A selective read, a filtered read, a refused read and a dry run, all between
    // the two full reads.
    let _ = describe(&router, "us-east-1", &indexed("ZoneName", &["us-east-1a"])).await;
    let _ = describe(&router, "us-east-1", &filters(&[("state", &["available"])])).await;
    let _ = describe(&router, "us-east-1", &indexed("ZoneName", &["us-east-1z"])).await;
    let _ = describe(
        &router,
        "us-east-1",
        &[("DryRun".to_string(), "true".to_string())],
    )
    .await;
    // …and a read in another Region, which must not leak into this one.
    let _ = describe(&router, "eu-west-1", &[]).await;

    let (status, second) = describe(&router, "us-east-1", &[]).await;
    assert_eq!(status, 200, "case \"second\": expected 200, got {status} {second}");
    assert_eq!(
        zones("second", &second),
        zones("first", &first),
        "case \"second\": a read must not change what a later caller sees"
    );
}
