// SPDX-License-Identifier: CC0-1.0

//! `GetDistributionBundles`: the distribution-bundle catalog Lightsail publishes
//! for its CDN, driven over the wire through the same front door a client uses.
//!
//! The rule under test: `GetDistributionBundles` reports the service's published
//! distribution-bundle catalog — every plan it holds, since the request carries no
//! member to select a slice with — each entry carrying the monthly network transfer
//! quota and the monthly price that catalog records for it and an `isActive` saying
//! whether a new or existing distribution may be put on it; the call is served by
//! the N. Virginia endpoint alone, and reporting the catalog changes nothing a later
//! caller sees.
//!
//! So every case here is a TABLE: one row per catalog entry for the published plans,
//! one row per catalog shape the "report every plan" rule has to hold over (empty,
//! all active, a retired plan among active ones, all retired), one row per transfer
//! quota the derived plan label has to be built from, and one row per Region the
//! restriction covers.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::ops::types::REGION_NAME_VALUES;
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    distribution_bundle_spec, distribution_bundles_from, DistributionBundleSpec,
    DISTRIBUTION_BUNDLE_SPECS,
};

use crate::instance_lifecycle::{error_code, Emulator};

/// Every member the modelled `DistributionBundle` structure declares
/// (`smithy_query(query="shape",shape="DistributionBundle")`), in wire spelling. A
/// reported bundle carries exactly these and nothing else.
const DISTRIBUTION_BUNDLE_MEMBERS: &[&str] =
    &["bundleId", "isActive", "name", "price", "transferPerMonthInGb"];

/// One published Lightsail CDN plan: `(bundleId, transfer GB, price USD/month)`.
///
/// SOURCE: the CDN distribution table of the Lightsail pricing page — 50 GB of data
/// transfer for $2.50 a month, 200 GB for $5 and 500 GB for $10 — each the first
/// (`_1_0`) generation of its size. Written out here independently of the service's
/// own `DISTRIBUTION_BUNDLE_SPECS`, so this row set pins the published figures
/// rather than restating whatever the code happens to hold.
const PUBLISHED_PLANS: &[(&str, i64, f64)] = &[
    ("small_1_0", 50, 2.5),
    ("medium_1_0", 200, 5.0),
    ("large_1_0", 500, 10.0),
];

/// The only Region Lightsail serves the distribution API from, from the modelled
/// `InvalidInputException` documentation
/// (`smithy_query(query="doc",shape="InvalidInputException")`).
const DISTRIBUTION_REGION: &str = "us-east-1";

/// The refusal that documentation states verbatim, so its text is a requirement and
/// not an example: "Domain and distribution APIs are only available in the N.
/// Virginia (us-east-1) Amazon Web Services Region. Please set your Amazon Web
/// Services Region configuration to us-east-1 to create, view, or edit these
/// resources."
const REFUSAL: &str = "Domain and distribution APIs are only available in the N. Virginia \
                       (us-east-1) Amazon Web Services Region. Please set your Amazon Web \
                       Services Region configuration to us-east-1 to create, view, or edit \
                       these resources.";

/// An Amazon Web Services Region the modelled `RegionName` enum does NOT list, so a
/// case can sit outside the catalog: a scope naming a Region Lightsail does not
/// serve at all is no more the distribution Region than any other.
const OFF_ENUM_REGION: &str = "il-central-1";

/// The instance blueprint, bundle and zone the origin instance is created from: the
/// origin's own configuration is not what any case here is about.
const ORIGIN_BLUEPRINT: &str = "amazon_linux_2023";
const ORIGIN_INSTANCE_BUNDLE: &str = "nano_3_0";
const ORIGIN_ZONE: &str = "us-east-1a";
/// The instance every distribution in this suite pulls its content from.
const ORIGIN_INSTANCE: &str = "bundle-origin";

