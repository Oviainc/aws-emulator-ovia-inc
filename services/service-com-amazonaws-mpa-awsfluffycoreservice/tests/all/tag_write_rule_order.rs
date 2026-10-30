// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning the ORDER the two tag-write rules are decided in, on every
//! path of this service that writes a resource's tags: `CreateApprovalTeam`,
//! `CreateIdentitySource` and `TagResource`.
//!
//! The rule: a path that writes tags holds the request to the constraints on its own
//! INPUT before it holds the resource to a limit on the state that input would leave
//! behind. A key in the namespace AWS reserves for its own tags
//! ([`state::RESERVED_TAG_KEY_PREFIX`]) is a property of the keys the request carries,
//! while `Tags` carries no modelled length constraint at all
//! (`smithy_query(query="shape",shape="Tags")`: a map with no `length`), so the
//! per-resource ceiling ([`state::MAX_TAGS_PER_RESOURCE`]) is the resource limit
//! `TooManyTagsException` is declared for ("The request exceeds the maximum number of
//! tags allowed for this resource"). So the namespace is decided FIRST and the ceiling
//! second, and a map that breaks BOTH earns the namespace refusal —
//! `ValidationException` (400), with one and the same sentence — whichever of the three
//! paths it was sent on, rather than one refusal per path.
//!
//! What the other tag suites already pin is the halves of this: `create_tag_ceiling.rs`
//! and `resource_tags.rs` the ceiling, `reserved_tag_namespace.rs` and
//! `write_path_input_validation.rs` the namespace. These tables are about a map that
//! breaks both at once, which no row there sends.
//!
//! The rows differ along which rules the map breaks (neither, the ceiling only, the
//! namespace only, both), how far past the ceiling the write would leave the resource,
//! whether the ceiling is reached by the request's own keys or by the tags the resource
//! ALREADY holds, how many keys are reserved and how the reserved prefix is spelled —
//! crossed with the three write paths.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::{BTreeMap, HashMap};

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// What a write path must answer a `Tags` map with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// Neither rule is broken: the map is written and reads back.
    Written,
    /// The namespace rule is broken — whether or not the ceiling is broken too — so the
    /// answer is the one sentence every writing path states for it.
    ReservedNamespace,
    /// Only the ceiling is broken: the write would leave the resource holding this many
    /// tags.
    PastCeiling(usize),
}

/// `count` conforming tag keys, named so they cannot collide with the reserved ones or
/// with the keys a resource is seeded with.
fn conforming(count: usize) -> Vec<String> {
    (1..=count).map(|n| format!("key{n:04}")).collect()
}

/// `keys` followed by `extra`, so a row can place reserved keys among conforming ones.
fn with(mut keys: Vec<String>, extra: &[&str]) -> Vec<String> {
    keys.extend(extra.iter().map(|key| (*key).to_string()));
    keys
}

/// A tag map from `keys`, each value derived from its key so a written key is
/// identifiable in the read-back map. `TagValue` is `length(0..256)`, so a value derived
/// from a key of up to 128 characters still fits.
fn map_of(keys: &[String]) -> BTreeMap<String, String> {
    keys.iter()
        .map(|key| (key.clone(), format!("value-of-{key}")))
        .collect()
}

/// The same map as the `Tags` member's JSON object.
fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// A `CreateApprovalTeam` body every modelled constraint accepts, carrying `tags`.
/// `Name` matches `^[a-zA-Z0-9._-]+$`; the attached policy version is one a fresh store
/// seeds.
fn team_body(name: &str, tags: &BTreeMap<String, String>) -> Value {
    json!({
        "Name": name,
        "Description": "tag write rule order",
        "ApprovalStrategy": { "MofN": { "MinApprovalsRequired": 1 } },
        "Approvers": [{
            "PrimaryIdentityId": "identity-0001",
            "PrimaryIdentitySourceArn": format!(
                "arn:aws:mpa:{REGION}:{}:identity-source/ssoins-0001",
                engine::emulator_account_id()
            ),
        }],
        "Policies": [{
            "PolicyArn": format!(
                "{}/{}",
                state::policy_arn("IdentityCenter", "MPAApprovalTeamPolicy"),
                state::DEFAULT_POLICY_VERSION
            )
        }],
        "Tags": tag_object(tags),
    })
}

