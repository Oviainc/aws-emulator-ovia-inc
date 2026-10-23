// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for CloudWatch RUM's resource-tagging surface:
//! `TagResource`, `UntagResource` and `ListTagsForResource`.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` with a signed
//! restJson1 request, the `ResourceArn` percent-encoded into the URI the way an SDK
//! encodes a non-greedy `@httpLabel` — so routing, deserialization, the modelled
//! constraints, the handler and the response envelope are all exercised.
//!
//! The assertions pin RULES over a SET of inputs rather than one request:
//!
//! * the tag/read-back table spans what a tag can differ along — the value's script and
//!   byte length (ASCII, non-ASCII, the empty string, the 256-character `TagValue`
//!   maximum), the key's form (every character class the modelled `TagKey` pattern
//!   admits), the number of keys, and whether a key is new or already on the resource —
//!   and reads every row back through BOTH implemented readers (`ListTagsForResource`
//!   and `GetAppMonitor`), plus a second identical write to pin idempotence;
//! * the removal table varies which keys are named relative to what the resource holds
//!   (one, several, all, none of them, a mix), and pins exact-key removal against
//!   neighbours that differ only in case or in a prefix/suffix;
//! * the refusal tables walk a catalog of keys `TagResource` must reject (the modelled
//!   `TagKey` pattern) and a catalog of ARNs naming no app monitor, the latter across all
//!   three tagging operations;
//! * the tag-count table tests the documented ceiling at the bound and one past it.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use smithy_server_runtime::multi_service_router::MultiServiceRouter;

use crate::support::{call, create, emulator, get_app_monitor, Reply, ACCOUNT, REGION};

/// Percent-encode one URI path segment or query value.
///
/// A `ResourceArn` is a non-greedy `@httpLabel`, so an SDK encodes it into ONE path
/// segment — its `:` and `/` included — and a tag key travels the same way in a
/// `@httpQuery` value (so a `+` in a key is not read as a space). Every byte outside the
/// RFC 3986 unreserved set is escaped, which is a superset of what either needs; the
/// sibling tagging suite in this crate encodes the same label the same way, so this
/// suite does it in kind rather than making the crate's manifest — contended by several
/// in-flight fixes — carry a dev-dependency for eight lines.
fn percent_encoded(label: &str) -> String {
    let mut out = String::new();
    for byte in label.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(*byte))
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

/// The wire error code RUM's tagging surface answers an ARN naming no resource with.
const NOT_FOUND_CODE: &str = "NotFoundException";

/// The wire error code RUM's tagging surface answers a refused request with.
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// The most tags one resource holds: `TagResource`'s documentation states "You can
/// associate as many as 50 tags with a resource".
const MAX_TAGS: usize = 50;

/// `TagValue` is `@length(min: 0, max: 256)` with no pattern.
/// Source: `smithy_query(query="shape", shape="TagValue")`.
const MAX_TAG_VALUE_LENGTH: usize = 256;

/// Keys the modelled `TagKey` pattern `^(?!aws:)[a-zA-Z+-=._:/]+$` ADMITS, one per
/// character class it allows: ASCII letters, `_`, and every character from `+` (0x2B) to
/// `=` (0x3D), which is what `+-=` spells inside a character class — so digits, `,`,
/// `-`, `.`, `/`, `:`, `;`, `<` and `=` are all admitted. The last two rows are the
/// boundary of the `(?!aws:)` lookahead: `aws` and `awsome` are NOT the reserved
/// `aws:` prefix.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
const ADMITTED_TAG_KEYS: [&str; 14] = [
    "Team",
    "key.with.dots",
    "key-with-dashes",
    "key_with_underscore",
    "key:with:colons",
    "path/like/key",
    "plus+key",
    "digits0123456789",
    "equals=key",
    "semi;colon",
    "less<than",
    "comma,key",
    "aws",
    "awsome",
];

/// Keys the modelled `TagKey` pattern REJECTS, with the reason each one fails it. The
/// first two fail the `(?!aws:)` lookahead (AWS's reserved prefix); the rest hold a
/// character outside the admitted class — a space, ASCII punctuation the class leaves
/// out, and non-ASCII text in two scripts.
/// Source: `smithy_query(query="shape", shape="TagKey")`.
const REJECTED_TAG_KEYS: [&str; 11] = [
    "aws:owner",
    "aws:",
    "key with space",
    "key!bang",
    "key#hash",
    "key@at",
    "key(paren)",
    "key%percent",
    "key[bracket]",
    "clé",
    "キー",
];