/// Every Region a distribution call can be made in that is NOT the distribution
/// Region: the modelled `RegionName` enum's values minus that Region, so the table
/// cannot drift from the model as the catalog grows, plus [`OFF_ENUM_REGION`].
fn other_regions() -> Vec<&'static str> {
    assert!(
        REGION_NAME_VALUES.contains(&DISTRIBUTION_REGION),
        "case the Region catalog: expected the modelled RegionName enum to list the \
         distribution Region, got {REGION_NAME_VALUES:?}"
    );
    assert!(
        !REGION_NAME_VALUES.contains(&OFF_ENUM_REGION),
        "case the Region catalog: expected {OFF_ENUM_REGION} to sit OUTSIDE the modelled \
         RegionName enum, got {REGION_NAME_VALUES:?}"
    );
    let mut regions: Vec<&'static str> = REGION_NAME_VALUES
        .iter()
        .copied()
        .filter(|region| *region != DISTRIBUTION_REGION)
        .collect();
    regions.push(OFF_ENUM_REGION);
    regions
}

/// The bundles `GetDistributionBundles` reports in `region`, keyed by `bundleId`.
async fn reported_in(emulator: &Emulator, region: Option<&str>) -> BTreeMap<String, Value> {
    let case = format!("{region:?}");
    let (status, body) = emulator
        .call_in(region, "GetDistributionBundles", json!({}))
        .await;
    assert_eq!(
        status, 200,
        "case region {case}: GetDistributionBundles must be served, got {status} {body:?}"
    );
    let bundles = body["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("case region {case}: expected a bundles array, got {body:?}"))
        .clone();
    let mut keyed = BTreeMap::new();
    for bundle in bundles {
        let id = bundle["bundleId"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case region {case}: every bundle carries a bundleId, got {bundle:?}")
            })
            .to_string();
        assert!(
            keyed.insert(id.clone(), bundle).is_none(),
            "case region {case}: bundleId {id:?} was reported twice"
        );
    }
    keyed
}

/// The bundles `GetDistributionBundles` reports to a request carrying no credential
/// scope at all, which the service serves in its default Region.
async fn reported(emulator: &Emulator) -> BTreeMap<String, Value> {
    reported_in(emulator, None).await
}

/// Whether `price` is `expected` dollars, to the cent.
fn is_price(price: Option<f64>, expected: f64) -> bool {
    price.is_some_and(|price| (price - expected).abs() < 0.005)
}

