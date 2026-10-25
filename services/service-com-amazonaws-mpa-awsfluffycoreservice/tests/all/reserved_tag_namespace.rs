// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning the reserved tag-key namespace rule where a request carries
//! MORE than one key, and on every kind of resource this service lets a caller tag.
//!
//! The rule: a tag key in the namespace AWS reserves for its own tags
//! ([`state::RESERVED_TAG_KEY_PREFIX`], matched at the start of the key and ignoring
//! case) is AWS's, not the caller's, so a request that names one is refused WHOLE with
//! `ValidationException` (400) and writes nothing — the keys it names outside the
//! namespace are not written either, whatever else the request carries and whichever
//! taggable resource it names. A request naming no reserved key is accepted and every
//! key it carries is readable afterwards.
//!
//! `write_path_input_validation.rs` spans the FORM of one key per request across the
//! three write paths; these tables span what the single-key rows cannot reach:
//!
//! * how many keys the request carries and where the reserved one sits among them (first,
//!   last, in the middle, several at once, the whole map) — so a handler that wrote the
//!   conforming part of a refused map, or that only inspected one entry, fails here;
//! * the kind of resource `TagResource` names (an approval team and an identity source,
//!   the two resources this service's tag reads resolve), and an ARN no resource carries
//!   at all, which is decided BEFORE the keys are and so answers 404;
//! * whether the resource already carries tags, which a refusal must leave untouched.
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// The tags every resource under test starts with, so a refusal has something to leave
/// alone and an acceptance something to merge into.
fn baseline() -> BTreeMap<String, String> {
    BTreeMap::from([
        ("env".to_string(), "test".to_string()),
        ("owner".to_string(), "platform".to_string()),
    ])
}

/// A `CreateApprovalTeam` body every modelled constraint accepts, attaching the policy
/// version a fresh store seeds and carrying [`baseline`] as its tags.
fn team_body(name: &str) -> Value {
    json!({
        "Name": name,
        "Description": "reserved namespace table",
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
        "Tags": tag_object(&baseline()),
    })
}

/// A `CreateIdentitySource` body every modelled constraint accepts, carrying
/// [`baseline`]. `InstanceArn` satisfies `IdcInstanceArn`'s pattern
/// `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`.
fn identity_source_body() -> Value {
    json!({
        "IdentitySourceParameters": {
            "IamIdentityCenter": {
                "InstanceArn": "arn:aws:sso:::instance/ssoins-1234567890abcdef",
                "Region": REGION,
            }
        },
        "Tags": tag_object(&baseline()),
    })
}

/// A tag map as the `Tags` member's JSON object.
fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// `PUT /tags/{ResourceArn}` — `TagResource`.
async fn tag_resource(harness: &Harness, arn: &str, tags: &BTreeMap<String, String>) -> (u16, Value) {
    harness
        .call(
            "PUT",
            &format!("/tags/{}", encode_label(arn)),
            Some(json!({ "Tags": tag_object(tags) })),
        )
        .await
}

/// `GET /tags/{ResourceArn}` — the tags `ListTagsForResource` reports.
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

/// `keys` as a tag map, every key carrying a value derived from it so a written key is
/// identifiable in the read-back map.
fn map_of(keys: &[&str]) -> BTreeMap<String, String> {
    keys.iter()
        .map(|key| ((*key).to_string(), format!("value-of-{key}")))
        .collect()
}

/// The multi-key table: the keys one request carries, and whether the namespace rule
/// refuses it. Rows differ along how many keys the request carries, how many of them are
/// reserved and where among the others they sit.
///
/// The reserved spellings come from the namespace rule itself
/// ([`state::RESERVED_TAG_KEY_PREFIX`], case-insensitive at the start of the key); the
/// unreserved ones include keys that merely resemble the prefix. `TagKey` is
/// `length(min: 1, max: 128)` with no modelled pattern, so every row reaches the handler.
fn multi_key_rows() -> Vec<(&'static str, Vec<&'static str>, bool)> {
    vec![
        ("one reserved key alone", vec!["aws:owner"], true),
        (
            "a reserved key before conforming keys",
            vec!["aws:cloudformation:stack-name", "team", "stage"],
            true,
        ),
        (
            "a reserved key after conforming keys",
            vec!["team", "stage", "aws:owner"],
            true,
        ),
        (
            "a reserved key between conforming keys",
            vec!["team", "aws:owner", "stage"],
            true,
        ),
        (
            "two reserved keys among conforming keys",
            vec!["aws:owner", "team", "AWS:Cost-Center"],
            true,
        ),
        (
            "a reserved key spelled in another case, among conforming keys",
            vec!["Aws:owner", "team"],
            true,
        ),
        (
            "the bare reserved prefix among conforming keys",
            vec![state::RESERVED_TAG_KEY_PREFIX, "team"],
            true,
        ),
        (
            "every key reserved",
            vec!["aws:owner", "aws:created-by"],
            true,
        ),
        ("one conforming key", vec!["team"], false),
        (
            "several conforming keys",
            vec!["team", "stage", "cost-center"],
            false,
        ),
        (
            "keys that only resemble the prefix",
            vec!["awsome", "aws", "my:aws:thing", " aws:owner"],
            false,
        ),
    ]
}

