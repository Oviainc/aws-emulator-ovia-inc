// SPDX-License-Identifier: CC0-1.0

//! The parent summary a listed catalog record carries — the `Associated*Summary` the
//! ontology listings project out of the stored parent.
//!
//! The rule is not about one listing or one member: wherever a listing reports the
//! parent a record belongs to, the summary's `Arn` is the ARN the CHILD stores, reported
//! exactly as stored, and its `Name` is the stored parent's name, reported only when
//! that ARN resolves in the collection the parent's own list operation reports (both
//! members are optional on every modeled `Associated*Summary`:
//! `smithy_query(query="shape",shape="AssociatedDomainSummary,AssociatedObjectiveSummary")`).
//!
//! Each test is therefore a TABLE over every parent summary the service reports — the
//! domain `ListObjectives` carries, and the domain and the objective
//! `ListCommonControls` carries — generated from the persisted catalog rather than from
//! one record picked by hand, driven over the shared HTTP harness in
//! `control_catalog.rs`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;
use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    CommonControl, ComAmazonawsControlcatalogControlcatalogState as Catalog, Objective,
};

use crate::control_catalog::{drain, Emulator};

/// One parent summary a listing carries: the route, the member its items come back in,
/// the summary member carrying the parent, and — per listed child ARN — the parent ARN
/// the child stores together with that parent's stored name.
struct ParentProjection {
    route: &'static str,
    items_key: &'static str,
    member: &'static str,
    /// child ARN -> (the parent ARN the child stores, the stored parent's name).
    parents: BTreeMap<String, (String, String)>,
}

/// Every parent summary the ontology listings report, read out of the persisted catalog.
fn parent_projections(catalog: &Catalog) -> Vec<ParentProjection> {
    let domain_name = |arn: &String| {
        catalog
            .domains
            .get(arn)
            .unwrap_or_else(|| panic!("case domain {arn}: the catalog holds it"))
            .name
            .clone()
    };
    let objective_name = |arn: &String| {
        catalog
            .objectives
            .get(arn)
            .unwrap_or_else(|| panic!("case objective {arn}: the catalog holds it"))
            .name
            .clone()
    };
    vec![
        ParentProjection {
            route: "/objectives",
            items_key: "Objectives",
            member: "Domain",
            parents: catalog
                .objectives
                .iter()
                .map(|(arn, objective)| {
                    (
                        arn.clone(),
                        (
                            objective.domain_arn.clone(),
                            domain_name(&objective.domain_arn),
                        ),
                    )
                })
                .collect(),
        },
        ParentProjection {
            route: "/common-controls",
            items_key: "CommonControls",
            member: "Domain",
            parents: catalog
                .common_controls
                .iter()
                .map(|(arn, common_control)| {
                    (
                        arn.clone(),
                        (
                            common_control.domain_arn.clone(),
                            domain_name(&common_control.domain_arn),
                        ),
                    )
                })
                .collect(),
        },
        ParentProjection {
            route: "/common-controls",
            items_key: "CommonControls",
            member: "Objective",
            parents: catalog
                .common_controls
                .iter()
                .map(|(arn, common_control)| {
                    (
                        arn.clone(),
                        (
                            common_control.objective_arn.clone(),
                            objective_name(&common_control.objective_arn),
                        ),
                    )
                })
                .collect(),
        },
    ]
}

/// One row of the unresolvable-parent table: a child the listing reports, the parent
/// member under test, and what that member must carry.
struct UnresolvedCase<'a> {
    case: &'a str,
    listing: &'a [Value],
    child: &'a str,
    member: &'a str,
    expected_arn: &'a str,
    /// `None` where the stored parent ARN resolves to no record.
    expected_name: Option<&'a str>,
}

/// A catalog ARN of `kind` carrying an identifier the published catalog does not hold,
/// shaped like the modeled pattern for that kind
/// (`^arn:(aws(?:[-a-z]*)?):controlcatalog:::<kind>/[0-9a-z]+$`, from
/// `smithy_query(query="shape",shape="DomainArn,ObjectiveArn,CommonControlArn")`).
fn unpublished_arn(kind: &str, identifier: &str) -> String {
    format!("arn:aws:controlcatalog:::{kind}/{identifier}")
}

