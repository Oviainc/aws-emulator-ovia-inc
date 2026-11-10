// SPDX-License-Identifier: CC0-1.0

//! Cover for the `WorkloadData` contract `GetWorkload` reports, driven over HTTP
//! through the same router the production emulator serves with.
//!
//! The not-found and validation halves of the workload-lookup rule are covered by
//! `not_found_contract.rs`; this suite is about the members a FOUND workload carries:
//! `GetWorkload` projects the stored catalogue entry, so every member it reports is
//! either read off that entry or absent, and it never disagrees with the summary
//! `ListWorkloads` pages — whatever page size the listing was walked at.
//!
//! Every assertion runs over a TABLE: the whole seeded catalogue (not just the `SAP`
//! entry the issue cited), and every page size from one to one past the end.

use serde_json::{json, Value};
use service_com_amazonaws_launchwizard_launchwizard::ops::types::WORKLOAD_STATUS_VALUES;

use crate::harness::{catalogue, walk_pages, Harness};

/// The `WorkloadData` entry `GetWorkload` reports for `name`.
async fn get_workload(harness: &Harness, case: &str, name: &str) -> Value {
    let body = harness
        .ok(case, "/getWorkload", json!({"workloadName": name}))
        .await;
    body["workload"].clone()
}

/// True when a member is absent from the response (restJson1 omits an absent member;
/// an explicit JSON null counts the same).
fn absent(workload: &Value, member: &str) -> bool {
    workload.get(member).is_none_or(Value::is_null)
}

#[tokio::test]
async fn a_reported_workload_carries_only_members_read_off_the_stored_entry() {
    let harness = Harness::new();
    let names = catalogue(&harness).await;
    assert!(
        names.len() >= 2,
        "the catalogue must hold more than one workload for this table to span it, got {names:?}"
    );

    for name in &names {
        let workload = get_workload(&harness, name, name).await;

        // `workloadName` is the catalogue key the request named, reported from the
        // stored entry and satisfying its own shape: len 1..100,
        // `^[A-Za-z][a-zA-Z0-9-_]*$` (the model's `WorkloadName` constraints).
        let reported = workload["workloadName"].as_str().unwrap_or_default();
        assert_eq!(
            reported, name,
            "case {name:?}: the reported workloadName must be the stored one, got {reported:?}"
        );
        assert!(
            (1..=100).contains(&reported.len())
                && reported.starts_with(|c: char| c.is_ascii_alphabetic())
                && reported
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'),
            "case {name:?}: reported workloadName must match ^[A-Za-z][a-zA-Z0-9-_]*$ with \
             len 1..100, got {reported:?}"
        );

        // `status` is the model's `WorkloadStatus` enum (shared_types'
        // WORKLOAD_STATUS_VALUES, i.e. ACTIVE/INACTIVE/DISABLED/DELETED).
        let status = workload["status"].as_str().unwrap_or_default();
        assert!(
            WORKLOAD_STATUS_VALUES.contains(&status),
            "case {name:?}: status {status:?} is not a WorkloadStatus value"
        );

        // The members the stored entry carries beyond the catalogue key: the display
        // name and description, and the document and icon the catalogue publishes for
        // the entry (whose own rule is `catalogue_published_urls.rs`).
        for member in ["displayName", "description", "documentationUrl", "iconUrl"] {
            assert!(
                workload[member]
                    .as_str()
                    .is_some_and(|value| !value.is_empty()),
                "case {name:?}: every entry must carry a non-empty {member}, got {}",
                workload[member]
            );
        }

        // And the members the emulator holds nothing for: an entry in a steady status
        // has nothing to say about it, so `statusMessage` is absent. `accountConstraints`
        // is NOT one of them: its list admits zero elements, so "no account constrains
        // this entry" is reported as the empty list rather than by leaving the member
        // out — the rule `tests/all/catalogue_aws_deployment_patterns.rs` tables.
        assert!(
            absent(&workload, "statusMessage"),
            "case {name:?}: statusMessage must be absent while the emulator holds no value \
             for it, got {}",
            workload["statusMessage"]
        );
        assert_eq!(
            workload["accountConstraints"],
            json!([]),
            "case {name:?}: accountConstraints must be the empty list for an entry no \
             account constraint applies to, got {}",
            workload["accountConstraints"]
        );
    }
}

#[tokio::test]
async fn a_read_workload_agrees_with_the_listing_at_every_page_size() {
    let harness = Harness::new();
    let names = catalogue(&harness).await;

    // The listing is walked at every page size from one to one past the end, so the
    // agreement holds however the summary reached the caller rather than only on a
    // single-page listing.
    for page_size in 1..=names.len() + 1 {
        let case = format!("listWorkloads maxResults={page_size}");
        let summaries = walk_pages(
            &harness,
            &case,
            "/listWorkloads",
            &json!({}),
            "workloads",
            page_size,
        )
        .await;
        assert_eq!(
            summaries.len(),
            names.len(),
            "case {case:?}: paging must report the whole catalogue, got {} entries",
            summaries.len()
        );

        for summary in &summaries {
            let name = summary["workloadName"]
                .as_str()
                .expect("a catalogue entry must carry workloadName");
            let workload = get_workload(&harness, &format!("{case} {name}"), name).await;
            // Both readers project the same stored entry, so every member they share
            // must match.
            for member in ["workloadName", "displayName", "status"] {
                assert_eq!(
                    workload[member], summary[member],
                    "case {case:?} {name:?}: GetWorkload's {member} must match the \
                     listing's, got {} vs {}",
                    workload[member], summary[member]
                );
            }
        }
    }
}
