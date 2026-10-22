// SPDX-License-Identifier: CC0-1.0

//! `GetBucketBundles`: the bucket-bundle catalog Lightsail publishes for object
//! storage, driven over the wire through the same front door a client uses.
//!
//! The rule under test: `GetBucketBundles` reports the service's published bucket
//! bundle catalog — every bundle whose `isActive` is true, plus the inactive ones
//! when `includeInactive` is set — each entry carrying the monthly price, storage
//! quota and transfer quota that catalog records for it, and reporting it changes
//! nothing a later caller sees.
//!
//! So every case here is a TABLE: one row per catalog entry for the published
//! plans, one row per form the optional `includeInactive` flag can take (absent,
//! `false`, `true`), and the catalog-wide invariants (the flag's slices nest, the
//! ids are unique and resolvable, the plans grow with price) are asserted over
//! whatever [`BUCKET_BUNDLE_SPECS`] holds rather than over one entry by hand.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    bucket_bundle_spec, bucket_bundles_from, BucketBundleSpec, BUCKET_BUNDLE_SPECS,
};

use crate::instance_lifecycle::Emulator;

/// Every member the modelled `BucketBundle` structure declares
/// (`smithy_query(query="shape",shape="BucketBundle")`), in wire spelling. A
/// reported bundle carries exactly these and nothing else.
const BUCKET_BUNDLE_MEMBERS: &[&str] = &[
    "bundleId",
    "isActive",
    "name",
    "price",
    "storagePerMonthInGb",
    "transferPerMonthInGb",
];

/// One published Lightsail object-storage plan: `(bundleId, storage GB, transfer
/// GB, price USD/month)`.
///
/// SOURCE: the object storage table of the Lightsail pricing page — 5 GB of
/// storage with 25 GB of data transfer for $1 a month, 100 GB with 250 GB for $3,
/// and 250 GB with 500 GB for $5. Written out here independently of the service's
/// own `BUCKET_BUNDLE_SPECS`, so this row set pins the published figures rather
/// than restating whatever the code happens to hold.
const PUBLISHED_PLANS: &[(&str, i64, i64, f64)] = &[
    ("small_1_0", 5, 25, 1.0),
    ("medium_1_0", 100, 250, 3.0),
    ("large_1_0", 250, 500, 5.0),
];

/// The bundles `GetBucketBundles` reports for `request`, keyed by `bundleId`.
async fn reported(emulator: &Emulator, request: Value) -> BTreeMap<String, Value> {
    let case = request.to_string();
    let body = emulator.ok("GetBucketBundles", request).await;
    let bundles = body["bundles"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected a bundles array, got {body:?}"))
        .clone();
    let mut keyed = BTreeMap::new();
    for bundle in bundles {
        let id = bundle["bundleId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: every bundle carries a bundleId, got {bundle:?}"))
            .to_string();
        assert!(
            keyed.insert(id.clone(), bundle).is_none(),
            "case {case}: bundleId {id:?} was reported twice"
        );
    }
    keyed
}

/// Whether `price` is `expected` dollars, to the cent.
fn is_price(price: Option<f64>, expected: f64) -> bool {
    price.is_some_and(|price| (price - expected).abs() < 0.005)
}

/// Every published plan comes back with the price, storage quota and transfer
/// quota the catalog publishes for it, and nothing else does.
#[tokio::test]
async fn every_published_plan_is_reported_with_its_own_price_and_quotas() {
    let emulator = Emulator::new();
    let bundles = reported(&emulator, json!({})).await;

    for (bundle_id, storage, transfer, price) in PUBLISHED_PLANS {
        let bundle = bundles
            .get(*bundle_id)
            .unwrap_or_else(|| panic!("case {bundle_id}: expected this published plan to be reported, got {:?}", bundles.keys().collect::<Vec<_>>()));

        assert_eq!(
            bundle["storagePerMonthInGb"].as_i64(),
            Some(*storage),
            "case {bundle_id}: expected storagePerMonthInGb {storage}, got {:?}",
            bundle["storagePerMonthInGb"]
        );
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
        // `name` is the plan's label: the product and the plan's storage size,
        // as Lightsail reports it (`Object Storage 5GB`).
        assert_eq!(
            bundle["name"].as_str(),
            Some(format!("Object Storage {storage}GB").as_str()),
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
            BUCKET_BUNDLE_MEMBERS.iter().copied().collect::<BTreeSet<_>>(),
            "case {bundle_id}: expected exactly the modelled BucketBundle members, got {members:?}"
        );
    }

    let reported_ids: BTreeSet<&str> = bundles.keys().map(String::as_str).collect();
    let published_ids: BTreeSet<&str> = PUBLISHED_PLANS.iter().map(|(id, ..)| *id).collect();
    assert_eq!(
        reported_ids, published_ids,
        "case default request: expected exactly the published plans, got {reported_ids:?}"
    );
}

