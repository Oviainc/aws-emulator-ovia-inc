// SPDX-License-Identifier: CC0-1.0

//! A key's tags, end to end over the wire: `TagResource`, `ListTagsForResource`,
//! `UntagResource`.
//!
//! Every call goes through the real `MultiServiceRouter` (`POST /` with
//! `X-Amz-Target: PaymentCryptographyControlPlane.<Op>`, awsJson1_0) and the
//! modelled-constraint walk, through the same harness `key_lifecycle` uses, so what
//! these tests assert is what a signed SDK request observes.
//!
//! The rule under test: a key's tags are ONE set persisted on that key's stored
//! record — a tag call merges per tag key into it, an untag call removes per tag key
//! from it, and a read answers a page of exactly that set, in tag-key order, with an
//! opaque token naming where the next page starts.
//!
//! Each case is a row of a table spanning what the rule generalises over: how many
//! tags a call carries, whether each tag key is new or already held, the published
//! bounds of a tag key and a tag value, tag counts and page sizes either side of the
//! documented default and of the published `MaxResults` bounds, and the refusal
//! classes a tag call earns.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state::{
    key_arn, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE, MAX_TAGS, RESERVED_TAG_PREFIX,
};
use service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::ComAmazonawsPaymentcryptographyPaymentcryptographycontrolplaneState as KeyStore;

use crate::key_lifecycle::{
    call, error_code, ok_call, router, router_with_engine, string, REGION,
};

/// The published bounds of a tag key's length, from
/// `smithy_query(query="shape",shape="TagKey")` (`length 1..128`).
const MAX_TAG_KEY_LEN: usize = 128;
/// The published bounds of a tag value's length, from
/// `smithy_query(query="shape",shape="TagValue")` (`length 1..256`).
const MAX_TAG_VALUE_LEN: usize = 256;

/// One `CreateKey` body: a symmetric data-encryption key carrying `tags`.
///
/// The rules under test are about a key's tag set rather than about one algorithm,
/// so every case uses the same key kind and varies the tags instead.
fn create_key_body(tags: &[(String, String)]) -> Value {
    json!({
        "KeyAttributes": {
            "KeyUsage": "TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY",
            "KeyClass": "SYMMETRIC_KEY",
            "KeyAlgorithm": "AES_128",
            "KeyModesOfUse": {"Encrypt": true, "Decrypt": true},
        },
        "Exportable": true,
        "Tags": tags_json(tags),
    })
}

/// Create one key carrying `tags` and answer its `KeyArn`.
async fn create_key(router: &MultiServiceRouter, tags: &[(String, String)]) -> String {
    let created = ok_call(router, "CreateKey", &create_key_body(tags)).await;
    let key = created.get("Key").expect("CreateKey answers a Key");
    string(key, "KeyArn").to_string()
}

/// A tag list as the wire shape `Tags`, in the order given.
fn tags_json(pairs: &[(String, String)]) -> Value {
    Value::Array(
        pairs
            .iter()
            .map(|(key, value)| json!({"Key": key, "Value": value}))
            .collect(),
    )
}

/// One tag pair.
fn pair(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// `count` distinct tags, named so their tag-key order is also their creation order.
fn numbered_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|i| (format!("tag-{i:03}"), format!("value-{i}")))
        .collect()
}

/// A tag list as the map it is stored and reported as.
fn expected_map(pairs: &[(String, String)]) -> BTreeMap<String, String> {
    pairs.iter().cloned().collect()
}

