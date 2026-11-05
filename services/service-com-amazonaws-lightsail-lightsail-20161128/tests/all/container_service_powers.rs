// SPDX-License-Identifier: CC0-1.0

//! `GetContainerServicePowers`: the container service power catalog Lightsail
//! publishes, driven over the wire through the same front door a client uses.
//!
//! The rule under test: `GetContainerServicePowers` reports the service's published
//! container service power catalog — one entry per power, in the catalog's own
//! order, each carrying that power's friendly name and id, the vCPUs and RAM it
//! provides and its base monthly price, and `isActive` true exactly for the powers
//! a container service can be specified with — and reporting it changes nothing a
//! later caller sees.
//!
//! So the cases here are TABLES: one row per published plan (the figures written
//! out independently of the service's own table), one row per catalog the entry rule
//! can be asked about (empty, one power, several, the same powers reordered, a power
//! name the service does not accept), and one row per power a create can name, each
//! driven through `CreateContainerService` and read back through
//! `GetContainerServices`. The input shape declares no member, so the request itself
//! has no dimension to vary: what varies across the read-invariance case is when a
//! read happens and what the store holds when it does.
//!
//! The HTTP harness is the one `instance_lifecycle.rs` already has.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_lightsail_lightsail_20161128::state::{
    container_service_powers, container_service_powers_from, ContainerPowerSpec,
    CONTAINER_SERVICE_POWERS,
};

use crate::instance_lifecycle::Emulator;

/// Every member the modelled `ContainerServicePower` structure declares
/// (`smithy_query(query="shape",shape="ContainerServicePower")`), in wire spelling.
/// A reported power carries exactly these and nothing else.
const POWER_MEMBERS: &[&str] = &[
    "cpuCount",
    "isActive",
    "name",
    "powerId",
    "price",
    "ramSizeInGb",
];

/// Every value the modelled `ContainerServicePowerName` enum admits — the set
/// `CreateContainerService$power` is constrained to
/// (`smithy_query(query="shape",shape="ContainerServicePowerName")`). Written out
/// here rather than imported, so these rows pin the modelled surface rather than
/// restating whatever the code holds.
const MODELLED_POWER_NAMES: &[&str] = &["nano", "micro", "small", "medium", "large", "xlarge"];

/// One published Lightsail container service plan: `(name, powerId, vCPUs, RAM GB,
/// price USD/month)`.
///
/// SOURCE: the container services table of the Lightsail pricing page — nano with
/// 0.25 vCPUs and 0.5 GB for $7 a month, micro with 0.25 vCPUs and 1 GB for $10,
/// small with 0.5 vCPUs and 1 GB for $15, medium with 1 vCPU and 2 GB for $40,
/// large with 2 vCPUs and 4 GB for $80 and xlarge with 4 vCPUs and 8 GB for $160;
/// the id form is the `ContainerServicePower$powerId` documentation's (`nano-1`).
///
/// The rows are each plan's OWN figures: the plans do not step in lockstep (micro
/// and small carry the same RAM and differ in vCPUs and price), so nothing here
/// may be derived from the row above or below it.
const PUBLISHED_PLANS: &[(&str, &str, f64, f64, f64)] = &[
    ("nano", "nano-1", 0.25, 0.5, 7.0),
    ("micro", "micro-1", 0.25, 1.0, 10.0),
    ("small", "small-1", 0.5, 1.0, 15.0),
    ("medium", "medium-1", 1.0, 2.0, 40.0),
    ("large", "large-1", 2.0, 4.0, 80.0),
    ("xlarge", "xlarge-1", 4.0, 8.0, 160.0),
];

/// The powers `GetContainerServicePowers` reports, keyed by `powerId`.
async fn reported(emulator: &Emulator) -> BTreeMap<String, Value> {
    let body = emulator.ok("GetContainerServicePowers", json!({})).await;
    let powers = body["powers"]
        .as_array()
        .unwrap_or_else(|| panic!("case the published catalog: expected a powers array, got {body:?}"))
        .clone();

    let mut keyed = BTreeMap::new();
    for power in powers {
        let members: BTreeSet<&str> = power
            .as_object()
            .unwrap_or_else(|| panic!("case the published catalog: a power is an object, got {power:?}"))
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            members,
            POWER_MEMBERS.iter().copied().collect::<BTreeSet<_>>(),
            "case the published catalog: expected exactly the modelled ContainerServicePower \
             members, got {members:?}"
        );

        let id = power["powerId"]
            .as_str()
            .unwrap_or_else(|| panic!("case the published catalog: every power carries a powerId, got {power:?}"))
            .to_string();
        assert!(
            keyed.insert(id.clone(), power).is_none(),
            "case the published catalog: powerId {id:?} was reported twice"
        );
    }
    keyed
}

