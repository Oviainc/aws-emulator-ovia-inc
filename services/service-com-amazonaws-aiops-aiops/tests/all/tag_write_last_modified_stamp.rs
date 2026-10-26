// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the record stamp a TAG write leaves behind.
//!
//! The rule under test: a successful write to a stored investigation group re-stamps
//! that record's `lastModifiedBy`/`lastModifiedAt` with the calling principal and the
//! instant of the call — whichever operation performed the write, the tagging
//! operations included, since `TagResource` and `UntagResource` mutate the same stored
//! group record. A call that writes nothing does not: a read leaves both members where
//! they were, and so does a refused write.
//!
//! The stamp is whole seconds, so the assertion is that it STRICTLY advances rather
//! than that it carries any particular value; `createdAt` never moves.
//!
//! Rows differ along the writing operation (`TagResource`, `UntagResource`), along
//! what the write does to the stored set (adds one key, adds several, replaces a
//! value, rewrites the value a key already has, removes one key, removes several,
//! names a key the resource never carried) and along the CALLER, so the rows also pin
//! that `lastModifiedBy` follows the principal of the call that wrote.

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::state::{MAX_TAGS, RESERVED_TAG_KEY_PREFIX};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every call here signs for; one group is enough for the whole table.
const REGION: &str = "us-west-2";

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN satisfying the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The error code a refused tag write reports (the tagging frontend's own
/// `BadRequestException`; the rule lives on `state::tag_capacity_rejection`).
const TAGGING_BAD_REQUEST: &str = "BadRequestException";

/// One emulator with an empty store, mounted exactly as the router mounts it.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(
        engine,
    ));
    router
}

/// A request signed for `aiops` in [`REGION`] with access key id `akid`, which is what
/// the service derives `lastModifiedBy` from.
fn request(
    akid: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{REGION}/aiops/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    let payload = match body {
        Some(value) => {
            builder = builder.header("content-type", "application/json");
            bytes::Bytes::from(serde_json::to_vec(value).expect("serialize the request body"))
        }
        None => bytes::Bytes::new(),
    };
    builder.body(payload).expect("a well-formed test request")
}

/// Status and parsed body of one call. An empty body reads as `Value::Null`.
async fn call_as(
    router: &MultiServiceRouter,
    akid: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router.dispatch(request(akid, method, uri, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so one accessor reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// The default caller, for the calls whose principal a case does not care about.
const DEFAULT_AKID: &str = "AKIDAIOPSTEST0";

async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    call_as(router, DEFAULT_AKID, method, uri, body).await
}

/// The modelled error shape a failed call reported.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit('#')
        .next()
        .unwrap_or("<no __type>")
        .to_string()
}

/// Percent-encode everything a URI component may not carry literally — what an SDK
/// does for an `@httpLabel` path segment and for an `@httpQuery` value.
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn tags_body(pairs: &[(&str, &str)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert((*key).to_string(), Value::String((*value).to_string()));
    }
    Value::Object(map)
}

/// Create the one group this suite writes to, answering its ARN.
async fn create_group(router: &MultiServiceRouter, name: &str) -> String {
    let (status, created) = call(
        router,
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": name, "roleArn": ROLE_ARN })),
    )
    .await;
    assert_eq!(
        status, 201,
        "create {name:?}: expected 201, got {status} / {created:?}"
    );
    created
        .get("arn")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("create {name:?}: response carried no arn, got {created:?}"))
        .to_string()
}

/// `GetInvestigationGroup`, asserting the read itself succeeded.
async fn get_group(router: &MultiServiceRouter, arn: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/investigationGroups/{}", encode(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetInvestigationGroup, got {status} / {body:?}"
    );
    body
}

/// The record stamp a read reports: `(lastModifiedAt, lastModifiedBy, createdAt)`.
async fn stamp(router: &MultiServiceRouter, arn: &str, case: &str) -> (i64, String, i64) {
    let group = get_group(router, arn, case).await;
    let at = group
        .get("lastModifiedAt")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: no lastModifiedAt in {group:?}"));
    let by = group
        .get("lastModifiedBy")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: no lastModifiedBy in {group:?}"))
        .to_string();
    let created = group
        .get("createdAt")
        .and_then(Value::as_i64)
        .unwrap_or_else(|| panic!("case {case:?}: no createdAt in {group:?}"));
    (at, by, created)
}

/// The tag set `ListTagsForResource` reports, as a comparable map.
async fn stored_tags(
    router: &MultiServiceRouter,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(router, "GET", &format!("/tags/{}", encode(arn)), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {status} / {body:?}"
    );
    body.get("tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: no tags in {body:?}"))
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case:?}: a tag value is a string, {value:?}"))
                    .to_string(),
            )
        })
        .collect()
}