/// The `includeInactive` flag selects a slice of the one catalog: absent and
/// `false` report the active entries, `true` reports every entry, and the first is
/// always contained in the second.
///
/// The expectations come from the catalog the service serves, so the rule holds
/// whatever that catalog grows to hold — including a plan Lightsail later retires.
#[tokio::test]
async fn the_include_inactive_flag_selects_a_slice_of_the_one_catalog() {
    let emulator = Emulator::new();

    let active_ids: BTreeSet<&str> = BUCKET_BUNDLE_SPECS
        .iter()
        .filter(|spec| spec.is_active)
        .map(|spec| spec.bundle_id)
        .collect();
    let all_ids: BTreeSet<&str> = BUCKET_BUNDLE_SPECS
        .iter()
        .map(|spec| spec.bundle_id)
        .collect();

    // The rows differ along the only dimension the request has: whether the flag is
    // absent, off or on.
    let rows: &[(&str, Value, &BTreeSet<&str>)] = &[
        ("absent", json!({}), &active_ids),
        ("false", json!({"includeInactive": false}), &active_ids),
        ("true", json!({"includeInactive": true}), &all_ids),
    ];

    let mut by_case: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (case, request, expected) in rows {
        let bundles = reported(&emulator, request.clone()).await;
        let ids: BTreeSet<String> = bundles.keys().cloned().collect();
        let expected_ids: BTreeSet<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            ids, expected_ids,
            "case includeInactive={case}: expected the catalog slice {expected_ids:?}, got {ids:?}"
        );

        if *case != "true" {
            for (id, bundle) in &bundles {
                assert_eq!(
                    bundle["isActive"].as_bool(),
                    Some(true),
                    "case includeInactive={case}: {id} must not be reported while inactive, got {bundle:?}"
                );
            }
        }
        by_case.insert(case, ids);
    }

    let active = &by_case["absent"];
    assert_eq!(
        active, &by_case["false"],
        "case includeInactive absent vs false: an absent flag means false, got {active:?} and {:?}",
        by_case["false"]
    );
    assert!(
        by_case["true"].is_superset(active),
        "case includeInactive=true: the full catalog must contain every active bundle, got {:?} against {active:?}",
        by_case["true"]
    );
}

/// The slice rule itself, over catalogs that DO hold a retired plan.
///
/// The published catalog has no retired plan yet (Lightsail has withdrawn none), so
/// the wire case above cannot tell "filtered on `isActive`" from "reported
/// everything". These rows put the rule's own dimension under test: how many of the
/// catalog's entries are inactive, and where they sit in it.
#[test]
fn the_slice_rule_reports_an_inactive_plan_only_when_asked_for_it() {
    /// A plan in the catalog, active or retired — the figures are irrelevant to the
    /// slice, only `is_active` and the id are.
    fn spec(bundle_id: &'static str, is_active: bool) -> BucketBundleSpec {
        BucketBundleSpec {
            bundle_id,
            storage_per_month_in_gb: 5,
            transfer_per_month_in_gb: 25,
            price_usd_per_month: 1.0,
            is_active,
        }
    }

    /// One row: the case name, the catalog, the ids reported by default, and the
    /// ids reported with `includeInactive`.
    struct Row {
        case: &'static str,
        catalog: Vec<BucketBundleSpec>,
        active_only: &'static [&'static str],
        everything: &'static [&'static str],
    }

    let rows = [
        Row {
            case: "an empty catalog",
            catalog: vec![],
            active_only: &[],
            everything: &[],
        },
        Row {
            case: "every plan active",
            catalog: vec![spec("small_1_0", true), spec("large_1_0", true)],
            active_only: &["small_1_0", "large_1_0"],
            everything: &["small_1_0", "large_1_0"],
        },
        Row {
            case: "a retired first generation beside its active successor",
            catalog: vec![spec("small_1_0", false), spec("small_2_0", true)],
            active_only: &["small_2_0"],
            everything: &["small_1_0", "small_2_0"],
        },
        Row {
            case: "a retired plan last",
            catalog: vec![spec("small_1_0", true), spec("large_1_0", false)],
            active_only: &["small_1_0"],
            everything: &["small_1_0", "large_1_0"],
        },
        Row {
            case: "every plan retired",
            catalog: vec![spec("small_1_0", false), spec("large_1_0", false)],
            active_only: &[],
            everything: &["small_1_0", "large_1_0"],
        },
    ];

    for row in &rows {
        let case = row.case;
        for (include_inactive, expected) in [(false, row.active_only), (true, row.everything)] {
            let reported: Vec<String> = bucket_bundles_from(&row.catalog, include_inactive)
                .into_iter()
                .map(|bundle| bundle.bundle_id.unwrap_or_default())
                .collect();
            assert_eq!(
                reported,
                expected.iter().map(|id| (*id).to_string()).collect::<Vec<_>>(),
                "case {case:?} with includeInactive={include_inactive}: expected {expected:?} \
                 in the catalog's own order, got {reported:?}"
            );
        }
    }
}