/// Every listing that carries a parent summary reports the ARN its child stores and the
/// stored parent's name, for every record the catalog holds.
#[tokio::test]
async fn every_listed_record_reports_the_stored_parent_it_belongs_to() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();

    for projection in parent_projections(&catalog) {
        // Page size 2, so the projection is exercised on every page of the walk and not
        // only on a single unpaginated one.
        let listed = drain(
            &emulator,
            projection.route,
            projection.items_key,
            Some(2),
            "{}",
        )
        .await;
        assert_eq!(
            listed.len(),
            projection.parents.len(),
            "case {} {}: expected the {} stored records, got {} ({listed:?})",
            projection.route,
            projection.member,
            projection.parents.len(),
            listed.len()
        );
        for entry in &listed {
            let child = entry
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {entry}: a listed record carries its ARN"));
            let (parent_arn, parent_name) = projection
                .parents
                .get(child)
                .unwrap_or_else(|| panic!("case {child}: the catalog does not hold it"));
            assert_eq!(
                entry
                    .get(projection.member)
                    .and_then(|parent| parent.get("Arn"))
                    .and_then(Value::as_str),
                Some(parent_arn.as_str()),
                "case {} {child}: expected {}.Arn {parent_arn:?}, got {entry}",
                projection.route,
                projection.member
            );
            assert_eq!(
                entry
                    .get(projection.member)
                    .and_then(|parent| parent.get("Name"))
                    .and_then(Value::as_str),
                Some(parent_name.as_str()),
                "case {} {child}: expected {}.Name {parent_name:?}, got {entry}",
                projection.route,
                projection.member
            );
        }
    }
}

/// Two listings that report the same parent describe it the same way: a domain ARN that
/// comes back from both `ListObjectives` and `ListCommonControls` carries one name, so a
/// client joining domain -> objective -> common control joins on consistent parents.
#[tokio::test]
async fn two_listings_describe_a_shared_parent_identically() {
    let emulator = Emulator::new();

    let objectives = drain(&emulator, "/objectives", "Objectives", None, "{}").await;
    let common_controls = drain(&emulator, "/common-controls", "CommonControls", None, "{}").await;

    let from_objectives = parent_names(&objectives, "Domain");
    let from_common_controls = parent_names(&common_controls, "Domain");
    let shared: BTreeSet<&String> = from_objectives
        .keys()
        .filter(|arn| from_common_controls.contains_key(*arn))
        .collect();
    assert!(
        !shared.is_empty(),
        "case Domain: the two listings must report domains in common, got {from_objectives:?} \
         and {from_common_controls:?}"
    );
    for arn in shared {
        assert_eq!(
            from_objectives.get(arn),
            from_common_controls.get(arn),
            "case domain {arn}: ListObjectives reports Name {:?} and ListCommonControls {:?}",
            from_objectives.get(arn),
            from_common_controls.get(arn)
        );
    }
}

