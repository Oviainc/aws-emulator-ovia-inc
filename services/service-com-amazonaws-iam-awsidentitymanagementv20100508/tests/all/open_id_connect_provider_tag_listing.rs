// SPDX-License-Identifier: CC0-1.0

//! `ListOpenIDConnectProviderTags`, end to end over the awsQuery wire.
//!
//! RULE under test: the listing REPORTS the tag set the named provider persists, sorted
//! by tag key ("the returned list of tags is sorted by tag key"), paged over that same
//! key order by `MaxItems` and `Marker` so every tag is reported exactly once, and it
//! writes nothing — while a value that is not an ARN, an ARN naming no held provider, a
//! `Marker` this service did not issue for that provider's tags and a caller whose
//! identity-based policies do not allow the read are each refused.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of tags that were PERSISTED
//! by `CreateOpenIDConnectProvider` and `TagOpenIDConnectProvider`, never a handler
//! called directly. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns,
//! the parameter and tag readers are `tests/all/role_lifecycle.rs`'s, the tag-list
//! builders are `tests/all/role_tags.rs`'s, the distinct-tag fixture is
//! `tests/all/role_tag_removal.rs`'s, the provider fixtures come from
//! `tests/all/open_id_connect_providers.rs` and
//! `tests/all/open_id_connect_provider_tag_updates.rs`, and the signing principal and
//! policy documents are `tests/all/identity_policy_enforcement.rs`'s — so a request is
//! built and read exactly one way across this crate.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the tag
//! count from none to the published maximum, the order the tags arrive in, the alphabet
//! and the published length bounds of the keys and values, the page size, the shape of
//! the URL the provider is named by, and the statements in force on the caller.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state::MAX_TAGS_PER_ENTITY;
use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::open_id_connect_provider_tag_updates::{create_provider, tag_provider};
use crate::open_id_connect_providers::{absence_message, expected_arn};
use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tag_removal::fixture;
use crate::role_tags::{expected_map, tags};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// One provider's tag set, as the create that persists it carries it.
#[derive(Debug, Clone)]
struct TagCase {
    /// What the row is called in an assertion message.
    name: String,
    /// The tags the create carries, in REQUEST order — which is not the sorted order for
    /// most rows, because the sorting is the rule under test.
    tags: Vec<(String, String)>,
}

impl TagCase {
    fn new(name: &str, pairs: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            tags: tags(pairs),
        }
    }

    /// The tag keys in the order the listing must report them: ascending by key.
    fn sorted_keys(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.tags.iter().map(|(key, _)| key.clone()).collect();
        keys.sort();
        keys
    }

    fn expected(&self) -> BTreeMap<String, String> {
        expected_map(&self.tags)
    }
}

/// A tag set of `count` tags whose keys arrive in an order that is NOT the sorted one
/// (the stride walks the key space), so a listing that reported insertion order would
/// differ from one that sorted.
fn scrambled(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| {
            let key = (index * 17) % count.max(1);
            (format!("key-{key:02}"), format!("value-{key:02}"))
        })
        .collect()
}

/// The tag-set table. Rows span the tag count (none, one, a few, the published maximum),
/// the arrival order (reversed, scrambled), and the alphabet and length bounds
/// `tagKeyType` (len 1..128, pattern `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$`) and `tagValueType`
/// (len 0..256, the same pattern with `*`) publish
/// (`smithy_query(query="shape", shape="Tag")`).
fn tag_cases() -> Vec<TagCase> {
    let longest_key = "k".repeat(128);
    let longest_value = "v".repeat(256);
    vec![
        // "If no tags are attached to the specified resource, the response contains an
        // empty list" (`ListOpenIDConnectProviderTags$Tags`).
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
        TagCase::new(
            "LengthBounds",
            &[
                (longest_key.as_str(), "at the key bound"),
                ("longest-value", longest_value.as_str()),
                ("x", ""),
            ],
        ),
        TagCase {
            name: "MaxTags".to_string(),
            tags: fixture(MAX_TAGS_PER_ENTITY),
        },
        TagCase {
            name: "MaxTagsScrambled".to_string(),
            tags: scrambled(MAX_TAGS_PER_ENTITY),
        },
    ]
}

