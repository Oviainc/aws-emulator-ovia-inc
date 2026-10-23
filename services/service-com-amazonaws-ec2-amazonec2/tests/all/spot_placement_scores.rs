// SPDX-License-Identifier: CC0-1.0

//! `GetSpotPlacementScores` end-to-end: which locations a request is scored over, what
//! the score depends on, and how a caller narrows and pages the scored locations.
//!
//! Every call goes over the wire the service actually speaks, through the shared
//! `super::ec2_query` harness: an `application/x-www-form-urlencoded` ec2Query POST
//! dispatched through the same `MultiServiceRouter` the emulator serves with.
//!
//! The rules under test:
//!
//! 1. Every scored location carries a score on the published 1..10 scale, and the score
//!    falls as the target capacity placed on that location grows — a request is likelier
//!    to be fulfilled the smaller it is. `TargetCapacity` is held to its modelled
//!    1..2000000000 range, and the unit it is counted in (`TargetCapacityUnitType`) does
//!    not change the score, because this emulator holds no per-instance-type vCPU or
//!    memory data to convert a unit with.
//! 2. `RegionNames` narrows the Regions scored — a Region named twice is scored once, and
//!    a name that is no Region of the catalog (or of the caller's own endpoint) is refused
//!    — and a request that narrows nothing scores at most the top ten Regions of the
//!    emulator's Region catalog, always including the Region the call was signed for.
//! 3. `SingleAvailabilityZone` scores each Availability Zone of those Regions instead of
//!    the Regions themselves, and a zone never scores above its own Region: a Region's
//!    score assumes the fleet uses all of its zones, while one zone would carry the whole
//!    target capacity alone. `IncludeLocalZones` changes nothing, because every zone this
//!    emulator has is an `availability-zone`.
//! 4. A request naming one or two DISTINCT instance types always scores low, while one
//!    naming at least three — or leaving Amazon EC2 to choose through
//!    `InstanceRequirementsWithMetadata` — is not held down. The two cannot be combined,
//!    and an instance type outside the modelled catalog is refused.
//! 5. `MaxResults` and `NextToken` page the scored locations: each page holds at most
//!    `MaxResults` items, the walk reports every scored location exactly once, and the last
//!    page carries no token. A token this service did not mint is refused with
//!    `InvalidPaginationToken`.
//! 6. `DryRun` performs nothing and answers 412 `DryRunOperation` for a request that would
//!    otherwise have succeeded, while a request EC2 refuses anyway carries that refusal.
//! 7. Scoring is a read: repeating a call answers exactly the same scores.
//!
//! Item ORDER within a response is deliberately not asserted — EC2 says the order of
//! elements in a response might vary — so every comparison here is over a sorted set.
//! What the pagination test does rely on is that the order is STABLE across calls, which
//! is what makes a cursor token see each location exactly once.

use std::collections::BTreeSet;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, error_code, indexed, unescaped};

/// Three instance types of the modelled `InstanceType` catalog — at least the
/// `RECOMMENDED_INSTANCE_TYPE_COUNT` a diversified request names, so a row that is about
/// something else is not also held down by the undiversified ceiling.
const DIVERSIFIED: [&str; 3] = ["c5.large", "m5.large", "m5.xlarge"];

/// One `GetSpotPlacementScores` call signed for `region` and carrying `params`, as its
/// HTTP status and XML body.
async fn get(
    router: &MultiServiceRouter,
    region: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, region, "GetSpotPlacementScores", params).await
}

/// The form fields of a request for `target_capacity` over `instance_types`, which is the
/// shape every row below starts from.
fn scoring_request(target_capacity: &str, instance_types: &[&str]) -> Vec<(String, String)> {
    let mut params = vec![(
        "TargetCapacity".to_string(),
        target_capacity.to_string(),
    )];
    params.extend(indexed("InstanceType", instance_types));
    params
}

/// One row of a table whose cases differ in the form fields they add to a request: the
/// case name, those fields, and what the row expects (a flag, or an error code).
type FieldCase<T> = (&'static str, Vec<(String, String)>, T);

/// One scored location of a `GetSpotPlacementScores` response.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Scored {
    region: String,
    /// The zone id, present only when the request scored single Availability Zones.
    availability_zone_id: Option<String>,
    score: i32,
}