/// Every published plan comes back with the transfer quota and price the catalog
/// publishes for it, exactly the modelled members, and nothing else is reported.
#[tokio::test]
async fn every_published_plan_is_reported_with_its_own_price_and_quota() {
    let emulator = Emulator::new();
    let bundles = reported(&emulator).await;

    for (bundle_id, transfer, price) in PUBLISHED_PLANS {
        let bundle = bundles.get(*bundle_id).unwrap_or_else(|| {
            panic!(
                "case {bundle_id}: expected this published plan to be reported, got {:?}",
                bundles.keys().collect::<Vec<_>>()
            )
        });

        assert_eq!(
            bundle["transferPerMonthInGb"].as_i64(),
            Some(*transfer),
            "case {bundle_id}: expected transferPerMonthInGb {transfer}, got {:?}",
            bundle["transferPerMonthInGb"]
        );
        assert!(
            is_price(bundle["price"].as_f64(), *price),
            "case {bundle_id}: expected price {price}, got {:?}",
            bundle["price"]
        );
        // `name` is "The name of the distribution bundle": the plan's monthly
        // transfer quota, as Lightsail reports it (`50GB`).
        assert_eq!(
            bundle["name"].as_str(),
            Some(format!("{transfer}GB").as_str()),
            "case {bundle_id}: expected the plan's published label as its name, got {:?}",
            bundle["name"]
        );
        assert_eq!(
            bundle["isActive"].as_bool(),
            Some(true),
            "case {bundle_id}: a plan that is still offered is active, got {:?}",
            bundle["isActive"]
        );

        // Exactly the modelled members, so nothing is reported that the shape does
        // not declare and nothing it declares is left out.
        let members: BTreeSet<&str> = bundle
            .as_object()
            .unwrap_or_else(|| panic!("case {bundle_id}: a bundle is an object, got {bundle:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            DISTRIBUTION_BUNDLE_MEMBERS
                .iter()
                .copied()
                .collect::<BTreeSet<_>>(),
            "case {bundle_id}: expected exactly the modelled DistributionBundle members, got \
             {members:?}"
        );
    }

    let reported_ids: BTreeSet<&str> = bundles.keys().map(String::as_str).collect();
    let published_ids: BTreeSet<&str> = PUBLISHED_PLANS.iter().map(|(id, ..)| *id).collect();
    assert_eq!(
        reported_ids, published_ids,
        "case the published catalog: expected exactly the published plans, got {reported_ids:?}"
    );
}

/// RULE: the report is the whole catalog — the request has no member to select a
/// slice with, so a retired plan is reported too, with `isActive` false, in the
/// catalog's own order.
///
/// The published catalog has no retired plan yet (Lightsail has withdrawn none), so
/// the wire case above cannot tell "every plan" from "the active plans". These rows
/// put the rule's own dimension under test: how many of the catalog's entries are
/// retired, and where they sit in it.
#[test]
fn the_report_is_every_plan_the_catalog_holds_whether_active_or_retired() {
    /// A plan in the catalog, active or retired — only `is_active` and the id matter
    /// to this rule.
    fn spec(bundle_id: &'static str, is_active: bool) -> DistributionBundleSpec {
        DistributionBundleSpec {
            bundle_id,
            transfer_per_month_in_gb: 50,
            price_usd_per_month: 2.5,
            is_active,
        }
    }

    /// One row: the case name, the catalog, and the ids the report must carry with
    /// the `isActive` each must carry.
    struct Row {
        case: &'static str,
        catalog: Vec<DistributionBundleSpec>,
        expected: &'static [(&'static str, bool)],
    }

    let rows = [
        Row {
            case: "an empty catalog",
            catalog: vec![],
            expected: &[],
        },
        Row {
            case: "one active plan",
            catalog: vec![spec("small_1_0", true)],
            expected: &[("small_1_0", true)],
        },
        Row {
            case: "every plan active",
            catalog: vec![spec("small_1_0", true), spec("large_1_0", true)],
            expected: &[("small_1_0", true), ("large_1_0", true)],
        },
        Row {
            case: "a retired first generation before its active successor",
            catalog: vec![spec("small_1_0", false), spec("small_2_0", true)],
            expected: &[("small_1_0", false), ("small_2_0", true)],
        },
        Row {
            case: "a retired plan last",
            catalog: vec![spec("small_1_0", true), spec("large_1_0", false)],
            expected: &[("small_1_0", true), ("large_1_0", false)],
        },
        Row {
            case: "every plan retired",
            catalog: vec![spec("small_1_0", false), spec("large_1_0", false)],
            expected: &[("small_1_0", false), ("large_1_0", false)],
        },
    ];

    for row in &rows {
        let case = row.case;
        let reported: Vec<(String, bool)> = distribution_bundles_from(&row.catalog)
            .into_iter()
            .map(|bundle| {
                (
                    bundle.bundle_id.unwrap_or_default(),
                    bundle.is_active.unwrap_or_default(),
                )
            })
            .collect();
        let expected: Vec<(String, bool)> = row
            .expected
            .iter()
            .map(|(id, active)| ((*id).to_string(), *active))
            .collect();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} in the catalog's own order, got {reported:?}"
        );
    }
}

/// The plan label is DERIVED from the plan's monthly transfer quota, never carried
/// per entry: rows span the published quotas and quotas the published catalog does
/// not carry.
///
/// SOURCE of the labels: the `DistributionBundle$name` values Lightsail reports —
/// `50GB`, `200GB`, `500GB`. The published rows carry those strings literally; the
/// rest state the same form for a quota the catalog does not publish.
#[test]
fn the_plan_label_names_the_plans_monthly_transfer_quota() {
    // (transfer GB, the label the plan must report)
    let rows: &[(i32, &str)] = &[
        (50, "50GB"),
        (200, "200GB"),
        (500, "500GB"),
        (1, "1GB"),
        (1_024, "1024GB"),
        (5_000, "5000GB"),
    ];

    for (transfer, expected) in rows {
        let catalog = vec![DistributionBundleSpec {
            bundle_id: "small_1_0",
            transfer_per_month_in_gb: *transfer,
            price_usd_per_month: 2.5,
            is_active: true,
        }];
        let name = distribution_bundles_from(&catalog)
            .first()
            .and_then(|bundle| bundle.name.clone())
            .unwrap_or_default();
        assert_eq!(
            name, *expected,
            "case a {transfer} GB plan: expected the published label {expected:?}, got {name:?}"
        );
    }
}

