// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, over every path of this service that writes or removes
//! tags, end to end over HTTP.
//!
//! RULE: a tag key beginning with the `aws:` prefix names a tag Amazon Web Services
//! applies itself, so no caller may create, change or remove one through a tagging API —
//! every request naming such a key is refused with the modelled 400 `ValidationException`
//! (reason `fieldValidationFailed`, the `fieldList` naming the member that carried the
//! key) and persists nothing, so no caller-created tag set ever reports one. The rule is a
//! PREFIX test on the key: a key that only resembles the namespace is an ordinary tag.
//!
//! Every case goes through the router — routing, the front door, deserialization, the
//! handler, the persisted state and serialization — so a refusal that does not come from
//! the handler, or a write that slipped through, cannot pass.
//!
//! The paths covered are every one that names a tag key: `CreateComplianceInquiry$tags`
//! (the create that persists the first tag set), `TagResource$tags` and
//! `UntagResource$tagKeys`. `ListTagsForResource` and `GetComplianceInquiryMetadata` are
//! the readers each case reads the tag set back through.
//!
//! Rows differ along what the rule generalises over: what follows the prefix (nothing, one
//! character, a real AWS system key, every character class the key pattern admits), the
//! key's length up to `TagKey`'s published ceiling, whether the refused key travels alone
//! or beside acceptable keys, and — for the lookalikes — where in the key the prefix's
//! characters appear.

use std::collections::BTreeMap;

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The router bootstrap, the credential scope and the region it signs for are the crate's
// test harness': one emulator over one shared engine.
use crate::harness::{emulator, signed_for, REGION};

/// The reserved tag-key prefix, spelled out here rather than imported from the crate so a
/// wrong constant in `src/state.rs` cannot make this suite pass. Source: the AWS-wide
/// tagging rule the issue cites — keys prefixed `aws:` are reserved for AWS use.
const RESERVED_PREFIX: &str = "aws:";

/// `TagKey`'s modelled ceiling, `@length(min: 1, max: 128)`
/// (`smithy_query(query="shape",shape="TagKey")`).
const TAG_KEY_CEILING: usize = 128;

/// The modelled refusal a request naming a reserved key must answer with: the status, the
/// wire error code and `ValidationExceptionReason`'s value for a member the service itself
/// refused.
const REFUSAL: (u16, &str, &str) = (400, "ValidationException", "fieldValidationFailed");

/// What the emulator answered: the HTTP status and the JSON body.
struct Answer {
    status: u16,
    body: Value,
}

impl Answer {
    /// The wire error code (`__type`), or the empty string on a success.
    fn error_code(&self) -> &str {
        self.body["__type"].as_str().unwrap_or_default()
    }

    /// The `ValidationExceptionReason` the answer carries.
    fn reason(&self) -> &str {
        self.body["reason"].as_str().unwrap_or_default()
    }

    /// The member the `fieldList` names, or the empty string.
    fn field_name(&self) -> &str {
        self.body["fieldList"][0]["name"]
            .as_str()
            .unwrap_or_default()
    }
}

/// Dispatch one request through the router, signed for [`REGION`]. The harness' own
/// request helper serves GET only, and these cases are POSTs and DELETEs carrying a body
/// or a query string.
async fn call(router: &MultiServiceRouter, method: &str, uri: &str, body: Option<Value>) -> Answer {
    let payload = body.map_or_else(Bytes::new, |value| Bytes::from(value.to_string()));
    let request = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "artifact.eu-west-1.amazonaws.com")
        .header("authorization", signed_for(REGION))
        .header("content-type", "application/json")
        .body(payload)
        .expect("the test request is well formed");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    Answer { status, body }
}

/// `resourceArn` is an `@httpLabel` on `/tags/{resourceArn}`, and a non-greedy label is
/// ONE path segment, so the `/` inside an ARN is percent-encoded.
fn label(resource_arn: &str) -> String {
    resource_arn.replace('/', "%2F")
}