/// One `ListOpenIDConnectProviderTags` call, with an optional page size and marker.
async fn list_tags(
    router: &MultiServiceRouter,
    arn: &str,
    max_items: Option<i32>,
    marker: Option<&str>,
) -> (u16, String) {
    list_tags_signed(router, None, arn, max_items, marker).await
}

/// One `ListOpenIDConnectProviderTags` call presented as signed by `access_key_id`.
async fn list_tags_signed(
    router: &MultiServiceRouter,
    access_key_id: Option<&str>,
    arn: &str,
    max_items: Option<i32>,
    marker: Option<&str>,
) -> (u16, String) {
    let mut params = vec![param("OpenIDConnectProviderArn", arn)];
    if let Some(size) = max_items {
        params.push(param("MaxItems", &size.to_string()));
    }
    if let Some(token) = marker {
        params.push(param("Marker", token));
    }
    call_signed_by(
        router,
        access_key_id,
        "ListOpenIDConnectProviderTags",
        &borrowed(&params),
    )
    .await
}

/// RULE: the listing reports the tag set the named provider PERSISTS, sorted by tag key,
/// and a set that fits one page is not truncated and carries no `Marker`.
#[tokio::test]
async fn the_listing_reports_the_persisted_tags_sorted_by_key() {
    let router = router();

    for (index, case) in tag_cases().iter().enumerate() {
        let arn = create_provider(
            &router,
            &format!("https://sorted-{index}.example.org/realms/main"),
            &case.tags,
        )
        .await;

        let (status, body) = list_tags(&router, &arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {body}",
            case.name
        );
        assert_eq!(
            elements(&body, "Key"),
            case.sorted_keys(),
            "case {:?}: expected every tag key sorted by key {:?}, got {body}",
            case.name,
            case.sorted_keys()
        );
        assert_eq!(
            reported_tags(&body),
            case.expected(),
            "case {:?}: expected the persisted tags {:?}, got {body}",
            case.name,
            case.expected()
        );
        // No tag set can exceed the published maximum of 50, and `MaxItems` defaults to
        // 100 ("if you do not include this parameter, the number of items defaults to
        // 100"), so an unpaged listing is never truncated.
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {:?}: expected a whole tag set to fit one default page, got {body}",
            case.name
        );
        assert!(
            elements(&body, "Marker").is_empty(),
            "case {:?}: expected an untruncated page to carry no Marker, got {body}",
            case.name
        );
    }
}

/// One row: a provider created with `initial`, then `writes` successive
/// `TagOpenIDConnectProvider` requests, whose merged result the listing must report.
struct WriteCase {
    name: &'static str,
    initial: Vec<(String, String)>,
    writes: Vec<Vec<(String, String)>>,
}