/// Every scored location in an ec2Query `GetSpotPlacementScores` body, in the order the
/// response carried them. Each one is an `<item>` of the `<spotPlacementScoreSet>` list —
/// and the list's first element carries an `xmlns` attribute, so the opening tag is read
/// up to its `>`.
fn scores(case: &str, body: &str) -> Vec<Scored> {
    let list = element(body, "spotPlacementScoreSet").unwrap_or_default();
    list.split("<item")
        .skip(1)
        .map(|rest| {
            let after_tag = rest.split_once('>').map(|(_, rest)| rest).unwrap_or(rest);
            let item = after_tag.split("</item>").next().unwrap_or_default();
            let member = |name: &str| {
                element(item, name).unwrap_or_else(|| {
                    panic!("case {case:?}: no <{name}> in scored location {item:?}")
                })
            };
            let score = member("score");
            Scored {
                region: member("region"),
                availability_zone_id: element(item, "availabilityZoneId"),
                score: score.parse().unwrap_or_else(|_| {
                    panic!("case {case:?}: <score> is not an integer: {score:?}")
                }),
            }
        })
        .collect()
}

/// The score every location of a response carries, asserting that there is at least one
/// and that they are all on the published 1..10 scale and all equal — this emulator holds
/// no per-location capacity data, so a request scores the same everywhere and the score is
/// one number per request.
fn one_score(case: &str, reported: &[Scored]) -> i32 {
    assert!(
        !reported.is_empty(),
        "case {case:?}: expected at least one scored location, got none"
    );
    for entry in reported {
        assert!(
            (ec2::state::MIN_PLACEMENT_SCORE..=ec2::state::MAX_PLACEMENT_SCORE)
                .contains(&entry.score),
            "case {case:?}: a score is on the 1..10 scale, got {entry:?}"
        );
        assert_eq!(
            entry.score, reported[0].score,
            "case {case:?}: every location of one request scores the same, got {entry:?} \
             beside {:?}",
            reported[0]
        );
    }
    reported[0].score
}

/// The Regions of a response, deduplicated.
fn regions_of(reported: &[Scored]) -> BTreeSet<String> {
    reported.iter().map(|entry| entry.region.clone()).collect()
}

// ---------------------------------------------------------------------------
// Rule 1: the score, the capacity it answers and the unit it is counted in
// ---------------------------------------------------------------------------

/// A score sits on the published 1..10 scale and never rises as the target capacity grows:
/// the floor of the modelled range is "highly likely to succeed" (10) and its ceiling is
/// "not likely to succeed" (1).
///
/// The rows are target capacities spanning the modelled
/// `SpotPlacementScoresTargetCapacity` range 1..2000000000
/// (`smithy_query(query="shape",shape="SpotPlacementScoresTargetCapacity")`): both
/// published bounds and a value in each order of magnitude between them.
#[tokio::test]
async fn a_score_never_rises_as_the_target_capacity_grows() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, TargetCapacity, the score the published scale pins for this row)
    let cases: [(&str, &str, Option<i32>); 8] = [
        ("the published minimum", "1", Some(ec2::state::MAX_PLACEMENT_SCORE)),
        ("a single-digit capacity", "9", None),
        ("tens", "10", None),
        ("hundreds", "500", None),
        ("thousands", "2000", None),
        ("hundreds of thousands", "123456", None),
        ("hundreds of millions", "900000000", None),
        ("the published maximum", "2000000000", Some(ec2::state::MIN_PLACEMENT_SCORE)),
    ];

    let mut previous: Option<(&str, i32)> = None;
    for (case, target_capacity, pinned) in cases {
        let params = scoring_request(target_capacity, &DIVERSIFIED);
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for TargetCapacity={target_capacity}, got {status} {body}"
        );
        let reported = scores(case, &body);
        let score = one_score(case, &reported);
        if let Some(pinned) = pinned {
            assert_eq!(
                score, pinned,
                "case {case:?}: expected the scale's {pinned}, got {score}"
            );
        }
        if let Some((previous_case, previous_score)) = previous {
            assert!(
                score <= previous_score,
                "case {case:?}: a larger target capacity cannot score above \
                 {previous_case:?} ({previous_score}), got {score}"
            );
        }
        previous = Some((case, score));
    }
    // The two published bounds are pinned to the two ends of the scale above, so the walk
    // also proves the score actually MOVES across the range rather than being constant.
}

