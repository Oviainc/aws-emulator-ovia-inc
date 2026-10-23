// SPDX-License-Identifier: CC0-1.0

//! The account-independent static catalog the metadata reads serve, end to end
//! over the awsQuery wire: the Availability Zones of a Region, the platforms each
//! zone advertises, and the engine build each offered version reports.
//!
//! Every case drives a real HTTP request through the same `MultiServiceRouter`
//! the production router builds, reusing the harness the cluster-lifecycle suite
//! owns rather than keeping a second copy of it.
//!
//! The suite asserts RULES over tables of inputs rather than one recorded
//! request:
//!
//! * a Region reports the Availability Zones AWS publishes FOR THAT REGION, whose
//!   count differs by Region, named `<region>` plus consecutive letters from `a`;
//! * every `AvailabilityZone` the service emits carries `SupportedPlatforms`, the
//!   platforms a cluster placed in that zone can run on (`VPC`, EC2-Classic being
//!   retired) — never the name alone;
//! * a `ClusterVersion` reports the ENGINE BUILD the version runs as its
//!   `Description`, not a restatement of the version number.

use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Emulator};

/// The Availability Zones AWS publishes for a Region, as `<region>` plus these
/// letters.
///
/// Source: the AWS global-infrastructure reference "Regions, Availability Zones,
/// and Local Zones" (the same list `aws ec2 describe-availability-zones` reports
/// per Region): most Regions publish three zones, Oregon and the two
/// Asia-Pacific Regions below publish four, and Northern Virginia publishes six.
/// Written out here rather than read off the production helper, so a wrong count
/// fails this suite. The rows span the smallest published count, the largest,
/// and Regions this issue never cited.
const REGION_ZONES: [(&str, &[&str]); 6] = [
    ("us-west-2", &["a", "b", "c", "d"]),
    ("us-east-1", &["a", "b", "c", "d", "e", "f"]),
    ("ap-northeast-1", &["a", "b", "c", "d"]),
    ("ap-northeast-2", &["a", "b", "c", "d"]),
    ("eu-central-1", &["a", "b", "c"]),
    ("ap-southeast-1", &["a", "b", "c"]),
];

/// The platforms every Availability Zone advertises.
///
/// Source: `smithy_query(query="shape", shape="SupportedPlatform")` — "A list of
/// supported platforms for orderable clusters". EC2-Classic is retired, so a
/// zone a cluster is orderable in supports exactly the VPC platform.
const EXPECTED_PLATFORMS: [&str; 1] = ["VPC"];

/// Every `AvailabilityZone` an awsQuery body carries, in document order, as its
/// `Name` and the `SupportedPlatform` names it advertises.
///
/// `AvailabilityZoneList`'s member carries `xmlName: AvailabilityZone`, so one
/// element per zone; `Name` is serialized before `SupportedPlatforms` (the order
/// the structure declares its members), so the zone's own name is the first
/// `<Name>` inside the element and the rest belong to the platforms.
pub(crate) fn listed_zones(xml: &str) -> Vec<(String, Vec<String>)> {
    all_text(xml, "AvailabilityZone")
        .into_iter()
        .map(|zone| {
            let name = all_text(&zone, "Name").into_iter().next().unwrap_or_default();
            let platforms = all_text(&zone, "SupportedPlatform")
                .into_iter()
                .map(|platform| {
                    all_text(&platform, "Name")
                        .into_iter()
                        .next()
                        .unwrap_or_default()
                })
                .collect();
            (name, platforms)
        })
        .collect()
}

/// The zones one option in this body carries, asserting every option in it
/// carries the same list (the zones belong to the Region, not to the option).
fn zones_per_option(case: &str, xml: &str, options: usize) -> Vec<(String, Vec<String>)> {
    let zones = listed_zones(xml);
    assert!(
        options > 0 && zones.len().is_multiple_of(options),
        "case {case}: expected the {options} option(s) to carry whole zone lists, got {zones:?}"
    );
    let per_option = zones.len() / options;
    for (index, chunk) in zones.chunks(per_option).enumerate() {
        assert_eq!(
            chunk,
            &zones[..per_option],
            "case {case}: option {index} must carry the same zone list as the first, got {chunk:?}"
        );
    }
    zones[..per_option].to_vec()
}

/// How many `<OrderableClusterOption>` elements a body carries.
fn option_count(xml: &str) -> usize {
    all_text(xml, "NodeType").len()
}