/// RULE: the listing reports the tag set the provider persists however that set got
/// there, so the tags a later `TagOpenIDConnectProvider` merged in are reported beside
/// the ones the create stored, and an overwritten key is reported once with its new
/// value. Rows span the held count, whether the written key is new or held, and the
/// number of writes.
#[tokio::test]
async fn the_listing_reports_what_a_later_tag_write_persisted() {
    let router = router();

    let rows = [
        WriteCase {
            name: "a provider created with no tags gains one",
            initial: Vec::new(),
            writes: vec![tags(&[("team", "federation")])],
        },
        WriteCase {
            name: "a new key is reported beside the created ones",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            writes: vec![tags(&[("owner", "identity")])],
        },
        WriteCase {
            name: "an overwritten key is reported once with its new value",
            initial: tags(&[("team", "federation"), ("env", "prod")]),
            writes: vec![tags(&[("env", "staging")])],
        },
        WriteCase {
            name: "successive writes accumulate, the last value winning",
            initial: tags(&[("a", "1")]),
            writes: vec![
                tags(&[("b", "2")]),
                tags(&[("c", "3")]),
                tags(&[("b", "two")]),
            ],
        },
        WriteCase {
            name: "a write that fills the provider to the published quota",
            initial: Vec::new(),
            writes: vec![fixture(MAX_TAGS_PER_ENTITY)],
        },
    ];

    for (index, row) in rows.iter().enumerate() {
        let WriteCase {
            name: case,
            initial,
            writes,
        } = row;
        let arn = create_provider(
            &router,
            &format!("https://written-{index}.example.org"),
            initial,
        )
        .await;

        let mut expected = expected_map(initial);
        for (write_index, list) in writes.iter().enumerate() {
            let (status, body) = tag_provider(&router, &arn, list).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the fixture tag write {write_index} must succeed, got \
                 {status} with body {body}"
            );
            expected.extend(list.iter().cloned());
        }

        let (status, body) = list_tags(&router, &arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            reported_tags(&body),
            expected,
            "case {case:?}: expected the merged tag set {expected:?}, got {body}"
        );
        let sorted: Vec<String> = expected.keys().cloned().collect();
        assert_eq!(
            elements(&body, "Key"),
            sorted,
            "case {case:?}: expected the reported keys sorted by key {sorted:?}, got {body}"
        );
    }
}

/// RULE: paging walks the SAME key order, so whatever the page size every tag is reported
/// exactly once, in ascending key order, and only a truncated page carries the `Marker`
/// that resumes after it.
#[tokio::test]
async fn paging_reports_every_tag_exactly_once_in_key_order() {
    let router = router();

    for (index, case) in tag_cases().iter().enumerate() {
        let arn = create_provider(
            &router,
            &format!("https://paged-{index}.example.org"),
            &case.tags,
        )
        .await;
        let expected = case.sorted_keys();
        // Rows span the page size: the published minimum, sizes that do and do not divide
        // the set, a size equal to the set, the published maximum, and the
        // absent-parameter default. `maxItemsType` is range 1..1000
        // (`smithy_query(query="operation", operation="ListOpenIDConnectProviderTags")`).
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
                        "case {label:?}: expected a truncated page to carry one Marker, \
                         got {page_marker:?}"
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
                        "case {label:?}: expected a final page to carry no Marker, got \
                         {page_marker:?}"
                    );
                    break;
                }

                pages += 1;
                assert!(
                    pages <= expected.len() + 1,
                    "case {label:?}: expected pagination to terminate, it did not after \
                     {pages} pages"
                );
            }

            assert_eq!(
                seen, expected,
                "case {label:?}: expected every tag exactly once in key order {expected:?}, \
                 got {seen:?}"
            );
        }
    }
}

/// RULE: the listing is a READ — two calls report the same tags, and the provider the
/// reporting half (`GetOpenIDConnectProvider`) reads back is unchanged by having been
/// listed.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let router = router();

    for (index, case) in tag_cases().iter().enumerate() {
        let arn = create_provider(
            &router,
            &format!("https://read-{index}.example.org"),
            &case.tags,
        )
        .await;

        let (_, first) = list_tags(&router, &arn, None, None).await;
        // A paged walk, which issues markers, must leave the set alone too.
        let (_, paged) = list_tags(&router, &arn, Some(1), None).await;
        let (_, second) = list_tags(&router, &arn, None, None).await;
        assert_eq!(
            elements(&first, "Key"),
            elements(&second, "Key"),
            "case {:?}: expected two listings to report the same tag keys in the same \
             order, the paged walk answered {paged}",
            case.name
        );
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {:?}: expected two listings to report the same tags, got {first} then \
             {second}",
            case.name
        );

        let (status, read) = call(
            &router,
            "GetOpenIDConnectProvider",
            &[("OpenIDConnectProviderArn", arn.as_str())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the provider to still read back, got {status} with body \
             {read}",
            case.name
        );
        assert_eq!(
            reported_tags(&read),
            case.expected(),
            "case {:?}: expected listing the tags to leave the held tags {:?} alone, got \
             {read}",
            case.name,
            case.expected()
        );
    }
}

