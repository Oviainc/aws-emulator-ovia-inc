// SPDX-License-Identifier: CC0-1.0

//! The top of the Control Catalog ontology: what `ListDomains` and `ListObjectives`
//! report, and where those records come from.
//!
//! The rule under test is not about one domain or one objective: EVERY record the
//! catalog holds must come back out of the listing, projected member for member from
//! the stored record (an objective carrying the stored domain it belongs to), under
//! every page size and whether or not the domain filter is set, so a client walking
//! domain -> objective -> common control reaches the whole ontology. Each test is
//! therefore a table generated from the persisted catalog rather than from one entry
//! picked by hand, and the HTTP harness is the shared one in `control_catalog.rs`.

use std::collections::BTreeSet;

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;

use crate::control_catalog::{by_arn, drain, Emulator};

/// The control domains Amazon Web Services publishes at the top of the Control Catalog
/// ontology, which `src/state.rs` seeds by name.
///
/// Source: the published Control Catalog domain listing, as quoted in this service's
/// issue corpus ("Asset management", "Business continuity and recovery", "Data
/// protection", "Identity and access management", ...) and in the common-control
/// records that name "Secure development lifecycle and change management". The last
/// three rows are domains the ListDomains issue did not cite.
const PUBLISHED_DOMAIN_NAMES: [&str; 6] = [
    "Asset management",
    "Business continuity and recovery",
    "Data protection",
    "Identity and access management",
    "Logging and monitoring",
    "Secure development lifecycle and change management",
];

/// The three ontology listings, and the member each carries its items in.
const ONTOLOGY_LISTINGS: [(&str, &str); 3] = [
    ("/domains", "Domains"),
    ("/objectives", "Objectives"),
    ("/common-controls", "CommonControls"),
];

/// The page sizes a listing is walked with. `None` sends no `MaxResults` at all; the
/// rest span one item per page up to the maximum the modeled `MaxList*Results` range
/// (1..100) allows, so the same set must come back however the walk is cut up.
const PAGE_SIZES: [Option<u32>; 4] = [None, Some(1), Some(3), Some(100)];

/// A timestamp member of a listed record, in seconds since the Unix epoch.
fn epoch_seconds(entry: &Value, member: &str) -> f64 {
    entry
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {entry}: {member} is missing or not a timestamp"))
}

/// The entry of `items` whose `Arn` is `arn`.
fn entry_for<'a>(items: &'a [Value], arn: &str) -> &'a Value {
    items
        .iter()
        .find(|item| item.get("Arn").and_then(Value::as_str) == Some(arn))
        .unwrap_or_else(|| {
            panic!("case {arn}: the listing does not report it; it returned {items:?}")
        })
}

/// `ListDomains` reports every domain the catalog holds, each projected from the
/// stored record, under every page size.
#[tokio::test]
async fn the_domain_listing_reports_every_stored_domain() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for page_size in PAGE_SIZES {
        let listed = drain(&emulator, "/domains", "Domains", page_size, "{}").await;
        assert!(
            !listed.is_empty(),
            "case maxResults={page_size:?}: the ontology must be browsable, but ListDomains \
             returned no domains"
        );
        assert_eq!(
            listed.len(),
            catalog.domains.len(),
            "case maxResults={page_size:?}: expected the {} stored domains, got {} ({listed:?})",
            catalog.domains.len(),
            listed.len()
        );
        for (arn, domain) in &catalog.domains {
            let entry = entry_for(&listed, arn);
            assert_eq!(
                entry.get("Name").and_then(Value::as_str),
                Some(domain.name.as_str()),
                "case domain {arn} maxResults={page_size:?}: expected Name {:?}, got {entry}",
                domain.name
            );
            assert_eq!(
                entry.get("Description").and_then(Value::as_str),
                Some(domain.description.as_str()),
                "case domain {arn} maxResults={page_size:?}: expected the stored Description, \
                 got {entry}"
            );
            assert_eq!(
                epoch_seconds(entry, "CreateTime"),
                domain.create_time as f64,
                "case domain {arn} maxResults={page_size:?}: expected CreateTime {}, got {entry}",
                domain.create_time
            );
        }
    }
}

/// Every domain Amazon Web Services publishes is served, exactly once, under its
/// published name.
///
/// This row pins the catalog's COVERAGE of the published taxonomy (that the seed in
/// `src/state.rs` holds each published domain once), not handler behaviour: the
/// behavioural rule — every stored record projected into the listing, under every page
/// size and filter form, with `LastUpdateTime` equal to `CreateTime` — is what the
/// other tables in this suite assert.
#[tokio::test]
async fn every_published_domain_name_is_served_exactly_once() {
    let emulator = Emulator::new();
    let listed = drain(&emulator, "/domains", "Domains", None, "{}").await;

    for name in PUBLISHED_DOMAIN_NAMES {
        let matching = listed
            .iter()
            .filter(|entry| entry.get("Name").and_then(Value::as_str) == Some(name))
            .count();
        assert_eq!(
            matching, 1,
            "case published domain {name:?}: expected exactly one entry, got {matching} \
             in {listed:?}"
        );
    }
}

