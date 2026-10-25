// SPDX-License-Identifier: CC0-1.0

//! `SimulatePrincipalPolicy`, end to end over the awsQuery wire.
//!
//! The rule under test: the simulation reports, for each action the request names and
//! each resource it names, what the identity-based policies in force on the named
//! principal decide — `allowed` when something about the request grants it,
//! `explicitDeny` when a statement about the request denies it, and `implicitDeny`
//! when nothing does either. It is a REPORT: every document it reads was persisted by
//! an earlier call (`PutUserPolicy`, `CreatePolicy` + `AttachRolePolicy`), and
//! simulating a call neither performs it nor changes what a later caller sees.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! effect, the action, the resource the statement is scoped to, how many actions and
//! resources one request asks about, where the documents come from (inline, attached
//! managed, supplied in the request, a simulated permissions boundary), whether a
//! condition's value is supplied, and the page size.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// The three values the model's `PolicyEvaluationDecisionType` enum declares
/// (`smithy_query(query="shape",shape="PolicyEvaluationDecisionType")`).
const ALLOWED: &str = "allowed";
const EXPLICIT_DENY: &str = "explicitDeny";
const IMPLICIT_DENY: &str = "implicitDeny";

/// Create a user carrying `policies`, answering the ARN the account holds it under.
async fn principal(
    router: &MultiServiceRouter,
    case: &str,
    user_name: &str,
    policies: &[(&str, &str)],
) -> String {
    let (status, body) = call(router, "CreateUser", &[("UserName", user_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser must succeed, got {status}: {body}"
    );
    let arn = element(&body, "Arn");
    for (policy_name, document) in policies {
        let (status, put) = call(
            router,
            "PutUserPolicy",
            &[
                ("UserName", user_name),
                ("PolicyName", policy_name),
                ("PolicyDocument", document),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: PutUserPolicy {policy_name:?} must succeed, got {status}: {put}"
        );
    }
    arn
}

/// One `SimulatePrincipalPolicy` call, with the list members the query protocol
/// flattens into `<member>.N` parameters.
async fn simulate(
    router: &MultiServiceRouter,
    policy_source_arn: &str,
    actions: &[&str],
    resources: &[&str],
    extra: &[(String, String)],
) -> (u16, String) {
    let mut owned: Vec<(String, String)> = vec![(
        "PolicySourceArn".to_string(),
        policy_source_arn.to_string(),
    )];
    for (position, action) in actions.iter().enumerate() {
        owned.push((
            format!("ActionNames.member.{}", position + 1),
            (*action).to_string(),
        ));
    }
    for (position, resource) in resources.iter().enumerate() {
        owned.push((
            format!("ResourceArns.member.{}", position + 1),
            (*resource).to_string(),
        ));
    }
    owned.extend(extra.iter().cloned());
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call(router, "SimulatePrincipalPolicy", &params).await
}

/// The `(EvalActionName, EvalDecision)` pairs a simulation reports, in document
/// order — one per action the page carries.
fn reported(body: &str) -> Vec<(String, String)> {
    let actions = elements(body, "EvalActionName");
    let decisions = elements(body, "EvalDecision");
    assert_eq!(
        actions.len(),
        decisions.len(),
        "every evaluation result must carry one EvalDecision: {body}"
    );
    actions.into_iter().zip(decisions).collect()
}

/// One row of the decision table.
struct Case {
    case: &'static str,
    /// The inline policies the simulated principal carries.
    policies: &'static [(&'static str, &'static str)],
    actions: &'static [&'static str],
    resources: &'static [&'static str],
    /// The decision each action must be reported with, in request order.
    expected: &'static [&'static str],
}

/// A grant for the two IAM reads this suite asks about.
const ALLOW_LISTS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:ListGroups","iam:ListRoles"],"Resource":"*"}]}"#;