/// RULE: a value that is not an ARN is an invalid input (HTTP 400), while a well-formed
/// ARN naming no provider the account holds is the different answer that the entity does
/// not exist (HTTP 404), whose message names the ARN the request carried.
#[tokio::test]
async fn a_non_arn_is_invalid_and_an_unheld_provider_is_not_found() {
    let router = router();
    let held = tags(&[("team", "federation")]);
    let url = "https://held.example.org";
    let arn = create_provider(&router, url, &held).await;

    // Values past `arnType`'s 20-character minimum, so the HANDLER is what refuses them
    // rather than the boundary validator.
    for value in [
        "https://held.example.org",
        "held.example.org-provider",
        "aws:iam::000000000000:oidc-provider/held.example.org",
    ] {
        let (status, body) = list_tags(&router, value, None, None).await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400 for a value that is not an ARN, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {value:?}: expected InvalidInput, got {body}"
        );
    }

    // A value SHORTER than `arnType`'s published 20-character minimum is refused by the
    // modelled constraint, before the operation runs at all.
    for short in ["held.example.org", "arn:aws:iam"] {
        let (status, body) = list_tags(&router, short, None, None).await;
        assert_eq!(
            status, 400,
            "case {short:?}: expected HTTP 400 for a value shorter than arnType's \
             minimum, got {status} with body {body}"
        );
    }

    // Rows: an ARN for a URL nothing holds, a near-miss of the held one, an ARN of
    // another IAM resource type, and one in another account.
    let unheld = [
        expected_arn("https://never.example.org"),
        expected_arn("https://held.example.org/x"),
        format!("arn:aws:iam::{}:role/held.example.org", account_id()),
        "arn:aws:iam::111122223333:oidc-provider/held.example.org".to_string(),
    ];
    for absent in unheld {
        // IAM names the ARN it found no provider under, so the message's variable part is
        // the ARN the request carried and its fixed text does not depend on the row.
        let expected_message = absence_message(&absent);
        let (status, body) = list_tags(&router, &absent, None, None).await;
        assert_eq!(
            status, 404,
            "case {absent:?}: expected HTTP 404 for an ARN the account holds no provider \
             under, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {absent:?}: expected NoSuchEntity, got {body}"
        );
        assert_eq!(
            element(&body, "Message"),
            expected_message,
            "case {absent:?}: expected the message {expected_message:?}, got {body}"
        );
    }

    // Every refusal above left the held provider's tags exactly as its create stored
    // them.
    let (status, body) = list_tags(&router, &arn, None, None).await;
    assert_eq!(
        status, 200,
        "expected the held provider's tags to still be served, got {status} with body \
         {body}"
    );
    assert_eq!(
        reported_tags(&body),
        expected_map(&held),
        "expected the held tags {:?}, got {body}",
        expected_map(&held)
    );
}

/// RULE: a `Marker` names a position in ONE provider's tag order, so a token this service
/// did not issue for that provider's tags — a foreign string, or a token it issued while
/// paging ANOTHER provider's tags — is refused rather than read as "start from the
/// beginning", while the provider's own marker resumes the walk.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_for_these_tags_is_refused() {
    let router = router();
    let paged = tags(&[("app", "portal"), ("owner", "ops"), ("zone", "c")]);
    let arn = create_provider(&router, "https://marker.example.org", &paged).await;
    let other_arn = create_provider(
        &router,
        "https://other-marker.example.org",
        &tags(&[("alpha", "1"), ("beta", "2")]),
    )
    .await;

    for bogus in ["not-a-marker", "zz", "0g", "  "] {
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
        "expected the fixture page to be served, got {status} with body {first_page}"
    );
    let foreign = element(&first_page, "Marker");
    let (status, body) = list_tags(&router, &arn, Some(1), Some(&foreign)).await;
    assert_eq!(
        status, 400,
        "case {foreign:?}: expected HTTP 400 for a marker issued for another provider's \
         tags, got {status} with body {body}"
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
        "expected a marker this service issued to be accepted, got {status} with body \
         {rest}"
    );
    let mut keys: Vec<String> = paged.iter().map(|(key, _)| key.clone()).collect();
    keys.sort();
    assert_eq!(
        elements(&rest, "Key"),
        keys[1..].to_vec(),
        "expected the page after the first marker to be the rest of the set in key order \
         {:?}, got {rest}",
        &keys[1..]
    );
}