/// `TargetCapacity` is held to its modelled range: each published bound is accepted, a
/// value one past the ceiling is refused, and a request that omits the member is refused
/// rather than scored as zero capacity.
#[tokio::test]
async fn target_capacity_is_held_to_its_modelled_range() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, TargetCapacity as sent — None omits the member, is it inside the range)
    let cases: [(&str, Option<&str>, bool); 6] = [
        ("one below the minimum", Some("0"), false),
        ("the minimum", Some("1"), true),
        ("inside", Some("60"), true),
        ("the maximum", Some("2000000000"), true),
        ("one above the maximum", Some("2000000001"), false),
        ("the member omitted", None, false),
    ];

    for (case, target_capacity, inside) in cases {
        let mut params = indexed("InstanceType", &DIVERSIFIED);
        if let Some(target_capacity) = target_capacity {
            params.push(("TargetCapacity".to_string(), target_capacity.to_string()));
        }
        let (status, body) = get(&router, "us-east-1", &params).await;
        if inside {
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            one_score(case, &scores(case, &body));
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: TargetCapacity={target_capacity:?} is not scorable, got \
                 {status} {body}"
            );
            assert!(
                element(&body, "spotPlacementScoreSet").is_none(),
                "case {case:?}: a refused request scores nothing, got {body}"
            );
        }
    }
}

/// The unit the target capacity is counted in is held to the modelled
/// `TargetCapacityUnitType` enum (`memory-mib`, `units`, `vcpu`), and — because the
/// emulator holds no per-instance-type vCPU or memory data to convert a unit with — the
/// same capacity scores the same in every one of them.
#[tokio::test]
async fn the_target_capacity_unit_type_is_held_to_its_modelled_enum() {
    let (router, _engine) = super::ec2_query::emulator();
    let unit_free = {
        let params = scoring_request("500", &DIVERSIFIED);
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(status, 200, "no unit: expected 200, got {status} {body}");
        one_score("no unit", &scores("no unit", &body))
    };

    // Every value of the modelled enum
    // (`smithy_query(query="shape",shape="TargetCapacityUnitType")`), then values outside
    // it: a near-miss case, another vocabulary's unit, and an empty one.
    let accepted = ec2::ops::types::TARGET_CAPACITY_UNIT_TYPE_VALUES;
    assert_eq!(
        accepted.len(),
        3,
        "the modelled TargetCapacityUnitType should carry three values, got {accepted:?}"
    );
    for unit_type in accepted {
        let mut params = scoring_request("500", &DIVERSIFIED);
        params.push((
            "TargetCapacityUnitType".to_string(),
            (*unit_type).to_string(),
        ));
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {unit_type:?}: expected 200, got {status} {body}"
        );
        let score = one_score(unit_type, &scores(unit_type, &body));
        assert_eq!(
            score, unit_free,
            "case {unit_type:?}: the unit does not change the score, expected {unit_free}, \
             got {score}"
        );
    }

    for (case, unit_type) in [
        ("member-cased", "Vcpu"),
        ("a near miss", "vcpus"),
        ("another vocabulary", "memory-gib"),
        ("empty", ""),
    ] {
        let mut params = scoring_request("500", &DIVERSIFIED);
        params.push((
            "TargetCapacityUnitType".to_string(),
            unit_type.to_string(),
        ));
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: TargetCapacityUnitType={unit_type:?} is outside the enum, got \
             {status} {body}"
        );
        assert!(
            element(&body, "spotPlacementScoreSet").is_none(),
            "case {case:?}: a refused request scores nothing, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: which Regions a request is scored over
// ---------------------------------------------------------------------------

/// `RegionNames` narrows the Regions scored to exactly the ones it names, and a Region
/// named twice is scored once.
///
/// The rows differ in how many Regions they name and in how: one, several out of order, a
/// repeat, the Region the call was signed for, and the modelled maximum of ten names. A
/// list longer than that maximum is refused rather than quietly narrowed.
#[tokio::test]
async fn region_names_narrow_which_regions_are_scored() {
    let (router, _engine) = super::ec2_query::emulator();
    let catalog = ec2::state::commercial_regions();
    let ten_names: Vec<&str> = catalog
        .iter()
        .copied()
        .rev()
        .take(ec2::state::MAX_SCORED_REGIONS)
        .collect();

    // (case, RegionNames, the Regions the response must score)
    let cases: [(&str, Vec<&str>, Vec<&str>); 5] = [
        ("one Region", vec!["eu-west-2"], vec!["eu-west-2"]),
        (
            "three Regions, out of catalog order",
            vec!["us-west-2", "ap-south-1", "eu-central-1"],
            vec!["ap-south-1", "eu-central-1", "us-west-2"],
        ),
        (
            "a Region named twice",
            vec!["sa-east-1", "sa-east-1"],
            vec!["sa-east-1"],
        ),
        (
            "the Region the call was signed for",
            vec!["us-east-1"],
            vec!["us-east-1"],
        ),
        (
            "the modelled maximum of ten Regions",
            ten_names.clone(),
            ten_names.clone(),
        ),
    ];

    for (case, region_names, expected) in cases {
        let mut params = scoring_request("100", &DIVERSIFIED);
        params.extend(indexed("RegionName", &region_names));
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for RegionNames={region_names:?}, got {status} {body}"
        );
        let reported = scores(case, &body);
        one_score(case, &reported);
        let expected: BTreeSet<String> =
            expected.iter().map(|region| (*region).to_string()).collect();
        assert_eq!(
            regions_of(&reported),
            expected,
            "case {case:?}: expected those Regions scored, got {reported:?}"
        );
        assert_eq!(
            reported.len(),
            expected.len(),
            "case {case:?}: a Region is scored exactly once, got {reported:?}"
        );
        for entry in &reported {
            assert!(
                entry.availability_zone_id.is_none(),
                "case {case:?}: a Region score names no Availability Zone, got {entry:?}"
            );
        }
    }

    // One past the modelled maximum: `RegionNames` is `len 0..10`
    // (`smithy_query(query="operation",operation="GetSpotPlacementScores")`), so an
    // eleventh name is refused at the boundary rather than dropped from the narrowing.
    let eleven: Vec<&str> = catalog.iter().copied().take(11).collect();
    let mut params = scoring_request("100", &DIVERSIFIED);
    params.extend(indexed("RegionName", &eleven));
    let (status, body) = get(&router, "us-east-1", &params).await;
    assert_eq!(
        status, 400,
        "case \"eleven Regions\": RegionNames holds at most ten names, got {status} {body}"
    );
    assert!(
        element(&body, "spotPlacementScoreSet").is_none(),
        "case \"eleven Regions\": a refused request scores nothing, got {body}"
    );
}

