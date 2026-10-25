// SPDX-License-Identifier: CC0-1.0

//! `ListSAMLProviderTags`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of tags that were
//! PERSISTED by `CreateSAMLProvider`, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the other suites in this
//! crate share, and the metadata document a create needs comes from
//! `tests/all/saml_providers.rs`.
//!
//! The rule under test is that the listing REPORTS the tag set the named provider
//! persists, sorted by tag key, paged over that same key order by `MaxItems` and
//! `Marker` so every tag is reported exactly once, and writes nothing — while a value
//! that is not an ARN, an ARN naming no held provider, and a `Marker` this service did
//! not issue for that provider's tags are each refused. Each suite is a TABLE whose rows
//! differ along what the rule generalises over (the tag count from none to the published
//! maximum, the order the tags arrive in, the alphabet of the keys and values, and the
//! page size), so no single request is the specification.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::saml_providers::{MetadataForm, expected_arn, metadata_document};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// One provider's tag set, as the create that persists it carries it.
#[derive(Debug, Clone)]
struct TagCase {
    /// The provider's `Name`, which its ARN is derived from.
    name: String,
    /// The tags the create carries, in REQUEST order — which is not the sorted order
    /// for most rows, because the sorting is the rule under test.
    tags: Vec<(String, String)>,
}

impl TagCase {
    fn new(name: &str, tags: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    /// The create's parameters: the provider's name, a metadata document, and the tags.
    fn create_params(&self) -> Vec<(String, String)> {
        let document = metadata_document(
            MetadataForm::Entity,
            "https://idp.example.com/saml",
            None,
        );
        let mut params = vec![
            param("Name", &self.name),
            param("SAMLMetadataDocument", &document),
        ];
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }

    /// The tag keys in the order the listing must report them: ascending by key.
    fn sorted_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.tags.iter().map(|(key, _)| key.clone()).collect();
        keys.sort();
        keys
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags.iter().cloned().collect()
    }
}

/// The published maximum number of tags one IAM resource may carry: `tagListType` is
/// len 0..50 (`smithy_query(query="shape", shape="tagListType")`).
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// A tag set of `count` tags whose keys arrive in an order that is NOT the sorted one
/// (the stride walks the key space), so a listing that reported insertion order would
/// differ from one that sorted.
fn scrambled_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| {
            let key = (index * 17) % count.max(1);
            (format!("key-{key:02}"), format!("value-{key}"))
        })
        .collect()
}

/// The tag-set table. Rows span the tag count (none, one, a few, the published
/// maximum), the arrival order (sorted, reversed, scrambled), and the alphabet
/// `tagKeyType` and `tagValueType` publish: both are
/// `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` / `*$` (`smithy_query(query="shape",
/// shape="Tag")`), and a VALUE may be empty because `tagValueType` is len 0..256.
fn tag_cases() -> Vec<TagCase> {
    vec![
        // "If no tags are attached to the specified resource, the response contains an
        // empty list" (`ListSAMLProviderTags$Tags`).
        TagCase::new("NoTags", &[]),
        // `Tag$Key`'s own documented example ("Department or Cost Center are common
        // choices").
        TagCase::new("OneTag", &[("Department", "Engineering")]),
        TagCase::new(
            "ArrivingReversed",
            &[("zone", "c"), ("owner", "ops"), ("app", "portal")],
        ),
        TagCase::new(
            "ArrivingUnsorted",
            &[
                ("zone", "c"),
                ("Application", "portal"),
                ("owner", "ops"),
                ("Account", "42"),
            ],
        ),
        TagCase::new(
            "AlphabetSpanning",
            &[
                ("Cost Center", "Human Resources"),
                ("owner@example.com", "ops"),
                ("project/name", "emulator"),
                ("key=value", "="),
                ("dash-key", "-"),
                ("dot.key", "."),
                ("colon:key", ":"),
                ("plus+key", "+"),
                ("under_score", "_"),
                ("Ünicode", "Grüße"),
                ("7", "seven"),
                ("empty-value", ""),
            ],
        ),
        TagCase {
            name: "MaxTags".to_string(),
            tags: scrambled_tags(MAX_TAGS_PER_RESOURCE),
        },
    ]
}

/// Create the case's provider and answer the ARN it is held under.
async fn persist(router: &MultiServiceRouter, case: &TagCase) -> String {
    let params = case.create_params();
    let (status, body) = call(router, "CreateSAMLProvider", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture provider must be created, got {status} with body {body}",
        case.name
    );
    let arn = element(&body, "SAMLProviderArn");
    assert_eq!(
        arn,
        expected_arn(&case.name),
        "case {:?}: the create must hold the provider under the ARN derived from its \
         name, got {arn:?}",
        case.name
    );
    arn
}

