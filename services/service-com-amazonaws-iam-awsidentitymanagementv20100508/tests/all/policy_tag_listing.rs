// SPDX-License-Identifier: CC0-1.0

//! `ListPolicyTags`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of tags that were PERSISTED
//! by `CreatePolicy` and `TagPolicy`, never a handler called directly. The awsQuery
//! harness is the one `tests/all/user_lifecycle.rs` owns and the tag-list encoding is
//! `tests/all/role_tags.rs`'s, so a request is built and read exactly one way across
//! this crate.
//!
//! The rule under test is that the listing REPORTS the tag set the managed policy its
//! `PolicyArn` names persists, sorted by tag key, paged over that same key order by
//! `MaxItems` and `Marker` so every tag is reported exactly once, and writes nothing —
//! while a value that is no managed-policy ARN at all, an ARN the account holds no
//! policy under, and a `Marker` this service did not issue for that policy's tags are
//! each refused, and a signed call is decided against the caller's identity-based
//! policies first. Each suite is a TABLE whose rows differ along what the rule
//! generalises over (the tag count from none to the published maximum, the order the
//! tags arrive in, the alphabet of the keys and values, the policy's path, the page
//! size), so no single request is the specification.

use std::collections::BTreeMap;

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param, reported_tags};
use crate::role_tags::{tags, with_tag_params};
// The account's managed-policy ARN format has ONE definition in this crate's test tree,
// the one `tests/all/policy_tags.rs` owns, so no suite spells that ARN a second way.
use crate::policy_tags::account_policy_arn;
use crate::user_lifecycle::{
    account_id, assert_access_denied_message, call, call_signed_by, element, elements, error_code,
    router,
};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// documentation describes. Every policy in this suite is created with it; none of
/// these assertions is about the document.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"sts:GetCallerIdentity","Resource":"*"}]}"#;

/// The published maximum number of tags one IAM resource may carry: `tagListType` is
/// len 0..50 (`smithy_query(query="operation", operation="ListPolicyTags")`).
const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One policy's tag set, as the create that persists it carries it.
#[derive(Debug, Clone)]
struct TagCase {
    /// The policy's `PolicyName`, which makes the row's ARN unique.
    name: String,
    /// The policy's `Path`, so the rule is exercised on more than one depth.
    path: String,
    /// The tags the create carries, in REQUEST order — which is not the sorted order for
    /// most rows, because the sorting is the rule under test.
    tags: Vec<(String, String)>,
}

