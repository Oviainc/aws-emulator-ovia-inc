// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the published `ManagedNotificationConfiguration`
//! catalog: `ListManagedNotificationConfigurations` and
//! `GetManagedNotificationConfiguration`, driven over HTTP through the same
//! multi-service router the emulator boots in production.
//!
//! The rule under test is that the service PUBLISHES this catalog (the model
//! declares no create or delete for one), that the listing reports every
//! published entry newest-first and pages through it, and that a read of a
//! listed entry's arn answers the same entry — with `category` and
//! `subCategory` being the two segments its arn is built from. Every case
//! derives its expectations from the store's own published set rather than from
//! a literal, so a catalog that grows moves the test with it, and the tables
//! vary the inputs the rule generalises over (page size, filter value, arn).

use serde_json::Value;
use service_com_amazonaws_notifications_notifications::state::MANAGED_DEFAULT_CHANNEL_IDENTIFIER;
use service_com_amazonaws_notifications_notifications::ComAmazonawsNotificationsNotificationsState;

use crate::harness::{encode_path_label, error_code, managed_configuration_arn_segments, Emulator};

/// One published entry as the store holds it, which is what the HTTP answers are
/// checked against.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Published {
    arn: String,
    name: String,
    description: String,
    category: String,
    sub_category: String,
    sequence: u64,
}

impl Emulator {
    /// List with a raw query string (empty for an unfiltered, unpaged listing).
    async fn list_managed(&self, query: &str) -> (u16, Value) {
        let uri = if query.is_empty() {
            "/managed-notification-configurations".to_string()
        } else {
            format!("/managed-notification-configurations?{query}")
        };
        self.send("GET", &uri, None).await
    }

    async fn get_managed(&self, arn: &str) -> (u16, Value) {
        let uri = format!(
            "/managed-notification-configurations/{}",
            encode_path_label(arn)
        );
        self.send("GET", &uri, None).await
    }

    /// The published catalog as the store holds it, newest-first — the order the
    /// listing must report.
    fn published_newest_first(&self) -> Vec<Published> {
        let state = self
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let store = state.lock();
        let mut published: Vec<Published> = store
            .managed_configurations_in_publication_order()
            .into_iter()
            .map(|record| Published {
                arn: record.arn.clone(),
                name: record.name.clone(),
                description: record.description.clone(),
                category: record.category.clone(),
                sub_category: record.sub_category.clone(),
                sequence: record.sequence,
            })
            .collect();
        published.reverse();
        published
    }
}