/// A request that narrows nothing is scored over the Region catalog, capped at the top
/// ten — and the Region the caller signed for is always one of them, because EC2 is
/// reached through a regional endpoint.
///
/// The rows are the Region the call is signed for: one the catalog lists early, one it
/// lists late (so the cap would drop it without the tie-break), and one of another
/// partition, which the commercial catalog does not list at all.
#[tokio::test]
async fn a_request_that_narrows_nothing_scores_the_catalog_and_the_signed_region() {
    let (router, _engine) = super::ec2_query::emulator();
    let catalog: BTreeSet<String> = ec2::state::commercial_regions()
        .into_iter()
        .map(str::to_string)
        .collect();
    assert!(
        catalog.len() > ec2::state::MAX_SCORED_REGIONS,
        "the catalog should hold more Regions than one response scores, got {}",
        catalog.len()
    );

    for case in ["ap-northeast-1", "us-west-2", "us-gov-west-1"] {
        let params = scoring_request("100", &DIVERSIFIED);
        let (status, body) = get(&router, case, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let reported = scores(case, &body);
        one_score(case, &reported);
        let scored = regions_of(&reported);
        assert_eq!(
            scored.len(),
            ec2::state::MAX_SCORED_REGIONS,
            "case {case:?}: expected the top {} Regions, got {reported:?}",
            ec2::state::MAX_SCORED_REGIONS
        );
        assert_eq!(
            reported.len(),
            scored.len(),
            "case {case:?}: a Region is scored exactly once, got {reported:?}"
        );
        assert!(
            scored.contains(case),
            "case {case:?}: the Region the call was signed for must be scored, got {scored:?}"
        );
        for region in &scored {
            assert!(
                catalog.contains(region) || region == case,
                "case {case:?}: only catalogued Regions are scored, got {region:?}"
            );
        }
    }
}

/// A `RegionNames` entry that names no Region this service can score is refused with
/// `InvalidParameterValue` naming it, rather than silently scoring nothing — or scoring a
/// Region that does not exist.
#[tokio::test]
async fn a_region_name_that_names_no_scoreable_region_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    // The rows differ in how the name is wrong: a zone rather than a Region, a zone id,
    // prose, an upper-cased code, a code missing its number, an empty name, a well-formed
    // code of no existing Region, and a Region of another partition.
    let cases: [(&str, &str); 9] = [
        ("an Availability Zone", "us-east-1a"),
        ("a zone id", "use1-az1"),
        ("prose", "US East (N. Virginia)"),
        ("upper-cased", "US-EAST-1"),
        ("no Region number", "us-east"),
        ("empty", ""),
        ("a Region code of no existing Region", "ap-southeast-9"),
        ("another Region code of no existing Region", "eu-west-4"),
        ("another partition's Region", "cn-north-1"),
    ];

    for (case, name) in cases {
        let mut params = scoring_request("100", &DIVERSIFIED);
        params.extend(indexed("RegionName", &[name]));
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for RegionNames=[{name:?}], got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(name),
            "case {case:?}: the refusal must name the value, got {message:?}"
        );
        assert!(
            element(&body, "spotPlacementScoreSet").is_none(),
            "case {case:?}: a refused request scores nothing, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: single Availability Zones
// ---------------------------------------------------------------------------

/// `SingleAvailabilityZone` scores every Availability Zone of the narrowed Regions, each
/// by its zone id, and a zone never scores above its own Region.
///
/// The rows differ in the Regions narrowed to and in the target capacity, which is what
/// decides how far a zone's score sits below its Region's.
#[tokio::test]
async fn single_availability_zone_scores_each_zone_no_higher_than_its_region() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, RegionNames, TargetCapacity)
    let cases: [(&str, Vec<&str>, &str); 5] = [
        ("one Region, the smallest capacity", vec!["us-east-1"], "1"),
        ("one Region, tens of instances", vec!["eu-west-1"], "40"),
        (
            "two Regions, thousands of instances",
            vec!["ap-northeast-2", "sa-east-1"],
            "5000",
        ),
        (
            "three Regions, the published maximum capacity",
            vec!["us-west-1", "us-west-2", "ca-central-1"],
            "2000000000",
        ),
        ("no RegionNames: the whole catalog", Vec::new(), "250"),
    ];

    for (case, region_names, target_capacity) in cases {
        let mut regional = scoring_request(target_capacity, &DIVERSIFIED);
        regional.extend(indexed("RegionName", &region_names));
        let mut zonal = regional.clone();
        zonal.push(("SingleAvailabilityZone".to_string(), "true".to_string()));
        zonal.push(("MaxResults".to_string(), "1000".to_string()));

        let (status, body) = get(&router, "us-east-1", &regional).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for the regional call, got {status} {body}"
        );
        let regional_scores = scores(case, &body);
        let regional_score = one_score(case, &regional_scores);

        let (status, body) = get(&router, "us-east-1", &zonal).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 for the zonal call, got {status} {body}"
        );
        let zonal_scores = scores(case, &body);
        let zonal_score = one_score(case, &zonal_scores);

        assert!(
            zonal_score <= regional_score,
            "case {case:?}: a single zone cannot score above its Region ({regional_score}), \
             got {zonal_score}"
        );
        assert_eq!(
            regions_of(&zonal_scores),
            regions_of(&regional_scores),
            "case {case:?}: the zonal call scores the zones of the same Regions, got \
             {zonal_scores:?}"
        );

        // Every zone of each scored Region is scored, by the zone id that Region reports
        // (`DescribeAvailabilityZones` pins the ids themselves).
        let expected_zones: BTreeSet<String> = regions_of(&regional_scores)
            .iter()
            .flat_map(|region| ec2::state::availability_zones_in_region(region))
            .map(|zone| zone.zone_id)
            .collect();
        let reported_zones: BTreeSet<String> = zonal_scores
            .iter()
            .map(|entry| {
                entry.availability_zone_id.clone().unwrap_or_else(|| {
                    panic!("case {case:?}: a zone score must name its zone, got {entry:?}")
                })
            })
            .collect();
        assert_eq!(
            reported_zones, expected_zones,
            "case {case:?}: expected every zone of the scored Regions, got {zonal_scores:?}"
        );
        assert_eq!(
            zonal_scores.len(),
            expected_zones.len(),
            "case {case:?}: a zone is scored exactly once, got {zonal_scores:?}"
        );
    }
}