/// Whether `value` is `expected`, to a hundredth — the figures are modelled as
/// `float`, so they come back as f32-rounded numbers.
fn is_close(value: Option<f64>, expected: f64) -> bool {
    value.is_some_and(|value| (value - expected).abs() < 0.005)
}

/// Every published plan comes back under its own id, carrying the hardware and the
/// base price the catalog publishes for it, and nothing else does.
#[tokio::test]
async fn every_published_power_is_reported_with_its_own_hardware_and_price() {
    let emulator = Emulator::new();
    let powers = reported(&emulator).await;

    for (name, power_id, cpu_count, ram_size_in_gb, price) in PUBLISHED_PLANS {
        let power = powers.get(*power_id).unwrap_or_else(|| {
            panic!(
                "case {power_id}: expected this published plan to be reported, got {:?}",
                powers.keys().collect::<Vec<_>>()
            )
        });

        assert_eq!(
            power["name"].as_str(),
            Some(*name),
            "case {power_id}: expected the plan's friendly name {name:?}, got {:?}",
            power["name"]
        );
        assert!(
            is_close(power["cpuCount"].as_f64(), *cpu_count),
            "case {power_id}: expected cpuCount {cpu_count}, got {:?}",
            power["cpuCount"]
        );
        assert!(
            is_close(power["ramSizeInGb"].as_f64(), *ram_size_in_gb),
            "case {power_id}: expected ramSizeInGb {ram_size_in_gb}, got {:?}",
            power["ramSizeInGb"]
        );
        assert!(
            is_close(power["price"].as_f64(), *price),
            "case {power_id}: expected price {price}, got {:?}",
            power["price"]
        );
        assert_eq!(
            power["isActive"].as_bool(),
            Some(true),
            "case {power_id}: a power that can still be specified is active, got {:?}",
            power["isActive"]
        );
    }

    let reported_ids: BTreeSet<&str> = powers.keys().map(String::as_str).collect();
    let published_ids: BTreeSet<&str> = PUBLISHED_PLANS.iter().map(|(_, id, ..)| *id).collect();
    assert_eq!(
        reported_ids, published_ids,
        "case the published catalog: expected exactly the published plans, got {reported_ids:?}"
    );

    // The friendly names are exactly the values a `power` member may take, so every
    // power a create can name is reported and nothing else is.
    let reported_names: BTreeSet<&str> = powers
        .values()
        .filter_map(|power| power["name"].as_str())
        .collect();
    assert_eq!(
        reported_names,
        MODELLED_POWER_NAMES.iter().copied().collect::<BTreeSet<_>>(),
        "case the modelled power names: expected the catalog to publish exactly the modelled \
         ContainerServicePowerName values, got {reported_names:?}"
    );

    // The plans grow together: a dearer power never provides less.
    for (cheaper, dearer) in CONTAINER_SERVICE_POWERS
        .iter()
        .zip(CONTAINER_SERVICE_POWERS.iter().skip(1))
    {
        assert!(
            dearer.price > cheaper.price
                && dearer.cpu_count >= cheaper.cpu_count
                && dearer.ram_size_in_gb >= cheaper.ram_size_in_gb,
            "case {} after {}: a dearer power must not provide less, got {dearer:?} after \
             {cheaper:?}",
            dearer.name,
            cheaper.name
        );
    }

    // Every reported id is `<friendly name>-<generation>`, the documented form.
    for (id, power) in &powers {
        let name = power["name"].as_str().unwrap_or_default();
        let (prefix, generation) = id
            .rsplit_once('-')
            .unwrap_or_else(|| panic!("case {id}: expected a <name>-<generation> power id"));
        assert_eq!(
            prefix, name,
            "case {id}: expected the id to carry the power's own name, got {prefix:?}"
        );
        assert!(
            !generation.is_empty() && generation.chars().all(|c| c.is_ascii_digit()),
            "case {id}: expected a numeric generation token, got {generation:?}"
        );
    }
}

