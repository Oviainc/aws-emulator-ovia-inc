// SPDX-License-Identifier: CC0-1.0

//! The paging side of the single-ARN resource-filter rule, driven through BOTH
//! operations that take such a filter.
//!
//! The rule under test is one rule for the class, not one per operation: a single-ARN
//! resource filter narrows the listed collection to the children whose stored parent ARN
//! is the zero-or-one ARN the filter resolved to, and it contributes exactly that ARN —
//! the resource's identity, not the caller's spelling of it — as the one dimension of the
//! page fingerprint a `NextToken` carries. So a token resumes the sequence for every
//! spelling of the same filter, and is refused for a different filter.
//!
//! `ListObjectives` (`ObjectiveFilter.Domains`) and `ListCommonControls`
//! (`CommonControlFilter.Objectives`) both answer by it, and both drive the same
//! narrowing-and-fingerprint path, so every row below is run against both. Sibling of
//! `single_resource_filter`, which covers which filters that path accepts and how it
//! refuses the rest.
//!
//! Each test is a TABLE whose rows vary what the rule generalises over — the operation
//! and its filter member, which published parent is narrowed to, the partition the filter
//! ARN is written in, and which other filter a token is replayed under — rather than one
//! filter picked by hand. The harness (`Emulator`, `drain`) is the one `control_catalog.rs`
//! already drives the service through: the same `MultiServiceRouter` front door.

use serde_json::{json, Value};
use service_com_amazonaws_controlcatalog_controlcatalog::state::ComAmazonawsControlcatalogControlcatalogState as Catalog;

use crate::catalog_resource_identity::{in_partition, PARTITIONS};
use crate::control_catalog::{drain, error_code, Emulator};
use crate::pagination_token_spelling::INVALID_NEXT_TOKEN;
use crate::validation_messages::message_of;

/// An identifier of the length the catalog's own ARNs carry that nothing is published
/// under, in the `[0-9a-z]` class the modeled ARN patterns admit.
const UNPUBLISHED_IDENTIFIER: &str = "zzzzzzzzzzzzzzzzzzzzzzzzz";

/// One of the service's filtered listings: the operation, the members its single-ARN
/// filter is spelled with, and the parents that filter can narrow to.
struct FilteredListing {
    /// Route of the operation the filter belongs to.
    route: &'static str,
    /// Response member carrying that operation's items.
    items_key: &'static str,
    /// The input member holding the filter structure.
    filter_member: &'static str,
    /// The filter structure's list member, holding the resource entries.
    list_member: &'static str,
    /// The member each returned item reports its parent under.
    parent_member: &'static str,
    /// The ARN kind segment the entry's `Arn` names.
    kind: &'static str,
    /// Published ARNs of that kind, straight out of the catalog the service serves.
    parents: Vec<String>,
}

impl FilteredListing {
    /// A request body narrowing this listing to the parent `arn` names.
    fn narrowed_to(&self, arn: &str) -> String {
        format!(
            r#"{{"{}":{{"{}":[{{"Arn":"{arn}"}}]}}}}"#,
            self.filter_member, self.list_member
        )
    }

    /// An ARN of this listing's parent kind carrying `identifier`, written in `partition`.
    fn arn_in(&self, partition: &str, identifier: &str) -> String {
        format!("arn:{partition}:controlcatalog:::{}/{identifier}", self.kind)
    }

    /// The first page of one item under `body`, and the token it carries — `None` when
    /// the sequence is too short to be continued.
    async fn first_page_token(&self, emulator: &Emulator, body: &str) -> Option<String> {
        let first = emulator
            .ok(&format!("{}?maxResults=1", self.route), body)
            .await;
        first
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string)
    }
}

/// Both filtered listings the service declares, with the published parent ARNs each
/// one's filter narrows to.
fn filtered_listings() -> Vec<FilteredListing> {
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
        FilteredListing {
            route: "/objectives",
            items_key: "Objectives",
            filter_member: "ObjectiveFilter",
            list_member: "Domains",
            parent_member: "Domain",
            kind: "domain",
            parents: domains,
        },
        FilteredListing {
            route: "/common-controls",
            items_key: "CommonControls",
            filter_member: "CommonControlFilter",
            list_member: "Objectives",
            parent_member: "Objective",
            kind: "objective",
            parents: objectives,
        },
    ]
}

