// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for WHEN the tag rules are judged, driven over real HTTP through the
//! same `MultiServiceRouter` the emulator boots.
//!
//! The rule under test: what a request's own tags SAY — the tag character set, the reserved
//! `aws:` key prefix, the per-tag key and value lengths and the number of tags the request
//! itself carries — is decided from the REQUEST ALONE, so every operation that names a tag
//! (`CreateChannel`, `CreateOriginEndpoint`, `TagResource`, `UntagResource`) answers such a
//! request with `UnprocessableEntityException` (HTTP 422) keyed by the member that carried
//! the tag, BEFORE the resource the request names is resolved against the store — and
//! therefore whether that resource exists or not. The only half of the tagging rules a
//! request cannot decide on its own is the ceiling on the set a resource is LEFT holding
//! (`MAX_TAGS`), which is judged against the merged result once the resource resolves.
//! A request whose tags are acceptable is still answered `NotFoundException` (404) keyed by
//! the resource type when the ARN names a resource this service does not hold: the lookup
//! is what decides that, and only that.
//!
//! The rule is not a modelled constraint — `smithy_query(query="operation",
//! operation="TagResource",model_path="services/service-com-amazonaws-mediapackage-mediapackage/smithy-model/com-amazonaws-mediapackage-mediapackage.json")`
//! types `Tags` as a plain `map<string,string>` with no key constraint — it is AWS's
//! universal tagging restriction, which the service enforces itself; WHAT it refuses is
//! covered by `tests/all/reserved_and_malformed_tags.rs`, and this suite covers WHEN.
//!
//! Each suite is a table whose rows differ along what the rule generalises over: which half
//! of the tag is at fault (key or value) and the way it is at fault (the reserved prefix, a
//! character outside the published set, a key or value past its published length, more tags
//! than the published maximum), whether the resource the request names is stored or was
//! never created, which resource type the ARN names, and which operation carries the tag.

