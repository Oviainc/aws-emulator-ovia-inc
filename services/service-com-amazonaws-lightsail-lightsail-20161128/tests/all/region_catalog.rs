// SPDX-License-Identifier: CC0-1.0

//! `GetRegions` — the Region catalog, driven over the wire.
//!
//! Every case goes through the same front door a client uses: `POST /` with
//! `X-Amz-Target: Lightsail_20161128.GetRegions` and an `awsJson1_1` body,
//! dispatched by the multi-service router, so the modelled-constraint walk, the
//! handler and the awsJson envelope are exercised rather than bypassed.
//!
//! The rule under test: `GetRegions` answers one entry for EVERY Region the
//! modelled `RegionName` enum declares and no others, each entry describing that
//! Region (name, display name, description, continent code), and each Availability
//! Zone list present only when the request asked for it — `<region><letter>`
//! zones, as `AvailabilityZone$zoneName` documents. The catalog is the same on
//! every call: a read of it changes nothing.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix this service routes on (`OPERATIONS.md`, "Wire
/// protocol").
const TARGET_PREFIX: &str = "Lightsail_20161128";

/// Every Region name the model declares, which is the catalog `GetRegions` must
/// report: `smithy_query(query="shape",shape="RegionName")`, generated into the
/// service crate as `REGION_NAME_VALUES`.
const MODELLED_REGION_NAMES: &[&str] =
    service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;

/// One emulator with one Lightsail service mounted on one shared engine.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        // The suite signs nothing; routing is by `X-Amz-Target`.
        router.set_request_authentication(false);
        router.register_service(
            service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine),
        );
        Self { router }
    }

    /// Call `operation` with `body`, answering its HTTP status and parsed body.
    async fn call(&self, operation: &str, body: Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("x-amz-target", format!("{TARGET_PREFIX}.{operation}"))
            .header("content-type", "application/x-amz-json-1.1")
            .body(bytes::Bytes::from(body.to_string()))
            .expect("the request is well formed");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or(Value::Null)
        };
        (status, parsed)
    }

    /// `GetRegions` with `body`, required to succeed, answering its `regions`.
    async fn regions(&self, case: &str, body: Value) -> Vec<Value> {
        let (status, parsed) = self.call("GetRegions", body).await;
        assert_eq!(
            status, 200,
            "case {case}: GetRegions must succeed, got {status} {parsed:?}"
        );
        parsed["regions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected a regions array, got {parsed:?}"))
            .clone()
    }
}

/// The `name` of each reported Region, in report order.
fn reported_names(case: &str, regions: &[Value]) -> Vec<String> {
    regions
        .iter()
        .map(|region| {
            region["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a region name, got {region:?}"))
                .to_string()
        })
        .collect()
}

/// Every Region the model declares is reported, exactly once, and nothing else
/// is — a hollow 200 (no `regions`, or an empty one) fails here.
#[tokio::test]
async fn get_regions_reports_every_modelled_region_exactly_once() {
    let emulator = Emulator::new();
    let regions = emulator.regions("the whole catalog", json!({})).await;
    let names = reported_names("the whole catalog", &regions);

    assert_eq!(
        names.len(),
        MODELLED_REGION_NAMES.len(),
        "case the whole catalog: expected one entry per modelled region ({}), got {}: {names:?}",
        MODELLED_REGION_NAMES.len(),
        names.len()
    );
    let reported: BTreeSet<&str> = names.iter().map(String::as_str).collect();
    let modelled: BTreeSet<&str> = MODELLED_REGION_NAMES.iter().copied().collect();
    assert_eq!(
        reported, modelled,
        "case the whole catalog: expected exactly the modelled region names, got {reported:?}"
    );
}

