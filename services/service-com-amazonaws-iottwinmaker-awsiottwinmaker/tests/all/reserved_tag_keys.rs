// SPDX-License-Identifier: CC0-1.0

//! The reserved tag-key namespace, over every operation of this service that
//! writes or removes a tag.
//!
//! RULE: the tag-key prefix `aws:`
//! ([`RESERVED_TAG_KEY_PREFIX`], matched however it is cased) belongs to Amazon
//! Web Services, so no caller-supplied key may lie under it: a request naming
//! one is refused with HTTP 400 `ValidationException` and NOTHING it asked for
//! is applied — not the resource a create would have minted, not the legal keys
//! sharing the request with it, and not the removals an untag asked for.
//!
//! The prefix itself is read from the service's own constant rather than
//! respelled here, so a row and the rule it checks cannot drift apart, and every
//! case drives the real request path (`MultiServiceRouter::dispatch`), so the
//! route, the status code and the serialized body are all exercised.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_iottwinmaker_awsiottwinmaker::state::RESERVED_TAG_KEY_PREFIX;

use crate::harness::{
    component_type_arn, entity_arn, scene_arn, sync_job_arn, workspace_arn, Emulator, Resp,
    SYNC_ROLE,
};

/// The workspace every child resource under test is created in.
const WORKSPACE: &str = "reservedTags";

/// A tag value every row shares: the rule is about the KEY, so the value is
/// never what decides a refusal.
const VALUE: &str = "whoever";

/// A legal key no row's rule touches, used to prove that a request mixing it
/// with a reserved key applies neither.
const LEGAL_KEY: &str = "team";