/// The ARN of the app monitor named `name`, as `crate::state::app_monitor_arn` derives
/// it: `arn:aws:rum:<region>:<account>:appmonitor/<name>`.
fn arn_of(name: &str) -> String {
    format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/{name}")
}

/// The URI the tagging operations are bound to: `/tags/{ResourceArn}`.
fn tags_uri(resource_arn: &str) -> String {
    format!("/tags/{}", percent_encoded(resource_arn))
}

/// Create one app monitor carrying `tags`, and return its ARN.
async fn create_monitor(router: &MultiServiceRouter, case: &str, name: &str, tags: Value) -> String {
    create(
        router,
        case,
        json!({"Name": name, "Domain": "example.com", "Tags": tags}),
    )
    .await;
    arn_of(name)
}

/// `TagResource`: merge `tags` onto the resource `resource_arn` names.
async fn tag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    tags: &BTreeMap<String, String>,
) -> Reply {
    let body: serde_json::Map<String, Value> = tags
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    call(
        router,
        "POST",
        &tags_uri(resource_arn),
        Some(json!({"Tags": Value::Object(body)})),
    )
    .await
}

/// `UntagResource`: remove `keys` from the resource `resource_arn` names.
async fn untag_resource(
    router: &MultiServiceRouter,
    resource_arn: &str,
    keys: &[&str],
) -> Reply {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", percent_encoded(key)))
        .collect::<Vec<_>>()
        .join("&");
    call(
        router,
        "DELETE",
        &format!("{}?{query}", tags_uri(resource_arn)),
        None,
    )
    .await
}

/// `ListTagsForResource`'s answer for `resource_arn`.
async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Reply {
    call(router, "GET", &tags_uri(resource_arn), None).await
}

/// The tag map `ListTagsForResource` reports for `resource_arn`, plus the `ResourceArn`
/// it names.
async fn listed_tags(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let reply = list_tags(router, resource_arn).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: expected ListTagsForResource to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let body = reply.json(case);
    assert_eq!(
        body["ResourceArn"].as_str(),
        Some(resource_arn),
        "case {case:?}: ListTagsForResource must name the resource it was asked about, got {:?}",
        body["ResourceArn"]
    );
    as_map(case, &body["Tags"])
}

/// The tag map `GetAppMonitor` reports for the monitor named `name` — the SECOND reader
/// of the one stored map, so a write is read back through both.
async fn monitor_tags(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
) -> BTreeMap<String, String> {
    let monitor = get_app_monitor(router, case, name).await;
    as_map(case, &monitor["Tags"])
}

/// A JSON tag map as a sorted Rust map. An omitted member reads as the empty map it
/// stands for: restJson1 draws no distinction between an absent map and an empty one.
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

/// `(key, value)` pairs as a sorted map.
fn map_of(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect()
}

/// One row of the tag/read-back table: what the monitor was created with, what one
/// `TagResource` call applies, and the map the resource must hold afterwards.
struct TagCase {
    case: &'static str,
    name: &'static str,
    created_with: BTreeMap<String, String>,
    applied: BTreeMap<String, String>,
}

impl TagCase {
    /// The map the resource holds once `applied` is merged onto `created_with`: a new
    /// key is appended, an existing key's value replaced — which is exactly what
    /// `TagResource` documents.
    fn expected(&self) -> BTreeMap<String, String> {
        let mut merged = self.created_with.clone();
        for (key, value) in &self.applied {
            merged.insert(key.clone(), value.clone());
        }
        merged
    }
}

/// The tag/read-back table. Each row differs from the others along what the rule
/// generalises over: the value's script and length, the key's character classes, how
/// many keys one call carries, and whether a key is new to the resource or already on
/// it.
fn tag_cases() -> Vec<TagCase> {
    let unicode_value = "équipe-naïve-日本語-🙂-Ωμέγα";
    let max_value = "v".repeat(MAX_TAG_VALUE_LENGTH);
    let many: BTreeMap<String, String> = (0..30)
        .map(|index| (format!("bulk{index:02}"), format!("value-{index}")))
        .collect();
    vec![
        TagCase {
            case: "one ASCII tag onto an untagged monitor",
            name: "tag-ascii",
            created_with: BTreeMap::new(),
            applied: map_of(&[("Team", "web")]),
        },
        TagCase {
            case: "a value in several non-ASCII scripts is preserved verbatim",
            name: "tag-unicode-value",
            created_with: BTreeMap::new(),
            applied: map_of(&[("Owner", unicode_value)]),
        },
        TagCase {
            case: "the empty string is a value, not an absent tag",
            name: "tag-empty-value",
            created_with: map_of(&[("Keep", "kept")]),
            applied: map_of(&[("Blank", ""), ("AlsoBlank", "")]),
        },
        TagCase {
            case: "a value at TagValue's 256-character maximum",
            name: "tag-max-value",
            created_with: BTreeMap::new(),
            applied: map_of(&[("Long", max_value.as_str())]),
        },
        TagCase {
            case: "a new key is appended and an existing key's value replaced",
            name: "tag-merge",
            created_with: map_of(&[("Env", "dev"), ("Keep", "yes")]),
            applied: map_of(&[("Env", "prod"), ("New", "added")]),
        },
        TagCase {
            case: "every character class TagKey admits",
            name: "tag-key-catalog",
            created_with: BTreeMap::new(),
            applied: ADMITTED_TAG_KEYS
                .iter()
                .enumerate()
                .map(|(index, key)| (key.to_string(), format!("value-{index}")))
                .collect(),
        },
        TagCase {
            case: "many keys in one call, onto a monitor that already has tags",
            name: "tag-many-keys",
            created_with: map_of(&[("Env", "dev")]),
            applied: many,
        },
    ]
}

