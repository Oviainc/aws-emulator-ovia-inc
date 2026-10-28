// SPDX-License-Identifier: CC0-1.0

//! How an ARN names a catalog resource wherever a filter accepts one.
//!
//! The rule under test: a catalog resource is named by its kind and the opaque
//! identifier after `{kind}/` — the `{PARTITION}` segment says only which partition the
//! caller writes the ARN in, and the catalog is global, publishing one resource per
//! kind and identifier. So every partition's spelling of a published resource's ARN
//! narrows a list to exactly the same entries that resource's own ARN does, and an
//! identifier the catalog publishes nowhere narrows it to none in every partition. It
//! is the rule `GetControl` and `ListControlMappings`' `ControlArns` filter already
//! apply to a control ARN (see `control_arn_resolution.rs`), here for the ontology
//! resources: domains, objectives and common controls.
//!
//! Each test is a TABLE: every published resource of the kind the filter accepts,
//! crossed with the partitions the modeled ARN patterns admit, rather than one ARN
//! picked by hand.
//!
//! The harness (`Emulator`, `drain`) is the one `control_catalog.rs` already drives the
//! service through — the same `MultiServiceRouter` front door.

use std::collections::BTreeSet;

use serde_json::json;
use service_com_amazonaws_controlcatalog_controlcatalog::state::{
    ComAmazonawsControlcatalogControlcatalogState as Catalog, MappingDetails,
};

use crate::control_catalog::{drain, Emulator};

/// Partitions a catalog ARN can be written in. Source: the modeled `DomainArn`,
/// `ObjectiveArn` and `CommonControlArn` patterns, each
/// `^arn:(aws(?:[-a-z]*)?):controlcatalog:::<kind>/[0-9a-z]+$`, whose partition
/// alternative admits the whole `aws` family.
///
/// `pub(crate)` so the sibling suites in this test target (see `tests/all.rs`) narrow by
/// the same set of spellings rather than keeping a second copy of it.
pub(crate) const PARTITIONS: [&str; 5] =
    ["aws", "aws-cn", "aws-us-gov", "aws-iso-b", "aws-iso-f"];

/// The opening of every stored catalog ARN: the `aws` partition, with neither a Region
/// nor an account id.
const STORED_PREFIX: &str = "arn:aws:";

/// An identifier of the length the catalog's own ARNs carry that nothing is published
/// under, in the `[0-9a-z]` class the ARN patterns admit.
const UNPUBLISHED_IDENTIFIER: &str = "zzzzzzzzzzzzzzzzzzzzzzzzz";

/// Rewrite a stored catalog ARN into `partition`, leaving the kind and the identifier —
/// the resource's identity — exactly as the catalog minted them.
pub(crate) fn in_partition(arn: &str, partition: &str) -> String {
    let rest = arn
        .strip_prefix(STORED_PREFIX)
        .unwrap_or_else(|| panic!("case {arn}: a stored catalog ARN opens {STORED_PREFIX}"));
    format!("arn:{partition}:{rest}")
}

/// `ListObjectives`' body narrowing to one domain.
fn objective_filter(arn: &str) -> String {
    format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{arn}"}}]}}}}"#)
}

