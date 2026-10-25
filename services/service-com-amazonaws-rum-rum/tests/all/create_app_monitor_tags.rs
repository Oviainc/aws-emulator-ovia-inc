// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the tag map a WRITE of this service may leave on an app
//! monitor, driven through BOTH operations that write one: `CreateAppMonitor$Tags`, which
//! seeds the map, and `TagResource$Tags`, which merges onto it.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request — so routing, deserialization, the modelled constraints, the handler
//! and the response envelope are all exercised rather than just a handler body, and every
//! accepted write is read back through both implemented readers (`GetAppMonitor` and
//! `ListTagsForResource`), so a case asserts PERSISTED state rather than the write's own
//! answer.
//!
//! The assertions pin the RULE over a SET of inputs rather than one request: both members
//! target the one `TagMap`, so a key is admitted or refused by its key shape and a map is
//! bounded by its resource's documented ceiling, whichever operation carries it. The
//! tables therefore walk a catalog of keys per character class the modelled pattern
//! admits, a catalog per way a key can fall outside it (the reserved namespace, and
//! characters on either side of the admitted range), the key length at both published
//! bounds and one step outside each of them, and the tag count at the ceiling and one past
//! it — each row through both write paths.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, emulator, percent_encoded, tag_map, Reply, ACCOUNT, REGION};

/// The status a successful `TagResource` answers with: 204 No Content
/// (`crate::state::TAGGING_NO_CONTENT_STATUS`).
const NO_CONTENT: u16 = 204;

/// The wire error code an app-monitor call refused on a modelled constraint answers:
/// the code this service's model declares for `CreateAppMonitor`.
const VALIDATION_CODE: &str = "ValidationException";

/// The wire error code RUM's ARN-addressed tagging frontend answers a refused request
/// with. The two frontends word the same classification under different codes; which keys
/// they classify as invalid is what must agree.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// `TagKey` is `@length(min: 1, max: 128)` with
/// `@pattern("^(?!aws:)[a-zA-Z+-=._:/]+$")`.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
const MIN_TAG_KEY_LENGTH: usize = 1;
const MAX_TAG_KEY_LENGTH: usize = 128;

/// The most tags one resource holds: `CreateAppMonitor$Tags` documents "You can associate
/// as many as 50 tags with an app monitor" and `TagResource` "as many as 50 tags with a
/// resource"; `UntagResource$TagKeys` carries the matching `@length(min: 0, max: 50)`.
const MAX_TAGS: usize = 50;

/// One row of the tag-key table: a key, and whether the modelled `TagKey` pattern admits
/// it.
struct KeyCase {
    case: &'static str,
    key: String,
    admitted: bool,
}

/// The tag-key table: one row per character class the modelled pattern admits, and one
/// per way a key can fall outside it. `[a-zA-Z+-=._:/]` admits the ASCII letters, `_`,
/// and — `+-=` inside a character class being the RANGE from `+` (0x2B) to `=` (0x3D) —
/// every character in that range, so the rows walk that range's two bounds and the two
/// characters just outside it (`*`, 0x2A, and `>`, 0x3E) as well as the reserved
/// namespace the leading `(?!aws:)` excludes. The last rows of each half are the key's
/// published LENGTH bounds and one step outside each of them.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
fn key_cases() -> Vec<KeyCase> {
    let admitted: Vec<(&'static str, String)> = vec![
        ("ASCII letters", "Team".to_string()),
        ("an underscore", "key_with_underscore".to_string()),
        ("digits, inside the admitted range", "digits0123456789".to_string()),
        ("+, the admitted range's low bound", "plus+key".to_string()),
        ("=, the admitted range's high bound", "equals=key".to_string()),
        ("a comma, inside the range", "comma,key".to_string()),
        ("a semicolon, inside the range", "semi;colon".to_string()),
        ("a less-than sign, inside the range", "less<than".to_string()),
        ("the dot, colon and slash the class also names", "a.b:c/d".to_string()),
        ("aws without the colon is not the reserved prefix", "aws".to_string()),
        (
            "the reserved prefix is case-sensitive",
            "AWS:owner".to_string(),
        ),
        (
            "the lookahead is anchored: aws: inside the key is not the prefix",
            "team-aws:owner".to_string(),
        ),
        (
            "one character, TagKey's minimum length",
            "k".repeat(MIN_TAG_KEY_LENGTH),
        ),
        (
            "128 characters, TagKey's maximum length",
            "k".repeat(MAX_TAG_KEY_LENGTH),
        ),
    ];
    let too_long = "k".repeat(MAX_TAG_KEY_LENGTH + 1);
    let too_short = "k".repeat(MIN_TAG_KEY_LENGTH - 1);
    let rejected: Vec<(&'static str, String)> = vec![
        ("AWS's reserved namespace", "aws:owner".to_string()),
        ("the reserved prefix alone", "aws:".to_string()),
        (
            "the reserved namespace, deeper",
            "aws:cloudformation:stack-name".to_string(),
        ),
        ("a space is outside the class", "key with space".to_string()),
        ("! is outside the class", "key!bang".to_string()),
        ("@ is outside the class", "key@at".to_string()),
        ("~ is outside the class", "key~tilde".to_string()),
        ("| is outside the class", "key|pipe".to_string()),
        (
            "*, the character just below the admitted range",
            "key*star".to_string(),
        ),
        (
            ">, the character just above the admitted range",
            "key>greater".to_string(),
        ),
        ("non-ASCII Latin text", "clé".to_string()),
        ("non-ASCII Japanese text", "キー".to_string()),
        (
            "the empty string, one short of TagKey's minimum length",
            too_short,
        ),
        (
            "129 characters, one past TagKey's maximum length",
            too_long,
        ),
    ];
    admitted
        .into_iter()
        .map(|(case, key)| KeyCase {
            case,
            key,
            admitted: true,
        })
        .chain(rejected.into_iter().map(|(case, key)| KeyCase {
            case,
            key,
            admitted: false,
        }))
        .collect()
}

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives it.
fn arn_of(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}")
}