/// The prefix with only its first character upper-cased — the spelling a caller
/// reaches for when it believes the reservation is case-sensitive.
fn prefix_capitalized() -> String {
    let mut chars = RESERVED_TAG_KEY_PREFIX.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Tag keys that lie in the reserved namespace, each with the shape of key it
/// stands for. The rows differ along what follows the prefix (nothing, a word, a
/// further namespace, non-ASCII text, the longest key `TagKey` admits) and along
/// how the prefix itself is cased, since the reservation is of the namespace and
/// not of one spelling of it.
fn reserved_keys() -> Vec<(&'static str, String)> {
    let prefix = RESERVED_TAG_KEY_PREFIX;
    vec![
        ("thePrefixAlone", prefix.to_string()),
        ("aWordUnderThePrefix", format!("{prefix}owner")),
        (
            "aFurtherNamespace",
            format!("{prefix}cloudformation:stack-name"),
        ),
        ("nonAsciiUnderThePrefix", format!("{prefix}équipe")),
        ("upperCasedPrefix", format!("{}owner", prefix.to_uppercase())),
        (
            "capitalizedPrefix",
            format!("{}created-by", prefix_capitalized()),
        ),
        (
            // The longest key the modelled `TagKey` admits (max length 128,
            // from `smithy_query(query="shape",shape="TagKey")`), so the rule
            // is checked at the far end of the key's own range too.
            "theLongestKeyTagKeyAdmits",
            format!(
                "{prefix}{}",
                "k".repeat(128 - prefix.chars().count())
            ),
        ),
    ]
}

/// Keys that merely LOOK like reserved ones: only the prefix, at the start of
/// the key, is reserved, so each of these is an ordinary caller-owned key the
/// service must keep accepting. The rows differ along how they miss the
/// reservation (no delimiter, the prefix further along, the letters alone, the
/// prefix after another character).
fn lookalike_keys() -> Vec<(&'static str, String)> {
    let letters = RESERVED_TAG_KEY_PREFIX.trim_end_matches(':');
    vec![
        ("thePrefixWithoutItsDelimiter", format!("{letters}ome")),
        (
            "thePrefixFurtherAlong",
            format!("owner.{RESERVED_TAG_KEY_PREFIX}team"),
        ),
        ("theLettersAlone", letters.to_string()),
        (
            "thePrefixAfterAnotherCharacter",
            format!("_{RESERVED_TAG_KEY_PREFIX}owner"),
        ),
    ]
}

/// The `tags` member of a `ListTagsForResource` response as a comparable map.
fn tags_of(resp: &Resp) -> BTreeMap<String, String> {
    resp.member("tags")
        .as_object()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|(key, value)| (key, value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// Every tag the resource at `resource_arn` carries, read back through the
/// listing API (a page large enough for any legal tag set).
async fn listed_tags(
    emulator: &Emulator,
    case: &str,
    resource_arn: &str,
) -> BTreeMap<String, String> {
    let resp = emulator
        .list_tags_for_resource(resource_arn, json!({"maxResults": 50}))
        .await;
    assert_eq!(
        resp.status, 200,
        "case {case:?}: expected 200 from ListTagsForResource, got {resp:?}"
    );
    tags_of(&resp)
}

/// A tag map as the JSON object the tagging and create APIs take.
fn tag_map<I, K>(entries: I) -> Value
where
    I: IntoIterator<Item = (K, &'static str)>,
    K: Into<String>,
{
    Value::Object(
        entries
            .into_iter()
            .map(|(key, value)| (key.into(), json!(value)))
            .collect(),
    )
}

/// RULE: `TagResource` refuses a request naming any tag key in the reserved
/// namespace with HTTP 400 `ValidationException` and applies NONE of the batch,
/// so no reserved key is ever readable as a caller-set tag; a key that only
/// resembles a reserved one is applied as usual.
#[tokio::test]
async fn tagging_a_reserved_key_is_refused_and_applies_nothing() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace under test must be created"
    );
    let resource_arn = workspace_arn(WORKSPACE);

    // The tag set the resource is known to carry, grown only by the rows the
    // rule says are legal.
    let mut expected: BTreeMap<String, String> = BTreeMap::new();

    for (case, key) in reserved_keys() {
        // Alone in the request.
        let resp = emulator
            .tag_resource(&resource_arn, tag_map([(key.clone(), VALUE)]))
            .await;
        assert_eq!(
            (resp.status, resp.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: tagging with the reserved key {key:?} expected a 400 \
             ValidationException, got {resp:?}"
        );

        // And sharing the request with a key the caller does own: the batch is
        // all-or-nothing, so the legal key is not stored either.
        let mixed = emulator
            .tag_resource(
                &resource_arn,
                tag_map([(key.clone(), VALUE), (LEGAL_KEY.to_string(), VALUE)]),
            )
            .await;
        assert_eq!(
            (mixed.status, mixed.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a batch mixing {LEGAL_KEY:?} with {key:?} expected a 400 \
             ValidationException, got {mixed:?}"
        );

        let listed = listed_tags(&emulator, case, &resource_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: a refused call must store nothing — expected {expected:?}, got \
             {listed:?}"
        );
        assert!(
            !listed.contains_key(&key),
            "case {case:?}: the reserved key {key:?} must not be readable as a caller-set tag, \
             got {listed:?}"
        );
    }

    for (case, key) in lookalike_keys() {
        let resp = emulator
            .tag_resource(&resource_arn, tag_map([(key.clone(), VALUE)]))
            .await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: the key {key:?} is the caller's own, so tagging with it expected 200, \
             got {resp:?}"
        );
        expected.insert(key.clone(), VALUE.to_string());
        let listed = listed_tags(&emulator, case, &resource_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the stored set {expected:?}, got {listed:?}"
        );
    }
}

/// RULE: a caller may no more REMOVE a tag in the reserved namespace than create
/// one, so `UntagResource` refuses a request naming such a key with HTTP 400
/// `ValidationException` and removes none of the named keys.
#[tokio::test]
async fn untagging_a_reserved_key_is_refused_and_removes_nothing() {
    let emulator = Emulator::new();
    assert_eq!(
        emulator.create_workspace(WORKSPACE).await.status,
        200,
        "the workspace under test must be created"
    );
    let resource_arn = workspace_arn(WORKSPACE);

    let stored = tag_map([(LEGAL_KEY.to_string(), VALUE), ("env".to_string(), VALUE)]);
    assert_eq!(
        emulator.tag_resource(&resource_arn, stored).await.status,
        200,
        "the tags under test must be stored"
    );
    let expected: BTreeMap<String, String> = BTreeMap::from([
        (LEGAL_KEY.to_string(), VALUE.to_string()),
        ("env".to_string(), VALUE.to_string()),
    ]);

    for (case, key) in reserved_keys() {
        let alone = emulator.untag_resource(&resource_arn, &[&key]).await;
        assert_eq!(
            (alone.status, alone.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: removing the reserved key {key:?} expected a 400 ValidationException, \
             got {alone:?}"
        );

        // Mixed with a key the resource really carries: nothing is removed.
        let mixed = emulator
            .untag_resource(&resource_arn, &[LEGAL_KEY, &key])
            .await;
        assert_eq!(
            (mixed.status, mixed.error_type.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: a removal mixing {LEGAL_KEY:?} with {key:?} expected a 400 \
             ValidationException, got {mixed:?}"
        );

        let listed = listed_tags(&emulator, case, &resource_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: a refused removal must leave {expected:?}, got {listed:?}"
        );
    }

    // The lookalikes are ordinary keys, so removing one works as usual.
    for (case, key) in lookalike_keys() {
        assert_eq!(
            emulator
                .tag_resource(&resource_arn, tag_map([(key.clone(), VALUE)]))
                .await
                .status,
            200,
            "case {case:?}: storing the lookalike key {key:?} must succeed"
        );
        let resp = emulator.untag_resource(&resource_arn, &[&key]).await;
        assert_eq!(
            resp.status, 200,
            "case {case:?}: removing the caller-owned key {key:?} expected 200, got {resp:?}"
        );
        let listed = listed_tags(&emulator, case, &resource_arn).await;
        assert_eq!(
            listed, expected,
            "case {case:?}: expected {expected:?} after removing {key:?}, got {listed:?}"
        );
    }
}

/// The kinds of resource this service mints an ARN for whose create accepts a
/// `tags` map.
#[derive(Clone, Copy, Debug)]
enum Creatable {
    Workspace,
    Entity,
    ComponentType,
    Scene,
    SyncJob,
}

impl Creatable {
    /// The id this row's resource is created under, and the ARN that names it.
    fn arn(self) -> String {
        match self {
            Self::Workspace => workspace_arn("reservedTagsOnCreate"),
            Self::Entity => entity_arn(WORKSPACE, "pump"),
            Self::ComponentType => component_type_arn(WORKSPACE, "com.example.pump"),
            Self::Scene => scene_arn(WORKSPACE, "floorPlan"),
            // A sync job has no id of its own: the sync SOURCE identifies it
            // inside its workspace, and the create serves exactly one source.
            Self::SyncJob => sync_job_arn(WORKSPACE, "SITEWISE"),
        }
    }

    /// Create this row's resource with `tags`, through the operation that owns
    /// it.
    async fn create(self, emulator: &Emulator, tags: Value) -> Resp {
        match self {
            Self::Workspace => {
                emulator
                    .call(
                        "POST",
                        "/workspaces/reservedTagsOnCreate",
                        Some(json!({"tags": tags})),
                    )
                    .await
            }
            Self::Entity => {
                emulator
                    .create_entity(
                        WORKSPACE,
                        json!({"entityId": "pump", "entityName": "Pump", "tags": tags}),
                    )
                    .await
            }
            Self::ComponentType => {
                emulator
                    .create_component_type(WORKSPACE, "com.example.pump", json!({"tags": tags}))
                    .await
            }
            Self::Scene => {
                emulator
                    .create_scene(
                        WORKSPACE,
                        json!({
                            "sceneId": "floorPlan",
                            "contentLocation": "s3://twinmaker-scenes/floor.json",
                            "tags": tags,
                        }),
                    )
                    .await
            }
            Self::SyncJob => {
                emulator
                    .create_sync_job(
                        WORKSPACE,
                        "SITEWISE",
                        json!({"syncRole": SYNC_ROLE, "tags": tags}),
                    )
                    .await
            }
        }
    }
}

/// RULE: the `tags` a create stores on the resource it mints follow the same
/// reservation as the tagging API — a key in the reserved namespace is refused
/// with HTTP 400 `ValidationException` and the resource is not created at all —
/// while a create whose keys are the caller's own stores them, so the tagging
/// API reads them back.
///
/// Driven against EVERY create of this service that accepts a `tags` map, since
/// the rule is per tag set and not per operation, and each one is checked with
/// the reserved key alone and sharing the request with a legal key.
#[tokio::test]
async fn creating_a_resource_with_a_reserved_tag_key_is_refused() {
    for kind in [
        Creatable::Workspace,
        Creatable::Entity,
        Creatable::ComponentType,
        Creatable::Scene,
        Creatable::SyncJob,
    ] {
        for (row, key) in reserved_keys() {
            // One emulator per case: a create either happens or does not, so a
            // row must not inherit the resource another row created.
            let emulator = Emulator::new();
            assert_eq!(
                emulator.create_workspace(WORKSPACE).await.status,
                200,
                "case {kind:?}/{row:?}: the parent workspace must be created"
            );
            let resource_arn = kind.arn();

            let refused = kind
                .create(&emulator, tag_map([(key.clone(), VALUE)]))
                .await;
            assert_eq!(
                (refused.status, refused.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {kind:?}/{row:?}: a create carrying the reserved key {key:?} expected a 400 \
                 ValidationException, got {refused:?}"
            );

            let mixed = kind
                .create(
                    &emulator,
                    tag_map([(key.clone(), VALUE), (LEGAL_KEY.to_string(), VALUE)]),
                )
                .await;
            assert_eq!(
                (mixed.status, mixed.error_type.as_deref()),
                (400, Some("ValidationException")),
                "case {kind:?}/{row:?}: a create mixing {LEGAL_KEY:?} with {key:?} expected a 400 \
                 ValidationException, got {mixed:?}"
            );

            // Nothing was created: the ARN the create would have minted names no
            // resource, so the tagging API cannot find one under it.
            let listed = emulator
                .list_tags_for_resource(&resource_arn, json!({}))
                .await;
            assert_eq!(
                (listed.status, listed.error_type.as_deref()),
                (404, Some("ResourceNotFoundException")),
                "case {kind:?}/{row:?}: a refused create must leave no resource at \
                 {resource_arn:?}, got {listed:?}"
            );

            // The same create with the caller's own key is accepted, and the
            // tags it stored are the set the tagging API reads back.
            let accepted = kind
                .create(&emulator, tag_map([(LEGAL_KEY.to_string(), VALUE)]))
                .await;
            assert_eq!(
                accepted.status, 200,
                "case {kind:?}/{row:?}: a create carrying only {LEGAL_KEY:?} expected 200, got \
                 {accepted:?}"
            );
            let listed = listed_tags(&emulator, row, &resource_arn).await;
            assert_eq!(
                listed,
                BTreeMap::from([(LEGAL_KEY.to_string(), VALUE.to_string())]),
                "case {kind:?}/{row:?}: expected the create's own tags, got {listed:?}"
            );
        }
    }
}