/// `IncludeLocalZones` changes nothing a request is scored over: every zone this emulator
/// has is an `availability-zone` (`DescribeAvailabilityZones` reports no other kind), so
/// there is no Local Zone to include or to ignore — and "Local Zone capacity counts toward
/// its parent Region" either way.
///
/// The rows are the regional and the zonal form of one request, each asked without the
/// flag, with it false and with it true.
#[tokio::test]
async fn include_local_zones_scores_the_same_locations() {
    let (router, _engine) = super::ec2_query::emulator();
    for (form, single_availability_zone) in [("by Region", false), ("by zone", true)] {
        let mut answers: Vec<Vec<Scored>> = Vec::new();
        for include in [None, Some("false"), Some("true")] {
            let case = format!("{form} with IncludeLocalZones={include:?}");
            let mut params = scoring_request("100", &DIVERSIFIED);
            params.extend(indexed("RegionName", &["us-east-1", "eu-west-1"]));
            if single_availability_zone {
                params.push(("SingleAvailabilityZone".to_string(), "true".to_string()));
            }
            if let Some(include) = include {
                params.push(("IncludeLocalZones".to_string(), include.to_string()));
            }
            let (status, body) = get(&router, "us-east-1", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200, got {status} {body}"
            );
            let mut reported = scores(&case, &body);
            one_score(&case, &reported);
            reported.sort();
            answers.push(reported);
        }
        assert_eq!(
            answers[1], answers[0],
            "case {form:?}: IncludeLocalZones=false scores exactly what omitting it does"
        );
        assert_eq!(
            answers[2], answers[0],
            "case {form:?}: this emulator has no Local Zone for IncludeLocalZones=true to add"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: how many instance types a request leaves EC2 to choose from
// ---------------------------------------------------------------------------

/// A request naming one or two DISTINCT instance types always scores low, while one naming
/// at least three — or none at all, leaving Amazon EC2 to choose — is scored on the
/// capacity alone.
///
/// Every row asks for the smallest capacity there is, so the undiversified ceiling is the
/// only thing that can hold a score down. The rows differ in how the compute requirements
/// are stated: a count of instance types from one to six, the same type twice, and
/// attribute-based selection instead of a list.
#[tokio::test]
async fn one_or_two_instance_types_always_score_low() {
    let (router, _engine) = super::ec2_query::emulator();
    let six = [
        "c5.large", "c5.xlarge", "m5.large", "m5.xlarge", "r5.large", "t3.micro",
    ];
    // (case, extra form fields beside TargetCapacity, is the request diversified)
    let cases: [FieldCase<bool>; 7] = [
        ("no compute requirements at all", Vec::new(), true),
        (
            "one instance type",
            indexed("InstanceType", &["m5.large"]),
            false,
        ),
        (
            "two instance types",
            indexed("InstanceType", &["m5.large", "c5.large"]),
            false,
        ),
        (
            "the same instance type twice",
            indexed("InstanceType", &["m5.large", "m5.large"]),
            false,
        ),
        (
            "three instance types",
            indexed("InstanceType", &DIVERSIFIED),
            true,
        ),
        ("six instance types", indexed("InstanceType", &six), true),
        (
            "instance requirements instead of a list",
            vec![
                (
                    "InstanceRequirementsWithMetadata.ArchitectureType.1".to_string(),
                    "x86_64".to_string(),
                ),
                (
                    "InstanceRequirementsWithMetadata.InstanceRequirements.VCpuCount.Min"
                        .to_string(),
                    "2".to_string(),
                ),
                (
                    "InstanceRequirementsWithMetadata.InstanceRequirements.MemoryMiB.Min"
                        .to_string(),
                    "4096".to_string(),
                ),
            ],
            true,
        ),
    ];

    for (case, extra, diversified) in cases {
        let mut params = vec![("TargetCapacity".to_string(), "1".to_string())];
        params.extend(extra);
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body}"
        );
        let score = one_score(case, &scores(case, &body));
        if diversified {
            assert_eq!(
                score,
                ec2::state::MAX_PLACEMENT_SCORE,
                "case {case:?}: the smallest capacity over three or more instance types is \
                 highly likely to succeed, got {score}"
            );
        } else {
            assert!(
                score <= ec2::state::UNDIVERSIFIED_PLACEMENT_SCORE,
                "case {case:?}: fewer than three instance types always scores low (at most \
                 {}), got {score}",
                ec2::state::UNDIVERSIFIED_PLACEMENT_SCORE
            );
        }
    }
}

/// `InstanceTypes` and `InstanceRequirementsWithMetadata` cannot be used together, and an
/// instance type outside the modelled catalog is refused.
#[tokio::test]
async fn compute_requirements_are_refused_when_they_cannot_be_honoured() {
    let (router, _engine) = super::ec2_query::emulator();
    let requirements = vec![
        (
            "InstanceRequirementsWithMetadata.InstanceRequirements.VCpuCount.Min".to_string(),
            "2".to_string(),
        ),
        (
            "InstanceRequirementsWithMetadata.InstanceRequirements.MemoryMiB.Min".to_string(),
            "4096".to_string(),
        ),
    ];
    // (case, extra form fields, the error code the refusal carries, text its message
    // must name — the parameters that clash, or the value that names no instance type)
    let mut both = indexed("InstanceType", &DIVERSIFIED);
    both.extend(requirements.clone());
    let cases: [FieldCase<(&'static str, &'static str)>; 5] = [
        (
            "both ways of stating them",
            both,
            ("InvalidParameterCombination", "InstanceRequirements"),
        ),
        (
            "an instance type that does not exist",
            indexed("InstanceType", &["m5.enormous"]),
            ("InvalidParameterValue", "m5.enormous"),
        ),
        (
            "a wildcard rather than a type",
            indexed("InstanceType", &["m5.*"]),
            ("InvalidParameterValue", "m5.*"),
        ),
        (
            "an instance family rather than a type",
            indexed("InstanceType", &["m5"]),
            ("InvalidParameterValue", "m5"),
        ),
        (
            "one unknown type beside known ones",
            indexed("InstanceType", &["m5.large", "c5.large", "z9.titanic"]),
            ("InvalidParameterValue", "z9.titanic"),
        ),
    ];

    for (case, extra, (expected_code, named)) in cases {
        let mut params = vec![("TargetCapacity".to_string(), "100".to_string())];
        params.extend(extra);
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert!(
            message.contains(named),
            "case {case:?}: the refusal must name {named:?}, got {message:?}"
        );
        assert!(
            element(&body, "spotPlacementScoreSet").is_none(),
            "case {case:?}: a refused request scores nothing, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5: MaxResults / NextToken
// ---------------------------------------------------------------------------

/// A `NextToken` walk reports every scored location exactly once, in pages of at most
/// `MaxResults`, and the last page carries no token.
///
/// The rows are page sizes spanning the modelled `SpotPlacementScoresMaxResults` range
/// 10..1000 (`smithy_query(query="shape",shape="SpotPlacementScoresMaxResults")`): its
/// lower bound, a size that does not divide the number of scored zones, and its upper
/// bound. The expectation is the un-paged answer to the same request, so paging is
/// asserted to change only the packaging.
#[tokio::test]
async fn a_next_token_walk_reports_every_scored_location_once() {
    let (router, _engine) = super::ec2_query::emulator();
    // Scoring single zones over the whole catalog is what makes a walk span several
    // pages: every Region scored carries its Availability Zones.
    let request = {
        let mut params = scoring_request("100", &DIVERSIFIED);
        params.push(("SingleAvailabilityZone".to_string(), "true".to_string()));
        params
    };

    let unpaged = {
        let mut params = request.clone();
        params.push(("MaxResults".to_string(), "1000".to_string()));
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(status, 200, "unpaged: expected 200, got {status} {body}");
        assert!(
            element(&body, "nextToken").is_none(),
            "unpaged: a page holding every location carries no token, got {body}"
        );
        let mut reported = scores("unpaged", &body);
        reported.sort();
        reported
    };
    assert!(
        unpaged.len() > 10,
        "the catalog's zones should not fit in one minimum-size page, got {} locations",
        unpaged.len()
    );

    for page_size in [10usize, 17, 1000] {
        let case = format!("MaxResults={page_size}");
        let mut seen: Vec<Scored> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut params = request.clone();
            params.push(("MaxResults".to_string(), page_size.to_string()));
            if let Some(token) = &token {
                params.push(("NextToken".to_string(), token.clone()));
            }
            let (status, body) = get(&router, "us-east-1", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on page {}, got {status} {body}",
                pages + 1
            );
            let page = scores(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most {page_size} locations, got {}",
                page.len()
            );
            seen.extend(page);
            pages += 1;
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
            assert!(
                pages < 100,
                "case {case:?}: the walk should end, got {pages} pages"
            );
        }
        seen.sort();
        assert_eq!(
            seen, unpaged,
            "case {case:?}: a paged walk reports exactly the un-paged locations"
        );
        let expected_pages = unpaged.len().div_ceil(page_size);
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages} pages for {} locations, got {pages}",
            unpaged.len()
        );
    }
}

/// A `NextToken` this service did not mint is refused with `InvalidPaginationToken`,
/// rather than read as a cursor.
#[tokio::test]
async fn a_token_this_service_did_not_mint_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let cases: [(&str, &str); 4] = [
        ("not hex", "zzzz"),
        ("hex that is not a token", "6162636465"),
        ("an unencoded cursor", "00 us-east-1 use1-az1"),
        ("empty", ""),
    ];

    for (case, token) in cases {
        let mut params = scoring_request("100", &DIVERSIFIED);
        params.push(("NextToken".to_string(), token.to_string()));
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400 for NextToken {token:?}, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
        assert!(
            element(&body, "spotPlacementScoreSet").is_none(),
            "case {case:?}: a refused request scores nothing, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rules 6 and 7: DryRun, and scoring as a read
// ---------------------------------------------------------------------------

/// A `DryRun` of a request that would have succeeded performs nothing and is answered 412
/// `DryRunOperation`; a `DryRun` of a request EC2 would refuse anyway is answered with
/// that refusal instead, because what a dry run reports is that the "request would have
/// succeeded".
#[tokio::test]
async fn a_dry_run_is_refused_with_dry_run_operation() {
    let (router, _engine) = super::ec2_query::emulator();
    for case in [
        "DryRun alone",
        "DryRun beside RegionNames",
        "DryRun beside SingleAvailabilityZone",
    ] {
        let mut params = scoring_request("100", &DIVERSIFIED);
        params.push(("DryRun".to_string(), "true".to_string()));
        if case.contains("beside RegionNames") {
            params.extend(indexed("RegionName", &["eu-west-3"]));
        }
        if case.contains("SingleAvailabilityZone") {
            params.push(("SingleAvailabilityZone".to_string(), "true".to_string()));
        }
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected 412, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "spotPlacementScoreSet").is_none(),
            "case {case:?}: a dry run scores nothing, got {body}"
        );
    }

    // The rows a dry run does NOT get to answer: each one is a request the handler
    // refuses on its own terms, so the refusal is reported rather than DryRunOperation.
    // Each row carries its own whole parameter list, so a row about `InstanceTypes` is not
    // also sent the diversified list.
    let refused: [(&str, Vec<(String, String)>); 3] = [
        ("a name that is no Region code", {
            let mut params = scoring_request("100", &DIVERSIFIED);
            params.extend(indexed("RegionName", &["US East (N. Virginia)"]));
            params
        }),
        ("an instance type that does not exist", {
            let mut params = vec![("TargetCapacity".to_string(), "100".to_string())];
            params.extend(indexed("InstanceType", &["m5.enormous"]));
            params
        }),
        ("a NextToken this service did not mint", {
            let mut params = scoring_request("100", &DIVERSIFIED);
            params.push(("NextToken".to_string(), "zzzz".to_string()));
            params
        }),
    ];
    for (case, extra) in refused {
        let mut params = extra;
        params.push(("DryRun".to_string(), "true".to_string()));
        let (status, body) = get(&router, "us-east-1", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a dry run of a request EC2 refuses carries that refusal, got \
             {status} {body}"
        );
        assert_ne!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: the refusal must not be DryRunOperation, got {body}"
        );
        assert!(
            element(&body, "spotPlacementScoreSet").is_none(),
            "case {case:?}: a refused request scores nothing, got {body}"
        );
    }
}