/// `ListObjectives` reports every objective the catalog holds, each carrying the
/// stored domain it belongs to, whether the domain filter is set or not and whatever
/// the page size.
#[tokio::test]
async fn the_objective_listing_reports_every_stored_objective_with_its_domain() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    let unfiltered = drain(&emulator, "/objectives", "Objectives", None, "{}").await;
    assert_eq!(
        unfiltered.len(),
        catalog.objectives.len(),
        "expected the {} stored objectives, got {} ({unfiltered:?})",
        catalog.objectives.len(),
        unfiltered.len()
    );

    for (arn, objective) in &catalog.objectives {
        let entry = entry_for(&unfiltered, arn);
        let domain = catalog
            .domains
            .get(&objective.domain_arn)
            .unwrap_or_else(|| panic!("case objective {arn}: its stored domain is in the catalog"));
        assert_eq!(
            entry.get("Name").and_then(Value::as_str),
            Some(objective.name.as_str()),
            "case objective {arn}: expected Name {:?}, got {entry}",
            objective.name
        );
        assert_eq!(
            entry.get("Description").and_then(Value::as_str),
            Some(objective.description.as_str()),
            "case objective {arn}: expected the stored Description, got {entry}"
        );
        assert_eq!(
            entry.pointer("/Domain/Arn").and_then(Value::as_str),
            Some(domain.arn.as_str()),
            "case objective {arn}: expected Domain.Arn {:?}, got {entry}",
            domain.arn
        );
        assert_eq!(
            entry.pointer("/Domain/Name").and_then(Value::as_str),
            Some(domain.name.as_str()),
            "case objective {arn}: expected Domain.Name {:?}, got {entry}",
            domain.name
        );
        assert_eq!(
            epoch_seconds(entry, "CreateTime"),
            objective.create_time as f64,
            "case objective {arn}: expected CreateTime {}, got {entry}",
            objective.create_time
        );
    }

    // The same set must come back however the walk is cut up, and however the request
    // names the (single, optional) filter dimension.
    let filter_forms = [
        "{}".to_string(),
        r#"{"ObjectiveFilter":{}}"#.to_string(),
        r#"{"ObjectiveFilter":{"Domains":[]}}"#.to_string(),
    ];
    for body in &filter_forms {
        for page_size in PAGE_SIZES {
            let listed = drain(&emulator, "/objectives", "Objectives", page_size, body).await;
            assert_eq!(
                by_arn(listed.clone()),
                by_arn(unfiltered.clone()),
                "case body {body} maxResults={page_size:?}: expected the whole objective list, \
                 got {listed:?}"
            );
        }
    }
}

/// A catalog record that has never been revised reports `LastUpdateTime` equal to its
/// `CreateTime`: the member is documented as the time the record was most recently
/// updated, and this service has no API that revises one.
#[tokio::test]
async fn an_unrevised_record_reports_lastupdatetime_equal_to_createtime() {
    let emulator = Emulator::new();

    for (path, items_key) in ONTOLOGY_LISTINGS {
        let listed = drain(&emulator, path, items_key, None, "{}").await;
        assert!(
            !listed.is_empty(),
            "case {path}: the listing must report the catalog's records, got none"
        );
        for entry in &listed {
            let created = epoch_seconds(entry, "CreateTime");
            let updated = epoch_seconds(entry, "LastUpdateTime");
            assert!(
                created > 0.0,
                "case {path} {}: CreateTime must be a real instant, got {created}",
                entry["Arn"]
            );
            assert_eq!(
                updated, created,
                "case {path} {}: an unrevised record must report LastUpdateTime {created}, \
                 got {updated}",
                entry["Arn"]
            );
        }
    }
}

/// The ontology is walkable from the top: every domain the listing reports has
/// objectives, every objective has common controls, and walking the whole tree reaches
/// every objective and every common control the unfiltered listings report.
#[tokio::test]
async fn the_walk_from_every_domain_reaches_the_whole_ontology() {
    let emulator = Emulator::new();

    let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;
    let all_objectives: BTreeSet<String> = arns(&drain(
        &emulator,
        "/objectives",
        "Objectives",
        None,
        "{}",
    )
    .await);
    let all_common_controls: BTreeSet<String> = arns(&drain(
        &emulator,
        "/common-controls",
        "CommonControls",
        None,
        "{}",
    )
    .await);

    let mut reached_objectives: BTreeSet<String> = BTreeSet::new();
    let mut reached_common_controls: BTreeSet<String> = BTreeSet::new();

    for domain in &domains {
        let domain_arn = domain["Arn"].as_str().expect("a domain carries its ARN");
        let body = format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{domain_arn}"}}]}}}}"#);
        let objectives = drain(&emulator, "/objectives", "Objectives", Some(2), &body).await;
        assert!(
            !objectives.is_empty(),
            "case domain {domain_arn}: a published domain must have objectives under it"
        );
        for objective in &objectives {
            let objective_arn = objective["Arn"]
                .as_str()
                .expect("an objective carries its ARN");
            reached_objectives.insert(objective_arn.to_string());
            let body =
                format!(r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{objective_arn}"}}]}}}}"#);
            let common_controls =
                drain(&emulator, "/common-controls", "CommonControls", Some(2), &body).await;
            assert!(
                !common_controls.is_empty(),
                "case objective {objective_arn}: an objective must have common controls under it"
            );
            reached_common_controls.extend(arns(&common_controls));
        }
    }

    assert_eq!(
        reached_objectives, all_objectives,
        "case walk domain -> objective: the walk must reach every objective the listing reports"
    );
    assert_eq!(
        reached_common_controls, all_common_controls,
        "case walk objective -> common control: the walk must reach every common control the \
         listing reports"
    );
}

/// The `Arn` of every item in a listing.
fn arns(items: &[Value]) -> BTreeSet<String> {
    items
        .iter()
        .map(|item| {
            item.get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {item}: an entry came back without an ARN"))
                .to_string()
        })
        .collect()
}
