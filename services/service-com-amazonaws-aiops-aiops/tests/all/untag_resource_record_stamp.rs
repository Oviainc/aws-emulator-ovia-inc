// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the record stamp `UntagResource` leaves behind.
//!
//! The rule under test: an accepted write to a stored investigation group re-stamps
//! that record's `lastModifiedBy`/`lastModifiedAt` with the calling principal and the
//! instant of the call, whatever the write did to the stored set — so a caller watching
//! those two members notices a tag removal and can order two writers by it. `createdAt`
//! and every tag the request did not name are left exactly where they were, and a later
//! read reports the stored stamp rather than moving it again.
//!
//! `tests/all/tag_write_last_modified_stamp.rs` walks one group through a sequence of
//! writes; this suite drives the removal side from the state a CREATE established, which
//! is the path a caller takes when it tags at create time and untags afterwards, and
//! spans what the removal can do to the set: take the only key (leaving the resource with
//! none), take one of several, take several at once, take every key, name keys the
//! resource never carried beside ones it does, name only absent keys, and name the most
//! keys one call may carry.
//!
//! The stamp is whole seconds, so each row asserts it STRICTLY advances rather than that
//! it carries any particular value.
//!
//! An account holds ONE investigation group per Region, so every row signs for its own
//! Region ([`region`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{json, Map, Value};
use service_com_amazonaws_aiops_aiops::state::MAX_TAGS;
use smithy_server_runtime::MultiServiceRouter;

/// The canonical emulator account (`engine::EMULATOR_ACCOUNT_ID`) every ARN carries.
const ACCOUNT: &str = "000000000000";

/// A role ARN satisfying the modelled `RoleArn` (len 20..2048, `^arn:.*$`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/AIOpsAssistantRole";

/// The most tag keys one `UntagResource` call may name.
///
/// The modelled length of its `tagKeys` member:
/// `smithy_query(query="operation",operation="UntagResource")` reports `TagKeys, len ..50`.
const MAX_TAG_KEYS_PER_CALL: usize = 50;

/// Regions the rows sign for, in order; each row needs its own group and so its own
/// Region. Past this list the names are synthetic ones inside what a SigV4 credential
/// scope admits (`[a-zA-Z0-9-]`).
const REGIONS: &[&str] = &[
    "us-west-2",
    "us-east-1",
    "eu-central-1",
    "eu-west-2",
    "ap-southeast-1",
    "ap-northeast-1",
    "sa-east-1",
    "ca-central-1",
    "eu-north-1",
];

/// The Region row `n` signs for.
fn region(n: usize) -> String {
    REGIONS
        .get(n)
        .map_or_else(|| format!("region-{n:03}"), |name| (*name).to_string())
}

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