/// Rows spanning the effects, the action forms (literal, wildcard run, an action no
/// statement mentions), the resource scopes, and the number of actions one request
/// asks about.
const CASES: &[Case] = &[
    Case {
        case: "an Allow naming the action",
        policies: &[("allow-lists", ALLOW_LISTS)],
        actions: &["iam:ListGroups"],
        resources: &[],
        expected: &[ALLOWED],
    },
    Case {
        case: "a Deny naming the action, beside the Allow",
        policies: &[
            ("allow-lists", ALLOW_LISTS),
            (
                "deny-listgroups",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListGroups","Resource":"*"}]}"#,
            ),
        ],
        actions: &["iam:ListGroups"],
        resources: &[],
        expected: &[EXPLICIT_DENY],
    },
    Case {
        case: "the sibling action the Deny does not name",
        policies: &[
            ("allow-lists", ALLOW_LISTS),
            (
                "deny-listgroups",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListGroups","Resource":"*"}]}"#,
            ),
        ],
        actions: &["iam:ListRoles"],
        resources: &[],
        expected: &[ALLOWED],
    },
    Case {
        case: "an action no statement is about",
        policies: &[("allow-lists", ALLOW_LISTS)],
        actions: &["iam:ListUsers"],
        resources: &[],
        expected: &[IMPLICIT_DENY],
    },
    Case {
        case: "a principal carrying no policy at all",
        policies: &[],
        actions: &["iam:ListGroups"],
        resources: &[],
        expected: &[IMPLICIT_DENY],
    },
    Case {
        case: "several actions in one request, decided one by one",
        policies: &[
            ("allow-lists", ALLOW_LISTS),
            (
                "deny-listroles",
                r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListRol*","Resource":"*"}]}"#,
            ),
        ],
        actions: &["iam:ListGroups", "iam:ListRoles", "iam:ListUsers"],
        resources: &[],
        expected: &[ALLOWED, EXPLICIT_DENY, IMPLICIT_DENY],
    },
    Case {
        case: "a statement scoped to the resource the request names",
        policies: &[(
            "allow-one-bucket",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/*"}]}"#,
        )],
        actions: &["s3:GetObject"],
        resources: &["arn:aws:s3:::reports/june.csv"],
        expected: &[ALLOWED],
    },
    Case {
        case: "a statement scoped to another resource",
        policies: &[(
            "allow-one-bucket",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/*"}]}"#,
        )],
        actions: &["s3:GetObject"],
        resources: &["arn:aws:s3:::ledger/june.csv"],
        expected: &[IMPLICIT_DENY],
    },
];

/// RULE: the simulation reports the decision the principal's identity-based policies
/// come to for each action the request names.
#[tokio::test]
async fn the_simulation_reports_the_decision_for_every_action() {
    let router = router();

    for (row, case) in CASES.iter().enumerate() {
        let user_name = format!("sim{row}");
        let arn = principal(&router, case.case, &user_name, case.policies).await;
        let (status, body) = simulate(&router, &arn, case.actions, case.resources, &[]).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected 200, got {status}: {body}",
            case.case
        );
        let expected: Vec<(String, String)> = case
            .actions
            .iter()
            .zip(case.expected)
            .map(|(action, decision)| ((*action).to_string(), (*decision).to_string()))
            .collect();
        assert_eq!(
            reported(&body),
            expected,
            "case {:?}: expected {expected:?}, got {body}",
            case.case
        );
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {:?}: expected a complete page, got {body}",
            case.case
        );
    }
}

/// RULE: every resource the request names is decided, and the per-resource decisions
/// are reported beside the action's own.
#[tokio::test]
async fn every_named_resource_is_decided() {
    let router = router();
    let arn = principal(
        &router,
        "several resources",
        "resources",
        &[(
            "allow-one-bucket",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/*"}]}"#,
        )],
    )
    .await;

    let (status, body) = simulate(
        &router,
        &arn,
        &["s3:GetObject"],
        &[
            "arn:aws:s3:::reports/june.csv",
            "arn:aws:s3:::ledger/june.csv",
        ],
        &[],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        elements(&body, "EvalResourceDecision"),
        vec![ALLOWED.to_string(), IMPLICIT_DENY.to_string()],
        "expected one decision per named resource, in request order: {body}"
    );
    assert_eq!(
        reported(&body),
        vec![("s3:GetObject".to_string(), ALLOWED.to_string())],
        "expected the action's own decision to be the one for the first resource: {body}"
    );
}

/// RULE: `PolicyInputList` policies are simulated BESIDE the principal's own, and a
/// `PolicyExclusionList` entry leaves the policies it names out of the simulation —
/// neither changes which policies are actually attached to the principal.
#[tokio::test]
async fn supplied_and_excluded_policies_change_only_the_simulation() {
    let router = router();
    let deny = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListGroups","Resource":"*"}]}"#;
    let arn = principal(
        &router,
        "supplied and excluded",
        "layers",
        &[("allow-lists", ALLOW_LISTS), ("deny-listgroups", deny)],
    )
    .await;

    // The principal's own policies alone: the Deny decides.
    let (status, body) = simulate(&router, &arn, &["iam:ListGroups"], &[], &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![("iam:ListGroups".to_string(), EXPLICIT_DENY.to_string())],
        "expected the embedded Deny to decide: {body}"
    );

    // Excluding every inline policy leaves no document in force at all.
    let (status, body) = simulate(
        &router,
        &arn,
        &["iam:ListGroups"],
        &[],
        &[(
            "PolicyExclusionList.member.1.PolicyType".to_string(),
            "inline".to_string(),
        )],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![("iam:ListGroups".to_string(), IMPLICIT_DENY.to_string())],
        "expected the excluded inline policies to decide nothing: {body}"
    );

    // Excluding them and supplying an Allow in the request: the supplied document is
    // the only one in force.
    let (status, body) = simulate(
        &router,
        &arn,
        &["iam:ListGroups"],
        &[],
        &[
            (
                "PolicyExclusionList.member.1.PolicyType".to_string(),
                "inline".to_string(),
            ),
            ("PolicyInputList.member.1".to_string(), ALLOW_LISTS.to_string()),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![("iam:ListGroups".to_string(), ALLOWED.to_string())],
        "expected the supplied Allow to decide: {body}"
    );

    // Excluding one inline policy BY NAME leaves the other in force.
    let (status, body) = simulate(
        &router,
        &arn,
        &["iam:ListGroups"],
        &[],
        &[
            (
                "PolicyExclusionList.member.1.InlinePolicyIdentifier.PolicyName".to_string(),
                "deny-listgroups".to_string(),
            ),
            (
                "PolicyExclusionList.member.1.InlinePolicyIdentifier.AttachmentType".to_string(),
                "user".to_string(),
            ),
            (
                "PolicyExclusionList.member.1.InlinePolicyIdentifier.AttachmentName".to_string(),
                "layers".to_string(),
            ),
        ],
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![("iam:ListGroups".to_string(), ALLOWED.to_string())],
        "expected the named exclusion to leave the Allow in force: {body}"
    );

    // Nothing above changed the attachments: the embedded Deny is still what the
    // principal carries, so a simulation with no exclusions decides as it did.
    let (status, body) = simulate(&router, &arn, &["iam:ListGroups"], &[], &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![("iam:ListGroups".to_string(), EXPLICIT_DENY.to_string())],
        "a simulation must not change which policies are attached: {body}"
    );
}

/// RULE: a condition is part of whether its statement applies; the values the request
/// supplies are what it is decided from, and a key the request leaves out is reported
/// as a missing context value rather than guessed at.
#[tokio::test]
async fn a_condition_is_decided_from_the_supplied_context() {
    let router = router();
    let arn = principal(
        &router,
        "conditions",
        "conditional",
        &[(
            "allow-tagged",
            r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListGroups","Resource":"*","Condition":{"StringEquals":{"aws:PrincipalTag/team":"storage"}}}]}"#,
        )],
    )
    .await;

    // No value for the key the condition tests: the statement cannot be decided, so it
    // grants nothing, and the key is reported as missing.
    let (status, body) = simulate(&router, &arn, &["iam:ListGroups"], &[], &[]).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    assert_eq!(
        reported(&body),
        vec![("iam:ListGroups".to_string(), IMPLICIT_DENY.to_string())],
        "expected an undecidable condition to grant nothing: {body}"
    );
    assert!(
        body.contains("<MissingContextValues><member>aws:PrincipalTag/team</member>"),
        "expected the key the condition tests, which the request supplied no value for, \
         to be reported as missing: {body}"
    );

    // Rows spanning the values the condition can be tested against: the one it admits,
    // and one it does not.
    for (value, expected) in [("storage", ALLOWED), ("compute", IMPLICIT_DENY)] {
        let (status, body) = simulate(
            &router,
            &arn,
            &["iam:ListGroups"],
            &[],
            &[
                (
                    "ContextEntries.member.1.ContextKeyName".to_string(),
                    "aws:PrincipalTag/team".to_string(),
                ),
                (
                    "ContextEntries.member.1.ContextKeyValues.member.1".to_string(),
                    value.to_string(),
                ),
                (
                    "ContextEntries.member.1.ContextKeyType".to_string(),
                    "string".to_string(),
                ),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {value:?}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            vec![("iam:ListGroups".to_string(), expected.to_string())],
            "case {value:?}: expected {expected}, got {body}"
        );
    }
}

/// RULE: a simulated permissions boundary sets the MAXIMUM permissions: a request it
/// does not allow is denied even where the identity policies allow it, and the
/// boundary's own verdict is reported.
#[tokio::test]
async fn a_simulated_permissions_boundary_bounds_the_decision() {
    let router = router();
    let arn = principal(
        &router,
        "boundary",
        "bounded",
        &[("allow-lists", ALLOW_LISTS)],
    )
    .await;
    let boundary = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListRoles","Resource":"*"}]}"#;

    // Rows: the action the boundary admits, and the one it does not.
    for (action, expected, allowed_by_boundary) in [
        ("iam:ListRoles", ALLOWED, "true"),
        ("iam:ListGroups", IMPLICIT_DENY, "false"),
    ] {
        let (status, body) = simulate(
            &router,
            &arn,
            &[action],
            &[],
            &[(
                "PermissionsBoundaryPolicyInputList.member.1".to_string(),
                boundary.to_string(),
            )],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {action}: expected 200, got {status}: {body}"
        );
        assert_eq!(
            reported(&body),
            vec![(action.to_string(), expected.to_string())],
            "case {action}: expected {expected}, got {body}"
        );
        assert!(
            elements(&body, "AllowedByPermissionsBoundary")
                .iter()
                .all(|reported| reported == allowed_by_boundary),
            "case {action}: expected AllowedByPermissionsBoundary {allowed_by_boundary}: {body}"
        );
    }
}

/// RULE: a page past `MaxItems` is truncated and carries a `Marker` that resumes
/// strictly after the last result reported, so draining the pages reports every
/// action's decision exactly once.
#[tokio::test]
async fn the_results_page_with_a_resumable_marker() {
    let router = router();
    let arn = principal(
        &router,
        "pagination",
        "paged",
        &[("allow-lists", ALLOW_LISTS)],
    )
    .await;
    let actions = ["iam:ListGroups", "iam:ListRoles", "iam:ListUsers"];
    let all: Vec<(String, String)> = vec![
        ("iam:ListGroups".to_string(), ALLOWED.to_string()),
        ("iam:ListRoles".to_string(), ALLOWED.to_string()),
        ("iam:ListUsers".to_string(), IMPLICIT_DENY.to_string()),
    ];

    for page_size in [1usize, 2, 3, 4] {
        let mut drained: Vec<(String, String)> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut extra: Vec<(String, String)> =
                vec![("MaxItems".to_string(), page_size.to_string())];
            if let Some(token) = marker.as_deref() {
                extra.push(("Marker".to_string(), token.to_string()));
            }
            let (status, body) = simulate(&router, &arn, &actions, &[], &extra).await;
            assert_eq!(
                status, 200,
                "case page size {page_size}: expected 200, got {status}: {body}"
            );
            let page = reported(&body);
            assert!(
                page.len() <= page_size,
                "case page size {page_size}: expected at most {page_size} results a page, \
                 got {}: {body}",
                page.len()
            );
            drained.extend(page);
            pages += 1;
            assert!(
                pages <= actions.len() + 1,
                "case page size {page_size}: paging did not terminate: {body}"
            );
            if element(&body, "IsTruncated") == "true" {
                marker = Some(element(&body, "Marker"));
            } else {
                break;
            }
        }
        assert_eq!(
            drained, all,
            "case page size {page_size}: expected every action's decision exactly once"
        );
    }

    // A token this service never issued is refused rather than read as the start.
    for bogus in ["not-a-marker", "zz", "0g"] {
        let (status, body) = simulate(
            &router,
            &arn,
            &actions,
            &[],
            &[("Marker".to_string(), bogus.to_string())],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {bogus:?}: expected a foreign Marker to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {bogus:?}: expected InvalidInput, got {:?}",
            error_code(&body)
        );
    }
}

/// RULE: a request the simulation cannot be made from is refused — an ARN naming no
/// identity the account holds is `NoSuchEntity`, and a value no simulation can be
/// made from (an ARN that is not an identity's, an action with no service prefix, a
/// document that is not a policy) is `InvalidInput`.
#[tokio::test]
async fn a_request_no_simulation_can_be_made_from_is_refused() {
    let router = router();
    let arn = principal(&router, "refusals", "refused", &[("allow-lists", ALLOW_LISTS)]).await;

    // An identity ARN the account holds nothing under.
    let absent = format!("arn:aws:iam::{}:user/nobody", account_id());
    let (status, body) = simulate(&router, &absent, &["iam:ListGroups"], &[], &[]).await;
    assert_eq!(status, 404, "expected 404, got {status}: {body}");
    assert_eq!(
        error_code(&body),
        "NoSuchEntity",
        "expected NoSuchEntity, got {:?}",
        error_code(&body)
    );

    // Rows spanning the inputs no simulation can be made from.
    struct Refusal {
        case: &'static str,
        extra: Vec<(String, String)>,
        source: String,
        actions: Vec<&'static str>,
    }
    let bad_source = format!("arn:aws:iam::{}:policy/not-an-identity", account_id());
    let refusals: Vec<Refusal> = vec![
        Refusal {
            case: "an ARN that names no identity",
            extra: Vec::new(),
            source: bad_source,
            actions: vec!["iam:ListGroups"],
        },
        Refusal {
            case: "an action with no service prefix",
            extra: Vec::new(),
            source: arn.clone(),
            actions: vec!["ListGroups"],
        },
        Refusal {
            case: "an action whose service prefix is empty",
            extra: Vec::new(),
            source: arn.clone(),
            actions: vec![":ListGroups"],
        },
        Refusal {
            case: "a supplied document that is not a policy",
            extra: vec![(
                "PolicyInputList.member.1".to_string(),
                "not a policy".to_string(),
            )],
            source: arn.clone(),
            actions: vec!["iam:ListGroups"],
        },
        Refusal {
            case: "a resource policy that is not a policy",
            extra: vec![("ResourcePolicy".to_string(), "{}".to_string())],
            source: arn.clone(),
            actions: vec!["iam:ListGroups"],
        },
        Refusal {
            case: "two permissions boundaries",
            extra: vec![
                (
                    "PermissionsBoundaryPolicyInputList.member.1".to_string(),
                    ALLOW_LISTS.to_string(),
                ),
                (
                    "PermissionsBoundaryPolicyInputList.member.2".to_string(),
                    ALLOW_LISTS.to_string(),
                ),
            ],
            source: arn.clone(),
            actions: vec!["iam:ListGroups"],
        },
        Refusal {
            case: "an exclusion naming a policy type that does not exist",
            extra: vec![(
                "PolicyExclusionList.member.1.PolicyType".to_string(),
                "made-up".to_string(),
            )],
            source: arn.clone(),
            actions: vec!["iam:ListGroups"],
        },
        Refusal {
            case: "an exclusion whose ARN is malformed",
            extra: vec![(
                "PolicyExclusionList.member.1.PolicyArn".to_string(),
                "arn:aws:iam::not-a-policy".to_string(),
            )],
            source: arn.clone(),
            actions: vec!["iam:ListGroups"],
        },
    ];
    for Refusal {
        case,
        extra,
        source,
        actions,
    } in refusals
    {
        let (status, body) = simulate(&router, &source, &actions, &[], &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}",
            error_code(&body)
        );
    }
}

/// How many resources one request may ask to have decided, how many extra policy
/// documents it may supply, and how many context keys one simulation may report as
/// missing a value — the bounds `src/ops/simulate_principal_policy.rs` states, because
/// the work and the response are the product of the actions, the resources, the
/// documents and that list.
const MAX_SIMULATED_RESOURCES: usize = 1_000;
const MAX_SIMULATED_DOCUMENTS: usize = 10;
const MAX_SIMULATED_CONTEXT_KEYS: usize = 100;
const MAX_SIMULATED_CONTEXT_VALUES: usize = 1_000;

/// A policy document whose one statement tests `keys` distinct condition keys, none of
/// which a request supplies a value for.
fn document_testing_keys(keys: usize) -> String {
    let tested: Vec<String> = (0..keys)
        .map(|position| format!(r#""aws:PrincipalTag/key{position}":"whatever""#))
        .collect();
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListGroups","Resource":"*","Condition":{{"StringEquals":{{{}}}}}}}]}}"#,
        tested.join(",")
    )
}

/// RULE: what one request may ask to have decided is bounded — at each bound the
/// request is served, one past it is refused with `InvalidInput`, and a page whose
/// (action, resource) pairs would run past the per-page bound reports fewer results
/// with `IsTruncated` true and a `Marker` that carries on, rather than deciding them all.
#[tokio::test]
async fn the_simulated_work_one_request_asks_for_is_bounded() {
    let router = router();
    let arn = principal(&router, "bounds", "bounded-work", &[("allow-lists", ALLOW_LISTS)]).await;
    let resource_at = |position: usize| format!("arn:aws:s3:::bucket{position}/key");

    // Each bound, and one past it.
    for (case, count, cap) in [
        ("resources at the bound", MAX_SIMULATED_RESOURCES, true),
        ("resources one past the bound", MAX_SIMULATED_RESOURCES + 1, false),
    ] {
        let resources: Vec<String> = (0..count).map(resource_at).collect();
        let borrowed: Vec<&str> = resources.iter().map(String::as_str).collect();
        let (status, body) = simulate(&router, &arn, &["iam:ListGroups"], &borrowed, &[]).await;
        if cap {
            assert_eq!(
                status, 200,
                "case {case}: expected the bound itself to be served, got {status}"
            );
        } else {
            assert_eq!(status, 400, "case {case}: expected 400, got {status}: {body}");
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case}: expected InvalidInput, got {:?}",
                error_code(&body)
            );
        }
    }
    for (case, count, cap) in [
        ("documents at the bound", MAX_SIMULATED_DOCUMENTS, true),
        ("documents one past the bound", MAX_SIMULATED_DOCUMENTS + 1, false),
    ] {
        let extra: Vec<(String, String)> = (1..=count)
            .map(|position| {
                (
                    format!("PolicyInputList.member.{position}"),
                    ALLOW_LISTS.to_string(),
                )
            })
            .collect();
        let (status, body) = simulate(&router, &arn, &["iam:ListGroups"], &[], &extra).await;
        if cap {
            assert_eq!(
                status, 200,
                "case {case}: expected the bound itself to be served, got {status}: {body}"
            );
        } else {
            assert_eq!(status, 400, "case {case}: expected 400, got {status}: {body}");
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case}: expected InvalidInput, got {:?}",
                error_code(&body)
            );
        }
    }

    // The condition values the request supplies are the other side of the key lookup,
    // so they are bounded the same way.
    for (case, count, served) in [
        (
            "context values at the bound",
            MAX_SIMULATED_CONTEXT_VALUES,
            true,
        ),
        (
            "context values one past the bound",
            MAX_SIMULATED_CONTEXT_VALUES + 1,
            false,
        ),
    ] {
        let mut extra: Vec<(String, String)> = vec![(
            "ContextEntries.member.1.ContextKeyName".to_string(),
            "aws:PrincipalTag/team".to_string(),
        )];
        extra.extend((1..=count).map(|position| {
            (
                format!("ContextEntries.member.1.ContextKeyValues.member.{position}"),
                format!("value{position}"),
            )
        }));
        let (status, body) = simulate(&router, &arn, &["iam:ListGroups"], &[], &extra).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case}: expected the bound itself to be served, got {status}"
            );
        } else {
            assert_eq!(status, 400, "case {case}: expected 400, got {status}: {body}");
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case}: expected InvalidInput, got {:?}",
                error_code(&body)
            );
        }
    }

    // The context-key list is reported on every result, so it is bounded too: at the
    // bound the request is served, one key past it refused.
    for (case, keys, served) in [
        ("context keys at the bound", MAX_SIMULATED_CONTEXT_KEYS, true),
        (
            "context keys one past the bound",
            MAX_SIMULATED_CONTEXT_KEYS + 1,
            false,
        ),
    ] {
        let extra = vec![(
            "PolicyInputList.member.1".to_string(),
            document_testing_keys(keys),
        )];
        let (status, body) = simulate(&router, &arn, &["iam:ListGroups"], &[], &extra).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case}: expected the bound itself to be served, got {status}: {body}"
            );
        } else {
            assert_eq!(status, 400, "case {case}: expected 400, got {status}: {body}");
            assert_eq!(
                error_code(&body),
                "InvalidInput",
                "case {case}: expected InvalidInput, got {:?}",
                error_code(&body)
            );
        }
    }

    // Many resources and three actions: the page carries fewer action results than
    // asked for, says so, and the marker reports the rest.
    let resources: Vec<String> = (0..600).map(resource_at).collect();
    let borrowed: Vec<&str> = resources.iter().map(String::as_str).collect();
    let actions = ["iam:ListGroups", "iam:ListRoles", "iam:ListUsers"];
    let mut drained: Vec<String> = Vec::new();
    let mut marker: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut extra: Vec<(String, String)> = vec![("MaxItems".to_string(), "3".to_string())];
        if let Some(token) = marker.as_deref() {
            extra.push(("Marker".to_string(), token.to_string()));
        }
        let (status, body) = simulate(&router, &arn, &actions, &borrowed, &extra).await;
        assert_eq!(status, 200, "expected 200, got {status}");
        let page = reported(&body);
        assert!(
            page.len() < actions.len(),
            "expected a page narrowed by the resource count, got {} results",
            page.len()
        );
        drained.extend(page.into_iter().map(|(action, _)| action));
        pages += 1;
        assert!(pages <= actions.len(), "paging did not terminate");
        if element(&body, "IsTruncated") == "true" {
            marker = Some(element(&body, "Marker"));
        } else {
            break;
        }
    }
    assert_eq!(
        drained, actions,
        "expected the narrowed pages to report every action exactly once, in order"
    );
}