/// The plan label is DERIVED from the plan's storage size, never carried per
/// entry: rows span the published sizes and sizes the published catalog does not
/// carry.
///
/// SOURCE of the labels: the `BucketBundle$name` values Lightsail reports —
/// `Object Storage 5GB`, `Object Storage 100GB`, `Object Storage 250GB`. The
/// published rows carry those strings literally; the rest state the same form
/// for a size the catalog does not publish.
#[test]
fn the_plan_label_names_the_product_and_the_plans_storage_size() {
    // (storage GB, the label the plan must report)
    let rows: &[(i32, &str)] = &[
        (5, "Object Storage 5GB"),
        (100, "Object Storage 100GB"),
        (250, "Object Storage 250GB"),
        (1, "Object Storage 1GB"),
        (50, "Object Storage 50GB"),
        (1000, "Object Storage 1000GB"),
    ];

    for (storage, expected) in rows {
        let catalog = vec![BucketBundleSpec {
            bundle_id: "small_1_0",
            storage_per_month_in_gb: *storage,
            transfer_per_month_in_gb: storage * 5,
            price_usd_per_month: 1.0,
            is_active: true,
        }];
        let reported = bucket_bundles_from(&catalog, false);
        let name = reported
            .first()
            .and_then(|bundle| bundle.name.clone())
            .unwrap_or_default();
        assert_eq!(
            name, *expected,
            "case a {storage} GB plan: expected the published label {expected:?}, got {name:?}"
        );
    }
}

/// Every reported bundle id resolves in the catalog and is a well-formed Lightsail
/// bundle id; the catalog's plans grow together, so a dearer plan never buys less.
#[tokio::test]
async fn reported_bundle_ids_resolve_and_the_plans_grow_with_their_price() {
    let emulator = Emulator::new();
    let bundles = reported(&emulator, json!({"includeInactive": true})).await;
    assert!(
        !bundles.is_empty(),
        "the service must publish at least one bucket bundle"
    );

    for (id, bundle) in &bundles {
        let spec = bucket_bundle_spec(id)
            .unwrap_or_else(|| panic!("case {id}: a reported id must resolve in the catalog"));
        assert_eq!(
            bundle["storagePerMonthInGb"].as_i64(),
            Some(i64::from(spec.storage_per_month_in_gb)),
            "case {id}: the reported storage must be the catalog's, got {:?}",
            bundle["storagePerMonthInGb"]
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
            spec.price_usd_per_month > 0.0
                && spec.storage_per_month_in_gb > 0
                && spec.transfer_per_month_in_gb > 0,
            "case {id}: a plan charges for storage and transfer, got {spec:?}"
        );
    }

    // A dearer plan buys at least as much storage and transfer as a cheaper one.
    for (cheaper, dearer) in BUCKET_BUNDLE_SPECS
        .iter()
        .zip(BUCKET_BUNDLE_SPECS.iter().skip(1))
    {
        assert!(
            dearer.price_usd_per_month > cheaper.price_usd_per_month
                && dearer.storage_per_month_in_gb >= cheaper.storage_per_month_in_gb
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
            bucket_bundle_spec(absent).is_none(),
            "case {absent:?}: an id the catalog does not publish must not resolve"
        );
    }
}

/// Reporting the catalog is a read: it answers the same thing every time, whatever
/// else the store holds, and two emulators publish the same catalog.
#[tokio::test]
async fn reporting_the_catalog_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();
    let requests = [
        json!({}),
        json!({"includeInactive": false}),
        json!({"includeInactive": true}),
    ];

    let mut first = Vec::new();
    for request in &requests {
        first.push(reported(&emulator, request.clone()).await);
    }

    // A mutation of unrelated state, then the same reads again: the published
    // catalog is not per-caller and not affected by what the store holds.
    emulator
        .ok(
            "CreateInstances",
            json!({
                "instanceNames": ["bundle-reader"],
                "availabilityZone": "us-east-2a",
                "blueprintId": "amazon_linux_2023",
                "bundleId": "nano_2_0"
            }),
        )
        .await;

    for (index, request) in requests.iter().enumerate() {
        let again = reported(&emulator, request.clone()).await;
        assert_eq!(
            again, first[index],
            "case {request}: two reads of the catalog must agree, got {again:?} after {:?}",
            first[index]
        );
    }

    // The instance the mutation created is still there: reading the catalog
    // consumed nothing.
    let instance = emulator
        .ok("GetInstance", json!({"instanceName": "bundle-reader"}))
        .await;
    assert_eq!(
        instance["instance"]["name"].as_str(),
        Some("bundle-reader"),
        "case store unchanged: expected the created instance to survive the reads, got {instance:?}"
    );

    // A second emulator, with its own store, publishes the same catalog.
    let other = Emulator::new();
    for (index, request) in requests.iter().enumerate() {
        let elsewhere = reported(&other, request.clone()).await;
        assert_eq!(
            elsewhere, first[index],
            "case {request}: the catalog must not depend on which emulator serves it, got {elsewhere:?}"
        );
    }
}