/// A JSON tag map from `(key, value)` pairs.
fn tags_body(pairs: &[(&str, &str)]) -> Value {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), Value::String(value.to_string())))
        .collect::<serde_json::Map<String, Value>>()
        .into()
}

/// `CreateAppMonitor` for the monitor named `name`, carrying `tags`.
async fn create_with_tags(router: &MultiServiceRouter, name: &str, tags: Value) -> Reply {
    call(
        router,
        "POST",
        "/appmonitor",
        Some(json!({"Name": name, "Domain": "example.com", "Tags": tags})),
    )
    .await
}

/// `TagResource` for the resource `resource_arn` names, carrying `tags`.
async fn tag_resource(router: &MultiServiceRouter, resource_arn: &str, tags: Value) -> Reply {
    call(
        router,
        "POST",
        &format!("/tags/{}", percent_encoded(resource_arn)),
        Some(json!({"Tags": tags})),
    )
    .await
}

/// `GetAppMonitor`'s raw answer for `name` — raw, because a case asserts the 404 a refused
/// create leaves behind as well as the 200 an accepted one does.
async fn get_monitor(router: &MultiServiceRouter, name: &str) -> Reply {
    call(router, "GET", &format!("/appmonitor/{name}"), None).await
}

/// The tag map `GetAppMonitor` reports for `name`, as a sorted Rust map.
async fn monitor_tags(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> BTreeMap<String, String> {
    let reply = get_monitor(router, name).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected GetAppMonitor to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    as_map(case, &tag_map(&reply.json(case)["AppMonitor"]))
}

/// The tag map `ListTagsForResource` reports for the monitor named `name`, as a sorted
/// Rust map — the SECOND reader of the one stored map.
async fn listed_tags(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> BTreeMap<String, String> {
    let arn = arn_of(name);
    let reply = call(router, "GET", &format!("/tags/{}", percent_encoded(&arn)), None).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    as_map(case, &tag_map(&reply.json(case)))
}

/// A JSON tag map as a sorted Rust map. An omitted member reads as the empty map it
/// stands for.
fn as_map(case: &str, value: &Value) -> BTreeMap<String, String> {
    match value {
        Value::Null => BTreeMap::new(),
        Value::Object(fields) => fields
            .iter()
            .map(|(key, value)| {
                (
                    key.clone(),
                    value
                        .as_str()
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: tag {key:?} is not a string: {value}")
                        })
                        .to_string(),
                )
            })
            .collect(),
        other => panic!("case {case:?}: expected a tag map, got {other}"),
    }
}