/// One query-string value, percent-encoded through the crate this service already depends
/// on: a tag key may carry `/`, `=`, `+`, `:` and whitespace, each of which means
/// something else in a query string.
fn query_value(value: &str) -> String {
    utf8_percent_encode(value, NON_ALPHANUMERIC).to_string()
}

/// Tag pairs as the `TagsMap` a request body carries.
fn tags_object(tags: &[(String, String)]) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), json!(value)))
            .collect(),
    )
}

/// `&[(&str, &str)]` as owned, comparable tag pairs.
fn pairs(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The tags a body reports, in a comparable order, or `None` when it carries no `tags`
/// member at all.
fn reported_tags(body: &Value) -> Option<BTreeMap<String, String>> {
    let tags = body.get("tags")?.as_object()?;
    Some(
        tags.iter()
            .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
            .collect(),
    )
}

/// `POST /v1/compliance-inquiry/create` — `CreateComplianceInquiry`, the create that
/// persists a resource's first tag set.
async fn create(router: &MultiServiceRouter, name: &str, tags: &[(String, String)]) -> Answer {
    let mut body = json!({
        "name": name,
        "inquiryContent": { "query": "Which controls does this report cover?" },
        "supportMode": "AI_ONLY"
    });
    if !tags.is_empty() {
        body["tags"] = tags_object(tags);
    }
    call(router, "POST", "/v1/compliance-inquiry/create", Some(body)).await
}

/// Create one inquiry carrying `tags` and answer its id and the ARN the service minted.
async fn created_inquiry(
    router: &MultiServiceRouter,
    name: &str,
    tags: &[(String, String)],
) -> (String, String) {
    let answer = create(router, name, tags).await;
    assert_eq!(
        answer.status, 202,
        "case {name:?}: creating the inquiry must answer HTTP 202, got {} with {:?}",
        answer.status, answer.body
    );
    let summary = &answer.body["complianceInquirySummary"];
    let id = summary["id"].as_str().unwrap_or_default().to_string();
    let arn = summary["arn"].as_str().unwrap_or_default().to_string();
    assert!(
        !id.is_empty() && !arn.is_empty(),
        "case {name:?}: the create must report an id and an ARN, got {:?}",
        answer.body
    );
    (id, arn)
}

/// `POST /tags/{resourceArn}` — `TagResource`.
async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &[(String, String)]) -> Answer {
    call(
        router,
        "POST",
        &format!("/tags/{}", label(resource_arn)),
        Some(json!({ "tags": tags_object(tags) })),
    )
    .await
}

/// `DELETE /tags/{resourceArn}?tagKeys=…` — `UntagResource`.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[String]) -> Answer {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", query_value(key)))
        .collect();
    call(
        router,
        "DELETE",
        &format!("/tags/{}?{}", label(resource_arn), query.join("&")),
        None,
    )
    .await
}

/// `GET /tags/{resourceArn}` — `ListTagsForResource`.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    call(router, "GET", &format!("/tags/{}", label(resource_arn)), None).await
}

/// `GET /v1/compliance-inquiry/getMetadata` — the other reader of the same tag set.
async fn get_metadata(router: &MultiServiceRouter, inquiry_id: &str) -> Answer {
    call(
        router,
        "GET",
        &format!("/v1/compliance-inquiry/getMetadata?complianceInquiryId={inquiry_id}"),
        None,
    )
    .await
}