/// Scoring is a read: the same request answers the same scores however often it is made,
/// and a request for one Region does not change what a request for another is answered.
#[tokio::test]
async fn scoring_is_a_read() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, RegionNames, SingleAvailabilityZone)
    let cases: [(&str, Vec<&str>, bool); 3] = [
        ("the whole catalog, by Region", Vec::new(), false),
        ("two Regions, by zone", vec!["us-east-2", "eu-north-1"], true),
        ("one Region, by Region", vec!["ap-southeast-1"], false),
    ];

    for (case, region_names, single_availability_zone) in &cases {
        let mut params = scoring_request("100", &DIVERSIFIED);
        params.extend(indexed("RegionName", region_names));
        if *single_availability_zone {
            params.push(("SingleAvailabilityZone".to_string(), "true".to_string()));
        }

        let mut answers: Vec<Vec<Scored>> = Vec::new();
        for attempt in 1..=3 {
            // Between attempts, every other row's request is made too, so a read that
            // changed what a later caller sees would show up here.
            for (_, other_names, other_single) in &cases {
                let mut other = scoring_request("7", &DIVERSIFIED);
                other.extend(indexed("RegionName", other_names));
                if *other_single {
                    other.push(("SingleAvailabilityZone".to_string(), "true".to_string()));
                }
                let (status, body) = get(&router, "us-west-2", &other).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: an interleaved call must answer 200, got {status} {body}"
                );
            }
            let (status, body) = get(&router, "us-east-1", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 on attempt {attempt}, got {status} {body}"
            );
            let mut reported = scores(case, &body);
            reported.sort();
            answers.push(reported);
        }
        assert_eq!(
            answers[1], answers[0],
            "case {case:?}: a repeated read answers the same scores"
        );
        assert_eq!(
            answers[2], answers[0],
            "case {case:?}: a repeated read answers the same scores"
        );
    }
}