#[tokio::test]
async fn each_region_reports_the_zones_it_publishes_with_their_platforms() {
    let emulator = Emulator::new();

    for (region, letters) in REGION_ZONES {
        // A node type the issue never cited, so the body stays small while the
        // rule under test (the zone list) is unchanged.
        let answer = emulator
            .call_in_region(
                region,
                "DescribeOrderableClusterOptions",
                &[("NodeType", "rg.4xlarge")],
            )
            .await;
        assert_eq!(
            answer.status, 200,
            "case zones in {region}: expected 200, got {} with body {}",
            answer.status, answer.body
        );

        let expected: Vec<String> = letters
            .iter()
            .map(|letter| format!("{region}{letter}"))
            .collect();
        assert_eq!(
            state::region_availability_zones(region),
            expected,
            "case zones in {region}: expected the published zone list {expected:?}, got {:?}",
            state::region_availability_zones(region)
        );

        let served = zones_per_option(
            &format!("zones in {region}"),
            &answer.body,
            option_count(&answer.body),
        );
        let names: Vec<String> = served.iter().map(|(name, _)| name.clone()).collect();
        assert_eq!(
            names, expected,
            "case zones in {region}: expected {expected:?} on the wire, got {names:?}"
        );
        for (name, platforms) in &served {
            assert_eq!(
                platforms.as_slice(),
                EXPECTED_PLATFORMS.as_slice(),
                "case zone {name}: expected the platforms {EXPECTED_PLATFORMS:?}, got {platforms:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_zone_the_whole_catalog_emits_advertises_its_platforms() {
    let emulator = Emulator::new();

    // The unfiltered catalog: every option, every zone of every option. The rule
    // holds for the whole catalog, not only for the filtered reads above.
    let answer = emulator.call("DescribeOrderableClusterOptions", &[]).await;
    assert_eq!(
        answer.status, 200,
        "case the whole catalog: expected 200, got {} with body {}",
        answer.status, answer.body
    );
    let options = option_count(&answer.body);
    assert!(
        options > 1,
        "case the whole catalog: expected more than one option, got {options}"
    );
    let zones = listed_zones(&answer.body);
    assert_eq!(
        zones.len(),
        options * state::region_availability_zones("us-west-2").len(),
        "case the whole catalog: expected every option to carry the Region's whole zone list, got {zones:?}"
    );
    for (name, platforms) in &zones {
        assert!(
            name.starts_with("us-west-2"),
            "case zone {name}: expected a zone of the signed Region, got {name:?}"
        );
        assert_eq!(
            platforms.as_slice(),
            EXPECTED_PLATFORMS.as_slice(),
            "case zone {name}: expected the platforms {EXPECTED_PLATFORMS:?}, got {platforms:?}"
        );
    }
}

#[tokio::test]
async fn an_engine_build_description_names_the_build_and_not_the_version() {
    // The rule as a function: a build's description is that build string behind
    // the engine's name, whatever the build is. Rows differ in the build's
    // version prefix, its build number and its length.
    for build in ["1.0.49795", "1.0.1", "2.1.100000", "10.25.3"] {
        assert_eq!(
            state::engine_build_description(build),
            format!("PADB {build}"),
            "case build {build}: expected the engine build behind `PADB `, got {:?}",
            state::engine_build_description(build)
        );
    }

    // Every version the service publishes carries the build it runs, and that
    // description is NOT derivable from the version number alone: it names a
    // build the version string does not contain.
    for (version, build) in state::PUBLISHED_ENGINE_BUILDS {
        let description = state::engine_build_description(build);
        assert!(
            build.starts_with(&format!("{version}.")),
            "case version {version}: expected a build of that version, got {build:?}"
        );
        assert_eq!(
            build.split('.').count(),
            3,
            "case version {version}: expected a <major>.<minor>.<build> engine build, got {build:?}"
        );
        assert!(
            build.split('.').all(|part| part.parse::<u64>().is_ok()),
            "case version {version}: expected a numeric engine build, got {build:?}"
        );
        assert_ne!(
            description,
            format!("Cluster version {version}"),
            "case version {version}: the description must name the engine build, not restate the version"
        );
        assert_ne!(
            description,
            state::engine_build_description(version),
            "case version {version}: the description must carry the build, not just the version, got {description:?}"
        );
    }
}

#[tokio::test]
async fn the_catalog_reports_each_version_with_its_engine_build() {
    let emulator = Emulator::new();

    let expected: Vec<String> = state::PUBLISHED_ENGINE_BUILDS
        .iter()
        .map(|(_, build)| state::engine_build_description(build))
        .collect();

    // The whole catalog, and then each version on its own: the description a
    // version reports is the same either way.
    let whole = emulator.call("DescribeClusterVersions", &[]).await;
    assert_eq!(
        whole.status, 200,
        "case the whole version catalog: expected 200, got {} with body {}",
        whole.status, whole.body
    );
    assert_eq!(
        all_text(&whole.body, "Description"),
        expected,
        "case the whole version catalog: expected {expected:?} on the wire, got {:?}",
        all_text(&whole.body, "Description")
    );

    for (version, build) in state::PUBLISHED_ENGINE_BUILDS {
        let answer = emulator
            .call("DescribeClusterVersions", &[("ClusterVersion", version)])
            .await;
        assert_eq!(
            answer.status, 200,
            "case version {version}: expected 200, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            all_text(&answer.body, "Description"),
            vec![state::engine_build_description(build)],
            "case version {version}: expected the engine build description, got {:?}",
            all_text(&answer.body, "Description")
        );
    }

    // A read leaves the catalog alone: the next caller reads the same builds,
    // before and after an account write.
    let created = emulator.create_minimal("catalog-probe").await;
    assert_eq!(
        created.status, 200,
        "case seed a cluster: expected 200, got {} with body {}",
        created.status, created.body
    );
    let again = emulator.call("DescribeClusterVersions", &[]).await;
    assert_eq!(
        all_text(&again.body, "Description"),
        expected,
        "case read after a create: expected an unchanged catalog, got {:?}",
        all_text(&again.body, "Description")
    );
}