/// The principal the service reports for a call signed with `akid`: the request's own
/// access key id, rendered as an IAM user ARN under the canonical emulator account.
fn principal_of(akid: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{akid}")
}

/// One tag write: the operation and what it writes.
enum Write<'a> {
    /// `TagResource` with these key-value pairs.
    Tag(&'a [(&'a str, &'a str)]),
    /// `UntagResource` with these keys.
    Untag(&'a [&'a str]),
}

/// Perform one tag write as `akid`.
async fn apply(
    router: &MultiServiceRouter,
    akid: &str,
    arn: &str,
    write: &Write<'_>,
) -> (u16, Value) {
    match write {
        Write::Tag(pairs) => {
            call_as(
                router,
                akid,
                "POST",
                &format!("/tags/{}", encode(arn)),
                Some(&json!({ "tags": tags_body(pairs) })),
            )
            .await
        }
        Write::Untag(keys) => {
            let query: Vec<String> = keys
                .iter()
                .map(|key| format!("tagKeys={}", encode(key)))
                .collect();
            call_as(
                router,
                akid,
                "DELETE",
                &format!("/tags/{}?{}", encode(arn), query.join("&")),
                None,
            )
            .await
        }
    }
}

/// The stored set after `write` is applied to `before`.
fn applied(before: &BTreeMap<String, String>, write: &Write<'_>) -> BTreeMap<String, String> {
    let mut after = before.clone();
    match write {
        Write::Tag(pairs) => {
            for (key, value) in *pairs {
                after.insert((*key).to_string(), (*value).to_string());
            }
        }
        Write::Untag(keys) => {
            for key in *keys {
                after.remove(*key);
            }
        }
    }
    after
}

/// Every successful tag write re-stamps the group record it mutated: `lastModifiedAt`
/// strictly advances and `lastModifiedBy` names the caller that wrote, while
/// `createdAt` stays where the create put it.
#[tokio::test]
async fn every_successful_tag_write_restamps_the_group_record() {
    let router = emulator();
    let arn = create_group(&router, "tag-write-stamps").await;

    let (created_stamp, _, created_at) = stamp(&router, &arn, "the create").await;
    assert_eq!(
        created_stamp, created_at,
        "an untouched group reports lastModifiedAt {created_at}, got {created_stamp}"
    );

    // The tag-value row's characters come from the modelled `TagKey`/`TagValue` pattern
    // `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`:
    // `smithy_query(query="shape",shape="Tags")`.
    let rows: Vec<(&str, &str, Write<'_>)> = vec![
        (
            "TagResource adding one new key",
            "AKIDAIOPSONE0",
            Write::Tag(&[("Team", "ops")]),
        ),
        (
            "TagResource adding several new keys",
            "AKIDAIOPSTWO0",
            Write::Tag(&[("Service", "aiops"), ("Env", "prod"), ("Owner", "sre")]),
        ),
        (
            "TagResource replacing the value of a stored key",
            "AKIDAIOPSTHREE",
            Write::Tag(&[("Team", "platform")]),
        ),
        (
            "TagResource rewriting a stored key with the value it already has",
            "AKIDAIOPSFOUR0",
            Write::Tag(&[("Team", "platform")]),
        ),
        (
            "TagResource writing the characters a key and value may carry",
            "AKIDAIOPSFIVE0",
            Write::Tag(&[("Cost.Center/1", "a b:c=d+e-f@g")]),
        ),
        (
            "UntagResource removing one stored key",
            "AKIDAIOPSSIX00",
            Write::Untag(&["Owner"]),
        ),
        (
            "UntagResource removing several stored keys",
            "AKIDAIOPSSEVEN",
            Write::Untag(&["Env", "Service"]),
        ),
        (
            "UntagResource naming a key the resource never carried",
            "AKIDAIOPSEIGHT",
            Write::Untag(&["NeverPresentKey"]),
        ),
    ];

    let mut previous = created_stamp;
    let mut expected_tags: BTreeMap<String, String> = BTreeMap::new();
    for (case, akid, write) in &rows {
        let (status, answer) = apply(&router, akid, &arn, write).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} / {answer:?}"
        );

        expected_tags = applied(&expected_tags, write);
        assert_eq!(
            stored_tags(&router, &arn, case).await,
            expected_tags,
            "case {case:?}: the write must leave exactly the expected stored tag set"
        );

        let (at, by, created) = stamp(&router, &arn, case).await;
        assert!(
            at > previous,
            "case {case:?}: expected lastModifiedAt to advance past {previous}, got {at}"
        );
        assert_eq!(
            by,
            principal_of(akid),
            "case {case:?}: expected lastModifiedBy {}, got {by:?}",
            principal_of(akid)
        );
        assert_eq!(
            created, created_at,
            "case {case:?}: createdAt must not move from {created_at}, got {created}"
        );

        // The stamp the write set is what the NEXT read reports too: reading does not
        // move it again.
        let (repeat, repeat_by, _) = stamp(&router, &arn, case).await;
        assert_eq!(
            (repeat, repeat_by.as_str()),
            (at, by.as_str()),
            "case {case:?}: a read must report the stored stamp {at} / {by:?}, \
             got {repeat} / {repeat_by:?}"
        );
        previous = at;
    }
}

