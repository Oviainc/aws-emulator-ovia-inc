// SPDX-License-Identifier: CC0-1.0

//! `ListTags` as a read of the tag set a private CA holds.
//!
//! The rule under test: a `ListTags` call reports exactly the tags persisted for the CA
//! its ARN names, in tag-key order, one `MaxResults`-sized page at a time, with
//! `NextToken` present only while tags remain after the page and `Tags` absent when the
//! page holds none — and reporting them changes nothing a later caller sees.
//!
//! The emulator is driven over the wire through the harness in
//! [`super::private_ca_lifecycle`], so every assertion here is about the HTTP behaviour a
//! client observes.

use serde_json::{json, Value};

use super::private_ca_lifecycle::{root_ca_request, Emulator};

/// `TagList` is `len 1..50` (`smithy_query(query="shape",shape="TagList")`), so 50 tags
/// is the largest set a CA can be created with and the widest row the table below spans.
const MAX_TAGS: usize = 50;

/// One tag as a test states it: a key and the value it was given, if any.
type TagRow = (&'static str, Option<&'static str>);

/// A tag set as it is reported back: owned, so a response can be compared against the
/// request that created it.
type ReportedTag = (String, Option<String>);

/// Create a private CA carrying `tags` (the member is omitted for an empty set, since
/// `TagList` admits no empty list) and answer its ARN.
async fn ca_with_tags(emulator: &Emulator, case: &str, subject: &str, tags: &[TagRow]) -> String {
    let mut body = root_ca_request(subject);
    if !tags.is_empty() {
        body["Tags"] = Value::Array(
            tags.iter()
                .map(|(key, value)| match value {
                    Some(value) => json!({"Key": key, "Value": value}),
                    None => json!({"Key": key}),
                })
                .collect(),
        );
    }
    let created = emulator
        .ok(case, "CreateCertificateAuthority", &body)
        .await;
    created
        .get("CertificateAuthorityArn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: CreateCertificateAuthority answers the CA ARN, got {created:?}")
        })
        .to_string()
}

/// The tags a `ListTags` response reports, as `(key, value)` pairs in the order they were
/// returned. An absent `Tags` member is the empty set.
fn reported_tags(case: &str, response: &Value) -> Vec<ReportedTag> {
    match response.get("Tags") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|tag| {
                let key = tag
                    .get("Key")
                    .and_then(Value::as_str)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: every reported tag carries a Key, got {tag:?}")
                    })
                    .to_string();
                let value = tag.get("Value").map(|value| {
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: a tag Value is a string, got {tag:?}")
                        })
                        .to_string()
                });
                (key, value)
            })
            .collect(),
        other => panic!("case {case:?}: Tags is a list when present, got {other:?}"),
    }
}

/// `response["NextToken"]`, as the continuation token a next page would be asked with.
fn reported_token(case: &str, response: &Value) -> Option<String> {
    response.get("NextToken").map(|token| {
        token
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: NextToken is a string, got {response:?}"))
            .to_string()
    })
}

/// `tags` in the order `ListTags` reports them: ascending by key.
fn in_key_order(tags: &[TagRow]) -> Vec<ReportedTag> {
    let mut expected: Vec<ReportedTag> = tags
        .iter()
        .map(|(key, value)| ((*key).to_string(), value.map(str::to_string)))
        .collect();
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    expected
}

