// SPDX-License-Identifier: CC0-1.0

//! AppConfig's tagging slice, end to end over HTTP: `TagResource`, `ListTagsForResource`
//! and `UntagResource`.
//!
//! Every case drives the real request path (routing, restJson1 deserialization, constraint
//! validation, handler, serialization) through the same router the emulator's front door
//! uses, so what these tests assert is what an AppConfig client sees on the wire. The
//! rules under test:
//!
//! * a tag operation addresses a resource by its ARN and acts on the tag map that
//!   resource already carries — `TagResource` MERGES the pairs it names into it (a named
//!   key is set or overwritten, a key it does not name keeps the value it had),
//!   `ListTagsForResource` reports that same stored map and changes nothing a later caller
//!   sees, and `UntagResource` removes exactly the keys it names, removing a key the
//!   resource does not carry being a no-op success;
//! * every resource kind whose tags AppConfig persists answers the same way — an
//!   application, an environment, a configuration profile and a deployment — and a write
//!   addresses ONE of them;
//! * a `ResourceArn` that addresses no resource this service holds is HTTP 404
//!   `ResourceNotFoundException` on all three, never a 501 and never a success;
//! * a tag pair outside the bounds the model's own `Tags` documentation states, and a
//!   write that would push a resource past the tags-per-resource maximum, are refused with
//!   HTTP 400 `BadRequestException` and store nothing — on `TagResource` and on the `Tags`
//!   member of every create that takes one.
//!
//! The tables span the class each rule covers rather than one tag set: the pair count
//! (none, one, several, the per-resource maximum and one past it), keys that are new and
//! keys that overwrite, empty, non-ASCII and URI-significant values, and the documented
//! key/value length bounds with one character past each.

use std::collections::BTreeMap;

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_appconfig_amazonappconfig::state as appconfig;

use crate::appconfig_workflow::{
    create_application, create_environment, create_profile, json_call, router, send, Answer,
};

/// The Region the harness signs for, and the one every ARN below is minted in.
const REGION: &str = "us-east-1";

/// Percent-encode one path segment or query value: everything outside RFC 3986 §2.3's
/// unreserved set is escaped, which is what an SDK client sends for an ARN bound to a
/// NON-greedy `@httpLabel` (`/tags/{ResourceArn}`) and for each `tagKeys` query pair.
/// `pub(crate)` because the other suites in this target address resources by ARN too.
pub(crate) fn pct(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                char::from(byte).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// A deterministic string of `length` ASCII letters, for exercising a documented length
/// bound rather than one hand-picked spelling. `pub(crate)` because the other suites in
/// this target exercise the same bounds.
pub(crate) fn filler(length: usize, seed: u8) -> String {
    (0..length)
        .map(|index| char::from(b'a' + ((index as u8).wrapping_add(seed) % 26)))
        .collect()
}

/// A JSON object from `(key, value)` pairs, the wire form of `TagMap`.
fn tag_map<'pair>(pairs: impl IntoIterator<Item = (&'pair str, &'pair str)>) -> Value {
    Value::Object(
        pairs
            .into_iter()
            .map(|(key, value)| (key.to_string(), Value::from(value)))
            .collect::<Map<String, Value>>(),
    )
}

/// `POST /tags/{ResourceArn}` with the pairs in `tags`.
pub(crate) async fn tag(router: &MultiServiceRouter, resource_arn: &str, tags: &Value) -> Answer {
    json_call(
        router,
        "POST",
        &format!("/tags/{}", pct(resource_arn)),
        &json!({ "Tags": tags }),
    )
    .await
}

/// `GET /tags/{ResourceArn}`.
pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> Answer {
    json_call(
        router,
        "GET",
        &format!("/tags/{}", pct(resource_arn)),
        &Value::Null,
    )
    .await
}

/// `DELETE /tags/{ResourceArn}?tagKeys=..&tagKeys=..` — `TagKeys` is bound to
/// `@httpQuery("tagKeys")`, so each key travels as its own query pair.
async fn untag(router: &MultiServiceRouter, resource_arn: &str, keys: &[&str]) -> Answer {
    let query = keys
        .iter()
        .map(|key| format!("tagKeys={}", pct(key)))
        .collect::<Vec<_>>()
        .join("&");
    let path = format!("/tags/{}", pct(resource_arn));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    send(router, "DELETE", &uri, "application/json", Vec::new(), &[]).await
}

