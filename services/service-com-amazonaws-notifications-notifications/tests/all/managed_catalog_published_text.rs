// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the published display TEXT of a managed notification
//! configuration — the whole `name` and the whole `description` — driven over
//! HTTP through the same multi-service router the emulator boots in production.
//!
//! The rule under test: the `name` and `description` of a published managed
//! notification configuration are that entry's own published text, carried
//! COMPLETE, and every path a caller can reach the entry by reports the same
//! text — a listing that names no page size, a listing at an explicit page size,
//! a paged walk, a `channelIdentifier`-filtered listing and
//! `GetManagedNotificationConfiguration`. Nothing a caller does creates, edits
//! or deletes one, so the text is a published value rather than a per-account or
//! per-run one, and a sentence that stops short of its own closing clause on any
//! of those paths is a defect.
//!
//! `tests/all/managed_notification_catalog.rs` pins WHICH entries the catalog
//! holds and their text on the unpaged listing; this suite is about that text
//! being identical and unabridged on every read path, and about an enumerating
//! description running to its final item.
//!
//! Shape source: `ManagedNotificationConfigurationStructure.{name,description}`
//! are both `@required`, `name` `@length` 1..64 and `description` `@length`
//! 0..256
//! (`smithy_query(query="shape",shape="ManagedNotificationConfigurationStructure,ManagedNotificationConfigurationName,ManagedNotificationConfigurationDescription",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`).
//! The model enumerates no entry, so the text itself is the service's published
//! behaviour.

use serde_json::Value;
use service_com_amazonaws_notifications_notifications::state::MANAGED_DEFAULT_CHANNEL_IDENTIFIER;

use crate::harness::{encode_path_label, managed_configuration_arn_segments, Emulator};

/// The Marketplace BUYER cluster of the published catalog, as
/// `(sub-category, name, description)`.
///
/// Catalog source: the service's published set,
/// `MANAGED_NOTIFICATION_CATALOG` in `src/state.rs` — the model enumerates no
/// entry, so this table is the published behaviour. The rows are the whole
/// `AWS-MP-Buyer` cluster rather than one entry of it, and each description is
/// the complete published sentence including the closing item of its
/// enumeration.
const MP_BUYER_PUBLISHED: &[(&str, &str, &str)] = &[
    (
        "MPBuyerProducts",
        "Products and solutions",
        "Notifications about changes to products you've subscribed to, including new versions, instance type updates, visibility changes, region restrictions, and security profile snapshots.",
    ),
    (
        "MPBuyerPrivateOffers",
        "Private offers",
        "Notifications about private offers extended to you, including new offers, offer set availability, and channel partner offer lifecycle updates.",
    ),
    (
        "MPBuyerPricing",
        "Pricing changes",
        "Notifications about price changes to products you're subscribed to, including hourly, monthly, and usage-based pricing updates.",
    ),
    (
        "MPBuyerAgreements",
        "Agreements and subscriptions",
        "Notifications about your agreements and subscriptions, including new agreements, amendments, cancellations, expirations, renewals, free trial reminders, payment requests, and billing adjustments.",
    ),
];

/// The arn `category/` segment of the cluster the table above spells out.
const MP_BUYER_CATEGORY: &str = "AWS-MP-Buyer";

/// The word a description's enumeration of covered events opens with.
const ENUMERATION_OPENER: &str = ", including ";

/// One entry as a read path reported it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Reported {
    arn: String,
    name: String,
    description: String,
}

/// Every entry a listing answered, as this suite compares them.
fn reported(response: &Value, case: &str) -> Vec<Reported> {
    response["managedNotificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: a listing answers an array, got {response:?}"))
        .iter()
        .map(|entry| {
            let field = |member: &str| {
                entry[member]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case}: every entry carries {member}, got {entry:?}")
                    })
                    .to_string()
            };
            Reported {
                arn: field("arn"),
                name: field("name"),
                description: field("description"),
            }
        })
        .collect()
}

/// A listing with a raw query string; the empty string names no parameter at all.
async fn listing(emulator: &Emulator, case: &str, query: &str) -> Vec<Reported> {
    let uri = if query.is_empty() {
        "/managed-notification-configurations".to_string()
    } else {
        format!("/managed-notification-configurations?{query}")
    };
    let (status, response) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case}: expected 200, got {status} {response:?}"
    );
    reported(&response, case)
}

/// The whole catalog walked one page at a time at `size` entries a page.
async fn paged_listing(emulator: &Emulator, case: &str, size: usize) -> Vec<Reported> {
    let mut seen: Vec<Reported> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match &token {
            Some(value) => format!("maxResults={size}&nextToken={value}"),
            None => format!("maxResults={size}"),
        };
        let uri = format!("/managed-notification-configurations?{query}");
        let (status, response) = emulator.send("GET", &uri, None).await;
        assert_eq!(
            status, 200,
            "case {case}: expected 200, got {status} {response:?}"
        );
        seen.extend(reported(&response, case));
        token = response["nextToken"].as_str().map(str::to_string);
        if token.is_none() {
            return seen;
        }
        assert!(
            seen.len() <= 1_000,
            "case {case}: paging did not terminate, saw {} entries",
            seen.len()
        );
    }
}