/// A filtered listing comes back narrowed to its parent, and a `NextToken` it issued
/// resumes that same sequence under every spelling of the same parent ARN — on both
/// operations that take a single-ARN filter.
#[tokio::test]
async fn a_filter_token_resumes_the_narrowed_sequence_on_both_filtered_listings() {
    let emulator = Emulator::new();

    for listing in filtered_listings() {
        let mut continued = 0usize;
        for parent in &listing.parents {
            let body = listing.narrowed_to(parent);
            let whole = drain(&emulator, listing.route, listing.items_key, None, &body).await;
            let case = format!("{} narrowed to {parent}", listing.route);
            assert!(
                !whole.is_empty(),
                "case {case:?}: every published {} has items under it, got none",
                listing.kind
            );
            for item in &whole {
                assert_eq!(
                    item[listing.parent_member]["Arn"].as_str(),
                    Some(parent.as_str()),
                    "case {case:?}: item {} reports a different {}",
                    item["Arn"],
                    listing.parent_member
                );
            }

            // A parent with a single item under it issues no token, so there is nothing
            // to resume for it; the counter below is what says the table did resume.
            if whole.len() < 2 {
                continue;
            }
            continued += 1;

            // Walking the narrowed sequence one item at a time returns it in full and in
            // the order the undivided listing returned it.
            let paged = drain(
                &emulator,
                listing.route,
                listing.items_key,
                Some(1),
                &body,
            )
            .await;
            assert_eq!(
                paged, whole,
                "case {case:?}: paging one item at a time must return the same {} \
                 sequence as one undivided page, got {paged:?}",
                listing.items_key
            );

            let token = listing
                .first_page_token(&emulator, &body)
                .await
                .unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: a first page of 1 of {} items carries a token",
                        whole.len()
                    )
                });
            for partition in PARTITIONS {
                let requested = in_partition(parent, partition);
                let case = format!("{} resumed with the filter written as {requested}", listing.route);
                let resumed = emulator
                    .ok(
                        &format!("{}?maxResults=1&nextToken={token}", listing.route),
                        &listing.narrowed_to(&requested),
                    )
                    .await;
                assert_eq!(
                    resumed[listing.items_key],
                    json!([whole[1]]),
                    "case {case:?}: resuming must return the second item of {parent}, got \
                     {resumed:?}"
                );
            }
        }
        assert!(
            continued > 0,
            "case {}: no published {} holds the two items a token needs, so this table \
             resumed nothing",
            listing.route,
            listing.kind
        );
    }
}

/// A `NextToken` is only meaningful for the filter that issued it: replaying one under a
/// different filter — another parent, an unpublished parent, or no filter at all — is
/// refused with the operation's one invalid-token sentence, on both operations.
#[tokio::test]
async fn a_filter_token_is_refused_under_a_different_filter_on_both_filtered_listings() {
    let emulator = Emulator::new();

    for listing in filtered_listings() {
        // The first published parent that holds enough items to issue a token, and
        // another published parent to replay that token under.
        let mut issued: Option<(String, String)> = None;
        for parent in &listing.parents {
            if let Some(token) = listing
                .first_page_token(&emulator, &listing.narrowed_to(parent))
                .await
            {
                issued = Some((parent.clone(), token));
                break;
            }
        }
        let (parent, token) = issued.unwrap_or_else(|| {
            panic!(
                "case {}: no published {} holds the two items a token needs",
                listing.route, listing.kind
            )
        });
        let other = listing
            .parents
            .iter()
            .find(|candidate| *candidate != &parent)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the catalog must publish a second {}",
                    listing.route, listing.kind
                )
            });

        // A token the UNFILTERED listing issued, for the reverse direction: it does not
        // resume a narrowed sequence either.
        let unfiltered_token = listing
            .first_page_token(&emulator, "{}")
            .await
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the unfiltered listing must hold the two items a token \
                     needs",
                    listing.route
                )
            });

        // (case, the token replayed, the filter it is replayed under). Each row pairs a
        // token with a filter other than the one that issued it.
        let rows: Vec<(String, &str, String)> = vec![
            (
                format!("{} token replayed with no filter at all", listing.route),
                token.as_str(),
                "{}".to_string(),
            ),
            (
                format!(
                    "{} token replayed under another {}",
                    listing.route, listing.kind
                ),
                token.as_str(),
                listing.narrowed_to(other),
            ),
            (
                format!(
                    "{} token replayed under an unpublished {}",
                    listing.route, listing.kind
                ),
                token.as_str(),
                listing.narrowed_to(&listing.arn_in("aws", UNPUBLISHED_IDENTIFIER)),
            ),
            (
                format!(
                    "{} unfiltered token replayed under {parent}",
                    listing.route
                ),
                unfiltered_token.as_str(),
                listing.narrowed_to(&parent),
            ),
        ];

        for (case, replayed, body) in rows {
            assert_refused(&emulator, &listing, &case, replayed, &body).await;
        }
    }
}

/// Replaying `token` on `listing` under `body` is refused with the service's one
/// invalid-token sentence.
async fn assert_refused(
    emulator: &Emulator,
    listing: &FilteredListing,
    case: &str,
    token: &str,
    body: &str,
) {
    let (status, response) = emulator
        .post(
            &format!("{}?maxResults=1&nextToken={token}", listing.route),
            body,
        )
        .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} with {response:?}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected a ValidationException, got {response:?}"
    );
    let message = message_of(&response)
        .unwrap_or_else(|| panic!("case {case:?}: the refusal carries no message: {response:?}"));
    let expected = INVALID_NEXT_TOKEN;
    assert_eq!(
        message, expected,
        "case {case:?}: expected {expected:?}, got {message:?}"
    );
}
