// SPDX-License-Identifier: CC0-1.0

//! The single-ARN resource-filter rule, driven through BOTH operations that take such
//! a filter.
//!
//! The rule under test is one rule for the class, not one per operation: a resource
//! filter list carries at most one entry, that entry must name the resource's ARN, a
//! list that carries neither (absent or empty) narrows nothing, and anything else is
//! refused with the service's one fixed invalid-filter sentence. `ListObjectives`
//! (`ObjectiveFilter.Domains`) and `ListCommonControls`
//! (`CommonControlFilter.Objectives`) both answer by it, so every row below is run
//! against both filters and the two are compared with each other.
//!
//! Each test is a TABLE whose rows vary what the rule generalises over — the operation
//! and filter member, the number of entries, whether an entry names a resource at all,
//! whether the named resource is published, and the partition the ARN is written in —
//! rather than one filter picked by hand.
//!
//! The harness (`Emulator`, `drain`, `by_arn`) is the one `control_catalog.rs` already
//! drives the service through: the same `MultiServiceRouter` front door.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;

use crate::control_catalog::{by_arn, drain, error_code, Emulator};
use crate::validation_messages::message_of;

/// The sentence this service answers a filter it does not accept with, on every
/// operation that takes one.
const INVALID_FILTER: &str =
    "The provided filter is not valid. Ensure that each part of the filter is valid, and try again.";

/// An identifier of the length the catalog's own ARNs carry that nothing is published
/// under, in the `[0-9a-z]` class the modeled ARN patterns admit.
const UNPUBLISHED_IDENTIFIER: &str = "zzzzzzzzzzzzzzzzzzzzzzzzz";

/// A partition other than the one the catalog stores its ARNs in. Source: the modeled
/// `DomainArn` / `ObjectiveArn` patterns, whose partition alternative
/// (`aws(?:[-a-z]*)?`) admits the whole `aws` family.
const OTHER_PARTITION: &str = "aws-us-gov";

/// One of the service's single-ARN resource filters: the operation that takes it, the
/// members it is spelled with, and the resource kind its entry names.
struct SingleArnFilter {
    /// Route of the operation the filter belongs to.
    route: &'static str,
    /// Response member carrying that operation's items.
    items_key: &'static str,
    /// The input member holding the filter structure.
    filter_member: &'static str,
    /// The filter structure's list member, holding the resource entries.
    list_member: &'static str,
    /// The member each returned item reports the filtered-by resource under.
    parent_member: &'static str,
    /// The ARN kind segment the entry's `Arn` names.
    kind: &'static str,
    /// Published ARNs of that kind, straight out of the catalog the service serves.
    published: Vec<String>,
}

impl SingleArnFilter {
    /// A request body whose filter list holds `entries` (each already JSON).
    fn body(&self, entries: &[String]) -> String {
        format!(
            r#"{{"{}":{{"{}":[{}]}}}}"#,
            self.filter_member,
            self.list_member,
            entries.join(",")
        )
    }

    /// An ARN of this filter's kind carrying `identifier`, written in `partition`.
    fn arn_in(&self, partition: &str, identifier: &str) -> String {
        format!("arn:{partition}:controlcatalog:::{}/{identifier}", self.kind)
    }
}

