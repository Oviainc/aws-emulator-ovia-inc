// SPDX-License-Identifier: CC0-1.0

//! `ListRoleTags`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of tags that were PERSISTED
//! by `CreateRole` and `TagRole`, never a handler called directly. The awsQuery harness is
//! the one `tests/all/user_lifecycle.rs` owns, and the parameter and tag readers are
//! `tests/all/role_lifecycle.rs`'s, so a request is built and read exactly one way across
//! this crate.
//!
//! The rule under test is that the listing REPORTS the tag set the named role persists,
//! sorted by tag key, paged over that same key order by `MaxItems` and `Marker` so every
//! tag is reported exactly once, and writes nothing — while a name the account holds no
//! role for and a `Marker` this service did not issue for that role's tags are each
//! refused, and a signed call is decided against the caller's identity-based policies
//! first. Each suite is a TABLE whose rows differ along what the rule generalises over
//! (the tag count from none to the published maximum, the order the tags arrive in, the
//! alphabet of the keys and values, the page size, the case the name is spelled in), so
//! no single request is the specification.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tags::{with_tag_params, TRUST_POLICY};
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// One role's tag set, as the create that persists it carries it.
#[derive(Debug, Clone)]
struct TagCase {
    /// The role's `RoleName`, which the tags are reached through.
    name: String,
    /// The role's `Path`, so the rule is exercised on more than one depth.
    path: String,
    /// The tags the create carries, in REQUEST order — which is not the sorted order for
    /// most rows, because the sorting is the rule under test.
    tags: Vec<(String, String)>,
}