/// `GetManagedNotificationConfiguration` for one arn.
async fn read_entry(emulator: &Emulator, case: &str, arn: &str) -> Reported {
    let uri = format!(
        "/managed-notification-configurations/{}",
        encode_path_label(arn)
    );
    let (status, response) = emulator.send("GET", &uri, None).await;
    assert_eq!(
        status, 200,
        "case {case}: a published arn must be readable, got {status} {response:?}"
    );
    let field = |member: &str| {
        response[member]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: the read answers {member}, got {response:?}"))
            .to_string()
    };
    Reported {
        arn: arn.to_string(),
        name: field("name"),
        description: field("description"),
    }
}

/// RULE: the published text of an entry is the same on every path a caller can
/// reach that entry by, so no path abridges or rewords it.
#[tokio::test]
async fn every_read_path_reports_the_same_published_text() {
    let emulator = Emulator::new();

    // The baseline: the listing a caller makes with no parameters at all.
    let baseline = listing(&emulator, "no parameters", "").await;
    assert!(
        baseline.len() >= 2,
        "case no parameters: the service must publish a catalog to read, got {baseline:?}"
    );

    // Rows: the ways a caller enumerates the catalog — an explicit page size at
    // the modelled maximum, the documented default, a walk one entry at a time,
    // a walk a few entries at a time, and the filter on the channel every
    // published entry starts associated with.
    let filter = format!(
        "channelIdentifier={}",
        encode_path_label(MANAGED_DEFAULT_CHANNEL_IDENTIFIER)
    );
    let mut paths: Vec<(String, Vec<Reported>)> = Vec::new();
    for (case, query) in [
        ("maxResults at the modelled maximum", "maxResults=100"),
        ("maxResults at the documented default", "maxResults=20"),
        ("the channelIdentifier filter", filter.as_str()),
    ] {
        paths.push((case.to_string(), listing(&emulator, case, query).await));
    }
    for size in [1_usize, 3, 100] {
        let case = format!("a paged walk at maxResults={size}");
        let walked = paged_listing(&emulator, &case, size).await;
        paths.push((case, walked));
    }

    for (case, entries) in &paths {
        let mut sorted = entries.clone();
        sorted.sort();
        let mut want = baseline.clone();
        want.sort();
        assert_eq!(
            sorted, want,
            "case {case}: expected the same published entries and text as a listing with no \
             parameters, got {sorted:?}"
        );
    }

    // And the single-entry read of each published arn reports that same text.
    for entry in &baseline {
        let case = format!("a read of {}", entry.arn);
        let read = read_entry(&emulator, &case, &entry.arn).await;
        assert_eq!(
            read, *entry,
            "case {case}: expected the listed text {entry:?}, got {read:?}"
        );
    }
}

/// RULE: an entry's published description is a COMPLETE sentence — one that
/// enumerates the events it covers runs to the closing item of that enumeration
/// rather than stopping after the first one.
#[tokio::test]
async fn a_published_description_runs_to_the_end_of_its_enumeration() {
    let emulator = Emulator::new();
    let listed = listing(&emulator, "the whole catalog", "maxResults=100").await;

    for entry in &listed {
        let (category, sub_category) = managed_configuration_arn_segments(&entry.arn)
            .unwrap_or_else(|| panic!("case {}: the arn matches the modelled pattern", entry.arn));
        let case = format!("{category}/{sub_category}");
        let description = &entry.description;
        assert!(
            description.ends_with('.'),
            "case {case}: a published description is a prose sentence, got {description:?}"
        );
        if let Some((_, enumerated)) = description.split_once(ENUMERATION_OPENER) {
            assert!(
                enumerated.contains(" and "),
                "case {case}: a description that enumerates the covered events must run to its \
                 closing item, got {description:?}"
            );
        }
    }
}

/// RULE applied to the Marketplace BUYER cluster: it publishes exactly the
/// entries of the table above, each with the whole published name and
/// description, on the listing and on the read of its arn.
#[tokio::test]
async fn the_marketplace_buyer_cluster_publishes_its_whole_text() {
    let emulator = Emulator::new();
    let listed = listing(&emulator, "the whole catalog", "maxResults=100").await;

    for (sub_category, name, description) in MP_BUYER_PUBLISHED {
        let case = format!("{MP_BUYER_CATEGORY}/{sub_category}");
        let suffix = format!(
            "managed-notification-configuration/category/{MP_BUYER_CATEGORY}/sub-category/{sub_category}"
        );
        let found = listed
            .iter()
            .find(|entry| entry.arn.ends_with(&suffix))
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the listing must publish this category, got {:?}",
                    listed.iter().map(|entry| &entry.arn).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            (found.name.as_str(), found.description.as_str()),
            (*name, *description),
            "case {case}: expected the published name {name:?} and description {description:?}, \
             got {:?} and {:?}",
            found.name,
            found.description
        );

        let read = read_entry(&emulator, &case, &found.arn).await;
        assert_eq!(
            (read.name.as_str(), read.description.as_str()),
            (*name, *description),
            "case {case}: a read must report the same published text, got {read:?}"
        );
    }

    // Exactly that cluster: an entry added to or removed from it fails here.
    let published: std::collections::BTreeSet<String> = listed
        .iter()
        .filter_map(|entry| managed_configuration_arn_segments(&entry.arn))
        .filter(|(category, _)| category == MP_BUYER_CATEGORY)
        .map(|(_, sub_category)| sub_category)
        .collect();
    let expected: std::collections::BTreeSet<String> = MP_BUYER_PUBLISHED
        .iter()
        .map(|(sub_category, _, _)| (*sub_category).to_string())
        .collect();
    assert_eq!(
        published, expected,
        "case {MP_BUYER_CATEGORY}: expected exactly the published sub-categories {expected:?}, \
         got {published:?}"
    );
}