#[tokio::test]
async fn both_operations_that_write_one_tag_map_admit_and_refuse_the_same_keys() {
    let (router, _engine) = emulator();
    // The resource every `TagResource` row writes to: created with no tags, so a row's
    // refusal is about the key it names and nothing else.
    let target = "ck-tag-target";
    let reply = create_with_tags(&router, target, json!({})).await;
    assert_eq!(
        reply.status,
        200,
        "expected the target monitor to be created, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let target_arn = arn_of(target);

    for (index, row) in key_cases().into_iter().enumerate() {
        let name = format!("ck-create-{index:02}");
        let case = format!("CreateAppMonitor with a key: {}", row.case);
        // The key travels beside one that WOULD be accepted, so a refusal cannot be read
        // as "the request held nothing valid".
        let requested = tags_body(&[(row.key.as_str(), "value"), ("Valid", "value")]);
        let reply = create_with_tags(&router, &name, requested.clone()).await;

        if row.admitted {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: expected HTTP 200 for key {:?}, got {} {:?}",
                row.key,
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            let expected: BTreeMap<String, String> = [
                (row.key.clone(), "value".to_string()),
                ("Valid".to_string(), "value".to_string()),
            ]
            .into_iter()
            .collect();
            let reported = monitor_tags(&router, &case, &name).await;
            assert_eq!(
                reported, expected,
                "case {case:?}: GetAppMonitor must report the seeded map {expected:?}, got {reported:?}"
            );
            let listed = listed_tags(&router, &case, &name).await;
            assert_eq!(
                listed, expected,
                "case {case:?}: ListTagsForResource must report the same map {expected:?}, got {listed:?}"
            );
        } else {
            assert_eq!(
                reply.status,
                400,
                "case {case:?}: expected HTTP 400 for key {:?}, got {} {:?}",
                row.key,
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(&case),
                VALIDATION_CODE,
                "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            let body = reply.json(&case);
            let message = crate::support::message(&body).unwrap_or_default();
            assert!(
                message.contains(row.key.as_str()),
                "case {case:?}: the refusal must name the key it refused, got {message:?}"
            );
            // A refused create persists NOTHING: the monitor does not exist, so its name
            // is still free for a create that carries an acceptable map.
            let missing = get_monitor(&router, &name).await;
            assert_eq!(
                missing.status,
                404,
                "case {case:?}: a refused create must persist no monitor, got {} {:?}",
                missing.status,
                String::from_utf8_lossy(&missing.body)
            );
            let retry = create_with_tags(&router, &name, tags_body(&[("Valid", "value")])).await;
            assert_eq!(
                retry.status,
                200,
                "case {case:?}: a refused create must leave the name free, got {} {:?}",
                retry.status,
                String::from_utf8_lossy(&retry.body)
            );
            let reported = monitor_tags(&router, &case, &name).await;
            assert!(
                !reported.contains_key(row.key.as_str()),
                "case {case:?}: the refused key must not be stored, got {reported:?}"
            );
        }

        // The SAME key through the other write path of the same map. The codes differ by
        // frontend; the classification must not.
        let tag_case = format!("TagResource with a key: {}", row.case);
        let reply = tag_resource(&router, &target_arn, requested).await;
        if row.admitted {
            assert_eq!(
                reply.status,
                NO_CONTENT,
                "case {tag_case:?}: expected HTTP {NO_CONTENT} for key {:?}, got {} {:?}",
                row.key,
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        } else {
            assert_eq!(
                reply.status,
                400,
                "case {tag_case:?}: expected HTTP 400 for key {:?}, got {} {:?}",
                row.key,
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(&tag_case),
                BAD_REQUEST_CODE,
                "case {tag_case:?}: expected {BAD_REQUEST_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
            let listed = listed_tags(&router, &tag_case, target).await;
            assert!(
                !listed.contains_key(row.key.as_str()),
                "case {tag_case:?}: the refused key must not be stored, got {listed:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_documented_tag_ceiling_bounds_a_create_as_well_as_a_tag_write() {
    let (router, _engine) = emulator();

    // At the bound: a create carrying exactly the documented maximum is accepted and all
    // of its tags are reported by both readers.
    let at_bound: Vec<(String, String)> = (0..MAX_TAGS)
        .map(|index| (format!("key{index:02}"), format!("value-{index}")))
        .collect();
    let body: Value = at_bound
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect::<serde_json::Map<String, Value>>()
        .into();
    let case = format!("a create carrying {MAX_TAGS} tags");
    let reply = create_with_tags(&router, "ck-ceiling-at", body).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected HTTP 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let expected: BTreeMap<String, String> = at_bound.iter().cloned().collect();
    let reported = monitor_tags(&router, &case, "ck-ceiling-at").await;
    assert_eq!(
        reported, expected,
        "case {case:?}: expected all {MAX_TAGS} tags to be reported, got {} of them",
        reported.len()
    );
    let listed = listed_tags(&router, &case, "ck-ceiling-at").await;
    assert_eq!(
        listed, expected,
        "case {case:?}: ListTagsForResource must report the same map, got {} tags",
        listed.len()
    );

    // And the resource is AT the ceiling, so the sibling write path refuses the next key:
    // what a create may seed is exactly what a tag write may leave.
    let over = tag_resource(
        &router,
        &arn_of("ck-ceiling-at"),
        tags_body(&[("one-too-many", "value")]),
    )
    .await;
    assert_eq!(
        over.status,
        400,
        "case {case:?}: expected TagResource to refuse the key past the ceiling, got {} {:?}",
        over.status,
        String::from_utf8_lossy(&over.body)
    );

    // One past the bound, and well past it: a create is refused and persists nothing.
    for count in [MAX_TAGS + 1, MAX_TAGS + 10] {
        let case = format!("a create carrying {count} tags");
        let name = format!("ck-ceiling-{count}");
        let body: Value = (0..count)
            .map(|index| {
                (
                    format!("key{index:03}"),
                    Value::String(format!("value-{index}")),
                )
            })
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let reply = create_with_tags(&router, &name, body).await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(&case),
            VALIDATION_CODE,
            "case {case:?}: expected {VALIDATION_CODE}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let body = reply.json(&case);
        let message = crate::support::message(&body).unwrap_or_default();
        assert!(
            message.contains(&count.to_string()) && message.contains(&MAX_TAGS.to_string()),
            "case {case:?}: the refusal must state the count and the ceiling, got {message:?}"
        );
        assert_eq!(
            get_monitor(&router, &name).await.status,
            404,
            "case {case:?}: a refused create must persist no monitor"
        );
    }
}