/// The entry rule itself, over catalogs the published one does not have: rows differ
/// along how many powers the catalog holds, their order, and whether the power's name
/// is one this service accepts for a container service.
///
/// This is what tells "one entry per power, in catalog order, with `isActive` read
/// off whether the power can be specified" apart from "whatever the published catalog
/// happens to be, with `isActive` always true".
#[test]
fn one_entry_per_power_is_reported_in_the_catalogs_own_order() {
    /// A power in a catalog; only `name` decides whether it can be specified, so
    /// the figures vary freely.
    fn spec(name: &'static str, power_id: &'static str) -> ContainerPowerSpec {
        ContainerPowerSpec {
            name,
            power_id,
            price: 7.0,
            cpu_count: 0.25,
            ram_size_in_gb: 0.5,
        }
    }

    /// One row: the case name, the catalog, and the `(powerId, isActive)` pairs the
    /// entry list must carry, in order.
    struct Row {
        case: &'static str,
        catalog: Vec<ContainerPowerSpec>,
        expected: &'static [(&'static str, bool)],
    }

    let rows = [
        Row {
            case: "an empty catalog",
            catalog: vec![],
            expected: &[],
        },
        Row {
            case: "one power",
            catalog: vec![spec("nano", "nano-1")],
            expected: &[("nano-1", true)],
        },
        Row {
            case: "several powers",
            catalog: vec![spec("nano", "nano-1"), spec("xlarge", "xlarge-1")],
            expected: &[("nano-1", true), ("xlarge-1", true)],
        },
        Row {
            case: "the same powers in the other order",
            catalog: vec![spec("xlarge", "xlarge-1"), spec("nano", "nano-1")],
            expected: &[("xlarge-1", true), ("nano-1", true)],
        },
        Row {
            case: "a power name this service does not accept",
            catalog: vec![spec("pico", "pico-1"), spec("small", "small-1")],
            expected: &[("pico-1", false), ("small-1", true)],
        },
        Row {
            case: "a later generation of an accepted power",
            catalog: vec![spec("medium", "medium-2")],
            expected: &[("medium-2", true)],
        },
    ];

    for row in &rows {
        let case = row.case;
        let entries = container_service_powers_from(&row.catalog);
        assert_eq!(
            entries.len(),
            row.catalog.len(),
            "case {case:?}: expected one entry per power, got {entries:?}"
        );

        for (index, (power_id, is_active)) in row.expected.iter().enumerate() {
            let entry = &entries[index];
            assert_eq!(
                entry.power_id.as_deref(),
                Some(*power_id),
                "case {case:?} entry {index}: expected the power's own id in catalog order, \
                 got {entry:?}"
            );
            assert_eq!(
                entry.is_active,
                Some(*is_active),
                "case {case:?} entry {index}: expected isActive {is_active}, got {entry:?}"
            );
            let spec = &row.catalog[index];
            assert_eq!(
                entry.name.as_deref(),
                Some(spec.name),
                "case {case:?} entry {index}: expected the power's own name, got {entry:?}"
            );
            assert_eq!(
                (entry.price, entry.cpu_count, entry.ram_size_in_gb),
                (
                    Some(spec.price),
                    Some(spec.cpu_count),
                    Some(spec.ram_size_in_gb)
                ),
                "case {case:?} entry {index}: expected the catalog's own figures, got {entry:?}"
            );
        }
    }
}