/// How many inquiries this account holds, read through `ListComplianceInquiries`: a
/// refused create must leave that count where it was.
async fn inquiry_count(router: &MultiServiceRouter) -> usize {
    let answer = call(router, "GET", "/v1/compliance-inquiry/list", None).await;
    assert_eq!(
        answer.status, 200,
        "ListComplianceInquiries must answer HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["complianceInquiries"]
        .as_array()
        .map_or(0, Vec::len)
}

/// Assert that both readers of the resource's one tag set report exactly `expected`.
async fn assert_tag_set(
    router: &MultiServiceRouter,
    case: &str,
    inquiry_id: &str,
    resource_arn: &str,
    expected: &[(String, String)],
) {
    let want: BTreeMap<String, String> = expected.iter().cloned().collect();
    let listed = list_tags(router, resource_arn).await;
    assert_eq!(
        listed.status, 200,
        "case {case:?}: ListTagsForResource must answer HTTP 200, got {} with {:?}",
        listed.status, listed.body
    );
    assert_eq!(
        reported_tags(&listed.body),
        Some(want.clone()),
        "case {case:?}: ListTagsForResource must report the persisted tag set, got {:?}",
        listed.body
    );

    let read = get_metadata(router, inquiry_id).await;
    assert_eq!(
        read.status, 200,
        "case {case:?}: GetComplianceInquiryMetadata must answer HTTP 200, got {} with {:?}",
        read.status, read.body
    );
    assert_eq!(
        reported_tags(&read.body).unwrap_or_default(),
        want,
        "case {case:?}: the metadata read must report the same tag set, got {:?}",
        read.body
    );
}

/// Assert `answer` is the modelled refusal, naming `member` in its field list.
fn assert_refused(case: &str, what: &str, member: &str, answer: &Answer) {
    assert_eq!(
        answer.status, REFUSAL.0,
        "case {case:?} on {what}: expected HTTP {}, got {} with {:?}",
        REFUSAL.0, answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        REFUSAL.1,
        "case {case:?} on {what}: expected the modelled {}, got {:?}",
        REFUSAL.1,
        answer.body
    );
    assert_eq!(
        answer.reason(),
        REFUSAL.2,
        "case {case:?} on {what}: expected reason {}, got {:?}",
        REFUSAL.2,
        answer.body
    );
    assert_eq!(
        answer.field_name(),
        member,
        "case {case:?} on {what}: the fieldList must name {member}, got {:?}",
        answer.body
    );
}

/// Tag keys INSIDE the reserved namespace. Rows differ along what follows the prefix:
/// nothing at all, one character, a real AWS system key, every character class
/// `TagKey`'s pattern admits beside the alphanumerics (`_ . : / = + - @` and a space), and
/// a key padded out to `TagKey`'s published length ceiling.
fn reserved_keys() -> Vec<(&'static str, String)> {
    vec![
        ("the prefix alone", RESERVED_PREFIX.to_string()),
        (
            "a key AWS itself applies",
            format!("{RESERVED_PREFIX}cloudformation:stack-name"),
        ),
        ("one character past the prefix", format!("{RESERVED_PREFIX}x")),
        (
            "every character class the key pattern admits",
            format!("{RESERVED_PREFIX}a_b.c:d/e=f+g-h@i j"),
        ),
        (
            "at the published key-length ceiling",
            format!(
                "{RESERVED_PREFIX}{}",
                "k".repeat(TAG_KEY_CEILING - RESERVED_PREFIX.len())
            ),
        ),
    ]
}

/// Keys that only RESEMBLE the reserved namespace: the prefix without its colon, the
/// prefix's letters inside a longer word, the prefix away from the start of the key, and a
/// key that carries the colon but not the letters. None of these begins with the reserved
/// prefix, so each is an ordinary tag.
fn lookalike_keys() -> Vec<(&'static str, &'static str)> {
    vec![
        ("the prefix without its colon", "aws"),
        ("the prefix's letters inside a word", "awsome"),
        ("the letters with another separator", "aws-cloudformation"),
        ("the prefix away from the start", "team:aws:owner"),
        ("a colon-led key that is not the prefix", ":aws"),
    ]
}

#[tokio::test]
async fn a_reserved_tag_key_is_refused_by_every_path_that_writes_tags() {
    let router = emulator();

    // The resource every tag write in this test addresses, and the tag set it must still
    // carry after each refusal.
    let kept = pairs(&[("team", "compliance"), ("env", "prod")]);
    let (id, arn) = created_inquiry(&router, "the tagged inquiry", &kept).await;
    let before = inquiry_count(&router).await;

    for (case, key) in reserved_keys() {
        // Alone, and beside keys the service would otherwise accept: the refusal is of the
        // whole request, so the acceptable keys beside it are not persisted either.
        let alone = vec![(key.clone(), "mine".to_string())];
        let beside = vec![
            ("alongside".to_string(), "kept out".to_string()),
            (key.clone(), "mine".to_string()),
        ];

        for (shape, tags) in [("alone", &alone), ("beside an acceptable key", &beside)] {
            let what = format!("TagResource with the key {shape}");
            let answer = tag(&router, &arn, tags).await;
            assert_refused(case, &what, "tags", &answer);

            let what = format!("CreateComplianceInquiry with the key {shape}");
            let answer = create(&router, case, tags).await;
            assert_refused(case, &what, "tags", &answer);
        }

        // Removal is refused for the same reason: an AWS-owned key is no more a caller's
        // to delete than to create.
        let answer = untag(&router, &arn, std::slice::from_ref(&key)).await;
        assert_refused(case, "UntagResource", "tagKeys", &answer);

        // A reserved key beside a key the resource DOES carry: the whole removal is
        // refused, so the carried key survives.
        let answer = untag(&router, &arn, &[key.clone(), "team".to_string()]).await;
        assert_refused(case, "UntagResource beside a carried key", "tagKeys", &answer);

        // Nothing any of those refusals touched: the resource keeps its tag set — so no
        // reserved key is ever reported for it — and no inquiry was created.
        assert_tag_set(&router, case, &id, &arn, &kept).await;
        assert_eq!(
            inquiry_count(&router).await,
            before,
            "case {case:?}: a refused create must not mint an inquiry, count moved from \
             {before}"
        );
    }
}

#[tokio::test]
async fn keys_that_only_resemble_the_reserved_prefix_are_ordinary_tags() {
    let router = emulator();

    for (case, key) in lookalike_keys() {
        // Written by the create, so the create path accepts it too.
        let created_with = pairs(&[(key, "first")]);
        let (id, arn) = created_inquiry(&router, case, &created_with).await;
        assert_tag_set(&router, case, &id, &arn, &created_with).await;

        // Overwritten by a tag write, then removed again: every path treats it as the
        // ordinary tag it is.
        let rewritten = pairs(&[(key, "second")]);
        let answer = tag(&router, &arn, &rewritten).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: TagResource must answer HTTP 204, got {} with {:?}",
            answer.status, answer.body
        );
        assert_tag_set(&router, case, &id, &arn, &rewritten).await;

        let answer = untag(&router, &arn, &[key.to_string()]).await;
        assert_eq!(
            answer.status, 204,
            "case {case:?}: UntagResource must answer HTTP 204, got {} with {:?}",
            answer.status, answer.body
        );
        assert_tag_set(&router, case, &id, &arn, &[]).await;
    }
}

#[tokio::test]
async fn a_reserved_key_is_refused_before_the_resource_arn_is_resolved() {
    let router = emulator();

    // An ARN this account holds no resource for: the reserved-namespace rule is decided
    // from the request alone, so the refusal is the validation failure rather than the
    // front door's authorization refusal, and it discloses nothing about the ARN.
    let absent = format!(
        "arn:aws:artifact:{REGION}:{}:compliance-inquiry/compliance-inquiry-0000000000000000",
        engine::emulator_account_id()
    );
    let key = format!("{RESERVED_PREFIX}cloudformation:stack-name");

    let answer = tag(&router, &absent, &[(key.clone(), "mine".to_string())]).await;
    assert_refused(
        "an ARN naming no resource this account holds",
        "TagResource",
        "tags",
        &answer,
    );

    let answer = untag(&router, &absent, &[key]).await;
    assert_refused(
        "an ARN naming no resource this account holds",
        "UntagResource",
        "tagKeys",
        &answer,
    );
}
