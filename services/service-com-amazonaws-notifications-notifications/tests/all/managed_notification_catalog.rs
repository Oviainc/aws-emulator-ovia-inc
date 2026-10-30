// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT the published `ManagedNotificationConfiguration`
//! catalog contains, driven over HTTP through the same multi-service router the
//! emulator boots in production.
//!
//! The ordering, paging and filtering of the listing are covered by
//! `tests/all/managed_notification_configurations.rs`; this suite is about the
//! catalog's CONTENTS and the rule they follow: the service publishes one entry
//! per event-type category of every service whose events it covers, and each
//! entry carries its OWN display `name` and prose `description` — neither of
//! them a rendering of the arn's `category`/`sub-category` tokens, so a caller
//! that renders `name` shows display text and not an arn segment.
//!
//! Shape source for the bounds and the arn layout the rows below check:
//! `smithy_query(query="shape",shape="ManagedNotificationConfigurationStructure,ManagedNotificationConfigurationName,ManagedNotificationConfigurationDescription,ManagedNotificationConfigurationOsArn",model_path="services/service-com-amazonaws-notifications-notifications/smithy-model/com-amazonaws-notifications-notifications.json")`.

use serde_json::Value;

use crate::harness::{encode_path_label, managed_configuration_arn_segments, Emulator};

/// `ManagedNotificationConfigurationName`'s modelled `@length`: 1..64.
const NAME_MIN: usize = 1;
const NAME_MAX: usize = 64;

/// `ManagedNotificationConfigurationDescription`'s modelled `@length`: 0..256.
/// The published descriptions are prose, so each one is also asserted non-empty.
const DESCRIPTION_MAX: usize = 256;

/// The `category`/`sub-category` pairs the service publishes.
///
/// Catalog source: the service's own published set, `MANAGED_NOTIFICATION_CATALOG`
/// in `src/state.rs` — the model enumerates no entries (only their shape, see
/// the module header's `smithy_query`), so this table is the published behaviour
/// and the rows span every category the service covers rather than one of them.
const PUBLISHED: &[(&str, &str)] = &[
    ("AWS-Health", "Issue"),
    ("AWS-Health", "AccountNotification"),
    ("AWS-Health", "ScheduledChange"),
    ("AWS-Health", "Investigation"),
    ("AWS-Partner-Central", "PCProductListings"),
    ("AWS-Partner-Central", "PCOffersAndAgreements"),
    ("AWS-Partner-Central", "PCPaymentsAndDisbursements"),
];

/// Every entry the unpaged listing answers, as `(arn, name, description)`.
async fn listed_catalog(emulator: &Emulator) -> Vec<(String, String, String)> {
    // `maxResults`' modelled maximum is 100, which is more than the catalog
    // holds, so one page carries all of it.
    let (status, response) = emulator
        .send("GET", "/managed-notification-configurations?maxResults=100", None)
        .await;
    assert_eq!(
        status, 200,
        "case unpaged listing: expected 200, got {status} {response:?}"
    );
    assert_eq!(
        response["nextToken"],
        Value::Null,
        "case unpaged listing: the whole catalog fits one page of 100, got {response:?}"
    );
    response["managedNotificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case unpaged listing: expected an array, got {response:?}"))
        .iter()
        .map(|entry| {
            let field = |name: &str| {
                entry[name]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case unpaged listing: every entry carries {name}, got {entry:?}")
                    })
                    .to_string()
            };
            (field("arn"), field("name"), field("description"))
        })
        .collect()
}

/// `GetManagedNotificationConfiguration` for one arn.
async fn read_catalog_entry(emulator: &Emulator, arn: &str) -> (u16, Value) {
    let uri = format!(
        "/managed-notification-configurations/{}",
        encode_path_label(arn)
    );
    emulator.send("GET", &uri, None).await
}