/// Every tag set a CA can be created with comes back from `ListTags` exactly as it was
/// stored, in key order, with no continuation token.
///
/// The rows differ along what the rule generalises over: how many tags there are (none,
/// one, a few, the `TagList` maximum), what form each takes (a value, no value, an empty
/// value) and the order the create request listed them in. Nothing here waits for the
/// clock, so each CA is still `CREATING` when its tags are read: tags are set when the CA
/// is created, so they are readable in that status too.
#[tokio::test]
async fn every_tag_set_a_ca_is_created_with_is_reported_in_key_order() {
    let emulator = Emulator::new();

    let fifty: Vec<TagRow> = FIFTY_TAG_KEYS
        .iter()
        .map(|key| (*key, Some("bulk")))
        .collect();
    let rows: Vec<(&str, Vec<TagRow>)> = vec![
        ("no tags at all", Vec::new()),
        ("one tag with a value", vec![("owner", Some("pki"))]),
        ("one tag with no value", vec![("owner", None)]),
        // `TagValue` is `len 0..256`, so the empty string is a value, not an absent one.
        ("one tag with an empty value", vec![("owner", Some(""))]),
        (
            "keys listed out of key order",
            vec![
                ("tier", Some("root")),
                ("owner", Some("pki")),
                ("Env", Some("prod")),
            ],
        ),
        (
            "a mix of valued and valueless keys",
            vec![
                ("zone", None),
                ("alpha", Some("1")),
                ("middle", None),
                ("beta", Some("2")),
            ],
        ),
        ("fifty tags, the TagList maximum", fifty),
    ];

    for (index, (case, tags)) in rows.iter().enumerate() {
        let arn = ca_with_tags(&emulator, case, &format!("tagged-{index}"), tags).await;
        let body = json!({"CertificateAuthorityArn": arn});
        let response = emulator.ok(case, "ListTags", &body).await;

        let expected = in_key_order(tags);
        let actual = reported_tags(case, &response);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored tags in key order {expected:?}, got {actual:?}"
        );
        let token = reported_token(case, &response);
        assert!(
            token.is_none(),
            "case {case:?}: expected no NextToken for an untruncated listing, got {token:?}"
        );

        // A read may not change what a later read sees.
        let again = emulator.ok(case, "ListTags", &body).await;
        let second = reported_tags(case, &again);
        assert_eq!(
            second, expected,
            "case {case:?}: expected a second ListTags to report {expected:?}, got {second:?}"
        );
    }
}

/// The fifty keys of the widest row, distinct and inside `TagKey`'s
/// `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` pattern.
const FIFTY_TAG_KEYS: [&str; MAX_TAGS] = [
    "key-00", "key-01", "key-02", "key-03", "key-04", "key-05", "key-06", "key-07", "key-08",
    "key-09", "key-10", "key-11", "key-12", "key-13", "key-14", "key-15", "key-16", "key-17",
    "key-18", "key-19", "key-20", "key-21", "key-22", "key-23", "key-24", "key-25", "key-26",
    "key-27", "key-28", "key-29", "key-30", "key-31", "key-32", "key-33", "key-34", "key-35",
    "key-36", "key-37", "key-38", "key-39", "key-40", "key-41", "key-42", "key-43", "key-44",
    "key-45", "key-46", "key-47", "key-48", "key-49",
];

/// Paging with any `MaxResults` walks the CA's whole tag set exactly once, in key order.
///
/// `MaxResults` is `range 1..1000`, so the rows span its lower bound, sizes that divide
/// the set and sizes larger than it; the tag counts span one tag, a few and the
/// `TagList` maximum.
#[tokio::test]
async fn paging_walks_every_tag_exactly_once_whatever_max_results_asks_for() {
    let emulator = Emulator::new();

    for tag_count in [1_usize, 2, 5, MAX_TAGS] {
        let tags: Vec<TagRow> = FIFTY_TAG_KEYS
            .iter()
            .take(tag_count)
            .rev() // listed in reverse key order, so the reported order is not the stored one
            .map(|key| (*key, Some("paged")))
            .collect();
        let expected = in_key_order(&tags);

        for max_results in [1_usize, 2, 3, MAX_TAGS, 1000] {
            let case = format!("{tag_count} tags, MaxResults={max_results}");
            let arn = ca_with_tags(
                &emulator,
                &case,
                &format!("paged-{tag_count}-{max_results}"),
                &tags,
            )
            .await;

            let mut collected: Vec<ReportedTag> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0_usize;
            loop {
                let mut body = json!({
                    "CertificateAuthorityArn": arn,
                    "MaxResults": max_results,
                });
                if let Some(token) = token.as_deref() {
                    body["NextToken"] = json!(token);
                }
                let response = emulator.ok(&case, "ListTags", &body).await;
                let page = reported_tags(&case, &response);
                pages += 1;
                assert!(
                    !page.is_empty() && page.len() <= max_results,
                    "case {case:?}: expected page {pages} to hold 1..={max_results} tags, got \
                     {page:?}"
                );
                collected.extend(page);
                token = reported_token(&case, &response);
                if token.is_none() {
                    break;
                }
                assert!(
                    pages <= tag_count,
                    "case {case:?}: expected at most {tag_count} pages for {tag_count} tags, \
                     still paging after {pages}"
                );
            }

            assert_eq!(
                collected, expected,
                "case {case:?}: expected the pages to concatenate to {expected:?}, got \
                 {collected:?}"
            );
            let wanted_pages = tag_count.div_ceil(max_results);
            assert_eq!(
                pages, wanted_pages,
                "case {case:?}: expected {wanted_pages} page(s), got {pages}"
            );
        }
    }
}