/// Every reported Region describes itself: the members `Region` models as the
/// description of a Region are all filled, the display name is not just the
/// region code, the description is the documented sentence form, and the
/// continent code is the geography the region name begins with.
#[tokio::test]
async fn every_reported_region_describes_itself() {
    let emulator = Emulator::new();
    let regions = emulator.regions("region members", json!({})).await;

    for region in &regions {
        let name = region["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case region members: expected a name, got {region:?}"));

        let display_name = region["displayName"].as_str().unwrap_or_else(|| {
            panic!("case {name}: expected a displayName, got {region:?}")
        });
        assert!(
            !display_name.is_empty() && display_name != name,
            "case {name}: expected a display name naming the region's location, got {display_name:?}"
        );

        let description = region["description"].as_str().unwrap_or_else(|| {
            panic!("case {name}: expected a description, got {region:?}")
        });
        // The form the modelled `Region$description` documentation shows.
        assert!(
            description.starts_with("This region is recommended to serve users in ")
                && description.len() > "This region is recommended to serve users in ".len(),
            "case {name}: expected the documented description form, got {description:?}"
        );

        // `Region$continentCode` is "the continent code (NA, meaning North
        // America)", and an AWS region name begins with its geography.
        let expected_continent = match name.split('-').next() {
            Some("us" | "ca") => "NA",
            Some("sa") => "SA",
            Some("eu") => "EU",
            Some("ap") => "AP",
            other => panic!("case {name}: unexpected geography prefix {other:?}"),
        };
        assert_eq!(
            region["continentCode"].as_str(),
            Some(expected_continent),
            "case {name}: expected continent code {expected_continent}, got {:?}",
            region["continentCode"]
        );
    }
}

/// The two zone lists are opt-in, one per request member, and independent of each
/// other: the table spans every combination of the two booleans, including each
/// one absent.
#[tokio::test]
async fn availability_zone_lists_are_opt_in_per_member() {
    let emulator = Emulator::new();

    // (case, request body, instance zones expected, database zones expected)
    let cases: &[(&str, Value, bool, bool)] = &[
        ("neither member sent", json!({}), false, false),
        (
            "both members false",
            json!({"includeAvailabilityZones": false, "includeRelationalDatabaseAvailabilityZones": false}),
            false,
            false,
        ),
        (
            "instance zones only",
            json!({"includeAvailabilityZones": true}),
            true,
            false,
        ),
        (
            "database zones only",
            json!({"includeRelationalDatabaseAvailabilityZones": true}),
            false,
            true,
        ),
        (
            "both zone lists",
            json!({"includeAvailabilityZones": true, "includeRelationalDatabaseAvailabilityZones": true}),
            true,
            true,
        ),
        (
            "instance zones asked, database zones declined",
            json!({"includeAvailabilityZones": true, "includeRelationalDatabaseAvailabilityZones": false}),
            true,
            false,
        ),
    ];

    for (case, body, want_zones, want_database_zones) in cases {
        let regions = emulator.regions(case, body.clone()).await;
        for region in &regions {
            let name = region["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a name, got {region:?}"));
            for (member, wanted) in [
                ("availabilityZones", *want_zones),
                ("relationalDatabaseAvailabilityZones", *want_database_zones),
            ] {
                let reported = region[member].as_array();
                if !wanted {
                    assert!(
                        reported.is_none(),
                        "case {case} / {name}: {member} was not asked for, so it must be absent, got {:?}",
                        region[member]
                    );
                    continue;
                }
                let zones = reported.unwrap_or_else(|| {
                    panic!("case {case} / {name}: expected {member}, got {region:?}")
                });
                assert!(
                    !zones.is_empty(),
                    "case {case} / {name}: expected at least one zone in {member}, got {zones:?}"
                );
                let mut letters = Vec::new();
                for zone in zones {
                    let zone_name = zone["zoneName"].as_str().unwrap_or_else(|| {
                        panic!("case {case} / {name}: expected a zoneName, got {zone:?}")
                    });
                    // `AvailabilityZone$zoneName`: "The format is us-east-2a
                    // (case-sensitive)".
                    let letter = zone_name.strip_prefix(name).unwrap_or_else(|| {
                        panic!(
                            "case {case} / {name}: expected a zone name in this region, got {zone_name:?}"
                        )
                    });
                    assert!(
                        letter.len() == 1
                            && letter.chars().all(|c| c.is_ascii_lowercase()),
                        "case {case} / {name}: expected a single lowercase zone letter, got {zone_name:?}"
                    );
                    assert_eq!(
                        zone["state"].as_str(),
                        Some("available"),
                        "case {case} / {name}: expected a serving zone state, got {:?}",
                        zone["state"]
                    );
                    letters.push(letter.to_string());
                }
                let unique: BTreeSet<&String> = letters.iter().collect();
                assert_eq!(
                    unique.len(),
                    letters.len(),
                    "case {case} / {name}: expected distinct zone letters in {member}, got {letters:?}"
                );
                let mut sorted = letters.clone();
                sorted.sort();
                assert_eq!(
                    sorted, letters,
                    "case {case} / {name}: expected {member} in zone-letter order, got {letters:?}"
                );
            }
        }
    }
}