/// A `CreateIdentitySource` body every modelled constraint accepts, carrying `tags`.
/// `InstanceArn` satisfies `IdcInstanceArn`'s pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`.
fn identity_source_body(tags: &BTreeMap<String, String>) -> Value {
    json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                "Region": REGION,
            }
        },
        "Tags": tag_object(tags),
    })
}

/// Create an approval team carrying `tags` and return its ARN.
async fn create_team(harness: &Harness, name: &str, tags: &BTreeMap<String, String>) -> String {
    let (status, body) = harness
        .call("POST", "/approval-teams", Some(team_body(name, tags)))
        .await;
    assert_eq!(
        status, 200,
        "creating the team to tag returned {status} with {body:?}"
    );
    text(&body, "Arn")
}

/// `PUT /tags/{ResourceArn}` — `TagResource`.
async fn tag_resource(
    harness: &Harness,
    arn: &str,
    tags: &BTreeMap<String, String>,
) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({ "Tags": tag_object(tags) })),
        )
        .await
}

/// The tags `ListTagsForResource` reports for `arn`.
async fn tags_of(harness: &Harness, arn: &str, case: &str) -> BTreeMap<String, String> {
    let (status, body) = harness
        .call("GET", &format!("/tags/{}", encode_label(arn)), None)
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListTagsForResource returned {status} with {body:?}"
    );
    body.get("Tags")
        .and_then(Value::as_object)
        .unwrap_or_else(|| panic!("case {case:?}: ListTagsForResource reported no Tags: {body:?}"))
        .iter()
        .map(|(key, value)| (key.clone(), value.as_str().unwrap_or_default().to_string()))
        .collect()
}

/// How many resources of each kind the store holds, so a refused create can be shown to
/// have stored nothing.
fn stored_counts(harness: &Harness) -> (usize, usize) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    (guard.approval_teams.len(), guard.identity_sources.len())
}

/// One path that writes a resource's tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WritePath {
    CreateApprovalTeam,
    CreateIdentitySource,
    TagResource,
}

impl WritePath {
    /// The three paths of this service that write a resource's tags.
    fn all() -> [Self; 3] {
        [
            Self::CreateApprovalTeam,
            Self::CreateIdentitySource,
            Self::TagResource,
        ]
    }

    fn name(self) -> &'static str {
        match self {
            Self::CreateApprovalTeam => "CreateApprovalTeam",
            Self::CreateIdentitySource => "CreateIdentitySource",
            Self::TagResource => "TagResource",
        }
    }

    /// Send `tags` on this path, and report the answer plus the ARN of the resource whose
    /// tags the call writes — `None` for a create the service refused, which mints
    /// nothing to read back.
    async fn send(
        self,
        harness: &Harness,
        index: usize,
        tags: &BTreeMap<String, String>,
    ) -> (u16, Value, Option<String>) {
        match self {
            Self::CreateApprovalTeam => {
                let (status, body) = harness
                    .call(
                        "POST",
                        "/approval-teams",
                        Some(team_body(&format!("tag-order-{index}"), tags)),
                    )
                    .await;
                let arn = (status == 200).then(|| text(&body, "Arn"));
                (status, body, arn)
            }
            Self::CreateIdentitySource => {
                let (status, body) = harness
                    .call("POST", "/identity-sources", Some(identity_source_body(tags)))
                    .await;
                let arn = (status == 200).then(|| text(&body, "IdentitySourceArn"));
                (status, body, arn)
            }
            Self::TagResource => {
                // The resource has to exist before it can be tagged, and existence is
                // decided ahead of both tag rules. It starts UNTAGGED, so the set the call
                // would leave behind is exactly the map it carries — the same quantity a
                // create holds against the ceiling.
                let arn = create_team(harness, &format!("tag-order-{index}"), &BTreeMap::new()).await;
                let (status, body) = tag_resource(harness, &arn, tags).await;
                (status, body, Some(arn))
            }
        }
    }

    /// A refused call wrote nothing: a create stored no resource, and a tagging call left
    /// the resource's tags as it found them (empty, on the resource [`Self::send`] makes).
    async fn assert_wrote_nothing(self, harness: &Harness, arn: Option<&str>, case: &str) {
        match self {
            Self::TagResource => {
                let arn = arn.unwrap_or_else(|| panic!("case {case:?}: no tagged resource ARN"));
                let listed = tags_of(harness, arn, case).await;
                assert!(
                    listed.is_empty(),
                    "case {case:?}: a refused tagging call wrote {listed:?}"
                );
            }
            _ => {
                let (teams, sources) = stored_counts(harness);
                assert_eq!(
                    (teams, sources),
                    (0, 0),
                    "case {case:?}: a refused create stored {teams} team(s) and {sources} \
                     identity source(s)"
                );
            }
        }
    }
}