/// The tags `ListTagsForResource` reports for `resource_arn`, asserted to be a 200.
async fn reported_tags(
    router: &MultiServiceRouter,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let answer = list_tags(router, resource_arn).await;
    assert_eq!(
        answer.status,
        200,
        "case {case:?}: ListTagsForResource expected 200, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    answer
        .json()
        .get("Tags")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// One ARN this emulator mints for an AppConfig resource: the resource portion is the
/// `/`-separated type/id path rooted at the owning application (or at the resource type,
/// for a resource that has no owning application), which is what
/// `crate::state::parse_resource_arn` reads.
pub(crate) fn arn(resource: &str) -> String {
    engine::emulator_arn("appconfig", REGION, resource)
}

/// One of each resource kind whose tags AppConfig persists, created through its own
/// operation and addressed by its ARN. The returned pairs are `(kind, ARN)`.
///
/// A deployment needs the whole tree below it — an application, an environment, a
/// configuration profile and a hosted configuration version to roll out — so one fixture
/// builds all four kinds rather than four.
async fn taggable_resources(router: &MultiServiceRouter) -> Vec<(&'static str, String)> {
    let application = create_application(router, "tagged-app").await;
    let environment = create_environment(router, &application, "tagged-env").await;
    let profile = create_profile(router, &application, "tagged-profile").await;

    let version = send(
        router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        version.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        version.status,
        String::from_utf8_lossy(&version.body)
    );

    let started = json_call(
        router,
        "POST",
        &format!("/applications/{application}/environments/{environment}/deployments"),
        &json!({
            "DeploymentStrategyId": "AppConfig.AllAtOnce",
            "ConfigurationProfileId": profile,
            "ConfigurationVersion": "1",
        }),
    )
    .await;
    assert_eq!(
        started.status,
        201,
        "the deployment to tag must start, got {} / {}",
        started.status,
        String::from_utf8_lossy(&started.body)
    );

    vec![
        ("application", arn(&format!("application/{application}"))),
        (
            "environment",
            arn(&format!("application/{application}/environment/{environment}")),
        ),
        (
            "configurationprofile",
            arn(&format!(
                "application/{application}/configurationprofile/{profile}"
            )),
        ),
        (
            "deployment",
            arn(&format!(
                "application/{application}/environment/{environment}/deployment/1"
            )),
        ),
    ]
}

/// One row of the merge/report/remove table: the pairs to write, then the keys to remove,
/// applied in that order to the resource under test.
struct Step {
    case: &'static str,
    add: Vec<(&'static str, &'static str)>,
    remove: Vec<&'static str>,
}

impl Step {
    fn write(case: &'static str, add: Vec<(&'static str, &'static str)>) -> Self {
        Self {
            case,
            add,
            remove: Vec::new(),
        }
    }

    fn erase(case: &'static str, remove: Vec<&'static str>) -> Self {
        Self {
            case,
            add: Vec::new(),
            remove,
        }
    }
}

/// The table every resource kind is walked through.
///
/// Rows differ along what the rule generalises over: the number of pairs written (none,
/// one, several), whether a key is new or overwrites one already carried, the form of the
/// value (empty, non-ASCII, URI-significant, numeric), and on the removal side the number
/// of keys named and whether the resource carries them at all.
fn steps() -> Vec<Step> {
    vec![
        Step::write("no pairs at all", Vec::new()),
        Step::write("one new pair", vec![("Env", "prod")]),
        Step::write(
            "several new pairs across the value forms",
            vec![
                ("Team", "platform"),
                ("Empty", ""),
                ("Unicode", "bâtiment-日本語"),
                ("Punctuated", "a/b:c=d+e_f.g"),
                ("Digits", "012345"),
            ],
        ),
        Step::write(
            "one key overwritten and one added",
            vec![("Env", "staging"), ("Owner", "ops")],
        ),
        Step::erase("one key the resource carries", vec!["Env"]),
        Step::erase("the same key again — removal is idempotent", vec!["Env"]),
        Step::erase("a key the resource never carried", vec!["never-set"]),
        Step::erase(
            "several keys at once, one of them absent",
            vec!["Team", "Digits", "also-absent"],
        ),
        Step::erase(
            "every remaining key",
            vec!["Empty", "Unicode", "Punctuated", "Owner"],
        ),
    ]
}

/// `TagResource` merges, `ListTagsForResource` reports and `UntagResource` removes — on
/// every resource kind whose tags AppConfig persists, and on exactly the resource the ARN
/// addresses.
///
/// The expected map is maintained by the RULE itself (insert the named keys, remove the
/// named keys, touch nothing else), so each row asserts the rule rather than a recorded
/// answer.
#[tokio::test]
async fn tagging_merges_reports_and_removes_on_every_taggable_resource_kind() {
    let router = router();
    let resources = taggable_resources(&router).await;

    let mut expected: BTreeMap<&str, BTreeMap<String, String>> = resources
        .iter()
        .map(|(kind, _)| (*kind, BTreeMap::new()))
        .collect();

    // A resource created without tags carries none, and that is what a read reports.
    for (kind, resource_arn) in &resources {
        let reported = reported_tags(&router, kind, resource_arn).await;
        assert!(
            reported.is_empty(),
            "case {kind:?}: a resource created with no Tags must report none, got {reported:?}"
        );
    }

    for (kind, resource_arn) in &resources {
        for step in steps() {
            let case = format!("{kind} / {}", step.case);
            let carried = expected
                .get_mut(kind)
                .expect("every kind starts with an expected map");

            if step.remove.is_empty() {
                // A write row, including the row that names no pair at all: `Tags` is
                // bounded at 0..50 entries, so an empty map is a request this operation
                // has, and it merges nothing.
                let answer = tag(&router, resource_arn, &tag_map(step.add.iter().copied())).await;
                assert_eq!(
                    answer.status,
                    204,
                    "case {case:?}: TagResource expected 204, got {} / {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                assert!(
                    answer.body.is_empty(),
                    "case {case:?}: a 204 carries no body, got {}",
                    String::from_utf8_lossy(&answer.body)
                );
                for (key, value) in &step.add {
                    carried.insert(key.to_string(), value.to_string());
                }
            } else {
                // A removal row. `TagKeys` is a REQUIRED `@httpQuery` list, so every
                // removal row names at least one key.
                let answer = untag(&router, resource_arn, &step.remove).await;
                assert_eq!(
                    answer.status,
                    204,
                    "case {case:?}: UntagResource expected 204, got {} / {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                for key in &step.remove {
                    carried.remove(*key);
                }
            }

            // Every resource is re-read after every write: the addressed one reports the
            // merged map, and no other kind's tags moved.
            for (other_kind, other_arn) in &resources {
                let reported = reported_tags(&router, &case, other_arn).await;
                assert_eq!(
                    &reported,
                    expected
                        .get(other_kind)
                        .expect("every kind starts with an expected map"),
                    "case {case:?}: {other_kind} must report {:?}, got {reported:?}",
                    expected.get(other_kind)
                );
            }
        }
    }
}

/// A read never changes what a later caller sees: `ListTagsForResource` called repeatedly
/// reports the same map, and the resource it addresses is still taggable afterwards.
#[tokio::test]
async fn listing_tags_reports_the_stored_map_without_changing_it() {
    let router = router();
    let resources = taggable_resources(&router).await;

    for (kind, resource_arn) in &resources {
        let written = tag_map([("Env", "prod"), ("Team", "platform")]);
        let answer = tag(&router, resource_arn, &written).await;
        assert_eq!(
            answer.status,
            204,
            "case {kind:?}: TagResource expected 204, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        let first = reported_tags(&router, kind, resource_arn).await;
        for round in 0..3 {
            let again = reported_tags(&router, kind, resource_arn).await;
            assert_eq!(
                again, first,
                "case {kind:?}: read {round} changed the reported tags: {first:?} then {again:?}"
            );
        }
        assert_eq!(
            first,
            BTreeMap::from([
                ("Env".to_string(), "prod".to_string()),
                ("Team".to_string(), "platform".to_string()),
            ]),
            "case {kind:?}: expected the two pairs written, got {first:?}"
        );
    }
}

/// A resource keeps at most [`appconfig::MAX_TAGS_PER_RESOURCE`] tags however many calls
/// fill it: the write that would take it past the maximum is refused with 400
/// `BadRequestException` and stores nothing, while a write that only re-states keys the
/// resource already carries does not grow it and is accepted.
#[tokio::test]
async fn a_write_past_the_tags_per_resource_maximum_is_refused_and_stores_nothing() {
    let router = router();
    let resources = taggable_resources(&router).await;
    let maximum = appconfig::MAX_TAGS_PER_RESOURCE;

    for (kind, resource_arn) in &resources {
        // Filled over TWO calls, so what the bound applies to is the merged map rather
        // than one request's `TagMap` (which the model itself bounds at 50 entries).
        let first_half: Vec<(String, String)> = (0..maximum / 2)
            .map(|index| (format!("key{index}"), format!("value{index}")))
            .collect();
        let second_half: Vec<(String, String)> = (maximum / 2..maximum)
            .map(|index| (format!("key{index}"), format!("value{index}")))
            .collect();

        for (half, pairs) in [("first half", &first_half), ("second half", &second_half)] {
            let answer = tag(
                &router,
                resource_arn,
                &tag_map(
                    pairs
                        .iter()
                        .map(|(key, value)| (key.as_str(), value.as_str())),
                ),
            )
            .await;
            assert_eq!(
                answer.status,
                204,
                "case {kind:?} / {half}: TagResource expected 204, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
        }

        let full = reported_tags(&router, kind, resource_arn).await;
        assert_eq!(
            full.len(),
            maximum,
            "case {kind:?}: expected {maximum} tags stored, got {}",
            full.len()
        );

        // One key past the maximum is refused, and the stored map is untouched.
        let refused = tag(&router, resource_arn, &tag_map([("one-too-many", "x")])).await;
        assert_eq!(
            refused.status,
            400,
            "case {kind:?}: a 51st tag must be refused with 400, got {} / {}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert!(
            refused.error_code().contains("BadRequestException"),
            "case {kind:?}: expected BadRequestException, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            reported_tags(&router, kind, resource_arn).await,
            full,
            "case {kind:?}: a refused write must store nothing"
        );

        // Re-stating keys the resource already carries does not grow it.
        let overwrite = tag(&router, resource_arn, &tag_map([("key0", "rewritten")])).await;
        assert_eq!(
            overwrite.status,
            204,
            "case {kind:?}: overwriting a carried key at the maximum expected 204, got {} / {}",
            overwrite.status,
            String::from_utf8_lossy(&overwrite.body)
        );
        let after = reported_tags(&router, kind, resource_arn).await;
        assert_eq!(
            after.len(),
            maximum,
            "case {kind:?}: an overwrite must not grow the map, got {} entries",
            after.len()
        );
        assert_eq!(
            after.get("key0").map(String::as_str),
            Some("rewritten"),
            "case {kind:?}: expected the overwritten value, got {:?}",
            after.get("key0")
        );
    }
}

/// What the wire must answer for one tag pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expected {
    /// The pair is stored and read back.
    Stored,
    /// The request is refused with HTTP 400 and this error code.
    Refused(&'static str),
}

/// Rows at and past the bounds a tag key and value must satisfy.
///
/// `TagKey` is `length 1..128` and `TagValue` is `length 0..256` — the same numbers the
/// `Tags` member documents ("The tag key can be up to 128 characters and must not start
/// with `aws:`. The tag value can be up to 256 characters."). A map VALUE's constraints
/// are part of the schema walk, so a value past its bound is refused before any handler
/// runs; a map KEY's are not, so the key rules and the reserved prefix (which the model
/// carries no trait for) are refused by the handler. Either way the code is the modelled
/// `BadRequestException` — this service's model declares no `ValidationException` at all.
///
/// Each bound is tested AT the published value and one character past it, so the row that
/// is accepted and the row that is refused differ by that one character alone.
fn tag_pair_rows() -> Vec<(String, String, String, Expected)> {
    let key_bound = appconfig::MAX_TAG_KEY_LENGTH;
    let value_bound = appconfig::MAX_TAG_VALUE_LENGTH;
    vec![
        (
            format!("a {key_bound}-character key, the published maximum"),
            filler(key_bound, 1),
            "at-the-bound".to_string(),
            Expected::Stored,
        ),
        (
            format!("a {}-character key, one past the maximum", key_bound + 1),
            filler(key_bound + 1, 2),
            "past-the-bound".to_string(),
            Expected::Refused("BadRequestException"),
        ),
        (
            format!("a {value_bound}-character value, the published maximum"),
            "value-at-the-bound".to_string(),
            filler(value_bound, 3),
            Expected::Stored,
        ),
        (
            format!("a {}-character value, one past the maximum", value_bound + 1),
            "value-past-the-bound".to_string(),
            filler(value_bound + 1, 4),
            Expected::Refused("BadRequestException"),
        ),
        (
            "an empty key, which names no tag".to_string(),
            String::new(),
            "orphan".to_string(),
            Expected::Refused("BadRequestException"),
        ),
        (
            "a key under the reserved aws: prefix".to_string(),
            format!("{}cloudformation", appconfig::RESERVED_TAG_KEY_PREFIX),
            "stack".to_string(),
            Expected::Refused("BadRequestException"),
        ),
        (
            "a key that merely contains aws: later on".to_string(),
            "team-aws:billing".to_string(),
            "kept".to_string(),
            Expected::Stored,
        ),
        (
            "a one-character key with an empty value".to_string(),
            "k".to_string(),
            String::new(),
            Expected::Stored,
        ),
    ]
}

/// `TagResource` stores a pair inside those bounds and refuses one outside them with HTTP
/// 400, storing nothing.
#[tokio::test]
async fn tag_pairs_outside_the_documented_bounds_are_refused_and_store_nothing() {
    let router = router();
    let resources = taggable_resources(&router).await;

    for (kind, resource_arn) in &resources {
        for (case, key, value, expected) in tag_pair_rows() {
            let case = format!("{kind} / {case}");
            let before = reported_tags(&router, &case, resource_arn).await;
            let answer = tag(&router, resource_arn, &tag_map([(&*key, &*value)])).await;
            let after = reported_tags(&router, &case, resource_arn).await;

            match expected {
                Expected::Stored => {
                    assert_eq!(
                        answer.status,
                        204,
                        "case {case:?}: expected 204, got {} / {}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert_eq!(
                        after.get(&key).map(String::as_str),
                        Some(value.as_str()),
                        "case {case:?}: expected the pair to be stored, got {:?}",
                        after.get(&key)
                    );
                }
                Expected::Refused(code) => {
                    assert_eq!(
                        answer.status,
                        400,
                        "case {case:?}: expected 400, got {} / {}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert!(
                        answer.error_code().contains(code),
                        "case {case:?}: expected {code}, got {:?}",
                        answer.error_code()
                    );
                    assert_eq!(
                        after, before,
                        "case {case:?}: a refused write must store nothing, got {after:?}"
                    );
                }
            }
        }
    }
}

/// The same pair rule governs the `Tags` member of every create that takes one, so a
/// resource cannot be created carrying a pair `TagResource` would refuse.
#[tokio::test]
async fn a_create_refuses_the_same_tag_pairs_tag_resource_refuses() {
    let router = router();
    let application = create_application(&router, "create-tags").await;
    let profile = create_profile(&router, &application, "create-tags-profile").await;
    let version = send(
        &router,
        "POST",
        &format!(
            "/applications/{application}/configurationprofiles/{profile}/hostedconfigurationversions"
        ),
        "application/json",
        br#"{"enabled":true}"#.to_vec(),
        &[],
    )
    .await;
    assert_eq!(
        version.status,
        201,
        "the configuration to deploy must be hosted first, got {} / {}",
        version.status,
        String::from_utf8_lossy(&version.body)
    );

    for (case, key, value, expected) in tag_pair_rows() {
        // One environment per row: a deployment needs an environment nothing is already
        // deploying into.
        let environment = create_environment(&router, &application, "create-tags-env").await;
        let tags = tag_map([(&*key, &*value)]);
        let creates: Vec<(&str, String, Value)> = vec![
            (
                "CreateApplication",
                "/applications".to_string(),
                json!({ "Name": "tagged-at-create", "Tags": tags }),
            ),
            (
                "CreateEnvironment",
                format!("/applications/{application}/environments"),
                json!({ "Name": "tagged-at-create", "Tags": tags }),
            ),
            (
                "CreateConfigurationProfile",
                format!("/applications/{application}/configurationprofiles"),
                json!({ "Name": "tagged-at-create", "LocationUri": "hosted", "Tags": tags }),
            ),
            (
                "StartDeployment",
                format!("/applications/{application}/environments/{environment}/deployments"),
                json!({
                    "DeploymentStrategyId": "AppConfig.AllAtOnce",
                    "ConfigurationProfileId": profile,
                    "ConfigurationVersion": "1",
                    "Tags": tags,
                }),
            ),
        ];

        for (operation, uri, body) in creates {
            let case = format!("{operation} / {case}");
            let answer = json_call(&router, "POST", &uri, &body).await;
            match expected {
                Expected::Stored => assert_eq!(
                    answer.status,
                    201,
                    "case {case:?}: expected 201, got {} / {}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                ),
                Expected::Refused(code) => {
                    assert_eq!(
                        answer.status,
                        400,
                        "case {case:?}: expected 400, got {} / {}",
                        answer.status,
                        String::from_utf8_lossy(&answer.body)
                    );
                    assert!(
                        answer.error_code().contains(code),
                        "case {case:?}: expected {code}, got {:?}",
                        answer.error_code()
                    );
                }
            }
        }
    }
}

/// A resource created WITH tags reports them through `ListTagsForResource`, which is how a
/// create's `Tags` member and the tagging slice are proved to be the one tag map rather
/// than two stores.
#[tokio::test]
async fn tags_set_at_create_time_are_the_map_the_tag_operations_act_on() {
    let router = router();

    let created = json_call(
        &router,
        "POST",
        "/applications",
        &json!({
            "Name": "tagged-at-create",
            "Tags": { "Env": "prod", "Team": "platform" },
        }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "CreateApplication expected 201, got {} / {}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    let application = created
        .string("Id")
        .expect("CreateApplication must answer an Id");
    let resource_arn = arn(&format!("application/{application}"));

    let reported = reported_tags(&router, "create-time tags", &resource_arn).await;
    assert_eq!(
        reported,
        BTreeMap::from([
            ("Env".to_string(), "prod".to_string()),
            ("Team".to_string(), "platform".to_string()),
        ]),
        "expected the tags CreateApplication stored, got {reported:?}"
    );

    // The tagging slice then acts on that same map, not on a second one.
    let answer = tag(&router, &resource_arn, &tag_map([("Env", "staging")])).await;
    assert_eq!(
        answer.status,
        204,
        "TagResource expected 204, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
    let merged = reported_tags(&router, "create-time tags overwritten", &resource_arn).await;
    assert_eq!(
        merged,
        BTreeMap::from([
            ("Env".to_string(), "staging".to_string()),
            ("Team".to_string(), "platform".to_string()),
        ]),
        "expected the create-time map with Env overwritten, got {merged:?}"
    );
}

/// Every ARN that addresses no resource this service holds is 404
/// `ResourceNotFoundException` on all three tag operations — never a 501 and never a
/// success.
#[tokio::test]
async fn an_arn_addressing_no_resource_is_not_found_on_every_tag_operation() {
    let router = router();
    let resources = taggable_resources(&router).await;
    let application = create_application(&router, "resolvable").await;
    let environment = create_environment(&router, &application, "resolvable-env").await;

    // Rows differ along WHY the ARN resolves to nothing: an unknown id at each level of
    // the tree, a deployment number never issued, an AppConfig resource type whose tags
    // this service does not persist, a resource portion that is not a type/id path at
    // all, and another service's ARN.
    let unresolvable: Vec<(&str, String)> = vec![
        ("unknown application", arn("application/nosuch")),
        (
            "unknown environment under a known application",
            arn(&format!("application/{application}/environment/nosuch")),
        ),
        (
            "unknown profile under a known application",
            arn(&format!(
                "application/{application}/configurationprofile/nosuch"
            )),
        ),
        (
            "known environment, deployment number never issued",
            arn(&format!(
                "application/{application}/environment/{environment}/deployment/99"
            )),
        ),
        (
            "deployment number that is not a number",
            arn(&format!(
                "application/{application}/environment/{environment}/deployment/latest"
            )),
        ),
        (
            "a predefined deployment strategy, whose tags this service does not hold",
            arn("deploymentstrategy/AppConfig.AllAtOnce"),
        ),
        (
            "a hosted configuration version, whose tags this service does not hold",
            arn(&format!(
                "application/{application}/configurationprofile/nosuch/hostedconfigurationversion/1"
            )),
        ),
        ("an unknown resource type", arn("widget/abcdefg")),
        (
            "an application ARN with an empty id",
            arn("application/"),
        ),
        (
            "another service's ARN",
            engine::emulator_arn("secretsmanager", REGION, "secret:configuration-abc"),
        ),
    ];

    for (case, resource_arn) in &unresolvable {
        for (operation, answer) in [
            ("TagResource", tag(&router, resource_arn, &tag_map([("Env", "prod")])).await),
            ("ListTagsForResource", list_tags(&router, resource_arn).await),
            ("UntagResource", untag(&router, resource_arn, &["Env"]).await),
        ] {
            let case = format!("{operation} / {case}");
            assert_eq!(
                answer.status,
                404,
                "case {case:?} with {resource_arn:?}: expected 404, got {} / {}",
                answer.status,
                String::from_utf8_lossy(&answer.body)
            );
            assert!(
                answer.error_code().contains("ResourceNotFoundException"),
                "case {case:?} with {resource_arn:?}: expected ResourceNotFoundException, got {:?}",
                answer.error_code()
            );
        }
    }

    // A refused call writes nothing anywhere.
    for (kind, resource_arn) in &resources {
        let reported = reported_tags(&router, kind, resource_arn).await;
        assert!(
            reported.is_empty(),
            "case {kind:?}: a 404 must store nothing, got {reported:?}"
        );
    }
}

/// What makes an AppConfig ARN address a resource is its service namespace and its
/// resource portion: this emulator presents ONE account and does not partition its store
/// by Region, so the partition, Region and account sections are not discriminating.
#[tokio::test]
async fn the_resource_portion_and_service_namespace_are_what_resolve_an_arn() {
    let router = router();
    let application = create_application(&router, "arn-sections").await;
    let resource = format!("application/{application}");
    let account = engine::EMULATOR_ACCOUNT_ID;

    // Rows differ along the ARN section that varies, with the resource portion fixed.
    let equivalent: Vec<(&str, String)> = vec![
        ("the ARN this emulator mints", arn(&resource)),
        (
            "another partition",
            format!("arn:aws-us-gov:appconfig:{REGION}:{account}:{resource}"),
        ),
        (
            "another Region",
            format!("arn:aws:appconfig:eu-west-1:{account}:{resource}"),
        ),
        (
            "another account",
            format!("arn:aws:appconfig:{REGION}:123456789012:{resource}"),
        ),
        (
            "no Region and no account, which the model's Arn pattern allows",
            format!("arn:aws:appconfig:::{resource}"),
        ),
    ];

    for (index, (case, resource_arn)) in equivalent.iter().enumerate() {
        let value = format!("written-via-{index}");
        let answer = tag(&router, resource_arn, &tag_map([("Section", &*value)])).await;
        assert_eq!(
            answer.status,
            204,
            "case {case:?} with {resource_arn:?}: expected 204, got {} / {}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );

        // Read back through the ARN this emulator mints: every row addressed the SAME
        // resource, so the last write is what it carries.
        let reported = reported_tags(&router, case, &arn(&resource)).await;
        assert_eq!(
            reported.get("Section").map(String::as_str),
            Some(value.as_str()),
            "case {case:?}: expected the write to reach the same resource, got {reported:?}"
        );
    }

    // A service namespace that is not AppConfig's addresses nothing in AppConfig's store,
    // however well-formed the rest of the ARN is.
    let other_service = format!("arn:aws:appconfigdata:{REGION}:{account}:{resource}");
    let answer = list_tags(&router, &other_service).await;
    assert_eq!(
        answer.status,
        404,
        "case \"another service's namespace\" with {other_service:?}: expected 404, got {} / {}",
        answer.status,
        String::from_utf8_lossy(&answer.body)
    );
}