/// `isActive` says what it claims: every power reported active is one
/// `CreateContainerService` accepts and reports back on the created service, and a
/// power the catalog does not publish is refused.
#[tokio::test]
async fn an_active_power_is_one_a_container_service_can_be_created_with() {
    let emulator = Emulator::new();
    let powers = reported(&emulator).await;

    for (power_id, power) in &powers {
        assert_eq!(
            power["isActive"].as_bool(),
            Some(true),
            "case {power_id}: this case covers the active powers, got {power:?}"
        );
        let name = power["name"]
            .as_str()
            .unwrap_or_else(|| panic!("case {power_id}: every power carries a name"));
        let service_name = format!("cs-{name}");

        let (status, created) = emulator
            .call(
                "CreateContainerService",
                json!({"serviceName": service_name, "power": name, "scale": 1}),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {power_id}: an active power must be one a create accepts, got {status} \
             {created:?}"
        );
        assert_eq!(
            created["containerService"]["power"].as_str(),
            Some(name),
            "case {power_id}: expected the created service to carry the power it was created \
             with, got {created:?}"
        );

        // Read back through the implemented reader: the power and its id are what
        // the catalog publishes for that name.
        let body = emulator
            .ok("GetContainerServices", json!({"serviceName": service_name}))
            .await;
        let service = &body["containerServices"][0];
        assert_eq!(
            service["power"].as_str(),
            Some(name),
            "case {power_id}: expected the stored service to report its power, got {service:?}"
        );
        assert_eq!(
            service["powerId"].as_str(),
            Some(power_id.as_str()),
            "case {power_id}: expected the stored service to report the catalog's power id, \
             got {service:?}"
        );
    }

    // Names the catalog does not publish: a generation id, a differently cased
    // name, an unpublished size and the empty string. None of them is a power a
    // create may name, so each is refused and creates nothing.
    for (index, power) in ["nano-1", "NANO", "pico", "gigantic", ""].iter().enumerate() {
        let service_name = format!("cs-refused-{index}");
        let (status, body) = emulator
            .call(
                "CreateContainerService",
                json!({"serviceName": service_name, "power": power, "scale": 1}),
            )
            .await;
        assert!(
            (400..500).contains(&status),
            "case power {power:?}: a power the catalog does not publish must be refused as \
             client input, got {status} {body:?}"
        );

        let listed = emulator
            .ok("GetContainerServices", json!({}))
            .await["containerServices"]
            .as_array()
            .map(|services| {
                services
                    .iter()
                    .filter_map(|service| service["containerServiceName"].as_str())
                    .any(|name| name == service_name)
            })
            .unwrap_or(false);
        assert!(
            !listed,
            "case power {power:?}: a refused create must store nothing, yet {service_name} \
             is listed"
        );
    }
}

/// Reporting the catalog is a read: it answers the same thing on every read,
/// whatever else the store holds, and two emulators publish the same catalog.
#[tokio::test]
async fn reporting_the_powers_changes_nothing_a_later_caller_sees() {
    let emulator = Emulator::new();

    // The input shape declares no member, so there is exactly one request form; what
    // varies across these reads is only WHEN they happen and what the store holds.
    let first = reported(&emulator).await;
    let again = reported(&emulator).await;
    assert_eq!(
        again, first,
        "case a repeated read: two reads of the catalog must agree, got {again:?} after {first:?}"
    );

    // A mutation of unrelated state, then the same read again: the published catalog
    // is not per-caller and does not depend on what the store holds.
    // `CreateContainerService` answers HTTP 201, its modelled success status.
    let (status, created) = emulator
        .call(
            "CreateContainerService",
            json!({"serviceName": "power-reader", "power": "micro", "scale": 2}),
        )
        .await;
    assert_eq!(
        status, 201,
        "case the unrelated mutation: expected HTTP 201, got {status} {created:?}"
    );

    let after_mutation = reported(&emulator).await;
    assert_eq!(
        after_mutation, first,
        "case after an unrelated mutation: the published catalog must not depend on what the \
         store holds, got {after_mutation:?} after {first:?}"
    );

    // The container service the mutation created is still there: reading the catalog
    // consumed nothing.
    let service = emulator
        .ok("GetContainerServices", json!({"serviceName": "power-reader"}))
        .await;
    assert_eq!(
        service["containerServices"][0]["containerServiceName"].as_str(),
        Some("power-reader"),
        "case store unchanged: expected the created container service to survive the reads, \
         got {service:?}"
    );

    // A second emulator, with its own store, publishes the same catalog, and the
    // in-process accessor agrees with what the wire reports.
    let other = Emulator::new();
    let elsewhere = reported(&other).await;
    assert_eq!(
        elsewhere, first,
        "case a second emulator: the catalog must not depend on which emulator serves it, \
         got {elsewhere:?}"
    );

    let in_process: BTreeSet<String> = container_service_powers()
        .into_iter()
        .map(|power| power.power_id.unwrap_or_default())
        .collect();
    let on_the_wire: BTreeSet<String> = first.keys().cloned().collect();
    assert_eq!(
        in_process, on_the_wire,
        "case the accessor and the wire: expected the same catalog, got {in_process:?}"
    );
}

/// The hours AWS prices one Lightsail monthly plan at: a monthly plan is billed by
/// the hour at `price / 730`, so a window of 730 hours is exactly one plan's worth.
const BILLING_MONTH_HOURS: f64 = 730.0;

/// How far after the call the estimated window starts, so the service under test
/// already exists for the whole of it and the figures are exact.
const WINDOW_STARTS_IN_SECONDS: f64 = 86_400.0;

/// Now, in epoch seconds.
fn now_epoch_seconds() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs_f64())
        .unwrap_or_default()
}