/// A call that writes nothing leaves the record stamp exactly where it was: neither a
/// read of the resource nor a REFUSED tag write re-stamps it.
///
/// Rows differ along the refusal: a reserved-namespace key from either tagging write
/// (400), a resulting tag set past the published maximum (400), and a `resourceArn`
/// naming no resource (404).
#[tokio::test]
async fn a_call_that_writes_nothing_leaves_the_stamp_alone() {
    let router = emulator();
    let arn = create_group(&router, "refused-writes-keep-the-stamp").await;

    // One accepted write first, so the stamp under test is not the create's.
    let (status, answer) = apply(
        &router,
        DEFAULT_AKID,
        &arn,
        &Write::Tag(&[("Team", "ops")]),
    )
    .await;
    assert_eq!(
        status, 200,
        "the setup tag write: expected 200, got {status} / {answer:?}"
    );
    let (baseline, baseline_by, created_at) = stamp(&router, &arn, "the setup tag write").await;

    // Reads: ListTagsForResource and GetInvestigationGroup, each twice.
    for case in ["the first read", "the second read"] {
        assert_eq!(
            stored_tags(&router, &arn, case).await.len(),
            1,
            "case {case:?}: the stored tag set is unchanged by a read"
        );
        let (at, by, created) = stamp(&router, &arn, case).await;
        assert_eq!(
            (at, by.as_str(), created),
            (baseline, baseline_by.as_str(), created_at),
            "case {case:?}: a read must leave the stamp at {baseline} / {baseline_by:?}, \
             got {at} / {by:?}"
        );
    }

    // A tag set one past the published per-resource maximum, which is refused whole.
    let past_max: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|i| (format!("key{i:02}"), format!("value{i:02}")))
        .collect();
    let past_max: Vec<(&str, &str)> = past_max
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();

    let reserved_key = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");
    let reserved_pair = [(reserved_key.as_str(), "stack")];
    let reserved_keys = [reserved_key.as_str()];
    let refusals: Vec<(&str, Write<'_>, u16, &str)> = vec![
        (
            "TagResource naming a reserved-namespace key",
            Write::Tag(&reserved_pair),
            400,
            TAGGING_BAD_REQUEST,
        ),
        (
            "UntagResource naming a reserved-namespace key",
            Write::Untag(&reserved_keys),
            400,
            TAGGING_BAD_REQUEST,
        ),
        (
            "TagResource taking the resource one past the tag maximum",
            Write::Tag(&past_max),
            400,
            TAGGING_BAD_REQUEST,
        ),
    ];

    for (case, write, expected_status, expected_code) in &refusals {
        let (status, answer) = apply(&router, "AKIDAIOPSREFUSED", &arn, write).await;
        assert_eq!(
            status, *expected_status, "case {case:?}: expected {expected_status}, got {status} / {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            *expected_code,
            "case {case:?}: expected {expected_code}, got {answer:?}"
        );
        let (at, by, created) = stamp(&router, &arn, case).await;
        assert_eq!(
            (at, by.as_str(), created),
            (baseline, baseline_by.as_str(), created_at),
            "case {case:?}: a refused write must leave the stamp at {baseline} / \
             {baseline_by:?}, got {at} / {by:?}"
        );
        assert_eq!(
            stored_tags(&router, &arn, case).await.len(),
            1,
            "case {case:?}: a refused write must leave the stored tag set alone"
        );
    }

    // And a write to a `resourceArn` naming no resource writes nothing anywhere.
    let absent = format!("arn:aws:aiops:{REGION}:{ACCOUNT}:investigation-group/aaaaaaaaaaaaaaaa");
    let (status, answer) = apply(
        &router,
        "AKIDAIOPSABSENT",
        &absent,
        &Write::Tag(&[("Team", "ops")]),
    )
    .await;
    assert_eq!(
        status, 404,
        "a tag write against an absent resource: expected 404, got {status} / {answer:?}"
    );
    let (at, by, created) = stamp(&router, &arn, "a tag write against an absent resource").await;
    assert_eq!(
        (at, by.as_str(), created),
        (baseline, baseline_by.as_str(), created_at),
        "a tag write against an absent resource must leave this group's stamp at \
         {baseline} / {baseline_by:?}, got {at} / {by:?}"
    );
}