/// RULE: `MaxItems` outside its published range is refused before the operation runs, and
/// each published bound itself is accepted.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    let arn = create_provider(
        &router,
        "https://bounds.example.org",
        &tags(&[("Department", "Engineering")]),
    )
    .await;

    // One past each bound of `maxItemsType` (range 1..1000).
    for out_of_range in [0, 1001, -1] {
        let (status, body) = list_tags(&router, &arn, Some(out_of_range), None).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
    // Each published bound itself.
    for in_range in [1, 1000] {
        let (status, body) = list_tags(&router, &arn, Some(in_range), None).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected HTTP 200, got {status} with body {body}"
        );
    }
}

/// What a signed row expects the call to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decided {
    /// HTTP 200: the policies in force permit the read.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// The documents in force on the row's caller, built from the row's own provider ARN —
/// which is only known once the provider exists.
type Policies = Box<dyn Fn(&str) -> Vec<(String, String)>>;

/// RULE: IAM authorizes its own API, so a signed listing is decided against the signing
/// principal's identity-based policies before any tag is reported — a `Deny` about the
/// request refuses it whatever any `Allow` says, an `Allow` with no such `Deny` serves
/// it, and a principal with neither is refused — and a refusal is HTTP 403 `AccessDenied`
/// naming the principal, the action and the provider.
#[tokio::test]
async fn a_signed_listing_is_decided_against_the_callers_identity_policies() {
    let router = router();
    let held = tags(&[("team", "federation"), ("env", "prod")]);

    let rows: Vec<(&str, Policies, Decided)> = vec![
        (
            "no identity-based policy at all",
            Box::new(|_arn: &str| Vec::new()),
            Decided::NoAllow,
        ),
        (
            "an Allow naming the action on every resource",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-list".to_string(),
                    statement("Allow", "iam:ListOpenIDConnectProviderTags", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow scoped to the provider the request addresses",
            Box::new(|arn: &str| {
                vec![(
                    "allow-this-provider".to_string(),
                    statement("Allow", "iam:ListOpenIDConnectProviderTags", arn),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow whose action element is a wildcard run",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-list-star".to_string(),
                    statement("Allow", "iam:List*", "*"),
                )]
            }),
            Decided::Served,
        ),
        (
            "an Allow for the tagging action only",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-tag".to_string(),
                    statement("Allow", "iam:TagOpenIDConnectProvider", "*"),
                )]
            }),
            Decided::NoAllow,
        ),
        (
            "an Allow scoped to another provider's ARN",
            Box::new(|_arn: &str| {
                vec![(
                    "allow-other-provider".to_string(),
                    statement(
                        "Allow",
                        "iam:ListOpenIDConnectProviderTags",
                        &expected_arn("https://elsewhere.example.org"),
                    ),
                )]
            }),
            Decided::NoAllow,
        ),
        (
            "a Deny naming the action beside an Allow",
            Box::new(|_arn: &str| {
                vec![
                    (
                        "allow-list".to_string(),
                        statement("Allow", "iam:ListOpenIDConnectProviderTags", "*"),
                    ),
                    (
                        "deny-list".to_string(),
                        statement("Deny", "iam:ListOpenIDConnectProviderTags", "*"),
                    ),
                ]
            }),
            Decided::ExplicitDeny,
        ),
        (
            "a Deny scoped to the provider the request addresses",
            Box::new(|arn: &str| {
                vec![
                    (
                        "allow-list".to_string(),
                        statement("Allow", "iam:ListOpenIDConnectProviderTags", "*"),
                    ),
                    (
                        "deny-this-provider".to_string(),
                        statement("Deny", "iam:ListOpenIDConnectProviderTags", arn),
                    ),
                ]
            }),
            Decided::ExplicitDeny,
        ),
    ];

    for (index, (case, policies, expect)) in rows.iter().enumerate() {
        let arn = create_provider(
            &router,
            &format!("https://signed-{index}.example.org"),
            &held,
        )
        .await;
        let user_name = format!("oidclister{index}");
        let access_key_id = principal(&router, case, &user_name, &policies(&arn)).await;

        let (status, body) = list_tags_signed(&router, Some(&access_key_id), &arn, None, None).await;

        match expect {
            Decided::Served => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected the call to be served, got {status} with body \
                     {body}"
                );
                assert_eq!(
                    reported_tags(&body),
                    expected_map(&held),
                    "case {case:?}: expected the persisted tags {:?}, got {body}",
                    expected_map(&held)
                );
            }
            Decided::ExplicitDeny | Decided::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected HTTP 403, got {status} with body {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected the error code AccessDenied, got {body}"
                );
                let reason = if *expect == Decided::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the \
                     iam:ListOpenIDConnectProviderTags action"
                        .to_string()
                };
                assert_access_denied_message(
                    case,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    "iam:ListOpenIDConnectProviderTags",
                    &format!("oidc-provider {arn}"),
                    &reason,
                );
            }
        }
    }
}