/// RULE: every figure a reported power carries is that plan's OWN published one —
/// no entry carries a neighbouring plan's RAM, vCPUs or price — and the price it
/// publishes is the price every operation that charges for that power charges.
///
/// This is the case a table built by stepping one column past its own row cannot
/// pass: the rows differ along what the rule generalises over (the plan, and so its
/// vCPU count, its RAM and its price), and two plans share a RAM figure while
/// differing in vCPUs and price, so no entry follows from the one beside it. The
/// price is then driven through a SECOND operation, `GetCostEstimate`, which bills
/// a container service at the catalog's price for the power it runs on.
#[tokio::test]
async fn each_power_carries_its_own_figures_and_is_billed_at_its_own_price() {
    let emulator = Emulator::new();
    let powers = reported(&emulator).await;

    // The published RAM ladder, read off the rows above: it is NOT one figure per
    // step — `micro` and `small` both carry 1 GB — so a catalog whose entries each
    // carried the next plan's RAM would differ here on every tier above nano.
    let reported_ram: Vec<(&str, Option<f64>)> = PUBLISHED_PLANS
        .iter()
        .map(|(_, power_id, ..)| {
            (
                *power_id,
                powers
                    .get(*power_id)
                    .and_then(|power| power["ramSizeInGb"].as_f64()),
            )
        })
        .collect();
    let published_ram: Vec<(&str, Option<f64>)> = PUBLISHED_PLANS
        .iter()
        .map(|(_, power_id, _, ram_size_in_gb, _)| (*power_id, Some(*ram_size_in_gb)))
        .collect();
    assert!(
        reported_ram
            .iter()
            .zip(published_ram.iter())
            .all(|((_, reported), (_, published))| match (reported, published) {
                (Some(reported), Some(published)) => (reported - published).abs() < 0.005,
                _ => false,
            }),
        "case the published RAM ladder: expected {published_ram:?}, got {reported_ram:?}"
    );

    // Two plans that share a RAM figure must still differ in the figures that do
    // distinguish them, so a reader cannot infer one plan from another.
    let micro = powers
        .get("micro-1")
        .unwrap_or_else(|| panic!("case micro-1: expected the plan to be reported"));
    let small = powers
        .get("small-1")
        .unwrap_or_else(|| panic!("case small-1: expected the plan to be reported"));
    assert!(
        is_close(micro["ramSizeInGb"].as_f64(), 1.0) && is_close(small["ramSizeInGb"].as_f64(), 1.0),
        "case micro-1 and small-1: both plans publish 1 GB of RAM, got {micro:?} and {small:?}"
    );
    assert!(
        micro["cpuCount"].as_f64() < small["cpuCount"].as_f64()
            && micro["price"].as_f64() < small["price"].as_f64(),
        "case micro-1 and small-1: the dearer of two plans with the same RAM must carry more \
         vCPUs, got {micro:?} and {small:?}"
    );

    // One billing month of each power, estimated through the operation that prices
    // it: the charge is the catalog's price for that power, times the nodes the
    // service runs.
    let start = now_epoch_seconds() + WINDOW_STARTS_IN_SECONDS;
    let end = start + BILLING_MONTH_HOURS * 3_600.0;
    let scale = 2;

    for (name, power_id, _, _, price) in PUBLISHED_PLANS {
        let service_name = format!("billed-{name}");
        let (status, created) = emulator
            .call(
                "CreateContainerService",
                json!({"serviceName": service_name, "power": name, "scale": scale}),
            )
            .await;
        assert_eq!(
            status, 201,
            "case {power_id}: expected the create to be accepted, got {status} {created:?}"
        );

        let body = emulator
            .ok(
                "GetCostEstimate",
                json!({"resourceName": service_name, "startTime": start, "endTime": end}),
            )
            .await;
        let charge = &body["resourcesBudgetEstimate"][0]["costEstimates"][0];
        assert_eq!(
            charge["usageType"].as_str(),
            Some(format!("PowerUsage:{power_id}").as_str()),
            "case {power_id}: expected the charge to name the power it is for, got {charge:?}"
        );
        let expected = price * f64::from(scale);
        assert!(
            charge["resultsByTime"][0]["usageCost"]
                .as_f64()
                .is_some_and(|cost| (cost - expected).abs() < 1e-6),
            "case {power_id}: expected ${expected} for {scale} nodes over a billing month, \
             got {charge:?}"
        );
    }
}
