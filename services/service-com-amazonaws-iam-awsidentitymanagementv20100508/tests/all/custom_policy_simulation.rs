// SPDX-License-Identifier: CC0-1.0

//! `SimulateCustomPolicy`, end to end over the awsQuery wire.
//!
//! The rule under test: a simulation reports, for each action the request names and each
//! resource it names, what the policies the REQUEST supplies decide — `allowed` when
//! something about the request grants it, `explicitDeny` when a statement about the
//! request denies it, and `implicitDeny` when nothing does either — bounded by the
//! ceilings the request also supplies (a permissions boundary, an Organizations
//! hierarchy's service control policies) and, where the request supplies one, together
//! with a resource-based policy decided against the simulated caller. "The policies are
//! provided as strings": nothing the account holds takes part in the decision, which is
//! what separates this operation from `SimulatePrincipalPolicy` — handed the same
//! document, the two agree. It is a REPORT: it decides, it does not perform, and it
//! leaves the account exactly as it was.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! effect, the action form (literal, wildcard run, an action no statement mentions), the
//! resource the statement is scoped to, how many documents and resources one request
//! carries, which ceiling is in force, which account owns the simulated resources,
//! whether a condition's value is supplied, and the page size. No single request is the
//! specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other
//! suites in this crate share; the decision values and the `(action, decision)` reader
//! are `tests/all/policy_simulation.rs`'s, so both simulation suites read a report one
//! way.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
use crate::policy_simulation::{reported, ALLOWED, EXPLICIT_DENY, IMPLICIT_DENY};
use crate::user_lifecycle::{account_id, call, call_signed_by, element, elements, error_code, router};

/// The awsQuery form parameters one `SimulateCustomPolicy` call carries, with the list
/// members the protocol flattens into `<member>.member.N` parameters.
fn simulation_params(
    documents: &[&str],
    actions: &[&str],
    resources: &[&str],
    extra: &[(String, String)],
) -> Vec<(String, String)> {
    let mut params: Vec<(String, String)> = Vec::new();
    for (position, document) in documents.iter().enumerate() {
        params.push((
            format!("PolicyInputList.member.{}", position + 1),
            (*document).to_string(),
        ));
    }
    for (position, action) in actions.iter().enumerate() {
        params.push((
            format!("ActionNames.member.{}", position + 1),
            (*action).to_string(),
        ));
    }
    for (position, resource) in resources.iter().enumerate() {
        params.push((
            format!("ResourceArns.member.{}", position + 1),
            (*resource).to_string(),
        ));
    }
    params.extend(extra.iter().cloned());
    params
}

/// One unsigned `SimulateCustomPolicy` call.
async fn simulate(
    router: &MultiServiceRouter,
    documents: &[&str],
    actions: &[&str],
    resources: &[&str],
    extra: &[(String, String)],
) -> (u16, String) {
    let owned = simulation_params(documents, actions, resources, extra);
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call(router, "SimulateCustomPolicy", &params).await
}

/// The same call, signed by `access_key_id`, for the suite about the gate on the call
/// itself.
async fn simulate_signed_by(
    router: &MultiServiceRouter,
    access_key_id: &str,
    documents: &[&str],
    actions: &[&str],
) -> (u16, String) {
    let owned = simulation_params(documents, actions, &[], &[]);
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call_signed_by(router, Some(access_key_id), "SimulateCustomPolicy", &params).await
}