/// `ListCommonControls`' body narrowing to one objective.
fn common_control_filter(arn: &str) -> String {
    format!(r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{arn}"}}]}}}}"#)
}

/// `ListControlMappings`' body narrowing to one common control.
fn mapping_filter(arn: &str) -> String {
    format!(r#"{{"Filter":{{"CommonControlArns":["{arn}"]}}}}"#)
}

/// Every published domain's objectives come back whichever partition the filter ARN is
/// written in.
#[tokio::test]
async fn the_objective_filter_resolves_a_domain_in_any_partition() {
    let emulator = Emulator::new();
    let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;
    assert!(
        !domains.is_empty(),
        "case catalog: the catalog must publish domains for this rule to have rows"
    );

    for domain in &domains {
        let arn = domain["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {domain}: a domain carries its ARN"));
        let baseline = drain(&emulator, "/objectives", "Objectives", None, &objective_filter(arn)).await;
        assert!(
            !baseline.is_empty(),
            "case domain {arn}: every published domain has objectives under it"
        );
        for partition in PARTITIONS {
            let requested = in_partition(arn, partition);
            // Page size 2, so the filter and the pagination cursor the filter
            // fingerprints are exercised together for every spelling.
            let filtered = drain(
                &emulator,
                "/objectives",
                "Objectives",
                Some(2),
                &objective_filter(&requested),
            )
            .await;
            assert_eq!(
                filtered,
                baseline,
                "case {requested}: expected the {} objective(s) of {arn}, got {filtered:?}",
                baseline.len()
            );
        }
    }
}

/// Every published objective's common controls come back whichever partition the filter
/// ARN is written in.
#[tokio::test]
async fn the_common_control_filter_resolves_an_objective_in_any_partition() {
    let emulator = Emulator::new();
    let objectives = drain(&emulator, "/objectives", "Objectives", None, "{}").await;
    assert!(
        !objectives.is_empty(),
        "case catalog: the catalog must publish objectives for this rule to have rows"
    );

    for objective in &objectives {
        let arn = objective["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {objective}: an objective carries its ARN"));
        let baseline = drain(
            &emulator,
            "/common-controls",
            "CommonControls",
            None,
            &common_control_filter(arn),
        )
        .await;
        assert!(
            !baseline.is_empty(),
            "case objective {arn}: every published objective has common controls under it"
        );
        for partition in PARTITIONS {
            let requested = in_partition(arn, partition);
            let filtered = drain(
                &emulator,
                "/common-controls",
                "CommonControls",
                Some(3),
                &common_control_filter(&requested),
            )
            .await;
            assert_eq!(
                filtered,
                baseline,
                "case {requested}: expected the {} common control(s) of {arn}, got {filtered:?}",
                baseline.len()
            );
        }
    }
}

/// The same rule holds for `ListControlMappings`' `CommonControlArns` filter: a mapped
/// common control written in any partition selects the same mappings.
#[tokio::test]
async fn the_mapping_filter_resolves_a_common_control_in_any_partition() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    // The common controls control mappings actually name — the rows for which a
    // baseline is non-empty, so a spelling that resolved to nothing would show.
    let mapped: BTreeSet<&String> = catalog
        .control_mappings
        .values()
        .filter_map(|record| match &record.mapping {
            MappingDetails::CommonControl { common_control_arn } => Some(common_control_arn),
            _ => None,
        })
        .collect();
    assert!(
        !mapped.is_empty(),
        "case catalog: the catalog must map controls to common controls for this rule to \
         have rows"
    );

    for arn in mapped {
        let baseline = drain(
            &emulator,
            "/list-control-mappings",
            "ControlMappings",
            None,
            &mapping_filter(arn),
        )
        .await;
        assert!(
            !baseline.is_empty(),
            "case common control {arn}: a mapped common control must select its mappings"
        );
        for partition in PARTITIONS {
            let requested = in_partition(arn, partition);
            let filtered = drain(
                &emulator,
                "/list-control-mappings",
                "ControlMappings",
                Some(2),
                &mapping_filter(&requested),
            )
            .await;
            assert_eq!(
                filtered,
                baseline,
                "case {requested}: expected the {} mapping(s) of {arn}, got {filtered:?}",
                baseline.len()
            );
        }
    }
}