/// One `ListSAMLProviderTags` call, with an optional page size and marker.
async fn list_tags(
    router: &MultiServiceRouter,
    arn: &str,
    max_items: Option<i32>,
    marker: Option<&str>,
) -> (u16, String) {
    let mut params = vec![param("SAMLProviderArn", arn)];
    if let Some(size) = max_items {
        params.push(param("MaxItems", &size.to_string()));
    }
    if let Some(token) = marker {
        params.push(param("Marker", token));
    }
    call(router, "ListSAMLProviderTags", &borrowed(&params)).await
}

/// RULE: the listing reports the tag set the named provider PERSISTS, sorted by tag
/// key, and a set that fits one page is not truncated and carries no `Marker`.
#[tokio::test]
async fn the_listing_reports_the_persisted_tags_sorted_by_key() {
    let router = router();

    for case in tag_cases() {
        let arn = persist(&router, &case).await;

        let (status, body) = list_tags(&router, &arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            case.name
        );
        assert_eq!(
            elements(&body, "Key"),
            case.sorted_keys(),
            "case {:?}: the listing must report every tag key sorted by key, got {body}",
            case.name
        );
        assert_eq!(
            reported_tags(&body),
            case.expected_tags(),
            "case {:?}: the listing must report the persisted tags, got {body}",
            case.name
        );
        // No tag set can exceed the published maximum of 50, and `MaxItems` defaults to
        // 100 ("if you do not include this parameter, the number of items defaults to
        // 100"), so an unpaged listing is never truncated.
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {:?}: a whole tag set fits one default page, got {body}",
            case.name
        );
        assert!(
            elements(&body, "Marker").is_empty(),
            "case {:?}: an untruncated page must carry no Marker, got {body}",
            case.name
        );
    }
}

/// RULE: paging walks the SAME key order, so whatever the page size every tag is
/// reported exactly once, in ascending key order, and only a truncated page carries the
/// `Marker` that resumes after it.
#[tokio::test]
async fn paging_reports_every_tag_exactly_once_in_key_order() {
    let router = router();

    for case in tag_cases() {
        let arn = persist(&router, &case).await;
        let expected = case.sorted_keys();
        // Rows span the page size: the published minimum, sizes that do and do not
        // divide the set, a size equal to the set, the published maximum, and the
        // absent-parameter default. `maxItemsType` is range 1..1000
        // (`smithy_query(query="operation", operation="ListSAMLProviderTags")`).
        let page_sizes: Vec<Option<i32>> = vec![
            Some(1),
            Some(2),
            Some(7),
            Some(expected.len().max(1) as i32),
            Some(1000),
            None,
        ];

        for page_size in page_sizes {
            let label = format!("{} with MaxItems={page_size:?}", case.name);
            let mut seen: Vec<String> = Vec::new();
            let mut marker: Option<String> = None;
            let mut pages = 0;

            loop {
                let (status, body) = list_tags(&router, &arn, page_size, marker.as_deref()).await;
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected HTTP 200, got {status} with body {body}"
                );
                let page = elements(&body, "Key");
                if let Some(size) = page_size {
                    assert!(
                        page.len() <= size as usize,
                        "case {label:?}: expected at most {size} tags on a page, got {}",
                        page.len()
                    );
                }
                seen.extend(page);

                let truncated = element(&body, "IsTruncated");
                let page_marker = elements(&body, "Marker");
                if truncated == "true" {
                    assert_eq!(
                        page_marker.len(),
                        1,
                        "case {label:?}: a truncated page must carry a Marker, got \
                         {page_marker:?}"
                    );
                    marker = Some(page_marker.into_iter().next().expect("a marker"));
                } else {
                    assert_eq!(
                        truncated, "false",
                        "case {label:?}: expected IsTruncated true or false, got \
                         {truncated:?}"
                    );
                    assert!(
                        page_marker.is_empty(),
                        "case {label:?}: a final page must carry no Marker, got \
                         {page_marker:?}"
                    );
                    break;
                }

                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {label:?}: pagination did not terminate after {pages} pages"
                );
            }

            assert_eq!(
                seen, expected,
                "case {label:?}: expected every tag exactly once in key order, got {seen:?}"
            );
        }
    }
}