/// One row of the shared table: the keys a request's `Tags` names, and what every write
/// path must answer it with.
struct Row {
    case: &'static str,
    keys: Vec<String>,
    expect: Expect,
}

/// The shared table, sent on each of the three write paths. Rows differ along which of
/// the two rules the map breaks, how far past the ceiling it would take the resource, how
/// many keys are reserved and how the prefix is spelled.
///
/// The reserved spellings come from the namespace rule itself
/// ([`state::RESERVED_TAG_KEY_PREFIX`], matched at the start of the key ignoring case),
/// and the ceiling from [`state::MAX_TAGS_PER_RESOURCE`]: each side of it, the ceiling
/// itself and well past it. `TagKey` is `length(1..128)` with no modelled pattern, so
/// every row reaches the handler.
fn rows() -> Vec<Row> {
    let max = state::MAX_TAGS_PER_RESOURCE;
    let prefix = state::RESERVED_TAG_KEY_PREFIX;
    vec![
        Row {
            case: "neither rule: a handful of conforming keys",
            keys: conforming(3),
            expect: Expect::Written,
        },
        Row {
            case: "neither rule: exactly the ceiling, including keys that only resemble \
                   the prefix",
            keys: with(conforming(max - 3), &["awsome", "aws", "my:aws:thing"]),
            expect: Expect::Written,
        },
        Row {
            case: "the ceiling only: one past it",
            keys: conforming(max + 1),
            expect: Expect::PastCeiling(max + 1),
        },
        Row {
            case: "the ceiling only: far past it",
            keys: conforming(max * 2),
            expect: Expect::PastCeiling(max * 2),
        },
        Row {
            case: "the namespace only: one reserved key among conforming keys",
            keys: with(conforming(2), &["aws:owner"]),
            expect: Expect::ReservedNamespace,
        },
        Row {
            case: "the namespace only: every key reserved, well below the ceiling",
            keys: with(Vec::new(), &["aws:owner", "AWS:Cost-Center"]),
            expect: Expect::ReservedNamespace,
        },
        Row {
            case: "both rules: one past the ceiling, one reserved key among the rest",
            keys: with(conforming(max), &["aws:owner"]),
            expect: Expect::ReservedNamespace,
        },
        Row {
            case: "both rules: one past the ceiling, the bare reserved prefix among the rest",
            keys: with(conforming(max), &[prefix]),
            expect: Expect::ReservedNamespace,
        },
        Row {
            case: "both rules: one past the ceiling, the prefix spelled in another case",
            keys: with(conforming(max), &["Aws:owner"]),
            expect: Expect::ReservedNamespace,
        },
        Row {
            case: "both rules: far past the ceiling, several reserved keys",
            keys: with(conforming(max * 2), &["aws:owner", "AWS:created-by"]),
            expect: Expect::ReservedNamespace,
        },
        Row {
            case: "both rules: every key reserved and past the ceiling",
            keys: (1..=max + 1).map(|n| format!("aws:key{n:04}")).collect(),
            expect: Expect::ReservedNamespace,
        },
    ]
}