use serde_json::{json, Value};
// The published tagging limits and the reserved prefix come from the service's own
// constants rather than being restated here, so the boundary rows cannot drift from the
// bounds the handlers enforce.
use service_com_amazonaws_mediapackage_mediapackage::state::{
    CHANNEL_KIND, MAX_TAGS, MAX_TAG_KEY_CHARS, MAX_TAG_VALUE_CHARS, ORIGIN_ENDPOINT_KIND,
    RESERVED_TAG_KEY_PREFIX,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::{call, create_channel, emulator, error_code};
use crate::resource_tags::{
    create_with_tags, described_tags, list_tags, one_tag, tags_path, untag_query, Kind,
    PARENT_CHANNEL,
};

/// The tag set every stored fixture below carries, and which a refused request must leave
/// exactly as it was.
fn baseline_tags() -> Value {
    json!({ "team": "video", "env": "staging" })
}

/// A tag map of `count` distinct entries, for the rows about how MANY tags a request or a
/// resource carries.
fn many_tags(count: usize) -> Value {
    let mut map = serde_json::Map::new();
    for index in 0..count {
        map.insert(format!("bulk-{index:03}"), json!(format!("value-{index}")));
    }
    Value::Object(map)
}

/// One tag map a caller may not store.
struct RefusedTags {
    /// What is wrong with it, for the assertion messages.
    case: String,
    /// The map as the request carries it.
    tags: Value,
    /// The substrings the refused member's requirement must state, so the caller can fix
    /// the request from the error alone.
    must_name: Vec<String>,
    /// The single key `UntagResource` can name to send the same fault — `None` for a row
    /// whose fault is not something a KEY says (a value's character, a request's size), and
    /// which that operation therefore cannot carry.
    untaggable_key: Option<String>,
}

/// A row whose KEY says something a tag key may not say, which every tag door refuses —
/// including `UntagResource`, which names keys and no values.
fn refused_key(case: &str, key: String, must_name: Vec<String>) -> RefusedTags {
    RefusedTags {
        case: case.to_string(),
        tags: one_tag(&key, "v"),
        must_name: {
            let mut names = vec![format!("{key:?}")];
            names.extend(must_name);
            names
        },
        untaggable_key: Some(key),
    }
}

/// A row no `UntagResource` request can carry: the fault is in a value, or in the size of
/// the map itself.
fn refused_map(case: &str, tags: Value, must_name: Vec<String>) -> RefusedTags {
    RefusedTags {
        case: case.to_string(),
        tags,
        must_name,
        untaggable_key: None,
    }
}

/// The tag maps a caller may not store, spanning every way a request's own tags can be at
/// fault: the reserved prefix, the character set (in a key and in a value, at the start, in
/// the middle and at the end), each published length bound by one character, and one tag
/// past the published maximum count.
fn refused_tag_cases() -> Vec<RefusedTags> {
    let long_key = "k".repeat(MAX_TAG_KEY_CHARS + 1);
    let long_value = "v".repeat(MAX_TAG_VALUE_CHARS + 1);
    vec![
        refused_key(
            "a key AWS itself mints, under the reserved prefix",
            format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name"),
            vec![format!("reserved `{RESERVED_TAG_KEY_PREFIX}` prefix")],
        ),
        refused_key(
            "the reserved prefix with nothing after it",
            RESERVED_TAG_KEY_PREFIX.to_string(),
            vec![format!("reserved `{RESERVED_TAG_KEY_PREFIX}` prefix")],
        ),
        refused_key(
            "a key opening on a currency sign, outside the published set",
            "€uro-cost".to_string(),
            vec![format!("{:?}", '€')],
        ),
        refused_key(
            "a key carrying a hash in the middle",
            "tag#1".to_string(),
            vec![format!("{:?}", '#')],
        ),
        refused_key(
            "a key ending in an exclamation mark",
            "owner!".to_string(),
            vec![format!("{:?}", '!')],
        ),
        refused_map(
            "a value carrying a percent sign",
            one_tag("ratio", "100%"),
            vec!["\"ratio\"".to_string(), format!("{:?}", '%')],
        ),
        refused_map(
            "a value carrying a comma",
            one_tag("owners", "ops,sre"),
            vec!["\"owners\"".to_string(), format!("{:?}", ',')],
        ),
        // How long a key may be is something the KEY says, so `UntagResource` carries this
        // row too: the length bound is part of the one rule every tag door applies to a key.
        refused_key(
            "a key one character past the published maximum",
            long_key,
            vec![format!("at most {MAX_TAG_KEY_CHARS} characters")],
        ),
        refused_map(
            "a value one character past the published maximum",
            one_tag("length", &long_value),
            vec![format!("at most {MAX_TAG_VALUE_CHARS} characters")],
        ),
        refused_map(
            "one tag more than the published maximum, in a single request",
            many_tags(MAX_TAGS + 1),
            vec![format!("at most {MAX_TAGS} entries")],
        ),
    ]
}

/// Assert `(status, body)` is this service's 422 refusal keyed by `member`, stating a
/// requirement that names each of `names`.
fn assert_member_refusal(case: &str, status: u16, body: &Value, member: &str, names: &[String]) {
    assert_eq!(
        status, 422,
        "case {case:?}: expected 422, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "UnprocessableEntityException",
        "case {case:?}: expected UnprocessableEntityException, got {body:?}"
    );
    let reported = match body.pointer(&format!("/errors/{member}")) {
        // A refused MEMBER reports its requirement as one sentence; everything decided
        // against state reports a list of reasons. Either is read as the text that has to
        // name the fault.
        Some(Value::String(sentence)) => sentence.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    for name in names {
        assert!(
            reported.contains(name.as_str()),
            "case {case:?}: expected the {member:?} requirement to name {name:?}, got {body:?}"
        );
    }
}

/// Assert `(status, body)` is this service's 404 for a resource it does not hold, keyed by
/// the resource type and naming the id the ARN spelled.
fn assert_not_found(case: &str, status: u16, body: &Value, kind: &str, id: &str) {
    assert_eq!(
        status, 404,
        "case {case:?}: expected 404, got {status} {body:?}"
    );
    assert_eq!(
        error_code(body),
        "NotFoundException",
        "case {case:?}: expected NotFoundException, got {body:?}"
    );
    assert_eq!(
        body.pointer(&format!("/errors/{kind}")),
        Some(&json!([format!("with id={id} not found")])),
        "case {case:?}: expected errors.{kind} to report the absent id {id:?}, got {body:?}"
    );
}

/// Assert the resource of `kind` with id `id` is not stored, so a refused request against
/// an ARN naming it created nothing.
async fn assert_absent(router: &MultiServiceRouter, case: &str, kind: &Kind, id: &str) {
    let (status, read) = call(router, "GET", &kind.describe_path(id), None).await;
    assert_eq!(
        status, 404,
        "case {case:?}: the refused request must store nothing, got {status} {read:?}"
    );
}

/// Assert the stored resource of `kind` with id `id` still carries `expected`, through both
/// readers that report a tag set.
async fn assert_tags_unchanged(
    router: &MultiServiceRouter,
    case: &str,
    kind: &Kind,
    id: &str,
    expected: &Value,
) {
    let arn = kind.arn(id);
    let (status, listed) = list_tags(router, &arn).await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource expected 200, got {status} {listed:?}"
    );
    assert_eq!(
        &listed, expected,
        "case {case:?}: expected the tag set {expected:?}, got {listed:?}"
    );
    let (_, described) = call(router, "GET", &kind.describe_path(id), None).await;
    assert_eq!(
        &described_tags(&described),
        expected,
        "case {case:?}: the stored record's tags changed, got {described:?}"
    );
}

/// The id of the resource of `kind` that is never created, so its ARN resolves to nothing.
fn absent_id(kind: &Kind) -> String {
    format!("absent-{}-never-created", kind.label().replace(' ', "-"))
}

/// How an error body keys a refusal about a resource of `kind` — the resource type, taken
/// from the service's own constants rather than restated here.
fn kind_key(kind: &Kind) -> &'static str {
    match kind {
        Kind::Channel => CHANNEL_KIND,
        Kind::OriginEndpoint => ORIGIN_ENDPOINT_KIND,
    }
}

