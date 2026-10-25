// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the TAG rule `CreateDomainName` holds a request to.
//!
//! The RULE under test comes from the `tags` member's own documentation ("The tag key can
//! be up to 128 characters and must not start with aws:. The tag value can be up to 256
//! characters"), which this service applies through one check for every path that stores
//! a tag: so the table below walks each documented bound AT the limit and ONE PAST it,
//! the reserved prefix, a non-ASCII pair, and the per-request entry count at and past
//! [`MAX_TAGS`]. Every accepted row is read back through `GetDomainName`, which is where
//! a stored tag is proved stored rather than echoed, and every refused row is checked to
//! have stored nothing at all.
//!
//! The shared bound itself (`state::bound_tag`) is already walked against `CreateRestApi`
//! and `TagResource` by `rest_api_lifecycle` and `resource_tagging`; what is specific here
//! — and what these rows are kept for — is that the CREATE of a domain name applies it at
//! all and that what survives the check reaches the stored record.
//!
//! The sibling `domain_name_collection` suite asserts what a created domain name reports
//! and how it is addressed; `domain_name_lifecycle` asserts the account's quota.
//! [`Gateway`], [`Answer`], [`assert_failure`] and [`string_member`] are this crate's one
//! wire-test harness, reused rather than standing a second emulator up another way.

use serde_json::{json, Map, Value};
use service_com_amazonaws_apigateway_backplanecontrolservice::state::{
    MAX_TAGS, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS,
};

use crate::rest_api_lifecycle::{assert_failure, string_member, Answer, Gateway};

/// `CreateDomainName` with `body`.
async fn create_domain(gateway: &Gateway, body: Value) -> Answer {
    gateway.call("POST", "/domainnames", Some(body)).await
}

/// `GetDomainName` for `domain_name`.
async fn get_domain(gateway: &Gateway, domain_name: &str) -> Answer {
    gateway
        .call("GET", &format!("/domainnames/{domain_name}"), None)
        .await
}

/// One tag collection a create carries, and what this service does with it.
struct TagCase {
    /// Names the row in every assertion.
    case: &'static str,
    /// The `tags` map, as `(key, value)` pairs so a row can build a key or a value AT a
    /// bound without a literal of that length.
    tags: Vec<(String, String)>,
    /// `true` when the collection must be STORED (and read back), `false` when the create
    /// must be refused as the bad request it is.
    stored: bool,
}

/// The tag collections the documented rule has to hold over.
fn tag_cases() -> Vec<TagCase> {
    let pair = |key: &str, value: &str| vec![(key.to_string(), value.to_string())];
    vec![
        TagCase {
            case: "no tags at all",
            tags: Vec::new(),
            stored: true,
        },
        TagCase {
            case: "a key AT the 128-character bound, beside a value AT the 256-character one",
            tags: pair(
                &"k".repeat(MAX_TAG_KEY_CHARS),
                &"v".repeat(MAX_TAG_VALUE_CHARS),
            ),
            stored: true,
        },
        TagCase {
            case: "a key ONE PAST the 128-character bound",
            tags: pair(&"k".repeat(MAX_TAG_KEY_CHARS + 1), "past-the-key-bound"),
            stored: false,
        },
        TagCase {
            case: "a value ONE PAST the 256-character bound",
            tags: pair("past-the-value-bound", &"v".repeat(MAX_TAG_VALUE_CHARS + 1)),
            stored: false,
        },
        TagCase {
            case: "the namespace AWS reserves for itself",
            tags: pair("aws:owner", "someone"),
            stored: false,
        },
        TagCase {
            case: "a non-ASCII key and value, which AWS round-trips",
            tags: pair("équipe", "plate-forme ✓"),
            stored: true,
        },
        TagCase {
            case: "entries AT the per-request bound",
            tags: (0..MAX_TAGS)
                .map(|index| (format!("key-{index}"), format!("value-{index}")))
                .collect(),
            stored: true,
        },
        TagCase {
            case: "entries ONE PAST the per-request bound",
            tags: (0..=MAX_TAGS)
                .map(|index| (format!("key-{index}"), format!("value-{index}")))
                .collect(),
            stored: false,
        },
    ]
}

#[tokio::test]
async fn the_tags_a_create_carries_follow_the_documented_tag_rule() {
    let gateway = Gateway::new();
    for (index, expectation) in tag_cases().into_iter().enumerate() {
        let case = expectation.case;
        let domain_name = format!("tags-{index}.example.com");
        let tags: Map<String, Value> = expectation
            .tags
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect();
        let answer = create_domain(
            &gateway,
            json!({ "domainName": domain_name, "tags": Value::Object(tags.clone()) }),
        )
        .await;

        if !expectation.stored {
            assert_failure(&answer, 400, "BadRequestException", case);
            // A refused create stores nothing, so the name it named resolves to nothing.
            let read = get_domain(&gateway, &domain_name).await;
            assert_failure(&read, 404, "NotFoundException", case);
            continue;
        }

        assert_eq!(
            answer.status, 201,
            "case {case:?}: expected 201, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.body["tags"],
            Value::Object(tags.clone()),
            "case {case:?}: expected the tags it was created with, got {:?}",
            answer.body["tags"]
        );
        // The collection is STORED, not echoed: a read of the record reports it too.
        let read = get_domain(&gateway, &domain_name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected GetDomainName 200, got {} with {:?}",
            read.status, read.body
        );
        assert_eq!(
            read.body["tags"],
            Value::Object(tags),
            "case {case:?}: expected the stored tags on the read, got {:?}",
            read.body["tags"]
        );
        assert_eq!(
            string_member(&read.body, "domainName", case),
            domain_name,
            "case {case:?}: expected the record its name addresses, got {:?}",
            read.body
        );
    }
}