/// The order, over the whole table and all three write paths: a map naming a reserved key
/// earns the namespace refusal even when it also takes the resource past the ceiling, and
/// every path words that refusal identically; a map that breaks only the ceiling earns the
/// ceiling refusal its own path declares; a map that breaks neither is written.
#[tokio::test]
async fn every_tag_write_path_decides_the_reserved_namespace_before_the_ceiling() {
    let max = state::MAX_TAGS_PER_RESOURCE;
    // The namespace sentence each path answered, per row, so the three paths are compared
    // with EACH OTHER and not only with the sentence the crate states.
    let mut namespace_refusals: BTreeMap<&str, Vec<(&str, String)>> = BTreeMap::new();

    for path in WritePath::all() {
        for (index, Row { case: row, keys, expect }) in rows().into_iter().enumerate() {
            // A store of its own per row, so nothing a previous row wrote carries in and a
            // refused call can be shown to have written nothing at all.
            let harness = Harness::new();
            let case = format!("{} / {row}", path.name());
            let requested = map_of(&keys);
            let (status, body, arn) = path.send(&harness, index, &requested).await;

            match expect {
                Expect::Written => {
                    assert_eq!(
                        status, 200,
                        "case {case:?}: a map breaking neither rule was refused: {body:?}"
                    );
                    let arn = arn.unwrap_or_else(|| {
                        panic!("case {case:?}: the accepted write reported no ARN: {body:?}")
                    });
                    let listed = tags_of(&harness, &arn, &case).await;
                    assert_eq!(
                        listed, requested,
                        "case {case:?}: the resource reports {listed:?}, not the {} tags the \
                         call wrote",
                        requested.len()
                    );
                }
                Expect::ReservedNamespace => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400 for {} keys, got {status} with \
                         {body:?}",
                        requested.len()
                    );
                    assert_eq!(
                        error_code(&body),
                        "ValidationException",
                        "case {case:?}: expected error code ValidationException, got {body:?}"
                    );
                    let message = text(&body, "Message");
                    assert_eq!(
                        message,
                        state::reserved_tag_key().message,
                        "case {case:?}: expected the reserved-namespace sentence, got \
                         {message:?}"
                    );
                    namespace_refusals
                        .entry(row)
                        .or_default()
                        .push((path.name(), message));
                    path.assert_wrote_nothing(&harness, arn.as_deref(), &case).await;
                }
                Expect::PastCeiling(count) => {
                    assert_eq!(
                        status, 400,
                        "case {case:?}: expected HTTP 400 for {count} tags, got {status} with \
                         {body:?}"
                    );
                    let message = text(&body, "Message");
                    if path == WritePath::TagResource {
                        // Only `TagResource` declares the error the model gives this
                        // condition; a create has to answer with its `ValidationException`.
                        assert_eq!(
                            error_code(&body),
                            "TooManyTagsException",
                            "case {case:?}: expected error code TooManyTagsException, got \
                             {body:?}"
                        );
                        assert!(
                            message.contains(&count.to_string())
                                && message.contains(&max.to_string()),
                            "case {case:?}: the refusal must name the {count} tags the write \
                             would leave and the maximum of {max}, it reads {message:?}"
                        );
                        assert_eq!(
                            text(&body, "ResourceName"),
                            arn.clone().unwrap_or_default(),
                            "case {case:?}: wrong ResourceName in {body:?}"
                        );
                    } else {
                        assert_eq!(
                            error_code(&body),
                            "ValidationException",
                            "case {case:?}: expected error code ValidationException, got \
                             {body:?}"
                        );
                        assert_eq!(
                            message,
                            state::tags_past_resource_ceiling(count).message,
                            "case {case:?}: expected the ceiling sentence for {count} tags, \
                             got {message:?}"
                        );
                    }
                    path.assert_wrote_nothing(&harness, arn.as_deref(), &case).await;
                }
            }
        }
    }

    // One order, one sentence: every path that refused a row for the namespace refused it
    // with the other paths' message, and every path refused it.
    for (row, raised) in &namespace_refusals {
        assert_eq!(
            raised.len(),
            WritePath::all().len(),
            "case {row:?}: only {} of the write paths answered the namespace refusal: \
             {raised:?}",
            raised.len()
        );
        let (first_path, first) = &raised[0];
        for (name, message) in &raised[1..] {
            assert_eq!(
                message, first,
                "case {row:?}: {name} refuses with {message:?}, {first_path} with {first:?}"
            );
        }
    }
}

/// One row of the stored-tags table: how many tags the resource already holds, the keys
/// the tagging call names, and what it must be answered with.
struct TagRow {
    case: &'static str,
    held: usize,
    keys: Vec<String>,
    expect: Expect,
}

