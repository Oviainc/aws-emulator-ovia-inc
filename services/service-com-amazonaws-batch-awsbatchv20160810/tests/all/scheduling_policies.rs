// SPDX-License-Identifier: CC0-1.0

//! `CreateSchedulingPolicy` and the reader that proves it persisted, over the wire.
//!
//! The rules under test, each over a SET of inputs rather than one recorded request:
//!
//! * a create persists every member the request carried and `DescribeSchedulingPolicies`
//!   reads it back unchanged — the name, the policy (fair-share or quota-share) and the
//!   tags;
//! * the name is the policy's identity: the ARN is
//!   `arn:aws:batch:<region>:<account>:scheduling-policy/<name>`, minted once and reported
//!   identically by every later read, and re-creating a name is refused with the modelled
//!   `ClientException` (HTTP 400) instead of overwriting;
//! * `name` is accepted only in the charset and length the member documents, and refused
//!   with a `ClientException` outside it;
//! * a request carries a `fairsharePolicy` OR a `quotaSharePolicy`, never both;
//! * every bound the policy members document is enforced at the bound and refused one past
//!   it (`shareDecaySeconds`, `computeReservation`, `weightFactor`, the `shareIdentifier`
//!   length and the `shareDistribution` entry count), as are the `shareIdentifier`
//!   charset, the no-overlap rule between share identifiers and the
//!   `idleResourceAssignmentStrategy` value set;
//! * `DescribeSchedulingPolicies` is a reporting operation: it collapses duplicates, skips
//!   an entry it holds nothing for, refuses more than the documented 100 entries, and two
//!   reads at the same instant answer identically.

use serde_json::{json, Value};

use service_com_amazonaws_batch_awsbatchv20160810::state;

use crate::job_lifecycle::{arn, Batch};

/// One create request and what it must read back.
#[derive(Debug, Clone)]
struct Case {
    /// What the row varies: named in every assertion.
    label: &'static str,
    name: String,
    /// `fairsharePolicy` as the request carries it, or `None` to leave the member out.
    fairshare_policy: Option<Value>,
    /// `quotaSharePolicy` as the request carries it, or `None` to leave the member out.
    quota_share_policy: Option<Value>,
    /// `tags`, or `None` to leave the member out.
    tags: Option<Value>,
}

/// The create body for `case`, with every member the row does not carry left out.
fn create_body(case: &Case) -> Value {
    let mut body = json!({"name": case.name});
    if let Some(policy) = case.fairshare_policy.as_ref() {
        body["fairsharePolicy"] = policy.clone();
    }
    if let Some(policy) = case.quota_share_policy.as_ref() {
        body["quotaSharePolicy"] = policy.clone();
    }
    if let Some(tags) = case.tags.as_ref() {
        body["tags"] = tags.clone();
    }
    body
}

/// The table: rows differ along what the rules generalise over — which policy kind the
/// request carries, which of its optional members are present, how many share identifiers
/// and of which form (exact or `<prefix>*`), which weight factors, and whether tags are
/// carried. Weight factors are dyadic fractions so the comparison is exact.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no policy of either kind, no tags",
            name: "sp-bare".to_string(),
            fairshare_policy: None,
            quota_share_policy: None,
            tags: None,
        },
        Case {
            label: "empty fairsharePolicy",
            name: "sp_empty_fairshare".to_string(),
            fairshare_policy: Some(json!({})),
            quota_share_policy: None,
            tags: None,
        },
        Case {
            label: "fairsharePolicy with decay and reservation only",
            name: "SP-Decay-1".to_string(),
            fairshare_policy: Some(json!({"shareDecaySeconds": 600, "computeReservation": 25})),
            quota_share_policy: None,
            tags: Some(json!({"team": "research"})),
        },
        Case {
            label: "fairsharePolicy with one exact share identifier",
            name: "sp-one-share".to_string(),
            fairshare_policy: Some(json!({
                "shareDecaySeconds": 0,
                "computeReservation": 0,
                "shareDistribution": [{"shareIdentifier": "UserA", "weightFactor": 0.125}],
            })),
            quota_share_policy: None,
            tags: None,
        },
        Case {
            label: "fairsharePolicy with three shares, prefixes and a default weight",
            name: "sp-three-shares".to_string(),
            fairshare_policy: Some(json!({
                "shareDecaySeconds": 3600,
                "computeReservation": 99,
                "shareDistribution": [
                    {"shareIdentifier": "teamA*", "weightFactor": 1.0},
                    {"shareIdentifier": "teamB1", "weightFactor": 8.5},
                    {"shareIdentifier": "teamC"},
                ],
            })),
            quota_share_policy: None,
            tags: Some(json!({"team": "research", "cost-centre": "42"})),
        },
        Case {
            label: "fairsharePolicy with an empty shareDistribution",
            name: "sp-no-shares".to_string(),
            fairshare_policy: Some(json!({"shareDistribution": []})),
            quota_share_policy: None,
            tags: None,
        },
        Case {
            label: "quotaSharePolicy",
            name: "sp-quota-share".to_string(),
            fairshare_policy: None,
            // The only `QuotaShareIdleResourceAssignmentStrategy` the model declares.
            quota_share_policy: Some(json!({"idleResourceAssignmentStrategy": "FIFO"})),
            tags: Some(json!({"unicode-ключ": "значение-✅"})),
        },
        Case {
            label: "longest accepted name",
            name: "a".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN),
            fairshare_policy: Some(json!({"computeReservation": 1})),
            quota_share_policy: None,
            tags: None,
        },
    ]
}