#[tokio::test]
async fn tag_resource_refuses_what_the_request_says_whether_or_not_the_resource_resolves() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        let stored = format!("judged-first-{}", kind.label().replace(' ', "-"));
        create_with_tags(&router, &kind, &stored, &baseline_tags()).await;
        let absent = absent_id(&kind);

        for row in refused_tag_cases() {
            // The SAME request against a resource this service holds and against one it
            // does not: the tag rules are decided from the request, so both answers are
            // the one 422 keyed by `tags`.
            for (target, id) in [("the stored", &stored), ("an absent", &absent)] {
                let case = format!(
                    "tagging {target} {} with {}",
                    kind.label(),
                    row.case
                );
                let body = json!({ "tags": row.tags });
                let (status, answered) =
                    call(&router, "POST", &tags_path(&kind.arn(id)), Some(&body)).await;
                assert_member_refusal(&case, status, &answered, "tags", &row.must_name);
            }

            assert_absent(&router, &row.case, &kind, &absent).await;
            assert_tags_unchanged(&router, &row.case, &kind, &stored, &baseline_tags()).await;
        }
    }
}

#[tokio::test]
async fn untag_resource_refuses_what_the_request_says_whether_or_not_the_resource_resolves() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        let stored = format!("untag-judged-first-{}", kind.label().replace(' ', "-"));
        create_with_tags(&router, &kind, &stored, &baseline_tags()).await;
        let absent = absent_id(&kind);

        for row in refused_tag_cases() {
            let Some(key) = row.untaggable_key.as_deref() else {
                continue;
            };
            for (target, id) in [("the stored", &stored), ("an absent", &absent)] {
                let case = format!(
                    "untagging {target} {} by {}",
                    kind.label(),
                    row.case
                );
                let uri = untag_query(&kind.arn(id), &[key]);
                let (status, answered) = call(&router, "DELETE", &uri, None).await;
                assert_member_refusal(&case, status, &answered, "tagKeys", &row.must_name);
            }

            assert_absent(&router, &row.case, &kind, &absent).await;
            assert_tags_unchanged(&router, &row.case, &kind, &stored, &baseline_tags()).await;
        }
    }
}

#[tokio::test]
async fn every_tag_door_answers_one_refusal_for_the_same_tag_content() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for (index, row) in refused_tag_cases().into_iter().enumerate() {
        let Some(key) = row.untaggable_key.clone() else {
            continue;
        };
        let absent_channel = absent_id(&Kind::Channel);
        let absent_endpoint = absent_id(&Kind::OriginEndpoint);

        // Every door that names a tag, each carrying the same offending key: the two
        // creates (which store nothing yet), and the two tagging operations addressed at an
        // ARN this service resolves to nothing.
        let (channel_path, channel_body) =
            Kind::Channel.create_request(&format!("door-channel-{index}"), &row.tags);
        let (endpoint_path, endpoint_body) =
            Kind::OriginEndpoint.create_request(&format!("door-endpoint-{index}"), &row.tags);
        let doors: Vec<(&str, &str, String, Option<Value>, &str)> = vec![
            (
                "CreateChannel",
                "POST",
                channel_path.to_string(),
                Some(channel_body),
                "tags",
            ),
            (
                "CreateOriginEndpoint",
                "POST",
                endpoint_path.to_string(),
                Some(endpoint_body),
                "tags",
            ),
            (
                "TagResource",
                "POST",
                tags_path(&Kind::Channel.arn(&absent_channel)),
                Some(json!({ "tags": row.tags })),
                "tags",
            ),
            (
                "UntagResource",
                "DELETE",
                untag_query(&Kind::OriginEndpoint.arn(&absent_endpoint), &[&key]),
                None,
                "tagKeys",
            ),
        ];

        for (door, method, uri, body, member) in doors {
            let case = format!("{door} carrying {}", row.case);
            let (status, answered) = call(&router, method, &uri, body.as_ref()).await;
            assert_member_refusal(&case, status, &answered, member, &row.must_name);
        }
    }
}