/// On `TagResource` the ceiling is a property of the MERGED set, so it can be broken by
/// the tags the resource already holds rather than by the request's size. The namespace
/// still wins: these rows pin the order where the two rules are broken by different parts
/// of the call.
#[tokio::test]
async fn tagging_decides_the_namespace_before_a_ceiling_the_stored_tags_reach() {
    let max = state::MAX_TAGS_PER_RESOURCE;
    let held_keys = |count: usize| -> Vec<String> {
        (1..=count).map(|n| format!("held{n:04}")).collect()
    };

    let rows = vec![
        TagRow {
            case: "the stored tags fill the resource and the call adds one conforming key",
            held: max,
            keys: with(Vec::new(), &["add0001"]),
            expect: Expect::PastCeiling(max + 1),
        },
        TagRow {
            case: "the stored tags fill the resource and the call adds two conforming keys",
            held: max,
            keys: with(Vec::new(), &["add0001", "add0002"]),
            expect: Expect::PastCeiling(max + 2),
        },
        TagRow {
            case: "the stored tags fill the resource and the call adds conforming keys and \
                   a reserved one",
            held: max,
            keys: with(Vec::new(), &["add0001", "add0002", "aws:owner"]),
            expect: Expect::ReservedNamespace,
        },
        TagRow {
            case: "the stored tags fill the resource and the call replaces a key it holds \
                   and adds a reserved one",
            held: max,
            keys: with(Vec::new(), &["held0001", "aws:owner"]),
            expect: Expect::ReservedNamespace,
        },
        TagRow {
            case: "the stored tags fill the resource and the call only replaces a key it \
                   already holds",
            held: max,
            keys: with(Vec::new(), &["held0001"]),
            expect: Expect::Written,
        },
        TagRow {
            case: "the resource holds a few tags and the call names a reserved key well \
                   within the ceiling",
            held: 2,
            keys: with(Vec::new(), &["aws:owner"]),
            expect: Expect::ReservedNamespace,
        },
        TagRow {
            case: "the resource holds a few tags and the call fills it exactly",
            held: max - 2,
            keys: with(Vec::new(), &["add0001", "add0002"]),
            expect: Expect::Written,
        },
    ];

    for (
        index,
        TagRow {
            case,
            held,
            keys,
            expect,
        },
    ) in rows.into_iter().enumerate()
    {
        let harness = Harness::new();
        let stored = map_of(&held_keys(held));
        let arn = create_team(&harness, &format!("stored-tags-{index}"), &stored).await;
        let before = tags_of(&harness, &arn, case).await;
        assert_eq!(
            before, stored,
            "case {case:?}: the created team reports {before:?}, not the {held} tags it was \
             created with"
        );

        // The requested values differ from the stored ones, so a row that only replaces a
        // key the resource holds is observable in the read-back map.
        let requested: BTreeMap<String, String> = keys
            .iter()
            .map(|key| (key.clone(), format!("requested-{key}")))
            .collect();
        let (status, body) = tag_resource(&harness, &arn, &requested).await;
        let mut expected = stored.clone();

        match expect {
            Expect::Written => {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a call breaking neither rule was refused: {body:?}"
                );
                expected.extend(requested.clone());
            }
            Expect::ReservedNamespace => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected error code ValidationException, got {body:?}"
                );
                let message = text(&body, "Message");
                assert_eq!(
                    message,
                    state::reserved_tag_key().message,
                    "case {case:?}: expected the reserved-namespace sentence, got {message:?}"
                );
            }
            Expect::PastCeiling(count) => {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400, got {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "TooManyTagsException",
                    "case {case:?}: expected error code TooManyTagsException, got {body:?}"
                );
                let message = text(&body, "Message");
                assert!(
                    message.contains(&count.to_string()) && message.contains(&max.to_string()),
                    "case {case:?}: the refusal must name the {count} tags the call would \
                     leave and the maximum of {max}, it reads {message:?}"
                );
            }
        }

        let after = tags_of(&harness, &arn, case).await;
        assert_eq!(
            after, expected,
            "case {case:?}: the resource holds {after:?}, not {expected:?}"
        );
        assert!(
            after.len() <= max,
            "case {case:?}: the resource holds {} tags, past the maximum of {max}",
            after.len()
        );
    }
}