impl TagCase {
    fn new(name: &str, path: &str, tags: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            path: path.to_string(),
            tags: tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    /// The create's parameters: the role's name, path and trust policy, and the tags.
    fn create_params(&self) -> Vec<(String, String)> {
        with_tag_params(
            vec![
                param("RoleName", &self.name),
                param("Path", &self.path),
                param("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
            &self.tags,
        )
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

/// The tag-set table. Rows span the tag count (none, one, a few, the published maximum),
/// the arrival order (sorted, reversed, scrambled), the role's path, and the alphabet
/// `tagKeyType` and `tagValueType` publish: both are `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` /
/// `*$` (`smithy_query(query="shape", shape="Tag")`), and a VALUE may be empty because
/// `tagValueType` is len 0..256.
fn tag_cases() -> Vec<TagCase> {
    vec![
        // "If no tags are attached to the specified resource, the response contains an
        // empty list" (`ListRoleTags$Tags`).
        TagCase::new("NoTags", "/", &[]),
        // `Tag$Key`'s own documented example ("Department or Cost Center are common
        // choices").
        TagCase::new("OneTag", "/eng/", &[("Department", "Engineering")]),
        TagCase::new(
            "arriving.reversed",
            "/eng/tools/",
            &[("zone", "c"), ("owner", "ops"), ("app", "portal")],
        ),
        TagCase::new(
            "Arriving_Unsorted+1",
            "/",
            &[
                ("zone", "c"),
                ("Application", "portal"),
                ("owner", "ops"),
                ("Account", "42"),
            ],
        ),
        TagCase::new(
            "alphabet=spanning",
            "/sales/west/",
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
            name: "MaxTags-4".to_string(),
            path: "/eng/".to_string(),
            tags: scrambled_tags(MAX_TAGS_PER_RESOURCE),
        },
    ]
}

/// Create the case's role, and answer the ARN it is held under.
async fn persist(router: &MultiServiceRouter, case: &TagCase) -> String {
    let params = case.create_params();
    let (status, body) = call(router, "CreateRole", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture role must be created, got {status} with body {body}",
        case.name
    );
    element(&body, "Arn")
}

/// One `ListRoleTags` call, with an optional page size and marker.
async fn list_tags(
    router: &MultiServiceRouter,
    role_name: &str,
    max_items: Option<i32>,
    marker: Option<&str>,
) -> (u16, String) {
    let mut params = vec![param("RoleName", role_name)];
    if let Some(size) = max_items {
        params.push(param("MaxItems", &size.to_string()));
    }
    if let Some(token) = marker {
        params.push(param("Marker", token));
    }
    call(router, "ListRoleTags", &borrowed(&params)).await
}

/// RULE: the listing reports the tag set the named role PERSISTS, sorted by tag key, and
/// a set that fits one page is not truncated and carries no `Marker`.
#[tokio::test]
async fn the_listing_reports_the_persisted_tags_sorted_by_key() {
    let router = router();

    for case in tag_cases() {
        persist(&router, &case).await;

        let (status, body) = list_tags(&router, &case.name, None, None).await;
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

        // IAM entity names "are not distinguished by case", so the same tags are
        // reported however the request spells the name.
        for spelling in [case.name.to_uppercase(), case.name.to_lowercase()] {
            let (status, other) = list_tags(&router, &spelling, None, None).await;
            assert_eq!(
                status, 200,
                "case {spelling:?}: a name spelled in another case names the same role, \
                 got {status} with body {other}"
            );
            assert_eq!(
                reported_tags(&other),
                case.expected_tags(),
                "case {spelling:?}: expected the same tags as {:?}, got {other}",
                case.name
            );
        }
    }
}

/// RULE: the listing reports what the LATEST write persisted, so a tag added after the
/// create and one removed by `UntagRole` are both reflected on the next read.
#[tokio::test]
async fn the_listing_follows_later_writes() {
    let router = router();
    let case = tag_cases()[2].clone();
    persist(&router, &case).await;

    // One tag added by `TagRole`, which the listing must then report beside the created
    // ones, in key order.
    let added = vec![param("RoleName", &case.name)];
    let added = with_tag_params(added, &[("added".to_string(), "later".to_string())]);
    let (status, body) = call(&router, "TagRole", &borrowed(&added)).await;
    assert_eq!(
        status, 200,
        "the fixture TagRole must succeed, got {status} with body {body}"
    );

    let mut expected = case.expected_tags();
    expected.insert("added".to_string(), "later".to_string());
    let (status, body) = list_tags(&router, &case.name, None, None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 after a tag was added, got {status} with body {body}"
    );
    assert_eq!(
        reported_tags(&body),
        expected,
        "case {:?}: the listing must report the tag just added, got {body}",
        case.name
    );
    let mut sorted_keys: Vec<String> = expected.keys().cloned().collect();
    sorted_keys.sort();
    assert_eq!(
        elements(&body, "Key"),
        sorted_keys,
        "case {:?}: the keys must stay sorted after a write, got {body}",
        case.name
    );

    // And one removed by `UntagRole`, which the listing must then omit.
    let removed = vec![
        param("RoleName", &case.name),
        param("TagKeys.member.1", "added"),
    ];
    let (status, body) = call(&router, "UntagRole", &borrowed(&removed)).await;
    assert_eq!(
        status, 200,
        "the fixture UntagRole must succeed, got {status} with body {body}"
    );
    let (status, body) = list_tags(&router, &case.name, None, None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 after a tag was removed, got {status} with body {body}"
    );
    assert_eq!(
        reported_tags(&body),
        case.expected_tags(),
        "case {:?}: the listing must omit the removed tag, got {body}",
        case.name
    );
}

/// RULE: paging walks the SAME key order, so whatever the page size every tag is reported
/// exactly once, in ascending key order, and only a truncated page carries the `Marker`
/// that resumes after it.
#[tokio::test]
async fn paging_reports_every_tag_exactly_once_in_key_order() {
    let router = router();

    for case in tag_cases() {
        persist(&router, &case).await;
        let expected = case.sorted_keys();
        // Rows span the page size: the published minimum, sizes that do and do not divide
        // the set, a size equal to the set, the published maximum, and the
        // absent-parameter default. `maxItemsType` is range 1..1000
        // (`smithy_query(query="operation", operation="ListRoleTags")`).
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
                let (status, body) =
                    list_tags(&router, &case.name, page_size, marker.as_deref()).await;
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

/// RULE: the listing is a READ — two calls report the same tags, and the role the
/// reporting half (`GetRole`) reads back is unchanged by having been listed.
#[tokio::test]
async fn listing_tags_changes_nothing_a_later_caller_sees() {
    let router = router();

    for case in tag_cases() {
        persist(&router, &case).await;

        let (_, first) = list_tags(&router, &case.name, None, None).await;
        // A paged walk, which issues markers, must leave the set alone too.
        let (_, paged) = list_tags(&router, &case.name, Some(1), None).await;
        let (_, second) = list_tags(&router, &case.name, None, None).await;
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
            "case {:?}: two listings must report the same tags, got {first} then {second}",
            case.name
        );

        let read_params = vec![param("RoleName", &case.name)];
        let (status, read) = call(&router, "GetRole", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the role must still read back, got {status} with body {read}",
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

/// RULE: a name the account holds no role for names no tag set, which is the entity-absent
/// refusal (HTTP 404 `NoSuchEntity`).
#[tokio::test]
async fn a_name_no_role_is_held_under_is_not_found() {
    let router = router();
    persist(&router, &tag_cases()[1]).await;

    for name in ["NeverCreated", "one.tag", "OneTagToo", "x"] {
        let (status, body) = list_tags(&router, name, None, None).await;
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

    // A name outside `roleNameType` (len 1..64, pattern `^[\w+=,.@-]+$`) is refused by the
    // modelled constraint, before the operation runs at all.
    for invalid in ["", "has space", &"a".repeat(65)] {
        let (status, body) = list_tags(&router, invalid, None, None).await;
        assert_eq!(
            status, 400,
            "case {invalid:?}: a name outside the published constraint must be refused \
             with HTTP 400, got {status} with body {body}"
        );
    }
}

/// RULE: a `Marker` names a position in ONE role's tag order, so a token this service did
/// not issue for that role's tags — a foreign string, or a token it issued while paging
/// ANOTHER role's tags — is refused rather than read as "start from the beginning".
#[tokio::test]
async fn a_marker_this_service_did_not_issue_for_these_tags_is_refused() {
    let router = router();
    let cases = tag_cases();
    let paged = cases
        .iter()
        .find(|case| case.tags.len() > 1)
        .expect("a case with more than one tag");
    let other = cases.last().expect("a second case").clone();
    persist(&router, paged).await;
    persist(&router, &other).await;

    for bogus in ["not-a-marker", "zz", "0g"] {
        let (status, body) = list_tags(&router, &paged.name, None, Some(bogus)).await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {bogus:?}: expected ValidationError, got {body}"
        );
    }

    // A marker issued while paging the OTHER role's tags names no position in this
    // role's.
    let (status, first_page) = list_tags(&router, &other.name, Some(1), None).await;
    assert_eq!(
        status, 200,
        "the fixture page must be served, got {status} with body {first_page}"
    );
    let foreign = element(&first_page, "Marker");
    let (status, body) = list_tags(&router, &paged.name, Some(1), Some(&foreign)).await;
    assert_eq!(
        status, 400,
        "case {foreign:?}: a marker issued for another role's tags must be refused with \
         HTTP 400, got {status} with body {body}"
    );

    // And the role's OWN marker resumes the walk: the page after it is the rest of the
    // set, in key order.
    let (_, own_first) = list_tags(&router, &paged.name, Some(1), None).await;
    let own_marker = element(&own_first, "Marker");
    let (status, rest) = list_tags(&router, &paged.name, None, Some(&own_marker)).await;
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

/// RULE: `MaxItems` outside its published range is refused before the operation runs, and
/// each published bound itself is accepted.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    let case = tag_cases()[1].clone();
    persist(&router, &case).await;

    // One past each bound of `maxItemsType` (range 1..1000).
    for out_of_range in [0, 1001, -1] {
        let (status, body) = list_tags(&router, &case.name, Some(out_of_range), None).await;
        assert_eq!(
            status, 400,
            "case {out_of_range:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
    for in_range in [1, 1000] {
        let (status, body) = list_tags(&router, &case.name, Some(in_range), None).await;
        assert_eq!(
            status, 200,
            "case {in_range:?}: expected HTTP 200, got {status} with body {body}"
        );
    }
}

/// A one-statement document with `effect` on `action` over `resource`.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// Create a user holding `document`, and answer the access key id a request signs with to
/// speak for it.
async fn signed_principal(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    document: Option<&String>,
) -> String {
    let created = vec![param("UserName", user_name)];
    let (status, body) = call(router, "CreateUser", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateUser must succeed, got {status}: {body}"
    );
    if let Some(document) = document {
        let put = vec![
            param("UserName", user_name),
            param("PolicyName", "grant"),
            param("PolicyDocument", document),
        ];
        let (status, body) = call(router, "PutUserPolicy", &borrowed(&put)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture PutUserPolicy must succeed, got {status}: {body}"
        );
    }
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    element(&key, "AccessKeyId")
}

/// RULE: the decision comes BEFORE the resource read, so a caller whose policies do not
/// permit the read is refused (HTTP 403) whether or not the account holds the role it
/// named, while a caller whose policies do permit it learns that the role is absent
/// (HTTP 404).
#[tokio::test]
async fn an_unauthorized_read_is_refused_before_the_role_is_looked_up() {
    let router = router();

    let grant = statement("Allow", "iam:ListRoleTags", "*");
    let rows: Vec<(&str, Option<&String>, u16, &str)> = vec![
        (
            "no policy, a name the account holds no role under",
            None,
            403,
            "AccessDenied",
        ),
        (
            "an Allow, a name the account holds no role under",
            Some(&grant),
            404,
            "NoSuchEntity",
        ),
    ];

    for (row, (case, document, status_expected, code_expected)) in rows.into_iter().enumerate() {
        let user_name = format!("absent-reader-{row}");
        let access_key_id = signed_principal(&router, case, &user_name, document).await;
        // Names no `CreateRole` in this suite minted, spanning the alphabet
        // `roleNameType` publishes.
        for absent in ["never-created", "Absent.Role+1", "x"] {
            let params = vec![param("RoleName", absent)];
            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                "ListRoleTags",
                &borrowed(&params),
            )
            .await;
            assert_eq!(
                status, status_expected,
                "case {case:?} on {absent:?}: expected HTTP {status_expected}, got \
                 {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                code_expected,
                "case {case:?} on {absent:?}: expected {code_expected}, got {body}"
            );
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the read.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// One signed-call row: the inline policy the caller holds and the answer the rule
/// requires when it reads a held role's tags.
struct SignedCase {
    case: &'static str,
    /// The policy document put on the caller, or none at all.
    document: Option<String>,
    expect: Expect,
}

/// RULE: a signed `ListRoleTags` is decided against the caller's identity-based policies
/// before any tag is reported — a `Deny` about the request refuses it whatever an `Allow`
/// says, an `Allow` with no such `Deny` serves it, and a principal with neither is refused
/// — and a refusal is HTTP 403 `AccessDenied` naming the principal, the action and the
/// role the request addressed.
#[tokio::test]
async fn a_signed_listing_is_decided_against_the_callers_policies() {
    let router = router();
    // The role every row reads sits at the default path, so the ARN a statement must name
    // is that role's.
    let role_prefix = format!("arn:aws:iam::{}:role/", account_id());
    let user_prefix = format!("arn:aws:iam::{}:user/", account_id());

    let rows = vec![
        SignedCase {
            case: "no identity-based policy at all",
            document: None,
            expect: Expect::NoAllow,
        },
        SignedCase {
            case: "an Allow naming the action on every resource",
            document: Some(statement("Allow", "iam:ListRoleTags", "*")),
            expect: Expect::Served,
        },
        SignedCase {
            case: "an Allow whose action element is a wildcard run",
            document: Some(statement("Allow", "iam:List*", "*")),
            expect: Expect::Served,
        },
        SignedCase {
            case: "an Allow for another action only",
            document: Some(statement("Allow", "iam:ListRoles", "*")),
            expect: Expect::NoAllow,
        },
        SignedCase {
            case: "an Allow scoped to another role's ARN",
            document: Some(statement(
                "Allow",
                "iam:ListRoleTags",
                &format!("{role_prefix}somebody-else"),
            )),
            expect: Expect::NoAllow,
        },
        SignedCase {
            case: "an Allow and a Deny, both about the request",
            document: Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListRoleTags","Resource":"*"},{"Effect":"Deny","Action":"iam:ListRoleTags","Resource":"*"}]}"#
                    .to_string(),
            ),
            expect: Expect::ExplicitDeny,
        },
    ];

    for (row, case) in rows.into_iter().enumerate() {
        let user_name = format!("tag-reader-{row}");
        let access_key_id =
            signed_principal(&router, case.case, &user_name, case.document.as_ref()).await;
        let role_name = format!("read-role-{row}");
        persist(
            &router,
            &TagCase::new(&role_name, "/", &[("Department", "Engineering")]),
        )
        .await;

        let params = vec![param("RoleName", &role_name)];
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListRoleTags",
            &borrowed(&params),
        )
        .await;

        match case.expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {:?}: expected the read to be served, got {status}: {body}",
                    case.case
                );
                assert_eq!(
                    elements(&body, "Key"),
                    vec!["Department".to_string()],
                    "case {:?}: expected the role's own tag, got {body}",
                    case.case
                );
            }
            Expect::ExplicitDeny | Expect::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {:?}: expected HTTP 403, got {status}: {body}",
                    case.case
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {:?}: expected the error code AccessDenied, got {body}",
                    case.case
                );
                // The message is built from the request: the principal that signed it, the
                // action it named, the role it addressed — named as the request named it,
                // not by ARN — which of the two refusals this is, and where this one
                // denial's authorization details are readable.
                let reason = if case.expect == Expect::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the iam:ListRoleTags action"
                        .to_string()
                };
                assert_access_denied_message(
                    case.case,
                    &element(&body, "Message"),
                    &format!("{user_prefix}{user_name}"),
                    "iam:ListRoleTags",
                    &format!("role {role_name}"),
                    &reason,
                );
            }
        }
    }
}