/// The one `SchedulingPolicyDetail` for `reference`.
async fn describe_one(batch: &Batch, reference: &str) -> Value {
    let body = batch
        .ok(
            "/v1/describeschedulingpolicies",
            json!({"arns": [reference]}),
        )
        .await;
    let found = body["schedulingPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        found.len(),
        1,
        "case {reference:?}: expected exactly one schedulingPolicy, got {found:?}"
    );
    found[0].clone()
}

/// Create `name` with a minimal fair-share policy, requiring a 200.
async fn create_minimal(batch: &Batch, name: &str) -> Value {
    batch
        .ok("/v1/createschedulingpolicy", json!({"name": name}))
        .await
}

/// `count` distinct, non-overlapping `shareDistribution` entries: `s0`, `s1`, … Each is
/// alphanumeric and none is a prefix ENTRY, so no pair overlaps however long the list is.
fn share_distribution(count: usize) -> Value {
    Value::Array(
        (0..count)
            .map(|index| json!({"shareIdentifier": format!("s{index}"), "weightFactor": 1.0}))
            .collect(),
    )
}

#[tokio::test]
async fn create_persists_every_member_and_describe_reads_it_back() {
    let batch = Batch::new();

    for case in cases() {
        let created = batch
            .ok("/v1/createschedulingpolicy", create_body(&case))
            .await;
        let expected_arn = arn(&format!("scheduling-policy/{}", case.name));

        assert_eq!(
            created["name"].as_str(),
            Some(case.name.as_str()),
            "case {:?}: expected the created name back, got {created}",
            case.label
        );
        assert_eq!(
            created["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: expected ARN {expected_arn}, got {created}",
            case.label
        );

        let detail = describe_one(&batch, &expected_arn).await;
        assert_eq!(
            detail["name"].as_str(),
            Some(case.name.as_str()),
            "case {:?}: the read-back name must be the created one, got {detail}",
            case.label
        );
        assert_eq!(
            detail["arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {:?}: the read-back ARN must be the minted one, got {detail}",
            case.label
        );
        match case.fairshare_policy.as_ref() {
            Some(policy) => assert_eq!(
                &detail["fairsharePolicy"],
                policy,
                "case {:?}: expected the submitted fairsharePolicy back, got {detail}",
                case.label
            ),
            None => assert!(
                detail["fairsharePolicy"].is_null(),
                "case {:?}: a create with no fairsharePolicy must report none, got {detail}",
                case.label
            ),
        }
        match case.quota_share_policy.as_ref() {
            Some(policy) => assert_eq!(
                &detail["quotaSharePolicy"],
                policy,
                "case {:?}: expected the submitted quotaSharePolicy back, got {detail}",
                case.label
            ),
            None => assert!(
                detail["quotaSharePolicy"].is_null(),
                "case {:?}: a create with no quotaSharePolicy must report none, got {detail}",
                case.label
            ),
        }
        match case.tags.as_ref() {
            Some(tags) => assert_eq!(
                &detail["tags"],
                tags,
                "case {:?}: expected the submitted tags back, got {detail}",
                case.label
            ),
            None => assert!(
                detail["tags"].is_null(),
                "case {:?}: a create with no tags must report none, got {detail}",
                case.label
            ),
        }

        // A read changes nothing: the next read at the same instant answers identically.
        let again = describe_one(&batch, &expected_arn).await;
        assert_eq!(
            again, detail,
            "case {:?}: two reads must agree, got {again}",
            case.label
        );
    }
}

#[tokio::test]
async fn recreating_a_name_is_refused_and_does_not_overwrite() {
    let batch = Batch::new();

    for case in cases() {
        batch
            .ok("/v1/createschedulingpolicy", create_body(&case))
            .await;
        let policy_arn = arn(&format!("scheduling-policy/{}", case.name));
        let stored = describe_one(&batch, &policy_arn).await;

        // A second create of the same NAME, carrying different members: it must be
        // refused, and the stored record must be untouched.
        let clashing = Case {
            fairshare_policy: Some(json!({"shareDecaySeconds": 42, "computeReservation": 7})),
            quota_share_policy: None,
            tags: Some(json!({"overwritten": "yes"})),
            ..case.clone()
        };
        let (status, body) = batch
            .call("/v1/createschedulingpolicy", create_body(&clashing))
            .await;
        assert_eq!(
            status, 400,
            "case {:?}: re-creating a name must answer 400, got {status} {body}",
            case.label
        );

        let after = describe_one(&batch, &policy_arn).await;
        assert_eq!(
            after, stored,
            "case {:?}: the refused create must not overwrite, got {after}",
            case.label
        );
    }
}

#[tokio::test]
async fn a_name_outside_the_documented_charset_is_refused() {
    let batch = Batch::new();

    // `name`'s doc: "It can be up to 128 letters long. It can contain uppercase and
    // lowercase letters, numbers, hyphens (-), and underscores (_)." The accepted rows
    // cover each published form including the length bound; the refused rows are one past
    // it and the characters outside the charset.
    let accepted: &[(&str, String)] = &[
        ("letters", "policy".to_string()),
        ("digits", "policy2024".to_string()),
        ("hyphen", "my-policy".to_string()),
        ("underscore", "my_policy".to_string()),
        ("mixed case", "MyPolicy-01_x".to_string()),
        ("single character", "p".to_string()),
        (
            "at the length bound",
            "b".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN),
        ),
    ];
    let refused: &[(&str, String)] = &[
        ("empty", String::new()),
        (
            "one past the length bound",
            "c".repeat(state::MAX_SCHEDULING_POLICY_NAME_LEN + 1),
        ),
        ("space", "my policy".to_string()),
        ("dot", "my.policy".to_string()),
        ("slash", "my/policy".to_string()),
        ("colon", "my:policy".to_string()),
        ("non-ASCII letter", "policy-é".to_string()),
    ];

    for (label, name) in accepted {
        let body = create_minimal(&batch, name).await;
        assert_eq!(
            body["name"].as_str(),
            Some(name.as_str()),
            "case {label:?}: a documented name must be accepted, got {body}"
        );
    }

    for (label, name) in refused {
        let (status, body) = batch
            .call("/v1/createschedulingpolicy", json!({"name": name}))
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: a name outside the documented charset must answer 400, got {status} {body}"
        );
        // Nothing was stored for it.
        let found = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [arn(&format!("scheduling-policy/{name}"))]}),
            )
            .await;
        assert_eq!(
            found["schedulingPolicies"],
            json!([]),
            "case {label:?}: a refused create must persist nothing, got {found}"
        );
    }
}