/// A request signed for `aiops` in `region` with access key id `akid`, which is what the
/// service derives `lastModifiedBy` from.
fn request(
    region: &str,
    akid: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> http::Request<bytes::Bytes> {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("aiops.{region}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={akid}/20260101/{region}/aiops/aws4_request, \
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
async fn call(
    router: &MultiServiceRouter,
    region: &str,
    akid: &str,
    method: &str,
    uri: &str,
    body: Option<&Value>,
) -> (u16, Value) {
    let response = router
        .dispatch(request(region, akid, method, uri, body))
        .await;
    let status = response.status().as_u16();
    let raw = response.into_body();
    let parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    (status, parsed)
}

/// Percent-encode everything a URI component may not carry literally — what an SDK does
/// for an `@httpLabel` path segment and for an `@httpQuery` value.
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

/// The `tags` map of a create request, from a list of pairs.
fn tags_body(pairs: &[(String, String)]) -> Value {
    let mut map = Map::new();
    for (key, value) in pairs {
        map.insert(key.clone(), Value::String(value.clone()));
    }
    Value::Object(map)
}

/// The principal the service reports for a call signed with `akid`: the request's own
/// access key id, rendered as an IAM user ARN under the canonical emulator account.
fn principal_of(akid: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:user/{akid}")
}

/// Create one group carrying `tags` in `region`, answering its ARN.
async fn create_group(
    router: &MultiServiceRouter,
    region: &str,
    name: &str,
    tags: &[(String, String)],
) -> String {
    let (status, created) = call(
        router,
        region,
        "AKIDAIOPSCREATE",
        "POST",
        "/investigationGroups",
        Some(&json!({ "name": name, "roleArn": ROLE_ARN, "tags": tags_body(tags) })),
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

/// The record stamp `GetInvestigationGroup` reports: `(lastModifiedAt, lastModifiedBy,
/// createdAt)`.
async fn stamp(
    router: &MultiServiceRouter,
    region: &str,
    arn: &str,
    case: &str,
) -> (i64, String, i64) {
    let (status, group) = call(
        router,
        region,
        "AKIDAIOPSREADER",
        "GET",
        &format!("/investigationGroups/{}", encode(arn)),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from GetInvestigationGroup, got {status} / {group:?}"
    );
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
    region: &str,
    arn: &str,
    case: &str,
) -> BTreeMap<String, String> {
    let (status, body) = call(
        router,
        region,
        "AKIDAIOPSREADER",
        "GET",
        &format!("/tags/{}", encode(arn)),
        None,
    )
    .await;
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

/// `UntagResource` as `akid`, naming `keys` through the `@httpQuery` `tagKeys` list.
async fn untag(
    router: &MultiServiceRouter,
    region: &str,
    akid: &str,
    arn: &str,
    keys: &[String],
) -> (u16, Value) {
    let query: Vec<String> = keys
        .iter()
        .map(|key| format!("tagKeys={}", encode(key)))
        .collect();
    call(
        router,
        region,
        akid,
        "DELETE",
        &format!("/tags/{}?{}", encode(arn), query.join("&")),
        None,
    )
    .await
}

fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
    entries
        .iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

fn keys(entries: &[&str]) -> Vec<String> {
    entries.iter().map(|key| (*key).to_string()).collect()
}

/// One row: a starting tag set, the keys one `UntagResource` call names and the set the
/// resource is left with.
struct Row {
    case: &'static str,
    akid: &'static str,
    initial: Vec<(String, String)>,
    remove: Vec<String>,
    expected: Vec<(String, String)>,
}

/// Every accepted `UntagResource` re-stamps the group record it wrote to: `lastModifiedAt`
/// strictly advances past the stamp the create left, `lastModifiedBy` names the caller
/// that removed the keys, `createdAt` does not move and the tags the request did not name
/// stay stored.
#[tokio::test]
async fn an_accepted_untag_restamps_the_group_record() {
    let router = emulator();

    // The characters in the key-and-value row come from the modelled `TagKey`/`TagValue`
    // pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]+)$`:
    // `smithy_query(query="shape",shape="Tags")`.
    let widest_key = "Cost.Center/1";

    // The bound row names the most keys one call may carry
    // ([`MAX_TAG_KEYS_PER_CALL`]) on a resource holding the most tags it may
    // (`state::MAX_TAGS`, the published per-resource maximum).
    let full_set: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|i| (format!("key{i:02}"), format!("value{i:02}")))
        .collect();
    let every_key: Vec<String> = full_set
        .iter()
        .map(|(key, _)| key.clone())
        .take(MAX_TAG_KEYS_PER_CALL)
        .collect();

    let rows = [
        Row {
            case: "the only key the resource carries, leaving it with none",
            akid: "AKIDAIOPSONE00",
            initial: pairs(&[("Team", "ops")]),
            remove: keys(&["Team"]),
            expected: Vec::new(),
        },
        Row {
            case: "one key out of several",
            akid: "AKIDAIOPSTWO00",
            initial: pairs(&[("Team", "ops"), ("Env", "prod"), ("Owner", "sre")]),
            remove: keys(&["Env"]),
            expected: pairs(&[("Team", "ops"), ("Owner", "sre")]),
        },
        Row {
            case: "several keys in one call",
            akid: "AKIDAIOPSTHREE",
            initial: pairs(&[
                ("Team", "ops"),
                ("Env", "prod"),
                ("Owner", "sre"),
                ("Service", "aiops"),
            ]),
            remove: keys(&["Env", "Owner"]),
            expected: pairs(&[("Team", "ops"), ("Service", "aiops")]),
        },
        Row {
            case: "every key the resource carries",
            akid: "AKIDAIOPSFOUR0",
            initial: pairs(&[("Team", "ops"), ("Env", "prod"), ("Service", "aiops")]),
            remove: keys(&["Team", "Env", "Service"]),
            expected: Vec::new(),
        },
        Row {
            case: "a stored key beside one the resource never carried",
            akid: "AKIDAIOPSFIVE0",
            initial: pairs(&[("Team", "ops"), ("Env", "prod")]),
            remove: keys(&["Env", "NeverCarriedKey"]),
            expected: pairs(&[("Team", "ops")]),
        },
        Row {
            case: "only keys the resource never carried",
            akid: "AKIDAIOPSSIX000",
            initial: pairs(&[("Team", "ops")]),
            remove: keys(&["AbsentOne", "AbsentTwo"]),
            expected: pairs(&[("Team", "ops")]),
        },
        Row {
            case: "a key using the characters the modelled pattern admits",
            akid: "AKIDAIOPSSEVEN",
            initial: vec![(widest_key.to_string(), "a b:c=d+e-f@g".to_string())],
            remove: keys(&[widest_key]),
            expected: Vec::new(),
        },
        Row {
            case: "the most keys one call may name, on a fully tagged resource",
            akid: "AKIDAIOPSEIGHT",
            initial: full_set.clone(),
            remove: every_key.clone(),
            expected: full_set
                .iter()
                .filter(|(key, _)| !every_key.contains(key))
                .cloned()
                .collect(),
        },
    ];


    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let region = region(index);
        let arn = create_group(&router, &region, &format!("untag-stamp-{index}"), &row.initial)
            .await;

        let (baseline, _, created_at) = stamp(&router, &region, &arn, case).await;
        assert_eq!(
            baseline, created_at,
            "case {case:?}: a freshly created group reports lastModifiedAt {created_at}, \
             got {baseline}"
        );

        let (status, answer) = untag(&router, &region, row.akid, &arn, &row.remove).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UntagResource, got {status} / {answer:?}"
        );

        let expected: BTreeMap<String, String> = row.expected.iter().cloned().collect();
        let actual = stored_tags(&router, &region, &arn, case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored tag set {expected:?}, got {actual:?}"
        );

        let (at, by, created) = stamp(&router, &region, &arn, case).await;
        assert!(
            at > baseline,
            "case {case:?}: expected lastModifiedAt to advance past {baseline}, got {at}"
        );
        assert_eq!(
            by,
            principal_of(row.akid),
            "case {case:?}: expected lastModifiedBy {}, got {by:?}",
            principal_of(row.akid)
        );
        assert_eq!(
            created, created_at,
            "case {case:?}: createdAt must not move from {created_at}, got {created}"
        );

        // The stamp the removal set is what the next read reports too: reading does not
        // move it again.
        let (repeat, repeat_by, _) = stamp(&router, &region, &arn, case).await;
        assert_eq!(
            (repeat, repeat_by.as_str()),
            (at, by.as_str()),
            "case {case:?}: a read must report the stored stamp {at} / {by:?}, \
             got {repeat} / {repeat_by:?}"
        );
    }
}

/// Repeating the same removal is accepted every time and re-stamps every time, so two
/// writers of the same keys are ordered by `lastModifiedAt` and the last one is reported
/// as `lastModifiedBy` — while the stored tag set reaches the same place on the first
/// call and stays there.
#[tokio::test]
async fn repeating_a_removal_orders_its_writers_by_the_stamp() {
    let router = emulator();
    let region = region(0);
    let arn = create_group(
        &router,
        &region,
        "untag-stamp-repeat",
        &pairs(&[("Team", "ops"), ("Env", "prod")]),
    )
    .await;

    let (mut previous, _, created_at) = stamp(&router, &region, &arn, "the create").await;
    let callers = ["AKIDAIOPSFIRST", "AKIDAIOPSSECOND", "AKIDAIOPSTHIRD0"];
    let removed = keys(&["Env"]);

    for (attempt, akid) in callers.iter().enumerate() {
        let case = format!("removal attempt {attempt} by {akid}");
        let (status, answer) = untag(&router, &region, akid, &arn, &removed).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from UntagResource, got {status} / {answer:?}"
        );

        let expected: BTreeMap<String, String> =
            pairs(&[("Team", "ops")]).into_iter().collect();
        let actual = stored_tags(&router, &region, &arn, &case).await;
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored tag set {expected:?}, got {actual:?}"
        );

        let (at, by, created) = stamp(&router, &region, &arn, &case).await;
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
        previous = at;
    }
}