#[tokio::test]
async fn tagging_a_resource_leaves_exactly_the_merged_map_for_both_readers_to_report() {
    let (router, _engine) = emulator();
    for case in tag_cases() {
        let created: Value = case
            .created_with
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let arn = create_monitor(&router, case.case, case.name, created).await;

        let reply = tag_resource(&router, &arn, &case.applied).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected TagResource to answer 200, got {} {:?}",
            case.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );

        let expected = case.expected();
        let listed = listed_tags(&router, case.case, &arn).await;
        assert_eq!(
            listed, expected,
            "case {:?}: ListTagsForResource must report the merged map {expected:?}, got {listed:?}",
            case.case
        );
        let reported = monitor_tags(&router, case.case, case.name).await;
        assert_eq!(
            reported, expected,
            "case {:?}: GetAppMonitor must report the same map {expected:?}, got {reported:?}",
            case.case
        );

        // Idempotence: the same write again leaves the same map.
        let again = tag_resource(&router, &arn, &case.applied).await;
        assert_eq!(
            again.status,
            200,
            "case {:?}: expected the repeated TagResource to answer 200, got {}",
            case.case,
            again.status
        );
        let listed_again = listed_tags(&router, case.case, &arn).await;
        assert_eq!(
            listed_again, expected,
            "case {:?}: repeating the same TagResource must leave {expected:?}, got {listed_again:?}",
            case.case
        );
    }
}

/// One row of the removal table: the tags the resource starts with, the keys
/// `UntagResource` names, and what must be left.
struct UntagCase {
    case: &'static str,
    name: &'static str,
    start: BTreeMap<String, String>,
    removed: Vec<&'static str>,
    expected: BTreeMap<String, String>,
}

/// The removal table. Rows differ along how the named keys relate to what the resource
/// holds — one of them, several, all of them, none of them, a mix — and on the
/// neighbours a removal must NOT touch (a key differing only in case, or in a prefix or
/// suffix).
fn untag_cases() -> Vec<UntagCase> {
    vec![
        UntagCase {
            case: "one present key",
            name: "untag-one",
            start: map_of(&[("Env", "dev"), ("Team", "web")]),
            removed: vec!["Env"],
            expected: map_of(&[("Team", "web")]),
        },
        UntagCase {
            case: "several present keys",
            name: "untag-several",
            start: map_of(&[("A", "1"), ("B", "2"), ("C", "3")]),
            removed: vec!["A", "C"],
            expected: map_of(&[("B", "2")]),
        },
        UntagCase {
            case: "every key the resource holds",
            name: "untag-all",
            start: map_of(&[("A", "1"), ("B", "2")]),
            removed: vec!["A", "B"],
            expected: BTreeMap::new(),
        },
        UntagCase {
            case: "a key the resource does not hold is a no-op, not an error",
            name: "untag-absent",
            start: map_of(&[("Env", "dev")]),
            removed: vec!["never-set"],
            expected: map_of(&[("Env", "dev")]),
        },
        UntagCase {
            case: "a mix of present and absent keys",
            name: "untag-mixed",
            start: map_of(&[("Env", "dev"), ("Team", "web")]),
            removed: vec!["Env", "never-set"],
            expected: map_of(&[("Team", "web")]),
        },
        UntagCase {
            case: "removal is by exact key: case and affix variants stay",
            name: "untag-exact",
            start: map_of(&[
                ("Env", "dev"),
                ("env", "lower"),
                ("ENV", "upper"),
                ("Env2", "suffixed"),
                ("preEnv", "prefixed"),
            ]),
            removed: vec!["Env"],
            expected: map_of(&[
                ("env", "lower"),
                ("ENV", "upper"),
                ("Env2", "suffixed"),
                ("preEnv", "prefixed"),
            ]),
        },
        UntagCase {
            case: "removing the same key twice is still a no-op",
            name: "untag-repeated",
            start: map_of(&[("Env", "dev"), ("Team", "web")]),
            removed: vec!["Env", "Env"],
            expected: map_of(&[("Team", "web")]),
        },
    ]
}