#[tokio::test]
async fn a_policy_of_both_kinds_is_refused() {
    let batch = Batch::new();

    // "Only one of fairsharePolicy or quotaSharePolicy can be set"
    // (`CreateSchedulingPolicy$fairsharePolicy`). Rows differ in what each kind carries,
    // so the refusal is about the PAIR being present and not about either value.
    let rows: &[(&str, Value, Value)] = &[
        ("both empty-ish", json!({}), json!({"idleResourceAssignmentStrategy": "FIFO"})),
        (
            "fair-share with a distribution beside a quota share",
            json!({"shareDistribution": [{"shareIdentifier": "UserA", "weightFactor": 1.0}]}),
            json!({"idleResourceAssignmentStrategy": "FIFO"}),
        ),
        (
            "fair-share with decay beside a quota share",
            json!({"shareDecaySeconds": 60}),
            json!({"idleResourceAssignmentStrategy": "FIFO"}),
        ),
    ];

    for (index, (label, fairshare, quota_share)) in rows.iter().enumerate() {
        let name = format!("sp-both-{index}");
        let (status, body) = batch
            .call(
                "/v1/createschedulingpolicy",
                json!({
                    "name": name,
                    "fairsharePolicy": fairshare,
                    "quotaSharePolicy": quota_share,
                }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {label:?}: a policy of both kinds must answer 400, got {status} {body}"
        );
        let found = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [arn(&format!("scheduling-policy/{name}"))]}),
            )
            .await;
        assert_eq!(
            found["schedulingPolicies"],
            json!([]),
            "case {label:?}: a refused create must persist nothing, got {found}"
        );
    }
}