/// One kind of resource this service lets a caller tag: how to create it and which member
/// of the create response carries its ARN.
struct Taggable {
    kind: &'static str,
    uri: &'static str,
    arn_member: &'static str,
}

/// Create one resource of `taggable`'s kind, tagged with [`baseline`], and return its ARN.
async fn create_taggable(harness: &Harness, taggable: &Taggable, name: &str) -> String {
    let body = if taggable.uri == "/approval-teams" {
        team_body(name)
    } else {
        identity_source_body()
    };
    let (status, created) = harness.call("POST", taggable.uri, Some(body)).await;
    assert_eq!(
        status, 200,
        "{}: create returned {status} with {created:?}",
        taggable.kind
    );
    let arn = text(&created, taggable.arn_member);
    assert!(
        !arn.is_empty(),
        "{}: create reported no {}: {created:?}",
        taggable.kind,
        taggable.arn_member
    );
    arn
}

/// `TagResource` refuses a request naming ANY reserved key, whatever else the request
/// carries, and writes nothing — not even the conforming keys of the same map. A request
/// naming none is accepted and every key it carries is reported back.
#[tokio::test]
async fn tagging_refuses_a_map_naming_any_reserved_key_and_writes_none_of_it() {
    for taggable in [
        Taggable {
            kind: "approval team",
            uri: "/approval-teams",
            arn_member: "Arn",
        },
        Taggable {
            kind: "identity source",
            uri: "/identity-sources",
            arn_member: "IdentitySourceArn",
        },
    ] {
        for (index, (row, keys, reserved)) in multi_key_rows().into_iter().enumerate() {
            // A store of its own per row, so what one row writes cannot carry into the
            // next and every row starts from exactly `baseline()`.
            let harness = Harness::new();
            let case = format!("{} / {row}", taggable.kind);
            let arn = create_taggable(&harness, &taggable, &format!("reserved-{index}")).await;
            let before = tags_of(&harness, &arn, &case).await;
            assert_eq!(
                before,
                baseline(),
                "case {case:?}: the created resource reports {before:?}, not the baseline"
            );

            let requested = map_of(&keys);
            let (status, body) = tag_resource(&harness, &arn, &requested).await;
            let listed = tags_of(&harness, &arn, &case).await;

            if reserved {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected HTTP 400 for {keys:?}, got {status} with {body:?}"
                );
                assert_eq!(
                    error_code(&body),
                    "ValidationException",
                    "case {case:?}: expected error code ValidationException, got {body:?}"
                );
                assert!(
                    !text(&body, "Message").is_empty(),
                    "case {case:?}: the refusal carries no Message: {body:?}"
                );
                assert_eq!(
                    listed,
                    baseline(),
                    "case {case:?}: a refused call changed the tags to {listed:?}"
                );
                continue;
            }

            assert_eq!(
                status, 200,
                "case {case:?}: a map naming no reserved key was refused: {body:?}"
            );
            let mut expected = baseline();
            expected.extend(requested.clone());
            assert_eq!(
                listed, expected,
                "case {case:?}: the resource reports {listed:?}, not {expected:?}"
            );
        }
    }
}

/// Whether the resource exists is decided BEFORE the keys are: a reserved key on an ARN
/// no resource carries is `ResourceNotFoundException` (404), so a reserved key never
/// reports a resource that does not exist as a validation problem.
#[tokio::test]
async fn a_reserved_key_on_an_unknown_arn_is_still_not_found() {
    let harness = Harness::new();
    let account = engine::emulator_account_id();
    let rows = [
        (
            "no such approval team",
            format!("arn:aws:mpa:{REGION}:{account}:approval-team/absent-team"),
        ),
        (
            "no such identity source",
            format!("arn:aws:mpa:{REGION}:{account}:identity-source/ssoins-absent"),
        ),
    ];

    for (case, arn) in rows {
        let (status, body) = tag_resource(&harness, &arn, &map_of(&["aws:owner", "team"])).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected HTTP 404 for {arn}, got {status} with {body:?}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected error code ResourceNotFoundException, got {body:?}"
        );
    }
}

/// A refused request consumes nothing: after a reserved key is refused, the same
/// conforming keys are accepted and the resource reports exactly the merged set — so the
/// refusal neither wrote a key nor spent one of the resource's tag slots.
#[tokio::test]
async fn a_refusal_leaves_the_conforming_keys_writable() {
    let harness = Harness::new();
    let taggable = Taggable {
        kind: "approval team",
        uri: "/approval-teams",
        arn_member: "Arn",
    };
    let arn = create_taggable(&harness, &taggable, "reserved-then-conforming").await;
    let case = "a refused map, then its conforming part";

    let refused = map_of(&["aws:owner", "team", "stage"]);
    let (status, body) = tag_resource(&harness, &arn, &refused).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 for {refused:?}, got {status} with {body:?}"
    );

    let conforming = map_of(&["team", "stage"]);
    let (status, body) = tag_resource(&harness, &arn, &conforming).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the conforming keys were refused after the refusal: {body:?}"
    );

    let listed = tags_of(&harness, &arn, case).await;
    let mut expected = baseline();
    expected.extend(conforming);
    assert_eq!(
        listed, expected,
        "case {case:?}: the resource reports {listed:?}, not {expected:?}"
    );
}