#[tokio::test]
async fn untagging_removes_exactly_the_named_keys_and_leaves_every_other_tag() {
    let (router, _engine) = emulator();
    for case in untag_cases() {
        let created: Value = case
            .start
            .iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect::<serde_json::Map<String, Value>>()
            .into();
        let arn = create_monitor(&router, case.case, case.name, created).await;

        let reply = untag_resource(&router, &arn, &case.removed).await;
        assert_eq!(
            reply.status,
            200,
            "case {:?}: expected UntagResource to answer 200, got {} {:?}",
            case.case,
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );

        let listed = listed_tags(&router, case.case, &arn).await;
        assert_eq!(
            listed, case.expected,
            "case {:?}: expected {:?} to be left, got {listed:?}",
            case.case, case.expected
        );
        let reported = monitor_tags(&router, case.case, case.name).await;
        assert_eq!(
            reported, case.expected,
            "case {:?}: GetAppMonitor must report the same map {:?}, got {reported:?}",
            case.case, case.expected
        );
    }
}

#[tokio::test]
async fn a_tag_key_outside_the_modelled_pattern_is_refused_and_writes_nothing() {
    let (router, _engine) = emulator();
    let start = map_of(&[("Env", "dev")]);
    let created: Value = json!({"Env": "dev"});
    let arn = create_monitor(&router, "invalid keys", "tag-invalid-keys", created).await;

    for key in REJECTED_TAG_KEYS {
        // The refused key travels beside a key that WOULD be accepted, so the refusal
        // cannot be read as "the request held nothing valid".
        let requested = map_of(&[(key, "value"), ("Valid", "value")]);
        let reply = tag_resource(&router, &arn, &requested).await;
        let case = format!("TagResource key={key:?}");
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: expected HTTP 400, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert_eq!(
            reply.error_code(&case),
            BAD_REQUEST_CODE,
            "case {case:?}: expected {BAD_REQUEST_CODE}, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
        let listed = listed_tags(&router, &case, &arn).await;
        assert_eq!(
            listed, start,
            "case {case:?}: a refused TagResource must write nothing, got {listed:?}"
        );
    }
}

#[tokio::test]
async fn an_arn_naming_no_app_monitor_is_refused_as_not_found_by_every_tagging_operation() {
    let (router, _engine) = emulator();
    // A monitor that exists, so the refusals below are about the ARN and nothing else.
    let live = create_monitor(&router, "live", "tagging-live", json!({})).await;
    assert_eq!(
        listed_tags(&router, "live", &live).await,
        BTreeMap::new(),
        "a monitor created with no tags reports an empty map"
    );

    // A monitor that was created and then deleted: its ARN named a resource once.
    let deleted = create_monitor(&router, "deleted", "tagging-deleted", json!({"a": "b"})).await;
    let gone = call(&router, "DELETE", "/appmonitor/tagging-deleted", None).await;
    assert_eq!(
        gone.status, 200,
        "expected DeleteAppMonitor to answer 200, got {}",
        gone.status
    );

    // Each row is an ARN that names no app monitor this emulator holds, for a different
    // reason: never created, deleted, another service's namespace, another resource
    // type in this one, and an empty resource name.
    let unknown = [
        ("never created", arn_of("tagging-never-created")),
        ("deleted", deleted),
        (
            "another service's namespace",
            format!("arn:aws:s3:{REGION}:{ACCOUNT}:appmonitor/tagging-live"),
        ),
        (
            "another resource type",
            format!("arn:aws:rum:{REGION}:{ACCOUNT}:dashboard/tagging-live"),
        ),
        (
            "no resource name",
            format!("arn:aws:rum:{REGION}:{ACCOUNT}:appmonitor/"),
        ),
    ];

    for (reason, resource_arn) in unknown {
        for operation in ["TagResource", "UntagResource", "ListTagsForResource"] {
            let case = format!("{operation} on an ARN: {reason}");
            let reply = match operation {
                "TagResource" => {
                    tag_resource(&router, &resource_arn, &map_of(&[("Team", "web")])).await
                }
                "UntagResource" => untag_resource(&router, &resource_arn, &["Team"]).await,
                _ => list_tags(&router, &resource_arn).await,
            };
            assert_eq!(
                reply.status,
                404,
                "case {case:?}: expected HTTP 404, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(&case),
                NOT_FOUND_CODE,
                "case {case:?}: expected {NOT_FOUND_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
        }
    }

    // The other side of the same rule: a `ResourceArn` names a resource by its WHOLE
    // ARN, so an ARN differing from the stored one in any component — the partition, the
    // region, the account — names a resource this emulator does not hold and is refused
    // exactly like an unknown name.
    let aliases = [
        (
            "another partition",
            format!("arn:aws-cn:rum:{REGION}:{ACCOUNT}:appmonitor/tagging-live"),
        ),
        (
            "another region",
            format!("arn:aws:rum:eu-west-1:{ACCOUNT}:appmonitor/tagging-live"),
        ),
        (
            "another account",
            "arn:aws:rum:us-east-1:123456789012:appmonitor/tagging-live".to_string(),
        ),
    ];
    for (reason, alias) in aliases {
        for operation in ["TagResource", "UntagResource", "ListTagsForResource"] {
            let case = format!("{operation} on an ARN naming the monitor through {reason}");
            let reply = match operation {
                "TagResource" => tag_resource(&router, &alias, &map_of(&[("Team", "web")])).await,
                "UntagResource" => untag_resource(&router, &alias, &["Team"]).await,
                _ => list_tags(&router, &alias).await,
            };
            assert_eq!(
                reply.status,
                404,
                "case {case:?}: expected HTTP 404, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            assert_eq!(
                reply.error_code(&case),
                NOT_FOUND_CODE,
                "case {case:?}: expected {NOT_FOUND_CODE}, got {:?}",
                String::from_utf8_lossy(&reply.body)
            );
        }
    }

    // And the live monitor is untouched by every one of those refusals.
    assert_eq!(
        listed_tags(&router, "live", &live).await,
        BTreeMap::new(),
        "a refused tagging call must not touch another resource"
    );
}

#[tokio::test]
async fn the_documented_fifty_tag_ceiling_is_enforced_at_the_bound_and_one_past_it() {
    let (router, _engine) = emulator();
    let arn = create_monitor(&router, "ceiling", "tag-ceiling", json!({})).await;

    // At the bound: exactly 50 tags are accepted and all 50 are reported.
    let at_bound: BTreeMap<String, String> = (0..MAX_TAGS)
        .map(|index| (format!("key{index:02}"), format!("value-{index}")))
        .collect();
    let reply = tag_resource(&router, &arn, &at_bound).await;
    assert_eq!(
        reply.status,
        200,
        "case \"{MAX_TAGS} tags\": expected TagResource to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let listed = listed_tags(&router, "at the bound", &arn).await;
    assert_eq!(
        listed, at_bound,
        "case \"{MAX_TAGS} tags\": expected all {MAX_TAGS} tags to be reported, got {} of them",
        listed.len()
    );

    // Replacing a value on a resource already at the bound adds no entry, so it stays
    // inside the ceiling.
    let replacement = map_of(&[("key00", "replaced")]);
    let reply = tag_resource(&router, &arn, &replacement).await;
    assert_eq!(
        reply.status,
        200,
        "case \"replace at the bound\": expected TagResource to answer 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let listed = listed_tags(&router, "replace at the bound", &arn).await;
    assert_eq!(
        listed.get("key00").map(String::as_str),
        Some("replaced"),
        "case \"replace at the bound\": expected the value to be replaced, got {:?}",
        listed.get("key00")
    );
    assert_eq!(
        listed.len(),
        MAX_TAGS,
        "case \"replace at the bound\": a replacement adds no entry, got {} tags",
        listed.len()
    );

    // One past it: the 51st key is refused and the resource keeps its 50 tags.
    let reply = tag_resource(&router, &arn, &map_of(&[("one-too-many", "value")])).await;
    assert_eq!(
        reply.status,
        400,
        "case \"{} tags\": expected HTTP 400, got {} {:?}",
        MAX_TAGS + 1,
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    let case = format!("{} tags", MAX_TAGS + 1);
    assert_eq!(
        reply.error_code(&case),
        BAD_REQUEST_CODE,
        "case {case:?}: expected {BAD_REQUEST_CODE}, got {:?}",
        String::from_utf8_lossy(&reply.body)
    );
    let listed = listed_tags(&router, &case, &arn).await;
    assert_eq!(
        listed.len(),
        MAX_TAGS,
        "case {case:?}: a refused TagResource must write nothing, got {} tags",
        listed.len()
    );
    assert!(
        !listed.contains_key("one-too-many"),
        "case {case:?}: the refused key must not be stored, got {listed:?}"
    );
}