/// `MaxResults` is `range 1..1000`: each published bound is honoured, and a value one
/// step outside either bound is refused instead of being clamped into a page.
#[tokio::test]
async fn max_results_is_honoured_on_its_bounds_and_refused_past_them() {
    let emulator = Emulator::new();

    let tags: Vec<TagRow> = vec![("alpha", Some("1")), ("beta", Some("2"))];
    let arn = ca_with_tags(&emulator, "the bounds of MaxResults", "bounded", &tags).await;

    for (case, max_results, wanted) in [("the lower bound", 1, 1), ("the upper bound", 1000, 2)] {
        let response = emulator
            .ok(
                case,
                "ListTags",
                &json!({"CertificateAuthorityArn": arn, "MaxResults": max_results}),
            )
            .await;
        let page = reported_tags(case, &response);
        assert_eq!(
            page.len(),
            wanted,
            "case {case:?}: expected {wanted} tag(s) on the page, got {page:?}"
        );
    }

    for (case, max_results) in [
        ("one below the lower bound", 0),
        ("one above the upper bound", 1001),
    ] {
        emulator
            .refused(
                case,
                "ListTags",
                &json!({"CertificateAuthorityArn": arn, "MaxResults": max_results}),
                "ValidationException",
            )
            .await;
    }
}

/// A continuation token is honoured only for the listing it belongs to: a token this
/// service could not have issued, and one issued for another CA, are both refused rather
/// than answered with a page.
#[tokio::test]
async fn list_tags_refuses_a_continuation_token_it_did_not_issue() {
    let emulator = Emulator::new();

    let tags: Vec<TagRow> = vec![("alpha", Some("1")), ("beta", Some("2"))];
    let first = ca_with_tags(&emulator, "the first CA", "token-first", &tags).await;
    let second = ca_with_tags(&emulator, "the second CA", "token-second", &tags).await;

    // A page of the first CA, so there is a real token to misuse.
    let page = emulator
        .ok(
            "a real token",
            "ListTags",
            &json!({"CertificateAuthorityArn": first, "MaxResults": 1}),
        )
        .await;
    let issued = reported_token("a real token", &page).expect("a truncated page carries a token");

    let foreign: &[(&str, String)] = &[
        ("not hexadecimal at all", "not-a-token".to_string()),
        ("hexadecimal but not a token", hex::encode("beta")),
        (
            "a token with no resume key",
            hex::encode(format!("{second}\u{1f}")),
        ),
        ("a token issued for another CA", issued.clone()),
    ];
    for (case, token) in foreign {
        emulator
            .refused(
                case,
                "ListTags",
                &json!({"CertificateAuthorityArn": second, "NextToken": token}),
                "InvalidNextTokenException",
            )
            .await;
    }

    // The same token on the listing it was issued for resumes it.
    let resumed = emulator
        .ok(
            "the token's own listing",
            "ListTags",
            &json!({"CertificateAuthorityArn": first, "NextToken": issued}),
        )
        .await;
    let actual = reported_tags("the token's own listing", &resumed);
    let expected = vec![("beta".to_string(), Some("2".to_string()))];
    assert_eq!(
        actual, expected,
        "case {:?}: expected the rest of the listing {expected:?}, got {actual:?}",
        "the token's own listing"
    );
}