#[tokio::test]
async fn every_documented_policy_bound_is_enforced() {
    let batch = Batch::new();

    // Each row is a `fairsharePolicy` (or `quotaSharePolicy`) and whether the service must
    // accept it. The bounds are the members' own docs:
    //  * `shareDecaySeconds`: "A value of zero (0) ... The maximum supported value is
    //    604800 (1 week)";
    //  * `computeReservation`: "The minimum value is 0 and the maximum value is 99";
    //  * `weightFactor`: "The smallest supported value is 0.0001, and the largest
    //    supported value is 999.9999";
    //  * `shareIdentifier`: "limited to 255 alphanumeric characters, and can be followed
    //    by an asterisk (*)", and "The list of share identifiers in a fair-share policy
    //    can't overlap";
    //  * `idleResourceAssignmentStrategy`: the model's own enum.
    let decay_min = state::MIN_SHARE_DECAY_SECONDS;
    let decay_max = state::MAX_SHARE_DECAY_SECONDS;
    let reservation_max = state::MAX_COMPUTE_RESERVATION;
    let longest_share = "s".repeat(state::MAX_SHARE_IDENTIFIER_LEN);
    let too_long_share = "s".repeat(state::MAX_SHARE_IDENTIFIER_LEN + 1);

    let rows: Vec<(&str, Value, bool)> = vec![
        ("shareDecaySeconds at the minimum", json!({"fairsharePolicy": {"shareDecaySeconds": decay_min}}), true),
        ("shareDecaySeconds at the maximum", json!({"fairsharePolicy": {"shareDecaySeconds": decay_max}}), true),
        ("shareDecaySeconds one below the minimum", json!({"fairsharePolicy": {"shareDecaySeconds": decay_min - 1}}), false),
        ("shareDecaySeconds one past the maximum", json!({"fairsharePolicy": {"shareDecaySeconds": decay_max + 1}}), false),
        ("computeReservation at the minimum", json!({"fairsharePolicy": {"computeReservation": 0}}), true),
        ("computeReservation at the maximum", json!({"fairsharePolicy": {"computeReservation": reservation_max}}), true),
        ("computeReservation one below the minimum", json!({"fairsharePolicy": {"computeReservation": -1}}), false),
        ("computeReservation one past the maximum", json!({"fairsharePolicy": {"computeReservation": reservation_max + 1}}), false),
        (
            "weightFactor at the smallest supported value",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "A", "weightFactor": state::MIN_WEIGHT_FACTOR}]}}),
            true,
        ),
        (
            "weightFactor at the largest supported value",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "A", "weightFactor": state::MAX_WEIGHT_FACTOR}]}}),
            true,
        ),
        (
            "weightFactor below the smallest supported value",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "A", "weightFactor": 0.00001}]}}),
            false,
        ),
        (
            "weightFactor past the largest supported value",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "A", "weightFactor": 1000.0}]}}),
            false,
        ),
        (
            "weightFactor of zero",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "A", "weightFactor": 0.0}]}}),
            false,
        ),
        (
            "shareIdentifier at the length bound",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": longest_share}]}}),
            true,
        ),
        (
            "shareIdentifier one past the length bound",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": too_long_share}]}}),
            false,
        ),
        (
            "shareIdentifier prefix at the length bound",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": format!("{longest_share}*")}]}}),
            true,
        ),
        (
            "shareIdentifier outside the alphanumeric charset",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "User-A"}]}}),
            false,
        ),
        (
            "empty shareIdentifier",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": ""}]}}),
            false,
        ),
        (
            "two disjoint share identifiers",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "UserA1"}, {"shareIdentifier": "UserB1"}]}}),
            true,
        ),
        (
            "two disjoint prefixes",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "UserA*"}, {"shareIdentifier": "UserB*"}]}}),
            true,
        ),
        (
            "a prefix overlapping an exact identifier",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "UserA*"}, {"shareIdentifier": "UserA1"}]}}),
            false,
        ),
        (
            "an exact identifier repeated",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "UserA"}, {"shareIdentifier": "UserA"}]}}),
            false,
        ),
        (
            "a prefix containing another prefix",
            json!({"fairsharePolicy": {"shareDistribution": [{"shareIdentifier": "User*"}, {"shareIdentifier": "UserA*"}]}}),
            false,
        ),
        (
            "shareDistribution at the documented entry maximum",
            json!({"fairsharePolicy": {"shareDistribution": share_distribution(state::MAX_SHARE_DISTRIBUTION_ENTRIES)}}),
            true,
        ),
        (
            "shareDistribution one past the documented entry maximum",
            json!({"fairsharePolicy": {"shareDistribution": share_distribution(state::MAX_SHARE_DISTRIBUTION_ENTRIES + 1)}}),
            false,
        ),
        (
            "idleResourceAssignmentStrategy the model declares",
            json!({"quotaSharePolicy": {"idleResourceAssignmentStrategy": "FIFO"}}),
            true,
        ),
        (
            "idleResourceAssignmentStrategy the model does not declare",
            json!({"quotaSharePolicy": {"idleResourceAssignmentStrategy": "LIFO"}}),
            false,
        ),
    ];

    for (index, (label, policy, accepted)) in rows.iter().enumerate() {
        let name = format!("sp-bound-{index}");
        let mut body = json!({"name": name});
        for (member, value) in policy.as_object().expect("the row is an object") {
            body[member] = value.clone();
        }
        let (status, response) = batch.call("/v1/createschedulingpolicy", body).await;
        let expected = if *accepted { 200 } else { 400 };
        assert_eq!(
            status, expected,
            "case {label:?}: expected {expected}, got {status} {response}"
        );

        let found = batch
            .ok(
                "/v1/describeschedulingpolicies",
                json!({"arns": [arn(&format!("scheduling-policy/{name}"))]}),
            )
            .await;
        let stored = found["schedulingPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            stored.len(),
            usize::from(*accepted),
            "case {label:?}: expected {} stored policy, got {stored:?}",
            usize::from(*accepted)
        );
    }
}