/// A record whose stored parent ARN the catalog does not hold still reports that ARN,
/// and leaves the optional `Name` out rather than failing the listing — driven through
/// both listings and all three parent members.
#[tokio::test]
async fn a_parent_the_catalog_does_not_hold_is_reported_by_arn_without_a_name() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let emulator = Emulator::with_engine(&engine);

    // Identifiers no published record carries: the seed derives every published one by
    // hashing, so none of them is a run of a single digit.
    let dangling_domain = unpublished_arn("domain", "1111111111111111111111111");
    let dangling_objective = unpublished_arn("objective", "2222222222222222222222222");
    let orphan_objective = unpublished_arn("objective", "3333333333333333333333333");
    let orphan_both = unpublished_arn("common-control", "4444444444444444444444444");
    let orphan_domain_only = unpublished_arn("common-control", "5555555555555555555555555");

    // One published objective, so the row below that keeps its objective resolvable has
    // a parent to resolve.
    let published_objective = {
        let state = engine.state::<Catalog>();
        let state = state.lock();
        state
            .objectives
            .keys()
            .next()
            .expect("the seeded catalog publishes objectives")
            .clone()
    };
    let published_objective_name = {
        let state = engine.state::<Catalog>();
        let state = state.lock();
        state.objectives[&published_objective].name.clone()
    };

    {
        let state = engine.state::<Catalog>();
        let mut state = state.lock();
        state.objectives.insert(
            orphan_objective.clone(),
            Objective {
                arn: orphan_objective.clone(),
                name: "An objective whose domain is not in the catalog".to_string(),
                description: "Its stored domain ARN resolves to no domain.".to_string(),
                domain_arn: dangling_domain.clone(),
                create_time: 1_700_000_000,
                last_update_time: 1_700_000_000,
            },
        );
        state.common_controls.insert(
            orphan_both.clone(),
            CommonControl {
                arn: orphan_both.clone(),
                name: "A common control whose parents are not in the catalog".to_string(),
                description: "Neither stored parent ARN resolves.".to_string(),
                objective_arn: dangling_objective.clone(),
                domain_arn: dangling_domain.clone(),
                create_time: 1_700_000_000,
                last_update_time: 1_700_000_000,
            },
        );
        state.common_controls.insert(
            orphan_domain_only.clone(),
            CommonControl {
                arn: orphan_domain_only.clone(),
                name: "A common control whose domain alone is not in the catalog".to_string(),
                description: "Its objective resolves; its domain does not.".to_string(),
                objective_arn: published_objective.clone(),
                domain_arn: dangling_domain.clone(),
                create_time: 1_700_000_000,
                last_update_time: 1_700_000_000,
            },
        );
    }

    let objectives = drain(&emulator, "/objectives", "Objectives", Some(3), "{}").await;
    let common_controls =
        drain(&emulator, "/common-controls", "CommonControls", Some(3), "{}").await;

    let rows = [
        UnresolvedCase {
            case: "an objective whose domain does not resolve",
            listing: &objectives,
            child: &orphan_objective,
            member: "Domain",
            expected_arn: &dangling_domain,
            expected_name: None,
        },
        UnresolvedCase {
            case: "a common control whose domain does not resolve",
            listing: &common_controls,
            child: &orphan_both,
            member: "Domain",
            expected_arn: &dangling_domain,
            expected_name: None,
        },
        UnresolvedCase {
            case: "a common control whose objective does not resolve",
            listing: &common_controls,
            child: &orphan_both,
            member: "Objective",
            expected_arn: &dangling_objective,
            expected_name: None,
        },
        UnresolvedCase {
            case: "a common control whose objective resolves and whose domain does not",
            listing: &common_controls,
            child: &orphan_domain_only,
            member: "Objective",
            expected_arn: &published_objective,
            expected_name: Some(published_objective_name.as_str()),
        },
    ];

    for row in rows {
        let entry = row
            .listing
            .iter()
            .find(|item| item.get("Arn").and_then(Value::as_str) == Some(row.child))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the listing does not report {}",
                    row.case, row.child
                )
            });
        assert_eq!(
            entry
                .get(row.member)
                .and_then(|parent| parent.get("Arn"))
                .and_then(Value::as_str),
            Some(row.expected_arn),
            "case {}: expected {}.Arn {:?}, got {entry}",
            row.case,
            row.member,
            row.expected_arn
        );
        assert_eq!(
            entry
                .get(row.member)
                .and_then(|parent| parent.get("Name"))
                .and_then(Value::as_str),
            row.expected_name,
            "case {}: expected {}.Name {:?}, got {entry}",
            row.case,
            row.member,
            row.expected_name
        );
    }
}

/// The name each listing reports for every parent ARN it carries at `member`.
fn parent_names(items: &[Value], member: &str) -> BTreeMap<String, Option<String>> {
    items
        .iter()
        .map(|item| {
            let parent = item
                .get(member)
                .unwrap_or_else(|| panic!("case {item}: it carries no {member} summary"));
            let arn = parent
                .get("Arn")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {item}: its {member} summary carries no ARN"))
                .to_string();
            let name = parent
                .get("Name")
                .and_then(Value::as_str)
                .map(str::to_string);
            (arn, name)
        })
        .collect()
}