/// Every tag `ListTagsForResource` reports for `arn`, walked page by page with
/// `max_results`, checking the pagination contract as it goes: no page carries more
/// than the page size asked for, a page carrying a `NextToken` is full, the last page
/// carries none, no tag key appears twice, and the keys arrive in order.
async fn list_all_tags(
    router: &MultiServiceRouter,
    case: &str,
    arn: &str,
    max_results: Option<i32>,
) -> BTreeMap<String, String> {
    let page_size = max_results.map_or(DEFAULT_PAGE_SIZE, |asked| asked as usize);
    let mut collected: BTreeMap<String, String> = BTreeMap::new();
    let mut ordered: Vec<String> = Vec::new();
    let mut next_token: Option<String> = None;
    loop {
        let mut body = json!({"ResourceArn": arn});
        if let Some(asked) = max_results {
            body["MaxResults"] = json!(asked);
        }
        if let Some(token) = &next_token {
            body["NextToken"] = json!(token);
        }
        let page = ok_call(router, "ListTagsForResource", &body).await;
        let tags = page
            .get("Tags")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case}: expected a Tags list, got {page}"));
        assert!(
            tags.len() <= page_size,
            "case {case}: expected at most {page_size} tags on a page, got {}: {page}",
            tags.len()
        );
        for tag in tags {
            let key = string(tag, "Key").to_string();
            let value = string(tag, "Value").to_string();
            assert!(
                collected.insert(key.clone(), value).is_none(),
                "case {case}: expected each tag key once, got {key:?} twice: {page}"
            );
            ordered.push(key);
        }
        next_token = page
            .get("NextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        match &next_token {
            // A truncated page is a FULL page: the token names the tag the next page
            // starts at, so a short page with a token would be dropping tags.
            Some(_) => assert_eq!(
                tags.len(),
                page_size,
                "case {case}: expected a full page before a NextToken, got {}: {page}",
                tags.len()
            ),
            None => break,
        }
    }
    let mut sorted = ordered.clone();
    sorted.sort();
    assert_eq!(
        ordered, sorted,
        "case {case}: expected the tags in tag-key order, got {ordered:?}"
    );
    collected
}

/// The tags the engine actually holds for `arn` must be what the reads reported, so a
/// response is never richer than the record it came from.
fn assert_stored(
    engine: &Arc<engine::InMemoryEngine>,
    case: &str,
    arn: &str,
    expected: &BTreeMap<String, String>,
) {
    let store = engine.state::<KeyStore>();
    let guard = store.lock();
    let stored = guard
        .keys
        .get(arn)
        .unwrap_or_else(|| panic!("case {case}: no key stored under {arn}"));
    assert_eq!(
        &stored.tags,
        expected,
        "case {case}: expected the stored tags {expected:?}, got {:?}",
        stored.tags
    );
}

/// A call that must be refused with the modelled error class `code`.
///
/// The status LINE of a client refusal is produced by this service's shared error
/// serializer rather than by a handler, so what a handler owns — and what this
/// asserts — is the error class in the body and that the refusal is a client error.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    code: &str,
) -> Value {
    let (status, parsed) = call(router, operation, body).await;
    assert!(
        (400..500).contains(&status),
        "case {case}: expected {operation} to refuse with a client status, got {status} {parsed}"
    );
    assert_eq!(
        error_code(&parsed),
        code,
        "case {case}: expected {code}, got {status} {parsed}"
    );
    parsed
}

/// One row of the tag round-trip table: what the key is created with, what one tag
/// call then adds or edits, what the tag set must be afterwards, what one untag call
/// removes, and what is left.
struct TagCase {
    name: &'static str,
    created_with: Vec<(String, String)>,
    tagged_with: Vec<(String, String)>,
    after_tag: Vec<(String, String)>,
    untagged: Vec<String>,
    after_untag: Vec<(String, String)>,
}