/// An identifier the catalog publishes nowhere selects nothing in every partition, and
/// an identifier of one kind never resolves as another kind's: it is the identifier,
/// not the partition, that a filter narrows by.
#[tokio::test]
async fn an_identifier_the_catalog_does_not_publish_selects_nothing_in_any_partition() {
    let emulator = Emulator::new();
    let catalog = Catalog::default();
    let published_domain = catalog
        .domains
        .keys()
        .next()
        .expect("the catalog publishes domains");
    let published_objective = catalog
        .objectives
        .keys()
        .next()
        .expect("the catalog publishes objectives");
    // A common control a control mapping names, so its filter has mappings to select.
    let mapped_common_control = catalog
        .control_mappings
        .values()
        .find_map(|record| match &record.mapping {
            MappingDetails::CommonControl { common_control_arn } => Some(common_control_arn),
            _ => None,
        })
        .expect("the catalog maps controls to common controls");

    /// The identifier a stored catalog ARN carries, for re-use under another kind.
    fn identifier(arn: &str) -> &str {
        arn.rsplit('/')
            .next()
            .unwrap_or_else(|| panic!("case {arn}: a stored catalog ARN carries an identifier"))
    }

    // (case, route, items member, the kind the filter accepts, the ARN body builder,
    //  the identifier written under that kind)
    #[allow(clippy::type_complexity)]
    let rows: [(&str, &str, &str, &str, fn(&str) -> String, &str); 6] = [
        (
            "a domain identifier the catalog does not publish",
            "/objectives",
            "Objectives",
            "domain",
            objective_filter,
            UNPUBLISHED_IDENTIFIER,
        ),
        (
            "an objective's identifier read as a domain",
            "/objectives",
            "Objectives",
            "domain",
            objective_filter,
            identifier(published_objective),
        ),
        (
            "an objective identifier the catalog does not publish",
            "/common-controls",
            "CommonControls",
            "objective",
            common_control_filter,
            UNPUBLISHED_IDENTIFIER,
        ),
        (
            "a domain's identifier read as an objective",
            "/common-controls",
            "CommonControls",
            "objective",
            common_control_filter,
            identifier(published_domain),
        ),
        (
            "a common control identifier the catalog does not publish",
            "/list-control-mappings",
            "ControlMappings",
            "common-control",
            mapping_filter,
            UNPUBLISHED_IDENTIFIER,
        ),
        (
            "a domain's identifier read as a common control",
            "/list-control-mappings",
            "ControlMappings",
            "common-control",
            mapping_filter,
            identifier(published_domain),
        ),
    ];

    for (case, path, items_key, kind, body_of, id) in rows {
        for partition in PARTITIONS {
            let requested = format!("arn:{partition}:controlcatalog:::{kind}/{id}");
            let response = emulator.ok(path, &body_of(&requested)).await;
            assert_eq!(
                response[items_key],
                json!([]),
                "case {case} ({requested}): expected an empty {items_key} list, got {response:?}"
            );
            assert!(
                response.get("NextToken").is_none(),
                "case {case} ({requested}): an empty page must carry no NextToken, got \
                 {response:?}"
            );
        }
    }

    // A published common control still selects its mappings, so the rows above fail for
    // the identifier they name and not because the filter stopped matching anything.
    let reported = drain(
        &emulator,
        "/list-control-mappings",
        "ControlMappings",
        None,
        &mapping_filter(mapped_common_control),
    )
    .await;
    assert!(
        !reported.is_empty(),
        "case common control {mapped_common_control}: a mapped common control must select \
         the mappings the catalog records for it, got {reported:?}"
    );
}

/// A `NextToken` issued for a filter resumes the same sequence when the filter ARN is
/// re-sent in another partition: the token travels with the resource's identity, not
/// with the spelling that produced it.
#[tokio::test]
async fn a_page_token_resumes_across_partitions_of_the_same_filter() {
    let emulator = Emulator::new();
    let domains = drain(&emulator, "/domains", "Domains", None, "{}").await;

    for domain in &domains {
        let arn = domain["Arn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {domain}: a domain carries its ARN"));
        let whole = drain(&emulator, "/objectives", "Objectives", None, &objective_filter(arn)).await;
        if whole.len() < 2 {
            continue;
        }
        let first = emulator
            .ok("/objectives?maxResults=1", &objective_filter(arn))
            .await;
        let token = first["NextToken"]
            .as_str()
            .unwrap_or_else(|| panic!("case domain {arn}: a first page of 1 of {} carries a token", whole.len()))
            .to_string();

        for partition in PARTITIONS {
            let requested = in_partition(arn, partition);
            let resumed = emulator
                .ok(
                    &format!("/objectives?maxResults=1&nextToken={token}"),
                    &objective_filter(&requested),
                )
                .await;
            assert_eq!(
                resumed["Objectives"],
                json!([whole[1]]),
                "case {requested}: resuming must return the second objective of {arn}, got \
                 {resumed:?}"
            );
        }
    }
}