impl TagCase {
    fn new(name: &str, path: &str, pairs: &[(&str, &str)]) -> Self {
        Self {
            name: name.to_string(),
            path: path.to_string(),
            tags: tags(pairs),
        }
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
/// the arrival order (reversed, scrambled), the policy's path, and the alphabet
/// `tagKeyType` and `tagValueType` publish: both are `^[\p{L}\p{Z}\p{N}_.:/=+\-@]+$` /
/// `*$` (`smithy_query(query="shape", shape="Tag")`), and a VALUE may be empty because
/// `tagValueType` is len 0..256.
fn tag_cases() -> Vec<TagCase> {
    vec![
        // "If no tags are attached to the specified resource, the response contains an
        // empty list" (`ListPolicyTags$Tags`).
        TagCase::new("no-tags", "/", &[]),
        // `Tag$Key`'s own documented example ("Department or Cost Center are common
        // choices").
        TagCase::new("one-tag", "/eng/", &[("Department", "Engineering")]),
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
            name: "max-tags".to_string(),
            path: "/a/b/c/".to_string(),
            tags: scrambled_tags(MAX_TAGS_PER_RESOURCE),
        },
    ]
}

/// Create the case's policy, and answer the ARN it reported — the handle every call in
/// this suite addresses it through.
async fn persist(router: &MultiServiceRouter, case: &TagCase) -> String {
    let params = with_tag_params(
        vec![
            param("PolicyName", &case.name),
            param("PolicyDocument", POLICY_DOCUMENT),
            param("Path", &case.path),
        ],
        &case.tags,
    );
    let (status, body) = call(router, "CreatePolicy", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreatePolicy must succeed, got {status}: {body}",
        case.name
    );
    element(&body, "Arn")
}

/// One `ListPolicyTags` call, with an optional page size and marker.
async fn list_tags(
    router: &MultiServiceRouter,
    policy_arn: &str,
    max_items: Option<i32>,
    marker: Option<&str>,
) -> (u16, String) {
    let mut params = vec![param("PolicyArn", policy_arn)];
    if let Some(size) = max_items {
        params.push(param("MaxItems", &size.to_string()));
    }
    if let Some(token) = marker {
        params.push(param("Marker", token));
    }
    call(router, "ListPolicyTags", &borrowed(&params)).await
}

/// RULE: the listing reports the tag set the policy its ARN names PERSISTS, sorted by
/// tag key, and a set that fits one page is not truncated and carries no `Marker`.
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

/// RULE: the listing reports the tag set as it stands when it is called, so a tag added
/// after the policy was created is reported and one removed from it is not.
#[tokio::test]
async fn the_listing_follows_the_tag_set_as_it_changes() {
    let router = router();
    let case = TagCase::new("following", "/", &[("env", "prod"), ("team", "storage")]);
    let arn = persist(&router, &case).await;

    // Rows: a new key, a held key overwritten, and a key removed — each the kind of
    // change a tag call makes, and each read back through this listing.
    let added = with_tag_params(
        vec![param("PolicyArn", &arn)],
        &tags(&[("owner", "dlm"), ("env", "staging")]),
    );
    let (status, body) = call(&router, "TagPolicy", &borrowed(&added)).await;
    assert_eq!(
        status, 200,
        "the fixture TagPolicy must succeed, got {status}: {body}"
    );

    let (status, body) = list_tags(&router, &arn, None, None).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {body}");
    let expected: BTreeMap<String, String> = [
        ("env".to_string(), "staging".to_string()),
        ("owner".to_string(), "dlm".to_string()),
        ("team".to_string(), "storage".to_string()),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        reported_tags(&body),
        expected,
        "case \"after a tag call\": expected {expected:?}, got {body}"
    );
    assert_eq!(
        elements(&body, "Key"),
        vec![
            "env".to_string(),
            "owner".to_string(),
            "team".to_string()
        ],
        "case \"after a tag call\": the merged set must still be reported in key order, \
         got {body}"
    );
}

/// RULE: paging walks the SAME key order, so whatever the page size every tag is reported
/// exactly once, in ascending key order, and only a truncated page carries the `Marker`
/// that resumes after it.
#[tokio::test]
async fn paging_reports_every_tag_exactly_once_in_key_order() {
    let router = router();

    for case in tag_cases() {
        let arn = persist(&router, &case).await;
        let expected = case.sorted_keys();
        // Rows span the page size: the published minimum, sizes that do and do not divide
        // the set, a size equal to the set, the published maximum, and the
        // absent-parameter default. `maxItemsType` is range 1..1000
        // (`smithy_query(query="operation", operation="ListPolicyTags")`).
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
                        "case {label:?}: expected IsTruncated true or false, got {truncated:?}"
                    );
                    assert!(
                        page_marker.is_empty(),
                        "case {label:?}: a final page must carry no Marker, got {page_marker:?}"
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

/// RULE: the listing is a READ — two calls report the same tags, and the policy the
/// reporting half (`GetPolicy`) reads back is unchanged by having been listed.
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
            "case {:?}: two listings must report the same tag keys in the same order, the \
             paged walk answered {paged}",
            case.name
        );
        assert_eq!(
            reported_tags(&first),
            reported_tags(&second),
            "case {:?}: two listings must report the same tags, got {first} then {second}",
            case.name
        );

        let (status, read) = call(&router, "GetPolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 200,
            "case {:?}: the policy must still read back, got {status} with body {read}",
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

/// RULE: the tags are reported for the policy the request's ARN names, read exactly as
/// every other read of a managed policy addresses it — so a value that is no
/// managed-policy ARN at all is refused as an invalid input parameter, and an ARN in the
/// account's namespace that names no held policy is the missing entity.
#[tokio::test]
async fn the_policy_is_addressed_by_its_own_arn() {
    let router = router();
    let case = TagCase::new("held", "/held/", &[("team", "storage")]);
    let held_arn = persist(&router, &case).await;
    assert_eq!(
        held_arn,
        account_policy_arn("/held/", "held"),
        "the fixture's ARN must be the account's managed-policy ARN, got {held_arn:?}"
    );

    // A value that cannot be used as a managed-policy ARN: `arnType` constrains only the
    // length, so each row is 20 characters or longer and still names no policy. The code
    // is the layer that took the form — `ValidationError` for a value that is not an ARN
    // at all, the modelled `InvalidInput` for an ARN naming another service or resource
    // type.
    for (label, arn, expected_code) in [
        (
            "a bare string of the right length",
            "not-an-arn-at-all-xx".to_string(),
            "ValidationError",
        ),
        (
            "a role ARN",
            format!("arn:aws:iam::{}:role/some-role", account_id()),
            "InvalidInput",
        ),
        (
            "a user ARN",
            format!("arn:aws:iam::{}:user/some-user", account_id()),
            "InvalidInput",
        ),
        (
            "another service's ARN",
            "arn:aws:s3:::some-bucket-name".to_string(),
            "InvalidInput",
        ),
        (
            "an IAM policy ARN carrying a region",
            format!("arn:aws:iam:us-west-2:{}:policy/held", account_id()),
            "InvalidInput",
        ),
        (
            "a policy ARN with no partition",
            format!("arn::iam::{}:policy/held", account_id()),
            "InvalidInput",
        ),
    ] {
        let (status, body) = list_tags(&router, &arn, None, None).await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected 400 for {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {label:?}: expected the code {expected_code}, got {body}"
        );
    }

    // An ARN in the account's own namespace that names no held policy: another name, the
    // held name under another path, and the held name in another case — a policy's ARN is
    // the identifier, so a different spelling of it names a different policy.
    for (label, arn) in [
        ("another name at the root", account_policy_arn("/", "absent")),
        ("the held name at the root", account_policy_arn("/", "held")),
        (
            "the held name under another path",
            account_policy_arn("/elsewhere/", "held"),
        ),
        ("a name under a nested path", account_policy_arn("/a/b/c/", "absent")),
        ("the held ARN in another case", account_policy_arn("/held/", "HELD")),
    ] {
        let (status, body) = list_tags(&router, &arn, None, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: expected 404 for the unheld ARN {arn:?}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {label:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // An `arnType` shorter than its published minimum length (20) is refused by the
    // modelled constraint, before the operation runs at all.
    for short in ["", "arn:aws:iam::1:p/a"] {
        let (status, body) = list_tags(&router, short, None, None).await;
        assert_eq!(
            status, 400,
            "case {short:?}: a value outside the published length must be refused with \
             HTTP 400, got {status}: {body}"
        );
    }
}

/// RULE: a managed policy AWS owns is part of every account's view of IAM, so its tags
/// are reported from the same catalog every other read of it is served from — it carries
/// none of the account's tags, which is the documented empty list — while an ARN in AWS's
/// namespace this emulator's catalog does not carry is not reported as absent at all.
#[tokio::test]
async fn an_aws_managed_policy_reports_the_catalog_entrys_tags() {
    let router = router();

    // SOURCE: the catalog this crate carries (`state::AWS_MANAGED_POLICIES`), which
    // records the AWS-managed policies the emulator serves; every entry is a row, so no
    // single one is the specification.
    let carried: Vec<(String, String)> = state::AWS_MANAGED_POLICIES
        .iter()
        .map(|entry| (format!("the catalog entry {}", entry.policy_name), entry.arn()))
        .collect();
    assert!(
        carried.len() >= 2,
        "the catalog must carry entries for this suite to span, got {} rows",
        carried.len()
    );

    for (label, arn) in &carried {
        let (status, body) = list_tags(&router, arn, None, None).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 for {arn:?}, got {status}: {body}"
        );
        let reported = reported_tags(&body);
        assert!(
            reported.is_empty(),
            "case {label:?}: an AWS-owned policy carries none of the account's tags, got \
             {reported:?}"
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {label:?}: an empty tag set is never truncated, got {body}"
        );
    }

    for (label, arn) in [
        (
            "an AWS-managed ARN the catalog does not carry",
            "arn:aws:iam::aws:policy/NoSuchAwsManagedPolicyHere",
        ),
        (
            "an AWS-managed ARN under a path the catalog does not carry",
            "arn:aws:iam::aws:policy/job-function/NoSuchAwsManagedPolicyHere",
        ),
    ] {
        let (status, body) = list_tags(&router, arn, None, None).await;
        assert_eq!(
            status, 404,
            "case {label:?}: an ARN resolving to no policy is the modelled missing \
             entity, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {label:?}: expected the modelled awsQuery code, got {body}"
        );
    }
}

/// RULE: a `Marker` names a position in ONE policy's tag order, so a token this service
/// did not issue for that policy's tags — a foreign string, or a token it issued while
/// paging ANOTHER policy's tags — is refused rather than read as "start from the
/// beginning", while the policy's own marker resumes the walk after it.
#[tokio::test]
async fn a_marker_this_service_did_not_issue_for_these_tags_is_refused() {
    let router = router();
    let cases = tag_cases();
    let paged = cases
        .iter()
        .find(|case| case.tags.len() > 1)
        .expect("a case with more than one tag")
        .clone();
    let other = cases.last().expect("a second case").clone();
    let paged_arn = persist(&router, &paged).await;
    let other_arn = persist(&router, &other).await;

    for bogus in ["not-a-marker", "zz", "0g"] {
        let (status, body) = list_tags(&router, &paged_arn, None, Some(bogus)).await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {bogus:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // A marker issued while paging the OTHER policy's tags names no position in this
    // policy's.
    let (status, first_page) = list_tags(&router, &other_arn, Some(1), None).await;
    assert_eq!(
        status, 200,
        "the fixture page must be served, got {status} with body {first_page}"
    );
    let foreign = element(&first_page, "Marker");
    let (status, body) = list_tags(&router, &paged_arn, Some(1), Some(&foreign)).await;
    assert_eq!(
        status, 400,
        "case {foreign:?}: a marker issued for another policy's tags must be refused with \
         HTTP 400, got {status} with body {body}"
    );

    // And the policy's OWN marker resumes the walk: the page after it is the rest of the
    // set, in key order.
    let (_, own_first) = list_tags(&router, &paged_arn, Some(1), None).await;
    let own_marker = element(&own_first, "Marker");
    let (status, rest) = list_tags(&router, &paged_arn, None, Some(&own_marker)).await;
    assert_eq!(
        status, 200,
        "a marker this service issued must be accepted, got {status} with body {rest}"
    );
    assert_eq!(
        elements(&rest, "Key"),
        paged.sorted_keys()[1..].to_vec(),
        "the page after the first marker must be the rest of the set in key order, got {rest}"
    );
}

/// RULE: `MaxItems` outside its published range is refused before the operation runs, and
/// each published bound itself is accepted.
#[tokio::test]
async fn a_page_size_outside_the_published_range_is_refused() {
    let router = router();
    let case = TagCase::new("page-bounds", "/", &[("env", "prod"), ("team", "storage")]);
    let arn = persist(&router, &case).await;

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

/// What a signed row's policies require of the answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// HTTP 200: the policies in force permit the read.
    Served,
    /// HTTP 403 `AccessDenied` because a statement about the request says `Deny`.
    ExplicitDeny,
    /// HTTP 403 `AccessDenied` because no statement about the request says `Allow`.
    NoAllow,
}

/// A one-statement document with `effect` on `action` over `resource`.
fn statement(effect: &str, action: &str, resource: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}

/// Create a user holding `document` and answer the access key id a request signs with to
/// speak for it.
async fn signed_principal(
    router: &MultiServiceRouter,
    label: &str,
    user_name: &str,
    document: Option<&String>,
) -> String {
    let created = vec![param("UserName", user_name)];
    let (status, body) = call(router, "CreateUser", &borrowed(&created)).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the fixture CreateUser must succeed, got {status}: {body}"
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
            "case {label:?}: the fixture PutUserPolicy must succeed, got {status}: {body}"
        );
    }
    let (status, key) = call(router, "CreateAccessKey", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {label:?}: the fixture CreateAccessKey must succeed, got {status}: {key}"
    );
    element(&key, "AccessKeyId")
}

/// RULE: a signed `ListPolicyTags` is decided against the caller's identity-based
/// policies before any tag is reported — a `Deny` about the request refuses it whatever
/// an `Allow` says, an `Allow` with no such `Deny` serves it, and a principal with
/// neither is refused — and a refusal is HTTP 403 `AccessDenied` naming the principal,
/// the action and the policy the request addressed.
#[tokio::test]
async fn a_signed_listing_is_decided_against_the_callers_policies() {
    let router = router();
    let case = TagCase::new("signed-read", "/", &[("env", "prod")]);
    let arn = persist(&router, &case).await;

    let rows: Vec<(&str, Option<String>, Expect)> = vec![
        ("no identity-based policy at all", None, Expect::NoAllow),
        (
            "an Allow naming the action on every resource",
            Some(statement("Allow", "iam:ListPolicyTags", "*")),
            Expect::Served,
        ),
        (
            "an Allow naming the action on this policy's ARN",
            Some(statement("Allow", "iam:ListPolicyTags", &arn)),
            Expect::Served,
        ),
        (
            "an Allow whose action element is a wildcard run",
            Some(statement("Allow", "iam:List*", "*")),
            Expect::Served,
        ),
        (
            "an Allow for another action only",
            Some(statement("Allow", "iam:ListPolicies", "*")),
            Expect::NoAllow,
        ),
        (
            "an Allow scoped to another policy's ARN",
            Some(statement(
                "Allow",
                "iam:ListPolicyTags",
                &account_policy_arn("/", "somebody-elses-policy"),
            )),
            Expect::NoAllow,
        ),
        (
            "an Allow and a Deny, both about the request",
            Some(
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListPolicyTags","Resource":"*"},{"Effect":"Deny","Action":"iam:ListPolicyTags","Resource":"*"}]}"#
                    .to_string(),
            ),
            Expect::ExplicitDeny,
        ),
    ];

    for (row, (label, document, expect)) in rows.into_iter().enumerate() {
        let user_name = format!("policy-tag-reader-{row}");
        let access_key_id = signed_principal(&router, label, &user_name, document.as_ref()).await;

        let params = vec![param("PolicyArn", &arn)];
        let (status, body) = call_signed_by(
            &router,
            Some(&access_key_id),
            "ListPolicyTags",
            &borrowed(&params),
        )
        .await;

        match expect {
            Expect::Served => {
                assert_eq!(
                    status, 200,
                    "case {label:?}: expected the read to be served, got {status}: {body}"
                );
                assert_eq!(
                    reported_tags(&body),
                    case.expected_tags(),
                    "case {label:?}: expected the persisted tags, got {body}"
                );
            }
            Expect::ExplicitDeny | Expect::NoAllow => {
                assert_eq!(
                    status, 403,
                    "case {label:?}: expected HTTP 403, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {label:?}: expected the error code AccessDenied, got {body}"
                );
                // The message is built from the request: the principal that signed it,
                // the action it named, the policy it addressed — a managed policy being
                // identified by its ARN — which of the two refusals this is, and where
                // this one denial's authorization details are readable.
                let reason = if expect == Expect::ExplicitDeny {
                    "with an explicit deny in an identity-based policy".to_string()
                } else {
                    "because no identity-based policy allows the iam:ListPolicyTags action"
                        .to_string()
                };
                assert_access_denied_message(
                    label,
                    &element(&body, "Message"),
                    &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
                    "iam:ListPolicyTags",
                    &arn,
                    &reason,
                );
            }
        }
    }

    // The refusals changed nothing: the policy still reports exactly the tags it was
    // created with.
    let (status, body) = list_tags(&router, &arn, None, None).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {body}");
    assert_eq!(
        reported_tags(&body),
        case.expected_tags(),
        "a refused read must leave the held tags alone, got {body}"
    );
}