/// The table: rows differ in how many tags each call carries, whether a tag key is
/// new or already held, the case of a tag key, the published length bounds of a tag
/// key and value, whether an untag names a tag the key never carried, and whether the
/// set spans more than one page.
fn tag_cases() -> Vec<TagCase> {
    let longest_key = "k".repeat(MAX_TAG_KEY_LEN);
    let longest_value = "v".repeat(MAX_TAG_VALUE_LEN);
    let mut paged_created = numbered_tags(DEFAULT_PAGE_SIZE + 1);
    let mut paged_after_tag = paged_created.clone();
    paged_after_tag.push(pair("zz-extra", "x"));
    paged_after_tag.sort();
    let mut paged_after_untag = paged_after_tag.clone();
    paged_after_untag.retain(|(key, _)| key != "tag-000" && key != "zz-extra");
    paged_created.sort();

    vec![
        TagCase {
            name: "a tag call on a key created with none",
            created_with: vec![],
            tagged_with: vec![pair("Environment", "Production")],
            after_tag: vec![pair("Environment", "Production")],
            untagged: vec!["Environment".to_string()],
            after_untag: vec![],
        },
        TagCase {
            name: "a tag call adds to the set the create stored",
            created_with: vec![pair("alpha", "1"), pair("beta", "2")],
            tagged_with: vec![pair("gamma", "3")],
            after_tag: vec![pair("alpha", "1"), pair("beta", "2"), pair("gamma", "3")],
            untagged: vec!["beta".to_string()],
            after_untag: vec![pair("alpha", "1"), pair("gamma", "3")],
        },
        TagCase {
            name: "a tag call edits one value and keeps the rest",
            created_with: vec![pair("alpha", "1"), pair("beta", "2")],
            tagged_with: vec![pair("beta", "two")],
            after_tag: vec![pair("alpha", "1"), pair("beta", "two")],
            // "If the key doesn't have the specified tag key, Payment Cryptography
            // doesn't throw an exception or return a response" (`TagKeys`' doc).
            untagged: vec!["never-held".to_string()],
            after_untag: vec![pair("alpha", "1"), pair("beta", "two")],
        },
        TagCase {
            name: "an empty tag list and an empty tag-key list change nothing",
            created_with: vec![pair("alpha", "1")],
            tagged_with: vec![],
            after_tag: vec![pair("alpha", "1")],
            untagged: vec![],
            after_untag: vec![pair("alpha", "1")],
        },
        TagCase {
            name: "tag keys and values at their published length bounds",
            created_with: vec![pair("short", "s")],
            tagged_with: vec![
                pair(&longest_key, "at the key bound"),
                pair("longest-value", &longest_value),
            ],
            after_tag: vec![
                pair(&longest_key, "at the key bound"),
                pair("longest-value", &longest_value),
                pair("short", "s"),
            ],
            untagged: vec![longest_key.clone()],
            after_untag: vec![pair("longest-value", &longest_value), pair("short", "s")],
        },
        TagCase {
            name: "tag keys are case-sensitive, so two cases are two tags",
            created_with: vec![pair("Env", "prod")],
            tagged_with: vec![pair("env", "dev")],
            after_tag: vec![pair("Env", "prod"), pair("env", "dev")],
            untagged: vec!["Env".to_string()],
            after_untag: vec![pair("env", "dev")],
        },
        TagCase {
            name: "an untag call removes several at once, including one never held",
            created_with: vec![pair("a", "1"), pair("b", "2"), pair("c", "3")],
            tagged_with: vec![pair("d", "4")],
            after_tag: vec![pair("a", "1"), pair("b", "2"), pair("c", "3"), pair("d", "4")],
            untagged: vec!["a".to_string(), "c".to_string(), "absent".to_string()],
            after_untag: vec![pair("b", "2"), pair("d", "4")],
        },
        TagCase {
            name: "a tag set larger than one default page",
            created_with: paged_created,
            tagged_with: vec![pair("zz-extra", "x")],
            after_tag: paged_after_tag,
            untagged: vec!["tag-000".to_string(), "zz-extra".to_string()],
            after_untag: paged_after_untag,
        },
    ]
}

#[tokio::test]
async fn tagging_and_untagging_write_the_one_tag_set_every_read_reports() {
    let (router, engine) = router_with_engine();
    for case in tag_cases() {
        let name = case.name;
        let arn = create_key(&router, &case.created_with).await;

        // What the create stored is what the read reports: the tag set has one home.
        let after_create = list_all_tags(&router, name, &arn, None).await;
        assert_eq!(
            after_create,
            expected_map(&case.created_with),
            "case {name}: expected the created tags, got {after_create:?}"
        );
        assert_stored(&engine, name, &arn, &after_create);

        ok_call(
            &router,
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": tags_json(&case.tagged_with)}),
        )
        .await;
        let after_tag = list_all_tags(&router, name, &arn, None).await;
        assert_eq!(
            after_tag,
            expected_map(&case.after_tag),
            "case {name}: expected the merged tags, got {after_tag:?}"
        );
        assert_stored(&engine, name, &arn, &after_tag);

        ok_call(
            &router,
            "UntagResource",
            &json!({"ResourceArn": arn, "TagKeys": case.untagged}),
        )
        .await;
        let after_untag = list_all_tags(&router, name, &arn, None).await;
        assert_eq!(
            after_untag,
            expected_map(&case.after_untag),
            "case {name}: expected the remaining tags, got {after_untag:?}"
        );
        assert_stored(&engine, name, &arn, &after_untag);
    }
}