/// The gate itself, over a table of (keys, resulting tag count): the namespace is answered
/// whenever a key is reserved, whatever the count, and the ceiling only when no key is.
#[test]
fn the_shared_tag_write_gate_decides_the_namespace_before_the_ceiling() {
    let max = state::MAX_TAGS_PER_RESOURCE;
    let prefix = state::RESERVED_TAG_KEY_PREFIX;
    let reserved = Some(state::TagWriteBreach::ReservedKey);
    let past = |count: usize| Some(state::TagWriteBreach::PastResourceCeiling(count));

    // (case, the keys the request names, the tags the write would leave, the breach)
    let rows: Vec<(&str, Vec<String>, usize, Option<state::TagWriteBreach>)> = vec![
        ("no keys at all", Vec::new(), 0, None),
        ("conforming keys well within the ceiling", conforming(3), 3, None),
        ("conforming keys that exactly fill the resource", conforming(max), max, None),
        (
            "conforming keys one past the ceiling",
            conforming(max + 1),
            max + 1,
            past(max + 1),
        ),
        (
            "conforming keys far past the ceiling",
            conforming(max * 2),
            max * 2,
            past(max * 2),
        ),
        (
            "one conforming key, past the ceiling because of the tags already held",
            conforming(1),
            max + 1,
            past(max + 1),
        ),
        ("one reserved key within the ceiling", with(Vec::new(), &["aws:owner"]), 1, reserved),
        (
            "one reserved key with a count exactly at the ceiling",
            with(conforming(max - 1), &["aws:owner"]),
            max,
            reserved,
        ),
        (
            "one reserved key with a count one past the ceiling",
            with(conforming(max), &["aws:owner"]),
            max + 1,
            reserved,
        ),
        (
            "one reserved key with a count far past the ceiling",
            with(conforming(max * 2), &["aws:owner"]),
            max * 2 + 1,
            reserved,
        ),
        (
            "the bare reserved prefix with a count past the ceiling",
            with(conforming(max), &[prefix]),
            max + 1,
            reserved,
        ),
        (
            "the prefix in another case with a count past the ceiling",
            with(conforming(max), &["AWS:Owner"]),
            max + 1,
            reserved,
        ),
        (
            "keys that only resemble the prefix, with a count past the ceiling",
            with(conforming(max), &["awsome", "my:aws:thing"]),
            max + 2,
            past(max + 2),
        ),
    ];

    for (case, keys, resulting, expected) in rows {
        let breach = state::tag_write_breach(keys.iter().map(String::as_str), resulting);
        assert_eq!(
            breach, expected,
            "case {case:?}: {} keys leaving {resulting} tags answered {breach:?}, expected \
             {expected:?}",
            keys.len()
        );
    }
}

/// One row of the create-gate table: the keys a create's `Tags` names (`None` for a create
/// carrying no `Tags` at all), and the message the gate words its refusal with.
struct CreateRow {
    case: &'static str,
    keys: Option<Vec<String>>,
    message: Option<String>,
}

/// The create paths' door to the gate words each breach with the one sentence this service
/// states for it, and picks the namespace sentence for a map that breaks both rules.
#[test]
fn a_create_tag_map_breaking_both_rules_is_refused_for_the_namespace() {
    let max = state::MAX_TAGS_PER_RESOURCE;

    let rows = vec![
        CreateRow {
            case: "no Tags member at all",
            keys: None,
            message: None,
        },
        CreateRow {
            case: "an empty Tags map",
            keys: Some(Vec::new()),
            message: None,
        },
        CreateRow {
            case: "conforming keys that exactly fill the resource",
            keys: Some(conforming(max)),
            message: None,
        },
        CreateRow {
            case: "conforming keys one past the ceiling",
            keys: Some(conforming(max + 1)),
            message: Some(state::tags_past_resource_ceiling(max + 1).message),
        },
        CreateRow {
            case: "conforming keys far past the ceiling",
            keys: Some(conforming(max * 2)),
            message: Some(state::tags_past_resource_ceiling(max * 2).message),
        },
        CreateRow {
            case: "one reserved key well within the ceiling",
            keys: Some(with(Vec::new(), &["aws:owner"])),
            message: Some(state::reserved_tag_key().message),
        },
        CreateRow {
            case: "one reserved key and a count one past the ceiling",
            keys: Some(with(conforming(max), &["aws:owner"])),
            message: Some(state::reserved_tag_key().message),
        },
        CreateRow {
            case: "several reserved keys and a count far past the ceiling",
            keys: Some(with(conforming(max * 2), &["aws:owner", "AWS:created-by"])),
            message: Some(state::reserved_tag_key().message),
        },
    ];

    for CreateRow {
        case,
        keys,
        message,
    } in rows
    {
        let tags: Option<HashMap<String, String>> = keys.as_ref().map(|keys| {
            keys.iter()
                .map(|key| (key.clone(), format!("value-of-{key}")))
                .collect()
        });
        let refusal = state::create_tags_refusal(tags.as_ref()).map(|refusal| refusal.message);
        assert_eq!(
            refusal, message,
            "case {case:?}: a create carrying {} tags is refused with {refusal:?}, expected \
             {message:?}",
            keys.as_ref().map_or(0, Vec::len)
        );
    }
}
