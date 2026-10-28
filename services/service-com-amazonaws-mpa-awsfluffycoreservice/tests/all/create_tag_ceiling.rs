// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests pinning the create-time per-resource tag ceiling on BOTH paths that
//! write a resource's tags at creation: `CreateApprovalTeam` and `CreateIdentitySource`.
//!
//! The rule both creates hold: a resource carries at most
//! [`state::MAX_TAGS_PER_RESOURCE`] tags (the ceiling `TagResource` reports with
//! `TooManyTagsException`), so a create whose `Tags` names more than that is refused with
//! the `ValidationException` (400) a create declares, with the same sentence whichever
//! resource is being created, and stores nothing — while a create at or below the ceiling
//! is accepted and reports every key it carried.
//!
//! The table's rows differ along the number of tags the request carries: none, one, each
//! side of the ceiling, the ceiling itself and well past it. The ceiling comes from
//! `state::MAX_TAGS_PER_RESOURCE`, whose own doc comment names the modelled member it is
//! corroborated by (`UntagResource$TagKeys`' `length(min: 0, max: 50)`).
//!
//! Every request goes over the wire through the `MultiServiceRouter` the harness in
//! `approval_team_lifecycle.rs` mounts, so the route, the URI bindings, the JSON envelope
//! and the HTTP status are all exercised.

use std::collections::BTreeMap;

use serde_json::{json, Value};
use service_com_amazonaws_mpa_awsfluffycoreservice::state;

use crate::approval_team_lifecycle::{encode_label, error_code, text, Harness, REGION};

/// `count` tags, each value derived from its key so a written key is identifiable in the
/// read-back map.
fn tags(count: usize) -> BTreeMap<String, String> {
    (1..=count)
        .map(|n| (format!("key{n:04}"), format!("value{n:04}")))
        .collect()
}

/// A tag map as the `Tags` member's JSON object.
fn tag_object(tags: &BTreeMap<String, String>) -> Value {
    Value::Object(
        tags.iter()
            .map(|(key, value)| (key.clone(), Value::String(value.clone())))
            .collect(),
    )
}

/// One create path under test: where it posts, which member of its response carries the
/// new resource's ARN, and the body it takes for a given tag map.
struct CreatePath {
    kind: &'static str,
    uri: &'static str,
    arn_member: &'static str,
}

impl CreatePath {
    /// The two create paths that write a resource's tags as the resource is minted.
    fn all() -> [Self; 2] {
        [
            Self {
                kind: "CreateApprovalTeam",
                uri: "/approval-teams",
                arn_member: "Arn",
            },
            Self {
                kind: "CreateIdentitySource",
                uri: "/identity-sources",
                arn_member: "IdentitySourceArn",
            },
        ]
    }

    /// A request body every modelled constraint of this path accepts, carrying `tags`.
    fn body(&self, name: &str, tags: &BTreeMap<String, String>) -> Value {
        if self.arn_member == "Arn" {
            // `Name` matches `^[a-zA-Z0-9._-]+$`; the attached policy version is one a
            // fresh store seeds.
            json!({
                "Name": name,
                "Description": "create-time tag ceiling table",
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
        } else {
            // `InstanceArn` satisfies `IdcInstanceArn`'s pattern
            // `^arn:.+:sso:::instance/(?:sso)?ins-[a-zA-Z0-9-.]{16}$`.
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
    }
}

/// How many resources of each kind the store holds.
fn stored_counts(harness: &Harness) -> (usize, usize) {
    let store = state::service_state(&*harness.engine).expect("the service store");
    let guard = store.lock();
    (guard.approval_teams.len(), guard.identity_sources.len())
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

/// Both creates hold the one ceiling: at or below it the request is accepted and every key
/// it carried is readable back; past it the request earns `ValidationException` (400) and
/// nothing is stored. The refusal both creates raise is the SAME sentence for the same tag
/// count, which is what a single rule serving both paths means.
#[tokio::test]
async fn both_creates_hold_the_one_per_resource_tag_ceiling() {
    let max = state::MAX_TAGS_PER_RESOURCE;

    // (case, how many tags the request carries, accepted)
    let rows: [(&str, usize, bool); 6] = [
        ("no tags", 0, true),
        ("one tag", 1, true),
        ("one below the maximum", max - 1, true),
        ("exactly the maximum", max, true),
        ("one past the maximum", max + 1, false),
        ("far past the maximum", max * 2, false),
    ];

    // The refusal sentence each path raised, per tag count, so the two paths can be
    // compared with each other rather than with a sentence spelled out here.
    let mut refusals: BTreeMap<usize, Vec<(&str, String)>> = BTreeMap::new();

    for path in CreatePath::all() {
        for (index, (row, count, accepted)) in rows.into_iter().enumerate() {
            // A store of its own per row: nothing a previous row wrote can carry in, and
            // a refused create must leave the store exactly as it found it.
            let harness = Harness::new();
            let case = format!("{} / {row}", path.kind);
            let requested = tags(count);
            let before = stored_counts(&harness);

            let (status, body) = harness
                .call(
                    "POST",
                    path.uri,
                    Some(path.body(&format!("tag-ceiling-{index}"), &requested)),
                )
                .await;

            if accepted {
                assert_eq!(
                    status, 200,
                    "case {case:?}: a create carrying {count} tags returned {status} with \
                     {body:?}"
                );
                let arn = text(&body, path.arn_member);
                let listed = tags_of(&harness, &arn, &case).await;
                assert_eq!(
                    listed, requested,
                    "case {case:?}: the created resource reports {listed:?}, not the \
                     {count} tags the create carried"
                );
                continue;
            }

            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400 for {count} tags, got {status} with {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected error code ValidationException, got {body:?}"
            );
            let message = text(&body, "Message");
            assert!(
                message.contains(&count.to_string()) && message.contains(&max.to_string()),
                "case {case:?}: the refusal must name the {count} tags asked for and the \
                 maximum of {max}, it reads {message:?}"
            );
            assert_eq!(
                stored_counts(&harness),
                before,
                "case {case:?}: a refused create changed what the store holds"
            );
            refusals
                .entry(count)
                .or_default()
                .push((path.kind, message));
        }
    }

    // One rule, one sentence: for every refused count both paths answered, each path's
    // message is the other's.
    for (count, raised) in &refusals {
        let (first_kind, first) = &raised[0];
        for (kind, message) in &raised[1..] {
            assert_eq!(
                message, first,
                "case {count} tags: {kind} refuses with {message:?}, {first_kind} with \
                 {first:?}"
            );
        }
        assert_eq!(
            raised.len(),
            CreatePath::all().len(),
            "case {count} tags: only {} of the create paths refused it",
            raised.len()
        );
    }
}