#[tokio::test]
async fn list_tags_for_resource_pages_every_stored_tag_at_every_page_size() {
    let router = router();
    // One tag set, read at page sizes either side of its own size and at the
    // published `MaxResults` bounds (range 1..100, from
    // `smithy_query(query="operation",operation="ListTagsForResource")`).
    let tags = numbered_tags(7);
    let arn = create_key(&router, &tags).await;
    let expected = expected_map(&tags);
    for max_results in [1, 2, 3, 6, 7, 8, MAX_PAGE_SIZE as i32] {
        let case = format!("MaxResults {max_results}");
        let walked = list_all_tags(&router, &case, &arn, Some(max_results)).await;
        assert_eq!(
            walked, expected,
            "case {case}: expected every stored tag across the pages, got {walked:?}"
        );
    }

    // No `MaxResults`: "If you do not include a value, it defaults to 50"
    // (`MaxResults`' own documentation), so a larger set is truncated at that many
    // and the rest arrives on the next page.
    let many = numbered_tags(DEFAULT_PAGE_SIZE + 3);
    let big_arn = create_key(&router, &many).await;
    let first = ok_call(&router, "ListTagsForResource", &json!({"ResourceArn": big_arn})).await;
    let page = first
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Tags list, got {first}"));
    assert_eq!(
        page.len(),
        DEFAULT_PAGE_SIZE,
        "the default page must carry {DEFAULT_PAGE_SIZE} tags, got {}: {first}",
        page.len()
    );
    let token = first
        .get("NextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a truncated default page must carry a NextToken: {first}"));
    let second = ok_call(
        &router,
        "ListTagsForResource",
        &json!({"ResourceArn": big_arn, "NextToken": token}),
    )
    .await;
    let rest = second
        .get("Tags")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("expected a Tags list, got {second}"));
    assert_eq!(
        rest.len(),
        many.len() - DEFAULT_PAGE_SIZE,
        "the last page must carry the remaining tags, got {}: {second}",
        rest.len()
    );
    assert!(
        second.get("NextToken").is_none(),
        "the last page carries no NextToken: {second}"
    );

    // A `MaxResults` outside the published 1..100 range is refused by the modelled
    // constraint walk, at both bounds and one past each.
    for out_of_range in [0, MAX_PAGE_SIZE as i32 + 1] {
        let case = format!("MaxResults {out_of_range}");
        let (status, parsed) = call(
            &router,
            "ListTagsForResource",
            &json!({"ResourceArn": arn, "MaxResults": out_of_range}),
        )
        .await;
        assert!(
            (400..500).contains(&status),
            "case {case}: expected a client refusal outside the published range, got {status} {parsed}"
        );
    }
}

#[tokio::test]
async fn list_tags_for_resource_refuses_a_token_it_did_not_issue() {
    let router = router();
    let arn = create_key(&router, &numbered_tags(3)).await;
    let other_arn = create_key(&router, &numbered_tags(1)).await;
    ok_call(
        &router,
        "CreateAlias",
        &json!({"AliasName": "alias/tagged", "KeyArn": other_arn}),
    )
    .await;

    // A token each neighbouring list operation really issued, plus a string that is
    // not a token at all: none of them names a tag, so none is accepted.
    let keys_page = ok_call(&router, "ListKeys", &json!({"MaxResults": 1})).await;
    let key_token = string(&keys_page, "NextToken").to_string();
    let aliases_page = ok_call(&router, "ListAliases", &json!({"MaxResults": 1})).await;
    let alias_token = aliases_page
        .get("NextToken")
        .and_then(Value::as_str)
        .map(str::to_string);
    let mut tokens = vec![
        ("a ListKeys token".to_string(), key_token),
        ("not hex at all".to_string(), "not-a-token".to_string()),
        // Hex, but naming nothing this service pages over.
        ("hex naming no tag".to_string(), "00".to_string()),
    ];
    if let Some(token) = alias_token {
        tokens.push(("a ListAliases token".to_string(), token));
    }
    for (case, token) in tokens {
        refused(
            &router,
            &case,
            "ListTagsForResource",
            &json!({"ResourceArn": arn, "NextToken": token}),
            "ValidationException",
        )
        .await;
    }
}