/// A filter entry naming `arn`. Both entry shapes carry the same single optional
/// `Arn` member
/// (`smithy_query(query="shape",shape="DomainResourceFilter,ObjectiveResourceFilter")`),
/// so one builder spells either.
fn entry(arn: &str) -> String {
    format!(r#"{{"Arn":"{arn}"}}"#)
}

/// An entry naming no resource: `Arn` is optional on both entry shapes, so such a body
/// passes the constraint walk and reaches the handler.
fn nameless_entry() -> String {
    "{}".to_string()
}

/// Both single-ARN resource filters the service declares, with the published ARNs each
/// one's entry can name.
fn filters() -> Vec<SingleArnFilter> {
    let catalog = Catalog::default();
    let domains: Vec<String> = catalog.domains.keys().cloned().collect();
    let objectives: Vec<String> = catalog.objectives.keys().cloned().collect();
    assert!(
        domains.len() >= 2 && objectives.len() >= 2,
        "case catalog: the catalog must publish several domains and objectives for these \
         tables, got {} domains and {} objectives",
        domains.len(),
        objectives.len()
    );
    vec![
        SingleArnFilter {
            route: "/objectives",
            items_key: "Objectives",
            filter_member: "ObjectiveFilter",
            list_member: "Domains",
            parent_member: "Domain",
            kind: "domain",
            published: domains,
        },
        SingleArnFilter {
            route: "/common-controls",
            items_key: "CommonControls",
            filter_member: "CommonControlFilter",
            list_member: "Objectives",
            parent_member: "Objective",
            kind: "objective",
            published: objectives,
        },
    ]
}

/// A filter list that names no resource narrows nothing, and one naming a published
/// resource narrows to exactly that resource's entries — on both operations, and
/// whichever partition the ARN is written in.
#[tokio::test]
async fn an_accepted_single_entry_filter_narrows_both_lists_the_same_way() {
    let emulator = Emulator::new();

    for filter in filters() {
        let unfiltered = drain(&emulator, filter.route, filter.items_key, None, "{}").await;
        assert!(
            !unfiltered.is_empty(),
            "case {}: the catalog must publish items for this table",
            filter.route
        );

        // (case, body). A filter list that carries no entry names no resource to
        // narrow by, however the request spells it.
        let narrows_nothing: Vec<(String, String)> = vec![
            (
                format!("{} with no filter at all", filter.route),
                "{}".to_string(),
            ),
            (
                format!("{} with a filter carrying no list", filter.route),
                format!(r#"{{"{}":{{}}}}"#, filter.filter_member),
            ),
            (
                format!("{} with an empty filter list", filter.route),
                filter.body(&[]),
            ),
        ];
        for (case, body) in narrows_nothing {
            // Page size 1, so the filter and the cursor it fingerprints are exercised
            // together.
            let listed = drain(&emulator, filter.route, filter.items_key, Some(1), &body).await;
            assert_eq!(
                by_arn(listed.clone()),
                by_arn(unfiltered.clone()),
                "case {case:?}: expected the whole {} list of {} items, got {listed:?}",
                filter.items_key,
                unfiltered.len()
            );
        }

        // One entry naming a published resource: every published one, in the partition
        // the catalog stores and in another the modeled pattern admits.
        let mut union: Vec<Value> = Vec::new();
        for parent in &filter.published {
            let identifier = parent
                .rsplit('/')
                .next()
                .unwrap_or_else(|| panic!("case {parent:?}: a catalog ARN carries an identifier"));
            let mut narrowed: Option<Vec<Value>> = None;
            for partition in ["aws", OTHER_PARTITION] {
                let requested = filter.arn_in(partition, identifier);
                let case = format!("{} narrowed to {requested}", filter.route);
                let body = filter.body(&[entry(&requested)]);
                let listed =
                    drain(&emulator, filter.route, filter.items_key, Some(2), &body).await;
                assert!(
                    !listed.is_empty(),
                    "case {case:?}: every published {} has items under it, got none",
                    filter.kind
                );
                for item in &listed {
                    assert_eq!(
                        item[filter.parent_member]["Arn"].as_str(),
                        Some(parent.as_str()),
                        "case {case:?}: item {} reports a different {}",
                        item["Arn"],
                        filter.parent_member
                    );
                }
                match &narrowed {
                    None => narrowed = Some(listed),
                    Some(first) => assert_eq!(
                        &listed, first,
                        "case {case:?}: every partition's spelling must narrow to the same \
                         {} items, got {listed:?}",
                        filter.items_key
                    ),
                }
            }
            union.extend(narrowed.expect("each parent is narrowed at least once"));
        }
        assert_eq!(
            by_arn(union),
            by_arn(unfiltered.clone()),
            "case {}: the per-{} filters must partition the unfiltered list",
            filter.route,
            filter.kind
        );

        // One entry naming an identifier the catalog publishes nowhere narrows to
        // nothing: it is an accepted filter, not a refused one.
        for partition in ["aws", OTHER_PARTITION] {
            let requested = filter.arn_in(partition, UNPUBLISHED_IDENTIFIER);
            let case = format!("{} narrowed to the unpublished {requested}", filter.route);
            let body = filter.body(&[entry(&requested)]);
            let response = emulator.ok(filter.route, &body).await;
            assert_eq!(
                response[filter.items_key],
                json!([]),
                "case {case:?}: expected an empty {} list, got {response:?}",
                filter.items_key
            );
        }
    }
}

/// A filter list outside the rule — more than one entry, or an entry naming no
/// resource — is refused with the one fixed sentence, identically on both operations.
#[tokio::test]
async fn an_unacceptable_single_entry_filter_is_refused_identically_on_both_operations() {
    let emulator = Emulator::new();
    let mut sentences: BTreeSet<String> = BTreeSet::new();

    for filter in filters() {
        let first = entry(&filter.published[0]);
        let second = entry(&filter.published[1]);
        let unpublished = entry(&filter.arn_in("aws", UNPUBLISHED_IDENTIFIER));
        let nameless = nameless_entry();

        // (case, entries). The rows vary the entry count, whether the entries name
        // published resources, whether they name the same one twice, and whether an
        // entry names a resource at all.
        let rows: Vec<(String, Vec<String>)> = vec![
            ("two published resources".to_string(), vec![first.clone(), second.clone()]),
            (
                "three published resources".to_string(),
                vec![first.clone(), second.clone(), entry(&filter.published[0])],
            ),
            (
                "every published resource".to_string(),
                filter.published.iter().map(|arn| entry(arn)).collect(),
            ),
            (
                "the same resource twice".to_string(),
                vec![first.clone(), first.clone()],
            ),
            (
                "a published and an unpublished resource".to_string(),
                vec![first.clone(), unpublished.clone()],
            ),
            (
                "two unpublished resources".to_string(),
                vec![unpublished.clone(), unpublished.clone()],
            ),
            (
                "one entry naming no resource".to_string(),
                vec![nameless.clone()],
            ),
            (
                "a named entry followed by a nameless one".to_string(),
                vec![first.clone(), nameless.clone()],
            ),
            (
                "a nameless entry followed by a named one".to_string(),
                vec![nameless.clone(), first.clone()],
            ),
            (
                "two entries, neither naming a resource".to_string(),
                vec![nameless.clone(), nameless.clone()],
            ),
        ];

        for (row, entries) in rows {
            let case = format!("{} with {row}", filter.route);
            let (status, response) = emulator.post(filter.route, &filter.body(&entries)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400, got {status} with {response:?}"
            );
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected a ValidationException, got {response:?}"
            );
            let message = message_of(&response).unwrap_or_else(|| {
                panic!("case {case:?}: the refusal carries no message: {response:?}")
            });
            assert_eq!(
                message, INVALID_FILTER,
                "case {case:?}: expected {INVALID_FILTER:?}, got {message:?}"
            );
            sentences.insert(message);
        }
    }

    assert_eq!(
        sentences.len(),
        1,
        "both filters must refuse with the one same sentence, got {sentences:?}"
    );
}