/// A specific catalog entry the issue did not cite, read through the wire: the
/// catalog is data, so one entry pins the data rather than the plumbing.
#[tokio::test]
async fn an_uncited_catalog_entry_reports_its_own_values() {
    let emulator = Emulator::new();
    let regions = emulator
        .regions(
            "ap-northeast-2",
            json!({"includeAvailabilityZones": true, "includeRelationalDatabaseAvailabilityZones": true}),
        )
        .await;
    let seoul = regions
        .iter()
        .find(|region| region["name"] == json!("ap-northeast-2"))
        .unwrap_or_else(|| {
            panic!("case ap-northeast-2: the catalog must carry this modelled region")
        });

    assert_eq!(
        seoul["displayName"].as_str(),
        Some("Seoul"),
        "case ap-northeast-2: expected the region's display name, got {:?}",
        seoul["displayName"]
    );
    assert_eq!(
        seoul["continentCode"].as_str(),
        Some("AP"),
        "case ap-northeast-2: expected continent code AP, got {:?}",
        seoul["continentCode"]
    );
    assert_eq!(
        seoul["availabilityZones"][0]["zoneName"].as_str(),
        Some("ap-northeast-2a"),
        "case ap-northeast-2: expected the first zone to be the region plus 'a', got {:?}",
        seoul["availabilityZones"]
    );
    assert_eq!(
        seoul["relationalDatabaseAvailabilityZones"][0]["zoneName"].as_str(),
        Some("ap-northeast-2a"),
        "case ap-northeast-2: expected database zones in the same region, got {:?}",
        seoul["relationalDatabaseAvailabilityZones"]
    );
}

/// Reading the catalog changes nothing a later caller sees: two identical calls,
/// and a call with the zone lists in between, answer the same catalog.
#[tokio::test]
async fn reading_the_catalog_does_not_change_it() {
    let emulator = Emulator::new();
    let first = emulator.regions("first read", json!({})).await;
    let with_zones = emulator
        .regions("read with zones", json!({"includeAvailabilityZones": true}))
        .await;
    let second = emulator.regions("second read", json!({})).await;

    assert_eq!(
        first, second,
        "case repeated read: the catalog must not change between reads"
    );
    assert_eq!(
        reported_names("read with zones", &with_zones),
        reported_names("first read", &first),
        "case read with zones: the same regions in the same order, zones or not"
    );
}

/// Every zone a Region advertises is a location this service accepts: the zone
/// names `GetRegions` publishes resolve back to their own Region, which is what
/// `CreateInstances` validates an `availabilityZone` against.
#[tokio::test]
async fn published_zones_belong_to_their_region() {
    let emulator = Emulator::new();
    let regions = emulator
        .regions("zone to region", json!({"includeAvailabilityZones": true}))
        .await;

    for region in &regions {
        let name = region["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case zone to region: expected a name, got {region:?}"));
        let zones = region["availabilityZones"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name}: expected availabilityZones, got {region:?}"));
        for zone in zones {
            let zone_name = zone["zoneName"]
                .as_str()
                .unwrap_or_else(|| panic!("case {name}: expected a zoneName, got {zone:?}"));
            let resolved =
                service_com_amazonaws_lightsail_lightsail_20161128::state::region_of_availability_zone(
                    zone_name,
                );
            assert_eq!(
                resolved,
                Some(name),
                "case {name}: zone {zone_name} must resolve back to its own region, got {resolved:?}"
            );
        }
    }
}