/// RULE: the decision precedes the store lookup, so a caller whose policies do not allow
/// the action is refused with HTTP 403 `AccessDenied` for an ARN the account holds no
/// provider under — the refusal says nothing about whether that provider exists — while
/// the same request from a caller the policies DO allow reaches the lookup and is answered
/// with the missing entity.
#[tokio::test]
async fn an_unauthorized_caller_is_refused_before_the_provider_is_looked_up() {
    let router = router();

    // Rows: an ARN for a bare hostname, one carrying a path, and one whose only unusual
    // feature is its case.
    let absent = [
        ("a bare hostname", expected_arn("https://absent.example.org")),
        (
            "a hostname with a path",
            expected_arn("https://absent.example.org/realms/main"),
        ),
        (
            "a host differing only in case",
            expected_arn("https://ABSENT.example.org"),
        ),
    ];

    let refused_key = principal(&router, "no grant", "unauthorized-oidc-lister", &[]).await;
    for (case, arn) in &absent {
        let (status, body) = list_tags_signed(&router, Some(&refused_key), arn, None, None).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected HTTP 403 for {arn:?}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected the error code AccessDenied, got {body}"
        );
    }

    let allowed_key = principal(
        &router,
        "an Allow on every resource",
        "authorized-oidc-lister",
        &[(
            "allow-list".to_string(),
            statement("Allow", "iam:ListOpenIDConnectProviderTags", "*"),
        )],
    )
    .await;
    for (case, arn) in &absent {
        let (status, body) = list_tags_signed(&router, Some(&allowed_key), arn, None, None).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {arn:?}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {case:?}: expected the error code NoSuchEntity, got {body}"
        );
    }

    // And the same allowed caller reads a provider the account does hold, so the grant is
    // what the two answers above differ by rather than the request.
    let held = tags(&[("team", "federation")]);
    let arn = create_provider(&router, "https://held-by-account.example.org", &held).await;
    let (status, body) = list_tags_signed(&router, Some(&allowed_key), &arn, None, None).await;
    assert_eq!(
        status, 200,
        "expected an allowed caller to read a held provider's tags, got {status} with \
         body {body}"
    );
    assert_eq!(
        reported_tags(&body),
        expected_map(&held),
        "expected the persisted tags {:?}, got {body}",
        expected_map(&held)
    );
}