/// Every reported bundle id resolves in the catalog and is a well-formed Lightsail
/// bundle id; the catalog's plans grow together, so a dearer plan never buys less.
#[tokio::test]
async fn reported_bundle_ids_resolve_and_the_plans_grow_with_their_price() {
    let emulator = Emulator::new();
    let bundles = reported(&emulator).await;
    assert!(
        !bundles.is_empty(),
        "the service must publish at least one distribution bundle"
    );

    for (id, bundle) in &bundles {
        let spec = distribution_bundle_spec(id)
            .unwrap_or_else(|| panic!("case {id}: a reported id must resolve in the catalog"));
        assert_eq!(
            bundle["transferPerMonthInGb"].as_i64(),
            Some(i64::from(spec.transfer_per_month_in_gb)),
            "case {id}: the reported transfer quota must be the catalog's, got {:?}",
            bundle["transferPerMonthInGb"]
        );

        // `<size>_<major>_<minor>`: the id form every Lightsail bundle id carries.
        let tokens: Vec<&str> = id.split('_').collect();
        assert_eq!(
            tokens.len(),
            3,
            "case {id}: expected a <size>_<major>_<minor> bundle id, got {tokens:?}"
        );
        assert!(
            !tokens[0].is_empty() && tokens[0].chars().all(|c| c.is_ascii_alphanumeric()),
            "case {id}: expected an alphanumeric size token, got {:?}",
            tokens[0]
        );
        for version in &tokens[1..] {
            assert!(
                !version.is_empty() && version.chars().all(|c| c.is_ascii_digit()),
                "case {id}: expected a numeric version token, got {version:?}"
            );
        }

        assert!(
            spec.price_usd_per_month > 0.0 && spec.transfer_per_month_in_gb > 0,
            "case {id}: a plan charges for a transfer quota, got {spec:?}"
        );
    }

    // A dearer plan buys at least as much transfer as a cheaper one.
    for (cheaper, dearer) in DISTRIBUTION_BUNDLE_SPECS
        .iter()
        .zip(DISTRIBUTION_BUNDLE_SPECS.iter().skip(1))
    {
        assert!(
            dearer.price_usd_per_month > cheaper.price_usd_per_month
                && dearer.transfer_per_month_in_gb >= cheaper.transfer_per_month_in_gb,
            "case {} after {}: a dearer plan must not buy less, got {dearer:?} after {cheaper:?}",
            dearer.bundle_id,
            cheaper.bundle_id
        );
    }

    // An id the catalog does not publish resolves to nothing — the lookup answers
    // from the catalog rather than from the shape of what it is asked.
    for absent in ["tiny_1_0", "small_9_9", "small", "SMALL_1_0", ""] {
        assert!(
            distribution_bundle_spec(absent).is_none(),
            "case {absent:?}: an id the catalog does not publish must not resolve"
        );
    }
}