/// RULE: the catalog covers more than one service's events — one entry per
/// event-type category of each covered service — so a caller enumerating it
/// discovers every category the service publishes, not one category's worth.
#[tokio::test]
async fn the_catalog_spans_every_published_category() {
    let emulator = Emulator::new();
    let listed = listed_catalog(&emulator).await;

    for (category, sub_category) in PUBLISHED {
        let case = format!("{category}/{sub_category}");
        let suffix = format!(
            "managed-notification-configuration/category/{category}/sub-category/{sub_category}"
        );
        let found = listed
            .iter()
            .find(|(arn, _, _)| arn.ends_with(&suffix))
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the listing must publish this category, got {:?}",
                    listed.iter().map(|(arn, _, _)| arn).collect::<Vec<_>>()
                )
            });

        // The entry is readable by the arn the listing answered, and the read
        // reports the same display text the listing did.
        let (status, response) = read_catalog_entry(&emulator, &found.0).await;
        assert_eq!(
            status, 200,
            "case {case}: a published arn must be readable, got {status} {response:?}"
        );
        assert_eq!(
            response["name"].as_str(),
            Some(found.1.as_str()),
            "case {case}: expected name {:?}, got {response:?}",
            found.1
        );
        assert_eq!(
            response["description"].as_str(),
            Some(found.2.as_str()),
            "case {case}: expected description {:?}, got {response:?}",
            found.2
        );
        assert_eq!(
            response["category"].as_str(),
            Some(*category),
            "case {case}: expected category {category}, got {response:?}"
        );
        assert_eq!(
            response["subCategory"].as_str(),
            Some(*sub_category),
            "case {case}: expected subCategory {sub_category}, got {response:?}"
        );
    }

    let categories: std::collections::BTreeSet<String> = listed
        .iter()
        .filter_map(|(arn, _, _)| managed_configuration_arn_segments(arn))
        .map(|(category, _)| category)
        .collect();
    assert!(
        categories.len() >= 2,
        "case covered services: the catalog must cover more than one service's events, got {categories:?}"
    );
    for category in &categories {
        let entries = listed
            .iter()
            .filter_map(|(arn, _, _)| managed_configuration_arn_segments(arn))
            .filter(|(listed_category, _)| listed_category == category)
            .count();
        assert!(
            entries >= 2,
            "case {category}: a covered service publishes one entry per event-type category, got {entries}"
        );
    }
}

/// RULE: an entry's `name` and `description` are its own published display
/// text, independent of the arn — so neither is any rendering of the arn's
/// `category`/`sub-category` tokens, and both stay inside the bounds the model
/// declares for them.
#[tokio::test]
async fn every_entry_carries_display_text_that_is_not_its_arn_segments() {
    let emulator = Emulator::new();
    let listed = listed_catalog(&emulator).await;
    assert_eq!(
        listed.len(),
        PUBLISHED.len(),
        "case published catalog: expected {} entries, got {:?}",
        PUBLISHED.len(),
        listed.iter().map(|(arn, _, _)| arn).collect::<Vec<_>>()
    );

    for (arn, name, description) in &listed {
        let (category, sub_category) = managed_configuration_arn_segments(arn)
            .unwrap_or_else(|| panic!("case {arn}: the arn must match the modelled pattern"));
        let case = format!("{category}/{sub_category}");

        assert!(
            (NAME_MIN..=NAME_MAX).contains(&name.chars().count()),
            "case {case}: name must be {NAME_MIN}..{NAME_MAX} characters, got {name:?}"
        );
        assert!(
            !description.is_empty() && description.chars().count() <= DESCRIPTION_MAX,
            "case {case}: description must be prose within {DESCRIPTION_MAX} characters, got {description:?}"
        );

        // Every rendering of the arn's own two segments: the name is none of
        // them, so a caller rendering `name` cannot be shown an arn token.
        let renderings = [
            format!("{category}-{sub_category}"),
            format!("{category}/{sub_category}"),
            format!("{category}{sub_category}"),
            format!("{sub_category}-{category}"),
            category.clone(),
            sub_category.clone(),
        ];
        for rendering in &renderings {
            assert_ne!(
                name, rendering,
                "case {case}: name must be published display text, not the arn rendering {rendering:?}"
            );
            assert_ne!(
                description, rendering,
                "case {case}: description must be published prose, not the arn rendering {rendering:?}"
            );
        }

        // Display text, not an identifier: it reads as words a console renders.
        assert!(
            name.contains(' ') || name.chars().filter(|c| c.is_uppercase()).count() <= 1,
            "case {case}: name must read as display text rather than a concatenated identifier, got {name:?}"
        );
        assert!(
            description.contains(' ') && description.ends_with('.'),
            "case {case}: description must be a prose sentence, got {description:?}"
        );
    }

    // No two entries share a name or an arn, so the catalog identifies each
    // published category distinctly.
    let mut names: Vec<&String> = listed.iter().map(|(_, name, _)| name).collect();
    names.sort();
    let unique = names.len();
    names.dedup();
    assert_eq!(
        names.len(),
        unique,
        "case distinct names: every published entry carries its own name, got {names:?}"
    );
}