/// One form parameter, owned.
fn param(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// A grant of the two IAM reads most rows ask about.
const ALLOW_LISTS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:ListUsers","iam:ListRoles"],"Resource":"*"}]}"#;

/// A deny of one of them, for the rows about an explicit deny.
const DENY_LIST_USERS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":"iam:ListUsers","Resource":"*"}]}"#;

/// A grant written as a wildcard RUN rather than as literal action names.
const ALLOW_LIST_PREFIX: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:List*","Resource":"*"}]}"#;

/// A grant scoped to the objects of one bucket, for the rows about resource scoping.
const ALLOW_REPORTS_OBJECTS: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::reports/*"}]}"#;

/// The two objects those rows ask about: one the scoped statement covers and one it does
/// not.
const REPORTS_OBJECT: &str = "arn:aws:s3:::reports/q1.csv";
const AUDIT_OBJECT: &str = "arn:aws:s3:::audit/q1.csv";

/// A document that states nothing about any request, for the rows where the identity-based
/// policies grant nothing: `PolicyInputList` is a required member, so a simulation that is
/// about something else still supplies a document.
const STATES_NOTHING: &str = r#"{"Version":"2012-10-17","Statement":[]}"#;

/// One row of the decision table.
struct Case {
    case: &'static str,
    /// The documents the request supplies in `PolicyInputList`.
    documents: &'static [&'static str],
    actions: &'static [&'static str],
    resources: &'static [&'static str],
    /// The decision each action must be reported with, in request order.
    expected: &'static [&'static str],
    /// The per-resource decisions the response must carry, in request order, for a row
    /// that names resources; empty for a row that leaves the per-resource report alone.
    per_resource: &'static [&'static str],
}

/// Rows spanning the effects, the action forms, the resource scoping, and how many
/// documents and resources one request carries.
const CASES: &[Case] = &[
    Case {
        case: "an allow of the action asked for",
        documents: &[ALLOW_LISTS],
        actions: &["iam:ListUsers"],
        resources: &[],
        expected: &[ALLOWED],
        per_resource: &[],
    },
    Case {
        case: "an action no supplied statement mentions",
        documents: &[ALLOW_LISTS],
        actions: &["iam:DeleteUser"],
        resources: &[],
        expected: &[IMPLICIT_DENY],
        per_resource: &[],
    },
    Case {
        case: "a deny beside the allow of the same action",
        documents: &[ALLOW_LISTS, DENY_LIST_USERS],
        actions: &["iam:ListUsers", "iam:ListRoles"],
        resources: &[],
        expected: &[EXPLICIT_DENY, ALLOWED],
        per_resource: &[],
    },
    Case {
        case: "a wildcard run covering two of three actions",
        documents: &[ALLOW_LIST_PREFIX],
        actions: &["iam:ListUsers", "iam:ListRoles", "iam:GetUser"],
        resources: &[],
        expected: &[ALLOWED, ALLOWED, IMPLICIT_DENY],
        per_resource: &[],
    },
    Case {
        case: "a statement scoped to one of the two resources asked about",
        documents: &[ALLOW_REPORTS_OBJECTS],
        actions: &["s3:GetObject"],
        resources: &[REPORTS_OBJECT, AUDIT_OBJECT],
        // "The top-level EvalDecision reflects the most restrictive decision across all
        // resources": one resource nothing grants makes the action's own decision the
        // implicit deny, while each resource still reports its own.
        expected: &[IMPLICIT_DENY],
        per_resource: &[ALLOWED, IMPLICIT_DENY],
    },
    Case {
        case: "the second of two documents granting the action",
        documents: &[ALLOW_REPORTS_OBJECTS, ALLOW_LISTS],
        actions: &["iam:ListUsers"],
        resources: &[],
        expected: &[ALLOWED],
        per_resource: &[],
    },
    Case {
        case: "a document that states nothing at all",
        documents: &[r#"{"Version":"2012-10-17","Statement":[]}"#],
        actions: &["iam:ListUsers"],
        resources: &[],
        expected: &[IMPLICIT_DENY],
        per_resource: &[],
    },
];

/// RULE: the decision reported for each action is what the supplied documents say about
/// it, over each resource the request names.
#[tokio::test]
async fn the_decision_follows_the_supplied_documents() {
    let router = router();
    for row in CASES {
        let Case {
            case,
            documents,
            actions,
            resources,
            expected,
            per_resource,
        } = row;
        let (status, body) = simulate(&router, documents, actions, resources, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the simulation must succeed, got {status}: {body}"
        );
        let decisions: Vec<String> = reported(&body)
            .into_iter()
            .map(|(_, decision)| decision)
            .collect();
        let expected_decisions: Vec<String> =
            expected.iter().map(|value| (*value).to_string()).collect();
        assert_eq!(
            decisions, expected_decisions,
            "case {case:?}: expected {expected:?}, got {decisions:?}: {body}"
        );
        let names: Vec<String> = reported(&body).into_iter().map(|(name, _)| name).collect();
        let expected_names: Vec<String> =
            actions.iter().map(|action| (*action).to_string()).collect();
        assert_eq!(
            names, expected_names,
            "case {case:?}: expected one result per action in request order, got {names:?}: \
             {body}"
        );
        if !per_resource.is_empty() {
            let reported_per_resource = elements(&body, "EvalResourceDecision");
            let expected_per_resource: Vec<String> = per_resource
                .iter()
                .map(|value| (*value).to_string())
                .collect();
            assert_eq!(
                reported_per_resource, expected_per_resource,
                "case {case:?}: expected the per-resource decisions {per_resource:?}, got \
                 {reported_per_resource:?}: {body}"
            );
        }
        assert_eq!(
            element(&body, "IsTruncated"),
            "false",
            "case {case:?}: the whole action list fits one page, got {body}"
        );
    }
}

/// RULE: the decision is the same whichever simulation operation is asked — handed ONE
/// document, `SimulateCustomPolicy` (which is given it as a string) and
/// `SimulatePrincipalPolicy` (which reads it off the principal it names) report the same
/// decision for the same action.
#[tokio::test]
async fn a_custom_simulation_agrees_with_the_principal_simulation() {
    let router = router();
    let case = "the same document, supplied and attached";
    let (status, created) = call(&router, "CreateUser", &[("UserName", "agreeing")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateUser must succeed, got {status}: {created}"
    );
    let user_arn = element(&created, "Arn");
    let document = ALLOW_LISTS;
    let (status, put) = call(
        &router,
        "PutUserPolicy",
        &[
            ("UserName", "agreeing"),
            ("PolicyName", "lists"),
            ("PolicyDocument", document),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: PutUserPolicy must succeed, got {status}: {put}"
    );

    // Rows: an action the document allows, another it allows, one it says nothing about,
    // and one asked as a wildcard-free literal of another service — so the agreement is
    // over a SET of actions rather than over one.
    let actions = [
        "iam:ListUsers",
        "iam:ListRoles",
        "iam:DeleteUser",
        "s3:GetObject",
    ];
    let (status, custom) = simulate(
        &router,
        &[document],
        &actions,
        &[],
        &[param("CallerArn", &user_arn)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the custom simulation must succeed, got {status}: {custom}"
    );

    let mut principal_params: Vec<(String, String)> = vec![param("PolicySourceArn", &user_arn)];
    for (position, action) in actions.iter().enumerate() {
        principal_params.push(param(
            &format!("ActionNames.member.{}", position + 1),
            action,
        ));
    }
    let borrowed: Vec<(&str, &str)> = principal_params
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    let (status, from_principal) =
        call(&router, "SimulatePrincipalPolicy", &borrowed).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the principal simulation must succeed, got {status}: {from_principal}"
    );

    assert_eq!(
        reported(&custom),
        reported(&from_principal),
        "case {case:?}: the two simulations must agree, got {custom} and {from_principal}"
    );
    // And the agreement is not "both report nothing": the document's own actions are
    // allowed and the others are not.
    let decisions: Vec<String> = reported(&custom)
        .into_iter()
        .map(|(_, decision)| decision)
        .collect();
    assert_eq!(
        decisions,
        vec![ALLOWED, ALLOWED, IMPLICIT_DENY, IMPLICIT_DENY],
        "case {case:?}: expected the document's actions allowed and the others not, got \
         {custom}"
    );
}

/// The ARN of a user of this account, which is what `CallerArn` names.
fn caller_arn(user_name: &str) -> String {
    format!("arn:aws:iam::{}:user/{user_name}", account_id())
}

/// A resource-based policy granting `principal` the object read, as a bucket policy
/// states it.
fn resource_policy(effect: &str, principal: &str) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"{effect}","Principal":{{"AWS":"{principal}"}},"Action":"s3:GetObject","Resource":"*"}}]}}"#
    )
}

/// RULE: a supplied resource-based policy is decided against the simulated caller — within
/// one account an allow from EITHER side grants, and an explicit deny from either refuses.
#[tokio::test]
async fn a_resource_policy_is_decided_against_the_simulated_caller() {
    let caller = caller_arn("resource-caller");
    let other = caller_arn("someone-else");
    // Rows: the resource policy alone granting, the resource policy denying what the
    // identity policies allow, a resource policy about another principal, and both sides
    // granting.
    let rows = [
        (
            "the resource policy alone grants the caller",
            vec![STATES_NOTHING],
            resource_policy("Allow", &caller),
            ALLOWED,
        ),
        (
            "the resource policy denies what the identity policy allows",
            vec![ALLOW_REPORTS_OBJECTS],
            resource_policy("Deny", &caller),
            EXPLICIT_DENY,
        ),
        (
            "the resource policy is about another principal",
            vec![STATES_NOTHING],
            resource_policy("Allow", &other),
            IMPLICIT_DENY,
        ),
        (
            "both sides grant",
            vec![ALLOW_REPORTS_OBJECTS],
            resource_policy("Allow", &caller),
            ALLOWED,
        ),
    ];
    let router = router();
    for (case, documents, policy, expected) in rows {
        let (status, body) = simulate(
            &router,
            &documents,
            &["s3:GetObject"],
            &[REPORTS_OBJECT],
            &[
                param("ResourcePolicy", &policy),
                param("CallerArn", &caller),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the simulation must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// RULE: when the account that owns the simulated resources is not the caller's, an allow
/// needs BOTH the identity-based policies and the resource-based policy; within one
/// account either side alone grants.
#[tokio::test]
async fn resources_another_account_owns_need_both_sides_to_allow() {
    let caller = caller_arn("cross-caller");
    let own_account = format!("arn:aws:iam::{}:root", account_id());
    // An account id no row's caller belongs to, written as the account root ARN
    // `ResourceOwner` documents.
    let other_account = "arn:aws:iam::210987654321:root".to_string();
    // Rows: the owner left to default, the caller's own account named, and another
    // account named with each side granting, both granting, and the resource side denying.
    let rows = [
        (
            "no owner named, the resource policy alone grants",
            None,
            vec![STATES_NOTHING],
            resource_policy("Allow", &caller),
            ALLOWED,
        ),
        (
            "the caller's own account owns the resources",
            Some(own_account.clone()),
            vec![STATES_NOTHING],
            resource_policy("Allow", &caller),
            ALLOWED,
        ),
        (
            "another account owns them and only the resource policy grants",
            Some(other_account.clone()),
            vec![STATES_NOTHING],
            resource_policy("Allow", &caller),
            IMPLICIT_DENY,
        ),
        (
            "another account owns them and only the identity policy grants",
            Some(other_account.clone()),
            vec![ALLOW_REPORTS_OBJECTS],
            resource_policy("Allow", &other_account),
            IMPLICIT_DENY,
        ),
        (
            "another account owns them and both sides grant",
            Some(other_account.clone()),
            vec![ALLOW_REPORTS_OBJECTS],
            resource_policy("Allow", &caller),
            ALLOWED,
        ),
        (
            "another account owns them and the resource policy denies",
            Some(other_account.clone()),
            vec![ALLOW_REPORTS_OBJECTS],
            resource_policy("Deny", &caller),
            EXPLICIT_DENY,
        ),
    ];
    let router = router();
    for (case, owner, documents, policy, expected) in rows {
        let mut extra = vec![
            param("ResourcePolicy", &policy),
            param("CallerArn", &caller),
        ];
        if let Some(owner) = &owner {
            extra.push(param("ResourceOwner", owner));
        }
        let (status, body) = simulate(
            &router,
            &documents,
            &["s3:GetObject"],
            &[REPORTS_OBJECT],
            &extra,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the simulation must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// An SCP allowing `action` on every resource, as one level of a hierarchy states it.
fn scp(effect: &str, action: &str) -> String {
    statement(effect, action, "*")
}

/// The form parameters naming an SCP hierarchy: one entry per level, in the order given.
fn hierarchy(levels: &[&str]) -> Vec<(String, String)> {
    levels
        .iter()
        .enumerate()
        .map(|(position, document)| {
            param(
                &format!(
                    "OrderedOrganizationPolicyInputList.member.{}.ServiceControlPolicyInputList.member.1",
                    position + 1
                ),
                document,
            )
        })
        .collect()
}

/// RULE: the service control policies of a named Organizations hierarchy bound the
/// decision — an action a level does not allow is not available to the account, one a
/// level denies is denied outright — and what they decided is reported as
/// `AllowedByOrganizations`.
#[tokio::test]
async fn an_organizations_hierarchy_bounds_the_decision() {
    let allow_everything = scp("Allow", "*");
    let allow_list_roles = scp("Allow", "iam:ListRoles");
    let deny_list_users = scp("Deny", "iam:ListUsers");
    // Rows: no hierarchy at all, two levels allowing, a level that allows another action,
    // a level that denies the action, and a three-level hierarchy whose innermost level
    // allows it.
    let rows: Vec<(&str, Vec<&str>, &str, Option<&str>)> = vec![
        ("no hierarchy named", Vec::new(), ALLOWED, None),
        (
            "every level allows everything",
            vec![allow_everything.as_str(), allow_everything.as_str()],
            ALLOWED,
            Some("true"),
        ),
        (
            "the account level allows another action",
            vec![allow_everything.as_str(), allow_list_roles.as_str()],
            IMPLICIT_DENY,
            Some("false"),
        ),
        (
            "a level denies the action",
            vec![allow_everything.as_str(), deny_list_users.as_str()],
            EXPLICIT_DENY,
            Some("false"),
        ),
        (
            "three levels, all of which allow it",
            vec![
                allow_everything.as_str(),
                allow_everything.as_str(),
                allow_everything.as_str(),
            ],
            ALLOWED,
            Some("true"),
        ),
    ];
    let router = router();
    for (case, levels, expected, allowed_by_organizations) in rows {
        let (status, body) = simulate(
            &router,
            &[ALLOW_LISTS],
            &["iam:ListUsers"],
            &[],
            &hierarchy(&levels),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: the simulation must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        match allowed_by_organizations {
            Some(allowed) => assert_eq!(
                elements(&body, "AllowedByOrganizations"),
                vec![allowed.to_string()],
                "case {case:?}: expected AllowedByOrganizations {allowed}, got {body}"
            ),
            None => assert!(
                elements(&body, "AllowedByOrganizations").is_empty(),
                "case {case:?}: a simulation that named no hierarchy must report nothing \
                 about one, got {body}"
            ),
        }
    }
}

/// One row of the permissions-boundary table: the ceiling the request supplies, the action
/// it asks about, the decision it must be answered with and the detail that decision must
/// be reported beside.
struct BoundaryCase {
    case: &'static str,
    boundary: Option<&'static str>,
    action: &'static str,
    expected: &'static str,
    allowed_by_boundary: Option<&'static str>,
}

/// RULE: a supplied permissions boundary caps the decision — it can take permission away
/// and never grant it — and what it decided is reported as
/// `AllowedByPermissionsBoundary`.
#[tokio::test]
async fn a_simulated_permissions_boundary_caps_the_decision() {
    let boundary_lists_users =
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*"}]}"#;
    // Rows: no boundary, a boundary covering the action, a boundary that does not, and a
    // boundary that covers an action the identity policies do not grant.
    let rows = [
        BoundaryCase {
            case: "no boundary",
            boundary: None,
            action: "iam:ListRoles",
            expected: ALLOWED,
            allowed_by_boundary: None,
        },
        BoundaryCase {
            case: "a boundary covering the action",
            boundary: Some(boundary_lists_users),
            action: "iam:ListUsers",
            expected: ALLOWED,
            allowed_by_boundary: Some("true"),
        },
        BoundaryCase {
            case: "a boundary that does not cover the action",
            boundary: Some(boundary_lists_users),
            action: "iam:ListRoles",
            expected: IMPLICIT_DENY,
            allowed_by_boundary: Some("false"),
        },
        BoundaryCase {
            case: "a boundary covering an action nothing grants",
            boundary: Some(boundary_lists_users),
            action: "iam:DeleteUser",
            expected: IMPLICIT_DENY,
            allowed_by_boundary: Some("false"),
        },
    ];
    let router = router();
    for row in rows {
        let BoundaryCase {
            case,
            boundary,
            action,
            expected,
            allowed_by_boundary,
        } = row;
        let extra: Vec<(String, String)> = boundary
            .map(|document| {
                vec![param(
                    "PermissionsBoundaryPolicyInputList.member.1",
                    document,
                )]
            })
            .unwrap_or_default();
        let (status, body) = simulate(&router, &[ALLOW_LISTS], &[action], &[], &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the simulation must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        match allowed_by_boundary {
            Some(allowed) => assert_eq!(
                elements(&body, "AllowedByPermissionsBoundary"),
                vec![allowed.to_string()],
                "case {case:?}: expected AllowedByPermissionsBoundary {allowed}, got {body}"
            ),
            None => assert!(
                elements(&body, "AllowedByPermissionsBoundary").is_empty(),
                "case {case:?}: a simulation with no boundary must report nothing about \
                 one, got {body}"
            ),
        }
    }
}

/// RULE: a condition clause is decided from the values the request supplies — IAM's set
/// qualifiers deciding a key the request carries several values for — and a key the
/// decision needed a value for and could not compare is reported in
/// `MissingContextValues`.
#[tokio::test]
async fn supplied_condition_values_decide_and_absent_ones_are_reported() {
    const TEAM_KEY: &str = "aws:PrincipalTag/team";
    // The clause forms: the single-valued operator, and the two set qualifiers IAM
    // publishes for a key that carries several values.
    let conditioned = |clause: &str| {
        format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*","Condition":{{"{clause}":{{"{TEAM_KEY}":"storage"}}}}}}]}}"#
        )
    };
    let equals = conditioned("StringEquals");
    let any_value = conditioned("ForAnyValue:StringEquals");
    let all_values = conditioned("ForAllValues:StringEquals");
    // Rows: no value supplied, the value the clause tests for, another value, several
    // values under the single-valued operator (which IAM cannot compare), and several
    // values under each qualifier.
    let rows: Vec<(&str, &str, Vec<&str>, &str, bool)> = vec![
        (
            "no value supplied",
            equals.as_str(),
            Vec::new(),
            IMPLICIT_DENY,
            true,
        ),
        (
            "the value the clause tests for",
            equals.as_str(),
            vec!["storage"],
            ALLOWED,
            false,
        ),
        (
            "another value",
            equals.as_str(),
            vec!["platform"],
            IMPLICIT_DENY,
            false,
        ),
        (
            // The request carried values for the key, so nothing about it is MISSING:
            // "a list of context keys that are required by the included input policies
            // but that were not provided by one of the input parameters". What IAM
            // cannot do is compare several values with a single-valued operator, so the
            // clause does not hold and nothing grants the action.
            "two values under a single-valued operator",
            equals.as_str(),
            vec!["platform", "storage"],
            IMPLICIT_DENY,
            false,
        ),
        (
            "two values, one of them covered, under ForAnyValue",
            any_value.as_str(),
            vec!["platform", "storage"],
            ALLOWED,
            false,
        ),
        (
            "two values, one of them covered, under ForAllValues",
            all_values.as_str(),
            vec!["platform", "storage"],
            IMPLICIT_DENY,
            false,
        ),
        (
            "one covered value under ForAllValues",
            all_values.as_str(),
            vec!["storage"],
            ALLOWED,
            false,
        ),
    ];
    let router = router();
    for (case, document, values, expected, reports_missing) in rows {
        let mut extra: Vec<(String, String)> = Vec::new();
        if !values.is_empty() {
            extra.push(param("ContextEntries.member.1.ContextKeyName", TEAM_KEY));
            extra.push(param(
                "ContextEntries.member.1.ContextKeyType",
                "stringList",
            ));
            for (position, value) in values.iter().enumerate() {
                extra.push(param(
                    &format!(
                        "ContextEntries.member.1.ContextKeyValues.member.{}",
                        position + 1
                    ),
                    value,
                ));
            }
        }
        let (status, body) = simulate(&router, &[document], &["iam:ListUsers"], &[], &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the simulation must succeed, got {status}: {body}"
        );
        assert_eq!(
            element(&body, "EvalDecision"),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        let missing = format!("<MissingContextValues><member>{TEAM_KEY}</member>");
        assert_eq!(
            body.contains(&missing),
            reports_missing,
            "case {case:?}: expected the missing-key report to be {reports_missing}, got \
             {body}"
        );
    }
}

/// RULE: one page carries at most `MaxItems` results, says so through `IsTruncated`, and
/// hands back a `Marker` the next call resumes strictly after — so paging a request walks
/// its actions exactly once.
#[tokio::test]
async fn one_page_is_answered_and_its_marker_resumes_the_rest() {
    let actions = [
        "iam:ListUsers",
        "iam:ListRoles",
        "iam:GetUser",
        "iam:DeleteUser",
        "s3:GetObject",
    ];
    let router = router();
    // Rows: the page sizes that divide the list, one that does not, and one larger than
    // the list.
    for page_size in [1usize, 2, 3, 5, 9] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut marker: Option<String> = None;
        let mut pages = 0usize;
        loop {
            let mut extra = vec![param("MaxItems", &page_size.to_string())];
            if let Some(marker) = &marker {
                extra.push(param("Marker", marker));
            }
            let (status, body) = simulate(&router, &[ALLOW_LISTS], &actions, &[], &extra).await;
            assert_eq!(
                status, 200,
                "case {case:?}: the simulation must succeed, got {status}: {body}"
            );
            let page: Vec<String> = reported(&body).into_iter().map(|(name, _)| name).collect();
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page must carry at most {page_size} results, got {page:?}"
            );
            assert!(
                !page.is_empty(),
                "case {case:?}: a page must carry at least one result, got {body}"
            );
            seen.extend(page);
            pages += 1;
            if element(&body, "IsTruncated") == "true" {
                marker = Some(element(&body, "Marker"));
            } else {
                assert!(
                    elements(&body, "Marker").is_empty(),
                    "case {case:?}: a last page carries no marker, got {body}"
                );
                break;
            }
            assert!(
                pages <= actions.len(),
                "case {case:?}: paging must terminate, got {pages} pages"
            );
        }
        let expected: Vec<String> = actions.iter().map(|action| (*action).to_string()).collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: paging must walk every action exactly once and in order, got \
             {seen:?}"
        );
        assert_eq!(
            pages,
            actions.len().div_ceil(page_size),
            "case {case:?}: expected the list to take {} pages, got {pages}",
            actions.len().div_ceil(page_size)
        );
    }
}

/// The EC2 resources the `EC2-VPC-EBS` simulation scenario requires, as ARNs of this
/// account's Region, one per type the scenario's documentation names
/// (`smithy_query(query="doc",shape="SimulateCustomPolicyRequest",member="ResourceHandlingOption")`).
fn ec2_ebs_resources() -> Vec<String> {
    ["instance/i-0123456789abcdef0", "image/ami-0123456789abcdef0", "security-group/sg-0123456789abcdef0", "network-interface/eni-0123456789abcdef0", "volume/vol-0123456789abcdef0"]
        .iter()
        .map(|resource| format!("arn:aws:ec2:us-east-1:{}:{resource}", account_id()))
        .collect()
}

/// RULE: a declared simulation scenario is run only once the resources that scenario
/// requires are named — "by specifying the type of simulation to run, you enable the
/// policy simulator to enforce the presence of the required resources".
#[tokio::test]
async fn a_declared_scenario_is_run_once_its_resources_are_named() {
    let router = router();
    let resources = ec2_ebs_resources();
    let named: Vec<&str> = resources.iter().map(String::as_str).collect();
    let allow_ec2 =
        r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"ec2:RunInstances","Resource":"*"}]}"#;

    let (status, body) = simulate(
        &router,
        &[allow_ec2],
        &["ec2:RunInstances"],
        &named,
        &[param("ResourceHandlingOption", "EC2-VPC-EBS")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the scenario's own resources must be enough to run it, got {status}: {body}"
    );
    assert_eq!(
        elements(&body, "EvalResourceDecision"),
        vec![ALLOWED.to_string(); named.len()],
        "expected each named resource to be decided, got {body}"
    );

    // One resource short of the scenario — each of its required types in turn — is
    // refused rather than decided, which is the enforcement the member asks for.
    for missing in 0..named.len() {
        let short: Vec<&str> = named
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != missing)
            .map(|(_, resource)| *resource)
            .collect();
        let (status, body) = simulate(
            &router,
            &[allow_ec2],
            &["ec2:RunInstances"],
            &short,
            &[param("ResourceHandlingOption", "EC2-VPC-EBS")],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {:?}: a scenario short of a resource must be refused, got {status}: {body}",
            named[missing]
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {:?}: expected InvalidInput, got {body}",
            named[missing]
        );
    }
}

/// One row of the refusal table: the request, and the awsQuery error code it must be
/// refused with.
struct Refused {
    case: &'static str,
    documents: Vec<&'static str>,
    actions: Vec<&'static str>,
    resources: Vec<&'static str>,
    extra: Vec<(String, String)>,
    code: &'static str,
}

/// The rows: every way a simulation request can fail to be decidable.
fn refused_rows() -> Vec<Refused> {
    let caller = caller_arn("refused-caller");
    vec![
        Refused {
            case: "a supplied member that is not a policy document",
            documents: vec!["not a policy at all"],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: Vec::new(),
            code: "InvalidInput",
        },
        Refused {
            case: "a resource policy that is not a policy document",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![
                param("ResourcePolicy", "{}"),
                param("CallerArn", &caller),
            ],
            code: "InvalidInput",
        },
        Refused {
            case: "an SCP that is not a policy document",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![param(
                "OrderedOrganizationPolicyInputList.member.1.ServiceControlPolicyInputList.member.1",
                "17",
            )],
            code: "InvalidInput",
        },
        Refused {
            case: "two permissions boundaries",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![
                param("PermissionsBoundaryPolicyInputList.member.1", ALLOW_LISTS),
                param("PermissionsBoundaryPolicyInputList.member.2", ALLOW_LISTS),
            ],
            code: "InvalidInput",
        },
        Refused {
            case: "an action name carrying no service",
            documents: vec![ALLOW_LISTS],
            actions: vec!["ListUsers"],
            resources: Vec::new(),
            extra: Vec::new(),
            code: "InvalidInput",
        },
        Refused {
            case: "an action name that is only a service",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:"],
            resources: Vec::new(),
            extra: Vec::new(),
            code: "InvalidInput",
        },
        Refused {
            case: "a resource policy with no simulated caller",
            documents: vec![ALLOW_LISTS],
            actions: vec!["s3:GetObject"],
            resources: Vec::new(),
            extra: vec![param("ResourcePolicy", &resource_policy("Allow", &caller))],
            code: "InvalidInput",
        },
        Refused {
            case: "a caller that is not an IAM user, group or role",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![param(
                "CallerArn",
                "arn:aws:sts::210987654321:assumed-role/deployer/session",
            )],
            code: "InvalidInput",
        },
        Refused {
            case: "a caller that is a managed policy",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![param("CallerArn", "arn:aws:iam::210987654321:policy/ReadOnly")],
            code: "InvalidInput",
        },
        Refused {
            case: "a scenario no simulation publishes",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![param("ResourceHandlingOption", "EC2-Classic-InstanceStore")],
            code: "InvalidInput",
        },
        Refused {
            case: "a marker this service cannot have produced",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![param("Marker", "not-a-marker")],
            code: "InvalidInput",
        },
        Refused {
            case: "a page size past the modelled bound",
            documents: vec![ALLOW_LISTS],
            actions: vec!["iam:ListUsers"],
            resources: Vec::new(),
            extra: vec![param("MaxItems", "1001")],
            code: "ValidationError",
        },
    ]
}

/// RULE: a request the simulator cannot decide from is refused with the modelled 400 and
/// nothing is reported for it.
#[tokio::test]
async fn a_request_the_simulator_cannot_decide_is_refused() {
    let router = router();
    for row in refused_rows() {
        let Refused {
            case,
            documents,
            actions,
            resources,
            extra,
            code,
        } = row;
        let (status, body) = simulate(&router, &documents, &actions, &resources, &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got {:?}: {body}",
            error_code(&body)
        );
        assert!(
            elements(&body, "EvalDecision").is_empty(),
            "case {case:?}: a refused request reports no decision, got {body}"
        );
    }

    // A refused request changes nothing about what the next one is answered: a decidable
    // request still decides.
    let (status, body) = simulate(&router, &[ALLOW_LISTS], &["iam:ListUsers"], &[], &[]).await;
    assert_eq!(
        status, 200,
        "the simulation must still succeed after the refusals, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "EvalDecision"),
        ALLOWED,
        "expected the supplied grant to decide, got {body}"
    );
}

/// RULE: asking for a simulation is itself an IAM call, so it is decided against the
/// signing principal's own identity-based policies on `iam:SimulateCustomPolicy` — and
/// because it only reports, the account is exactly as it was afterwards.
#[tokio::test]
async fn the_call_is_authorized_on_its_own_action_and_changes_nothing() {
    // Rows: a caller whose grant names another action, and one whose grant names this
    // action.
    let rows = [
        ("a grant for another action", "iam:ListUsers", false),
        (
            "a grant for the simulation itself",
            "iam:SimulateCustomPolicy",
            true,
        ),
    ];
    for (case, granted, served) in rows {
        let router = router();
        let access_key_id = principal(
            &router,
            case,
            "simulating-caller",
            &[("grant".to_string(), statement("Allow", granted, "*"))],
        )
        .await;

        let (status, body) =
            simulate_signed_by(&router, &access_key_id, &[ALLOW_LISTS], &["iam:ListUsers"]).await;
        if served {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the granted call to be served, got {status}: {body}"
            );
            assert_eq!(
                element(&body, "EvalDecision"),
                ALLOWED,
                "case {case:?}: expected the supplied grant to decide, got {body}"
            );
            // A read: the same call answers the same thing again, and the caller's own
            // inline grant is still there to be read.
            let (status, again) =
                simulate_signed_by(&router, &access_key_id, &[ALLOW_LISTS], &["iam:ListUsers"])
                    .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the second call must be served too, got {status}: {again}"
            );
            assert_eq!(
                reported(&again),
                reported(&body),
                "case {case:?}: a report must not change what a later caller sees, got \
                 {again}"
            );
            let (status, held) = call(
                &router,
                "GetUserPolicy",
                &[("UserName", "simulating-caller"), ("PolicyName", "grant")],
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: the caller's grant must survive the simulation, got \
                 {status}: {held}"
            );
            continue;
        }
        assert_eq!(
            status, 403,
            "case {case:?}: expected the ungranted call to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert!(
            message.contains("is not authorized to perform: iam:SimulateCustomPolicy on resource: *"),
            "case {case:?}: expected the refusal to name the action and the account-wide \
             resource, got {message:?}"
        );
    }
}