/// RULE: every plan the report says is active is one a distribution may be put on,
/// and an id the report does not carry is refused — so the reported catalog is the
/// one the create path answers from, not a second list.
#[tokio::test]
async fn an_active_reported_plan_is_one_a_distribution_can_be_created_on() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [ORIGIN_INSTANCE],
                "availabilityZone": ORIGIN_ZONE,
                "blueprintId": ORIGIN_BLUEPRINT,
                "bundleId": ORIGIN_INSTANCE_BUNDLE,
            }),
        )
        .await;

    let bundles = reported(&emulator).await;
    for (id, bundle) in &bundles {
        if bundle["isActive"].as_bool() != Some(true) {
            continue;
        }
        let name = format!("cdn-{id}");
        let created = emulator
            .ok(
                "CreateDistribution",
                json!({
                    "distributionName": name,
                    "bundleId": id,
                    "origin": {"name": ORIGIN_INSTANCE, "regionName": DISTRIBUTION_REGION},
                    "defaultCacheBehavior": {"behavior": "cache"},
                }),
            )
            .await;
        assert_eq!(
            created["distribution"]["bundleId"].as_str(),
            Some(id.as_str()),
            "case {id}: expected the created distribution to carry the plan it was created on, \
             got {:?}",
            created["distribution"]
        );

        // Read it back through the implemented reader: the plan is what the stored
        // record holds, not just what the create answered.
        let read = emulator
            .ok("GetDistributions", json!({"distributionName": name}))
            .await;
        assert_eq!(
            read["distributions"][0]["bundleId"].as_str(),
            Some(id.as_str()),
            "case {id}: expected the stored distribution to report the same plan, got {:?}",
            read["distributions"]
        );
    }

    // An id the report does not carry is not a plan a distribution can be created
    // on, whatever its shape.
    for unpublished in ["tiny_1_0", "small_9_9", "medium_2_0"] {
        assert!(
            !bundles.contains_key(unpublished),
            "case {unpublished}: this id must not be in the published report"
        );
        let (status, body) = emulator
            .call(
                "CreateDistribution",
                json!({
                    "distributionName": format!("cdn-{unpublished}"),
                    "bundleId": unpublished,
                    "origin": {"name": ORIGIN_INSTANCE, "regionName": DISTRIBUTION_REGION},
                    "defaultCacheBehavior": {"behavior": "cache"},
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {unpublished}: expected a 400 refusal for an unpublished plan, got {status} \
             {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case {unpublished}: expected the modelled InvalidInputException, got {body:?}"
        );
    }
}

/// RULE: the report is served by the N. Virginia endpoint ALONE, so a request whose
/// credential scope names any other Region is refused with the modelled
/// `InvalidInputException` (HTTP 400) carrying the documented restriction, while the
/// distribution Region's endpoint — and a request carrying no credential scope at
/// all, which is served in the service's default Region — answers the catalog.
#[tokio::test]
async fn the_report_is_served_by_the_distribution_region_alone() {
    let emulator = Emulator::new();
    let served = reported_in(&emulator, Some(DISTRIBUTION_REGION)).await;
    assert_eq!(
        served,
        reported(&emulator).await,
        "case no credential scope: expected the same catalog the distribution Region answers, \
         got {served:?}"
    );

    for region in other_regions() {
        let (status, body) = emulator
            .call_in(Some(region), "GetDistributionBundles", json!({}))
            .await;
        assert_eq!(
            status, 400,
            "case in {region}: expected a 400 refusal, got {status} {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInputException",
            "case in {region}: expected the modelled InvalidInputException, got {body:?}"
        );
        assert_eq!(
            body["message"].as_str(),
            Some(REFUSAL),
            "case in {region}: expected the documented restriction, got {:?}",
            body["message"]
        );
        assert!(
            body["bundles"].is_null(),
            "case in {region}: a refusal reports no bundles, got {body:?}"
        );
    }
}

/// Reporting the catalog is a read: it answers the same thing every time, whatever
/// else the store holds, and two emulators publish the same catalog.
#[tokio::test]
async fn reporting_the_catalog_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let first = reported(&emulator).await;

    // A mutation of unrelated state, then the same read again: the published
    // catalog is not per-caller and not affected by what the store holds.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": [ORIGIN_INSTANCE],
                "availabilityZone": ORIGIN_ZONE,
                "blueprintId": ORIGIN_BLUEPRINT,
                "bundleId": ORIGIN_INSTANCE_BUNDLE,
            }),
        )
        .await;

    let again = reported(&emulator).await;
    assert_eq!(
        again, first,
        "case two reads: the catalog must not change between reads, got {again:?} after {first:?}"
    );

    // The instance the mutation created is still there: reading the catalog
    // consumed nothing.
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": ORIGIN_INSTANCE}))
        .await;
    assert_eq!(
        instance["instance"]["name"].as_str(),
        Some(ORIGIN_INSTANCE),
        "case store unchanged: expected the created instance to survive the read, got \
         {instance:?}"
    );

    // A second emulator, with its own store, publishes the same catalog.
    let other = Emulator::new();
    let elsewhere = reported(&other).await;
    assert_eq!(
        elsewhere, first,
        "case a second emulator: the catalog must not depend on which emulator serves it, got \
         {elsewhere:?}"
    );
}