/// RULE: the listing is a READ — two calls report the same tags, and the provider the
/// reporting half (`GetSAMLProvider`) reads back is unchanged by having been listed.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let router = router();

    for case in tag_cases() {
        let arn = persist(&router, &case).await;

        let (_, first) = list_tags(&router, &arn, None, None).await;
        // A paged walk, which issues markers, must leave the set alone too.
        let (_, paged) = list_tags(&router, &arn, Some(1), None).await;
        let (_, second) = list_tags(&router, &arn, None, None).await;
        assert_eq!(
            elements(&first, "Key"),
            elements(&second, "Key"),
            "case {:?}: two listings must report the same tag keys in the same order, \
             the paged walk answered {paged}",
            case.name
        );
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {:?}: two listings must report the same tags, got {first} then \
             {second}",
            case.name
        );

        let read_params = vec![param("SAMLProviderArn", &arn)];
        let (status, read) = call(&router, "GetSAMLProvider", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the provider must still read back, got {status} with body {read}",
            case.name
        );
        assert_eq!(
            reported_tags(&read),
            case.expected_tags(),
            "case {:?}: listing the tags must leave the held tags alone, got {read}",
            case.name
        );
    }
}

/// RULE: a value that is not an ARN is an invalid input (HTTP 400), while a well-formed
/// ARN naming no provider the account holds is the different answer that the entity does
/// not exist (HTTP 404).
#[tokio::test]
async fn a_non_arn_is_invalid_and_an_unheld_provider_is_not_found() {
    let router = router();
    persist(&router, &tag_cases()[1]).await;

    // Values past `arnType`'s 20-character minimum, so the HANDLER is what refuses
    // them rather than the boundary validator.
    for value in [
        "OneTagButNotAnArnAtAll",
        "iam::saml-provider/OneTag",
        "arn-but-not-really-one/OneTag",
    ] {
        let (status, body) = list_tags(&router, value, None, None).await;
        assert_eq!(
            status, 400,
            "case {value:?}: a value that is not an ARN must be refused with HTTP 400, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );
    }

    // A value SHORTER than `arnType`'s published 20-character minimum is refused by the
    // modelled constraint, before the operation runs at all.
    let (status, body) = list_tags(&router, "OneTag", None, None).await;
    assert_eq!(
        status, 400,
        "a value shorter than arnType's minimum must be refused with HTTP 400, got \
         {status} with body {body}"
    );

    for name in ["NeverCreated", "another.provider-1"] {
        let (status, body) = list_tags(&router, &expected_arn(name), None, None).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
    }
}

/// RULE: a `Marker` names a position in ONE provider's tag order, so a token this
/// service did not issue for that provider's tags — a foreign string, or a token it
/// issued while paging ANOTHER provider's tags — is refused rather than read as "start
/// from the beginning".
#[tokio::test]
async fn a_marker_this_service_did_not_issue_for_these_tags_is_refused() {
    let router = router();
    let cases = tag_cases();
    let paged = cases
        .iter()
        .find(|case| case.tags.len() > 1)
        .expect("a case with more than one tag");
    let other = cases
        .last()
        .expect("a second case")
        .clone();
    let arn = persist(&router, paged).await;
    let other_arn = persist(&router, &other).await;

    for bogus in ["not-a-marker", "zz", "0g"] {
        let (status, body) = list_tags(&router, &arn, None, Some(bogus)).await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {bogus:?}: expected InvalidInput, got {body}"
        );
    }

    // A marker issued while paging the OTHER provider's tags names no position in this
    // provider's.
    let (status, first_page) = list_tags(&router, &other_arn, Some(1), None).await;
    assert_eq!(
        status, 200,
        "the fixture page must be served, got {status} with body {first_page}"
    );
    let foreign = element(&first_page, "Marker");
    let (status, body) = list_tags(&router, &arn, Some(1), Some(&foreign)).await;
    assert_eq!(
        status, 400,
        "case {foreign:?}: a marker issued for another provider's tags must be refused \
         with HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidInput",
        "case {foreign:?}: expected InvalidInput, got {body}"
    );

    // And the provider's OWN marker resumes the walk: the page after it is the rest of
    // the set, in key order.
    let (_, own_first) = list_tags(&router, &arn, Some(1), None).await;
    let own_marker = element(&own_first, "Marker");
    let (status, rest) = list_tags(&router, &arn, None, Some(&own_marker)).await;
    assert_eq!(
        status, 200,
        "a marker this service issued must be accepted, got {status} with body {rest}"
    );
    assert_eq!(
        elements(&rest, "Key"),
        paged.sorted_keys()[1..].to_vec(),
        "the page after the first marker must be the rest of the set in key order, got \
         {rest}"
    );
}

/// RULE: `MaxItems` outside its published range is refused before the operation runs,
/// and each published bound itself is accepted.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    let arn = persist(&router, &tag_cases()[1]).await;

    // One past each bound of `maxItemsType` (range 1..1000).
    for out_of_range in [0, 1001, -1] {
        let (status, body) = list_tags(&router, &arn, Some(out_of_range), None).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
    for in_range in [1, 1000] {
        let (status, body) = list_tags(&router, &arn, Some(in_range), None).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected HTTP 200, got {status} with body {body}"
        );
    }
}