#[tokio::test]
async fn the_lookup_decides_only_what_the_request_cannot() {
    let router = emulator();
    create_channel(&router, PARENT_CHANNEL).await;

    for kind in [Kind::Channel, Kind::OriginEndpoint] {
        let absent = absent_id(&kind);
        let arn = kind.arn(&absent);

        // A request whose tags are acceptable: the absent resource is what fails, so the
        // answer is the 404 keyed by the resource type.
        for (case, tags) in [
            ("one acceptable tag", one_tag("team", "video")),
            ("no tags at all", json!({})),
            ("the published maximum number of tags", many_tags(MAX_TAGS)),
        ] {
            let case = format!("tagging an absent {} with {case}", kind.label());
            let body = json!({ "tags": tags });
            let (status, answered) = call(&router, "POST", &tags_path(&arn), Some(&body)).await;
            assert_not_found(&case, status, &answered, kind_key(&kind), &absent);
        }

        // The ceiling is the one tag rule the request cannot decide alone: it is about the
        // set the resource is LEFT holding, so it is answered only once the resource
        // resolves — the very same one-tag request is a 404 against the ARN above.
        let full = format!("ceiling-{}", kind.label().replace(' ', "-"));
        create_with_tags(&router, &kind, &full, &many_tags(MAX_TAGS)).await;
        let full_arn = kind.arn(&full);

        let case = format!("tagging a full {} with one more distinct tag", kind.label());
        let body = json!({ "tags": one_tag("one-too-many", "v") });
        let (status, answered) = call(&router, "POST", &tags_path(&full_arn), Some(&body)).await;
        assert_member_refusal(
            &case,
            status,
            &answered,
            "tags",
            &[format!("at most {MAX_TAGS} entries")],
        );
        assert_tags_unchanged(&router, &case, &kind, &full, &many_tags(MAX_TAGS)).await;

        // And a request that REPLACES a key the resource already carries leaves the set the
        // same size, so the ceiling does not refuse it.
        let case = format!("retagging a full {}'s own key", kind.label());
        let body = json!({ "tags": one_tag("bulk-000", "replaced") });
        let (status, answered) = call(&router, "POST", &tags_path(&full_arn), Some(&body)).await;
        assert_eq!(
            status, 204,
            "case {case:?}: expected 204, got {status} {answered:?}"
        );
        let mut replaced = many_tags(MAX_TAGS);
        replaced["bulk-000"] = json!("replaced");
        assert_tags_unchanged(&router, &case, &kind, &full, &replaced).await;
    }
}

#[tokio::test]
async fn an_arn_that_names_no_mediapackage_resource_type_is_refused_before_its_tags() {
    let router = emulator();

    // The ARN's FORM is decided from the request too, and it is what names the resource the
    // tag rules would be applied to, so it is reported first: the rows differ along what is
    // wrong with the ARN and along which door carries it.
    let malformed_arns = [
        ("another service's ARN", "arn:aws:s3:::a-bucket".to_string()),
        (
            "a MediaPackage ARN of a resource type this service does not tag",
            "arn:aws:mediapackage:eu-west-1:000000000000:harvest_jobs/j1".to_string(),
        ),
        ("not an ARN at all", "just-an-id".to_string()),
    ];
    let reserved = format!("{RESERVED_TAG_KEY_PREFIX}cloudformation:stack-name");

    for (case, arn) in malformed_arns {
        let case = format!("TagResource on {case}");
        let body = json!({ "tags": one_tag(&reserved, "x") });
        let (status, answered) = call(&router, "POST", &tags_path(&arn), Some(&body)).await;
        assert_member_refusal(
            &case,
            status,
            &answered,
            "resourceArn",
            &["MediaPackage channel or origin endpoint ARN".to_string()],
        );
    }
}