#[tokio::test]
async fn describe_collapses_duplicates_skips_unknown_and_bounds_its_input() {
    let batch = Batch::new();

    let names: Vec<String> = (0..5).map(|index| format!("sp-read-{index}")).collect();
    for name in &names {
        create_minimal(&batch, name).await;
    }
    let arn_of = |name: &str| arn(&format!("scheduling-policy/{name}"));

    let rows: &[(&str, Vec<String>, usize)] = &[
        ("one ARN", vec![arn_of(&names[0])], 1),
        ("by name", vec![names[0].clone()], 1),
        (
            "the same policy named twice collapses",
            vec![arn_of(&names[1]), arn_of(&names[1])],
            1,
        ),
        (
            "name and ARN of the SAME policy collapse",
            vec![names[1].clone(), arn_of(&names[1])],
            1,
        ),
        (
            "three distinct ARNs",
            vec![arn_of(&names[2]), arn_of(&names[3]), arn_of(&names[4])],
            3,
        ),
        ("an unknown ARN is skipped", vec![arn_of("sp-absent")], 0),
        (
            "an unknown ARN beside a known one",
            vec![arn_of("sp-absent"), arn_of(&names[0])],
            1,
        ),
        ("no entries at all", Vec::new(), 0),
        (
            "every entry, at the documented maximum",
            {
                let mut entries: Vec<String> = names.iter().map(|name| arn_of(name)).collect();
                while entries.len() < state::MAX_DESCRIBE_FILTER_ENTRIES {
                    entries.push(arn_of("sp-absent"));
                }
                entries
            },
            names.len(),
        ),
    ];

    for (label, references, expected) in rows {
        let body = batch
            .ok("/v1/describeschedulingpolicies", json!({"arns": references}))
            .await;
        let found = body["schedulingPolicies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            found.len(),
            *expected,
            "case {label:?}: expected {expected} policy/policies, got {found:?}"
        );
    }

    // "A list of up to 100 scheduling policy Amazon Resource Name (ARN) entries" (`arns`).
    let too_many: Vec<String> = (0..=state::MAX_DESCRIBE_FILTER_ENTRIES)
        .map(|index| arn_of(&format!("sp-read-{index}")))
        .collect();
    let (status, body) = batch
        .call("/v1/describeschedulingpolicies", json!({"arns": too_many}))
        .await;
    assert_eq!(
        status, 400,
        "case one past the documented arns maximum: expected 400, got {status} {body}"
    );
}

#[tokio::test]
async fn the_store_is_bounded_and_a_create_past_the_cap_is_refused() {
    let batch = Batch::new();

    for index in 0..state::limits::MAX_SCHEDULING_POLICIES {
        let name = format!("sp-cap-{index}");
        let (status, body) = batch
            .call("/v1/createschedulingpolicy", json!({"name": name}))
            .await;
        assert_eq!(
            status, 200,
            "case {index}: a create inside the cap must succeed, got {status} {body}"
        );
    }

    let (status, body) = batch
        .call("/v1/createschedulingpolicy", json!({"name": "sp-cap-over"}))
        .await;
    assert_eq!(
        status, 400,
        "case past the cap: expected 400, got {status} {body}"
    );
}