/// The arns a listing answered, in the order it answered them.
fn listed_arns(response: &Value) -> Vec<String> {
    response["managedNotificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("a listing must answer an array, got {response:?}"))
        .iter()
        .map(|entry| {
            entry["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("every listed entry must carry an arn, got {entry:?}"))
                .to_string()
        })
        .collect()
}

#[tokio::test]
async fn listing_reports_every_published_configuration_newest_first() {
    let emulator = Emulator::new();
    let expected = emulator.published_newest_first();
    assert!(
        expected.len() >= 2,
        "case published catalog: the service must publish a catalog to list, got {expected:?}"
    );
    assert!(
        expected.windows(2).all(|pair| pair[0].sequence > pair[1].sequence),
        "case published catalog: entries must be newest-first, got {:?}",
        expected.iter().map(|entry| entry.sequence).collect::<Vec<_>>()
    );

    let (status, response) = emulator.list_managed("").await;
    assert_eq!(
        status, 200,
        "case unfiltered listing: expected 200, got {status} {response:?}"
    );
    assert_eq!(
        response["nextToken"],
        Value::Null,
        "case unfiltered listing: a listing that fits one page carries no token, got {response:?}"
    );

    let entries = response["managedNotificationConfigurations"]
        .as_array()
        .unwrap_or_else(|| panic!("case unfiltered listing: expected an array, got {response:?}"))
        .clone();
    assert_eq!(
        entries.len(),
        expected.len(),
        "case unfiltered listing: expected {} published entries, got {entries:?}",
        expected.len()
    );

    for (entry, want) in entries.iter().zip(expected.iter()) {
        assert_eq!(
            entry["arn"].as_str(),
            Some(want.arn.as_str()),
            "case {}: expected arn {}, got {entry:?}",
            want.name,
            want.arn
        );
        assert_eq!(
            entry["name"].as_str(),
            Some(want.name.as_str()),
            "case {}: expected name {}, got {entry:?}",
            want.name,
            want.name
        );
        assert_eq!(
            entry["description"].as_str(),
            Some(want.description.as_str()),
            "case {}: expected the published description, got {entry:?}",
            want.name
        );
        assert!(
            managed_configuration_arn_segments(&want.arn).is_some(),
            "case {}: the arn must match the modelled pattern, got {:?}",
            want.name,
            want.arn
        );
    }
}

#[tokio::test]
async fn reading_a_listed_entry_answers_that_entry() {
    let emulator = Emulator::new();
    // The table spans EVERY published entry, so no row is the one case a
    // recording happened to name.
    for want in emulator.published_newest_first() {
        let (status, response) = emulator.get_managed(&want.arn).await;
        assert_eq!(
            status, 200,
            "case {}: expected 200 for a published arn, got {status} {response:?}",
            want.name
        );
        assert_eq!(
            response["arn"].as_str(),
            Some(want.arn.as_str()),
            "case {}: expected arn {}, got {response:?}",
            want.name,
            want.arn
        );
        assert_eq!(
            response["name"].as_str(),
            Some(want.name.as_str()),
            "case {}: expected name {}, got {response:?}",
            want.name,
            want.name
        );
        assert_eq!(
            response["description"].as_str(),
            Some(want.description.as_str()),
            "case {}: expected the listed description, got {response:?}",
            want.name
        );

        // `category` and `subCategory` are the arn's own two segments, so a read
        // can never disagree with the arn it was asked about.
        let (category, sub_category) = managed_configuration_arn_segments(&want.arn)
            .unwrap_or_else(|| {
                panic!(
                    "case {}: the arn must match the modelled pattern, got {:?}",
                    want.name, want.arn
                )
            });
        assert_eq!(
            response["category"].as_str(),
            Some(category.as_str()),
            "case {}: expected category {category}, got {response:?}",
            want.name
        );
        assert_eq!(
            response["subCategory"].as_str(),
            Some(sub_category.as_str()),
            "case {}: expected subCategory {sub_category}, got {response:?}",
            want.name
        );
        assert_eq!(
            (
                response["category"].as_str().unwrap_or_default().to_string(),
                response["subCategory"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string(),
            ),
            (want.category.clone(), want.sub_category.clone()),
            "case {}: expected the stored category/subCategory, got {response:?}",
            want.name
        );
    }
}

#[tokio::test]
async fn paging_walks_the_catalog_once_at_every_page_size() {
    let emulator = Emulator::new();
    let expected: Vec<String> = emulator
        .published_newest_first()
        .into_iter()
        .map(|entry| entry.arn)
        .collect();

    // Rows differ in page size: the modelled `maxResults` minimum (1), a size
    // that splits the catalog, exactly the catalog's size, one past it, and the
    // modelled maximum (100).
    let sizes = [1usize, 2, expected.len(), expected.len() + 1, 100];
    for size in sizes {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0usize;
        loop {
            pages += 1;
            assert!(
                pages <= expected.len() + 2,
                "case maxResults={size}: paging did not terminate, saw {seen:?}"
            );
            let query = match &token {
                Some(value) => format!("maxResults={size}&nextToken={value}"),
                None => format!("maxResults={size}"),
            };
            let (status, response) = emulator.list_managed(&query).await;
            assert_eq!(
                status, 200,
                "case maxResults={size}: expected 200, got {status} {response:?}"
            );
            let page = listed_arns(&response);
            assert!(
                page.len() <= size,
                "case maxResults={size}: a page must not exceed the requested size, got {page:?}"
            );
            seen.extend(page.iter().cloned());
            token = response["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                assert!(
                    !page.is_empty() || expected.is_empty(),
                    "case maxResults={size}: the final page must carry entries, got {response:?}"
                );
                break;
            }
        }
        assert_eq!(
            seen, expected,
            "case maxResults={size}: paging must walk the catalog once in listing order, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn the_channel_identifier_filter_selects_on_the_stored_associations() {
    let emulator = Emulator::new();
    let every_arn: Vec<String> = emulator
        .published_newest_first()
        .into_iter()
        .map(|entry| entry.arn)
        .collect();

    // Rows differ in the kind of `ChannelIdentifier` the pattern allows: the
    // account contact the catalog is seeded with, the other account contacts it
    // is not, and the arn forms of a chatbot, console-app and contact channel.
    let cases: Vec<(&str, bool)> = vec![
        (MANAGED_DEFAULT_CHANNEL_IDENTIFIER, true),
        ("ACCOUNT_ALTERNATE_BILLING", false),
        ("ACCOUNT_ALTERNATE_OPERATIONS", false),
        ("ACCOUNT_ALTERNATE_SECURITY", false),
        (
            "arn:aws:chatbot:us-east-1:000000000000:chat-configuration/slack-channel/ops",
            false,
        ),
        (
            "arn:aws:consoleapp:us-east-1:000000000000:device/00000000-0000-0000-0000-000000000000",
            false,
        ),
        (
            "arn:aws:notifications-contacts:us-east-1:000000000000:emailcontact/team.ops@example.com",
            false,
        ),
    ];

    for (identifier, expect_catalog) in cases {
        let query = format!("channelIdentifier={}", encode_path_label(identifier));
        let (status, response) = emulator.list_managed(&query).await;
        assert_eq!(
            status, 200,
            "case channelIdentifier={identifier}: expected 200, got {status} {response:?}"
        );
        let arns = listed_arns(&response);
        let want: Vec<String> = if expect_catalog {
            every_arn.clone()
        } else {
            Vec::new()
        };
        assert_eq!(
            arns, want,
            "case channelIdentifier={identifier}: expected {want:?}, got {arns:?}"
        );
    }

    // A row where only SOME entries carry the identifier: the filter is a
    // per-record lookup on the stored associations, not an all-or-nothing match.
    // The association is dropped in the store, since the operation that
    // disassociates a managed channel is a stub of its own.
    let dropped = every_arn
        .first()
        .cloned()
        .expect("the catalog has an entry to drop the association from");
    {
        let state = emulator
            .engine
            .state::<ComAmazonawsNotificationsNotificationsState>();
        let mut store = state.lock();
        store
            .managed_configurations
            .get_mut(&dropped)
            .expect("the dropped entry is published")
            .channel_identifiers
            .remove(MANAGED_DEFAULT_CHANNEL_IDENTIFIER);
    }
    let query = format!(
        "channelIdentifier={}",
        encode_path_label(MANAGED_DEFAULT_CHANNEL_IDENTIFIER)
    );
    let (status, response) = emulator.list_managed(&query).await;
    assert_eq!(
        status, 200,
        "case one entry unassociated: expected 200, got {status} {response:?}"
    );
    let want: Vec<String> = every_arn
        .iter()
        .filter(|arn| **arn != dropped)
        .cloned()
        .collect();
    assert_eq!(
        listed_arns(&response),
        want,
        "case one entry unassociated: expected every entry but {dropped}, got {response:?}"
    );
}

#[tokio::test]
async fn an_unpublished_arn_is_not_found() {
    let emulator = Emulator::new();
    let published: Vec<String> = emulator
        .published_newest_first()
        .into_iter()
        .map(|entry| entry.arn)
        .collect();

    // Rows differ in which arn segment is unpublished: an unknown sub-category
    // under a published category, an unknown category, and both unknown — each
    // well-formed under the modelled pattern, so the refusal is the not-found
    // the model declares rather than a validation failure.
    let cases = [
        (
            "unpublished sub-category",
            "managed-notification-configuration/category/AWS-Health/sub-category/NoSuchSubCategory",
        ),
        (
            "unpublished category",
            "managed-notification-configuration/category/NoSuchCategory/sub-category/Issue",
        ),
        (
            "both segments unpublished",
            "managed-notification-configuration/category/Zzz/sub-category/Qqq",
        ),
    ];

    for (case, resource) in cases {
        let arn = format!("arn:aws:notifications::000000000000:{resource}");
        assert!(
            !published.contains(&arn),
            "case {case}: the test arn must not be published, got {published:?}"
        );
        let (status, response) = emulator.get_managed(&arn).await;
        assert_eq!(
            status, 404,
            "case {case}: expected 404, got {status} {response:?}"
        );
        assert!(
            error_code(&response).contains("ResourceNotFoundException"),
            "case {case}: expected ResourceNotFoundException, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let emulator = Emulator::new();

    // `maxResults` is modelled `@range(min: 1, max: 100)`, so each published
    // bound is served and one past it on either side is refused.
    for size in [1, 100] {
        let (status, response) = emulator.list_managed(&format!("maxResults={size}")).await;
        assert_eq!(
            status, 200,
            "case maxResults={size} (a published bound): expected 200, got {status} {response:?}"
        );
    }
    for size in [0, 101] {
        let (status, response) = emulator.list_managed(&format!("maxResults={size}")).await;
        assert_eq!(
            status, 400,
            "case maxResults={size} (past a published bound): expected 400, got {status} {response:?}"
        );
        assert!(
            error_code(&response).contains("ValidationException"),
            "case maxResults={size}: expected ValidationException, got {response:?}"
        );
    }
}

#[tokio::test]
async fn a_token_this_service_did_not_issue_is_refused() {
    let emulator = Emulator::new();

    // Rows differ in how the token is wrong while still satisfying the modelled
    // `NextToken` pattern (`^[\w+-/=]+$`): not hex, hex of the wrong width, and
    // hex of the right width carrying another process's tag.
    let cases = [
        ("not a codec token", "notatoken"),
        ("hex of the wrong width", "00ff00ff"),
        (
            "right width, foreign tag",
            "ffffffffffffffff0000000000000001",
        ),
    ];

    for (case, token) in cases {
        let (status, response) = emulator.list_managed(&format!("nextToken={token}")).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400, got {status} {response:?}"
        );
        assert!(
            error_code(&response).contains("ValidationException"),
            "case {case}: expected ValidationException, got {response:?}"
        );
    }
}

#[tokio::test]
async fn listing_and_reading_leave_the_catalog_unchanged() {
    let emulator = Emulator::new();
    let before = emulator.published_newest_first();

    let (_, first) = emulator.list_managed("").await;
    for arn in listed_arns(&first) {
        let (status, _) = emulator.get_managed(&arn).await;
        assert_eq!(status, 200, "case repeat read of {arn}: expected 200");
    }
    let (_, second) = emulator.list_managed("maxResults=1").await;
    assert_eq!(
        listed_arns(&second),
        listed_arns(&first)[..1].to_vec(),
        "case first page of one: a page must lead with the newest entry, got {second:?}"
    );
    let (_, third) = emulator.list_managed("").await;

    assert_eq!(
        listed_arns(&first),
        listed_arns(&third),
        "case repeated listing: a read must not change what a later caller sees"
    );
    assert_eq!(
        before,
        emulator.published_newest_first(),
        "case repeated listing: the stored catalog must be untouched by reads"
    );
}