#[tokio::test]
async fn a_tag_call_naming_no_key_or_an_inadmissible_tag_set_stores_nothing() {
    let router = router();
    let held = vec![pair("alpha", "1"), pair("beta", "2")];
    let arn = create_key(&router, &held).await;

    // A well-formed `KeyArn` (its pattern and 70..150 length are modelled) that names
    // no key: every operation of this slice answers the not-found class for it.
    let unknown = key_arn(REGION, "zzzzzzzzzzzzzzzz");
    let not_found = refused(
        &router,
        "ListTagsForResource on an unknown key",
        "ListTagsForResource",
        &json!({"ResourceArn": unknown}),
        "ResourceNotFoundException",
    )
    .await;
    assert_eq!(
        not_found.get("ResourceId").and_then(Value::as_str),
        Some(unknown.as_str()),
        "the refusal names the resource that was not found: {not_found}"
    );
    refused(
        &router,
        "TagResource on an unknown key",
        "TagResource",
        &json!({"ResourceArn": unknown, "Tags": tags_json(&[pair("a", "1")])}),
        "ResourceNotFoundException",
    )
    .await;
    refused(
        &router,
        "UntagResource on an unknown key",
        "UntagResource",
        &json!({"ResourceArn": unknown, "TagKeys": ["a"]}),
        "ResourceNotFoundException",
    )
    .await;

    // Tag sets the model cannot refuse but the service does: the reserved namespace
    // (in either case, since the namespace is matched case-insensitively) and one tag
    // key carried twice.
    let inadmissible = vec![
        (
            "the reserved namespace",
            vec![pair(&format!("{RESERVED_TAG_PREFIX}created-by"), "x")],
        ),
        (
            "the reserved namespace in upper case",
            vec![pair(
                &format!("{}created-by", RESERVED_TAG_PREFIX.to_uppercase()),
                "x",
            )],
        ),
        (
            "one tag key carried twice",
            vec![pair("duplicate", "1"), pair("duplicate", "2")],
        ),
    ];
    for (case, tags) in inadmissible {
        refused(
            &router,
            case,
            "TagResource",
            &json!({"ResourceArn": arn, "Tags": tags_json(&tags)}),
            "ValidationException",
        )
        .await;
        let after = list_all_tags(&router, case, &arn, None).await;
        assert_eq!(
            after,
            expected_map(&held),
            "case {case}: a refused tag call stores nothing, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_tag_call_that_would_take_a_key_past_its_tag_capacity_is_refused() {
    let router = router();
    // A key already carrying as many tags as one key may hold.
    let full = numbered_tags(MAX_TAGS);
    let arn = create_key(&router, &full).await;
    let before = list_all_tags(&router, "a full tag set", &arn, Some(MAX_PAGE_SIZE as i32)).await;
    assert_eq!(
        before.len(),
        MAX_TAGS,
        "expected {MAX_TAGS} stored tags, got {}",
        before.len()
    );

    // One more tag key would take it past that, so it is refused and nothing is
    // stored.
    refused(
        &router,
        "one tag key past the capacity",
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": tags_json(&[pair("one-too-many", "x")])}),
        "ServiceQuotaExceededException",
    )
    .await;
    let after = list_all_tags(&router, "one tag key past the capacity", &arn, Some(100)).await;
    assert_eq!(
        after, before,
        "a refused tag call stores nothing, got {} tags",
        after.len()
    );

    // Editing a tag key the key already carries does not grow the set, so it is
    // accepted at capacity.
    let edited = full
        .first()
        .map(|(key, _)| pair(key, "edited at capacity"))
        .expect("the table is not empty");
    ok_call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": tags_json(std::slice::from_ref(&edited))}),
    )
    .await;
    let reread = list_all_tags(&router, "an edit at capacity", &arn, Some(100)).await;
    assert_eq!(
        reread.len(),
        MAX_TAGS,
        "an edit at capacity leaves {MAX_TAGS} tags, got {}",
        reread.len()
    );
    assert_eq!(
        reread.get(&edited.0),
        Some(&edited.1),
        "an edit at capacity replaces the value, got {:?}",
        reread.get(&edited.0)
    );

    // Freeing one tag key makes room for another.
    ok_call(
        &router,
        "UntagResource",
        &json!({"ResourceArn": arn, "TagKeys": [edited.0]}),
    )
    .await;
    ok_call(
        &router,
        "TagResource",
        &json!({"ResourceArn": arn, "Tags": tags_json(&[pair("room-now", "x")])}),
    )
    .await;
    let final_tags = list_all_tags(&router, "room after an untag", &arn, Some(100)).await;
    assert_eq!(
        final_tags.get("room-now").map(String::as_str),
        Some("x"),
        "after freeing a tag key the next tag call is accepted, got {final_tags:?}"
    );
    assert_eq!(
        final_tags.len(),
        MAX_TAGS,
        "expected {MAX_TAGS} tags again, got {}",
        final_tags.len()
    );
}
