// SPDX-License-Identifier: CC0-1.0

//! Every IAM operation is decided against the caller's identity-based policies, so a
//! policy statement that names an action really governs that action.
//!
//! The rule under test: IAM authorizes its OWN API — each operation evaluates the signing
//! principal's identity-based policies on the action it names and the ARN of the entity it
//! addresses before it reports or changes anything, and a call nothing in force allows is
//! refused HTTP 403 `AccessDenied`. The consequence this suite pins is the one a policy
//! author depends on: an action is DENIABLE. A grant that names one action and not another
//! serves the first and refuses the second, so moving a grant from one action to the other
//! swaps both answers.
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the kind
//! of entity the call addresses (user, group, role, managed policy, instance profile),
//! whether the call reports or changes it, and how the entity is named (a friendly name or
//! an ARN). No single operation is the specification.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and the other suites
//! in this crate share.

use smithy_server_runtime::MultiServiceRouter;

use crate::identity_policy_enforcement::{principal, statement};
// The user-ARN spelling is the one `tests/all/request_condition_context.rs` already owns, so
// this suite adds no second copy of it; the four other entity kinds it spans have no shared
// helper yet, so theirs are written once below.
use crate::request_condition_context::user_arn;
use crate::user_lifecycle::{account_id, call, call_signed_by, element, error_code, router};

/// The caller every row signs as.
const CALLER: &str = "gated-caller";

/// The fixture entities the rows address, one per IAM entity kind the table spans.
const GROUP: &str = "gated-group";
const ROLE: &str = "gated-role";
const PROFILE: &str = "gated-profile";
const POLICY: &str = "gated-policy";

/// The policy name a row's grant is put under on the caller.
const GRANT_NAME: &str = "grant";

/// A trust policy `CreateRole` accepts: the account's own principal, which is what the
/// other role suites in this crate create a fixture role with.
fn trust_policy() -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Principal":{{"AWS":"arn:aws:iam::{}:root"}},"Action":"sts:AssumeRole"}}]}}"#,
        account_id()
    )
}

/// A permission document a managed or inline policy can carry.
const SOME_DOCUMENT: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"iam:ListUsers","Resource":"*"}]}"#;

/// One row: an operation that addresses ONE named IAM entity, the IAM action its decision
/// is made on, the ARN that decision names, how the refusal spells that entity, and the
/// form parameters the call carries.
struct Gated {
    /// The awsQuery `Action`.
    action: &'static str,
    /// The IAM action a statement has to name to govern the call.
    iam_action: &'static str,
    /// The resource element a grant for this call needs, as a function of the account id.
    resource: fn() -> String,
    /// How the refusal NAMES the entity the request addressed.
    named_as: fn() -> String,
    /// The call's own parameters.
    params: fn() -> Vec<(String, String)>,
}

/// The ARN of an IAM group of this account at the root path.
fn group_arn(group_name: &str) -> String {
    format!("arn:aws:iam::{}:group/{group_name}", account_id())
}

/// The ARN of an IAM role of this account at the root path.
fn role_arn(role_name: &str) -> String {
    format!("arn:aws:iam::{}:role/{role_name}", account_id())
}

/// The ARN of an IAM instance profile of this account at the root path.
fn instance_profile_arn(profile_name: &str) -> String {
    format!(
        "arn:aws:iam::{}:instance-profile/{profile_name}",
        account_id()
    )
}

/// The ARN of a customer-managed policy of this account at the root path.
fn policy_arn(policy_name: &str) -> String {
    format!("arn:aws:iam::{}:policy/{policy_name}", account_id())
}

/// The table. Rows span the five entity kinds and both directions — a report and a
/// change — so the rule is asserted over the class rather than over one operation.
const GATED: &[Gated] = &[
    Gated {
        action: "ListAttachedUserPolicies",
        iam_action: "iam:ListAttachedUserPolicies",
        resource: || user_arn(CALLER),
        named_as: || format!("user {CALLER}"),
        params: || vec![("UserName".to_string(), CALLER.to_string())],
    },
    Gated {
        action: "ListAttachedGroupPolicies",
        iam_action: "iam:ListAttachedGroupPolicies",
        resource: || group_arn(GROUP),
        named_as: || format!("group {GROUP}"),
        params: || vec![("GroupName".to_string(), GROUP.to_string())],
    },
    Gated {
        action: "ListAttachedRolePolicies",
        iam_action: "iam:ListAttachedRolePolicies",
        resource: || role_arn(ROLE),
        named_as: || format!("role {ROLE}"),
        params: || vec![("RoleName".to_string(), ROLE.to_string())],
    },
    Gated {
        action: "ListGroupPolicies",
        iam_action: "iam:ListGroupPolicies",
        resource: || group_arn(GROUP),
        named_as: || format!("group {GROUP}"),
        params: || vec![("GroupName".to_string(), GROUP.to_string())],
    },
    Gated {
        action: "ListRolePolicies",
        iam_action: "iam:ListRolePolicies",
        resource: || role_arn(ROLE),
        named_as: || format!("role {ROLE}"),
        params: || vec![("RoleName".to_string(), ROLE.to_string())],
    },
    Gated {
        action: "GetRole",
        iam_action: "iam:GetRole",
        resource: || role_arn(ROLE),
        named_as: || format!("role {ROLE}"),
        params: || vec![("RoleName".to_string(), ROLE.to_string())],
    },
    Gated {
        action: "GetInstanceProfile",
        iam_action: "iam:GetInstanceProfile",
        resource: || instance_profile_arn(PROFILE),
        named_as: || format!("instance profile {PROFILE}"),
        params: || vec![("InstanceProfileName".to_string(), PROFILE.to_string())],
    },
    Gated {
        action: "GetPolicy",
        iam_action: "iam:GetPolicy",
        resource: || policy_arn(POLICY),
        named_as: || format!("policy {}", policy_arn(POLICY)),
        params: || vec![("PolicyArn".to_string(), policy_arn(POLICY))],
    },
    Gated {
        action: "ListPolicyVersions",
        iam_action: "iam:ListPolicyVersions",
        resource: || policy_arn(POLICY),
        named_as: || format!("policy {}", policy_arn(POLICY)),
        params: || vec![("PolicyArn".to_string(), policy_arn(POLICY))],
    },
    Gated {
        action: "ListEntitiesForPolicy",
        iam_action: "iam:ListEntitiesForPolicy",
        resource: || policy_arn(POLICY),
        named_as: || format!("policy {}", policy_arn(POLICY)),
        params: || vec![("PolicyArn".to_string(), policy_arn(POLICY))],
    },
    Gated {
        action: "AddUserToGroup",
        iam_action: "iam:AddUserToGroup",
        resource: || group_arn(GROUP),
        named_as: || format!("group {GROUP}"),
        params: || {
            vec![
                ("GroupName".to_string(), GROUP.to_string()),
                ("UserName".to_string(), CALLER.to_string()),
            ]
        },
    },
    Gated {
        action: "PutRolePolicy",
        iam_action: "iam:PutRolePolicy",
        resource: || role_arn(ROLE),
        named_as: || format!("role {ROLE}"),
        params: || {
            vec![
                ("RoleName".to_string(), ROLE.to_string()),
                ("PolicyName".to_string(), "embedded".to_string()),
                ("PolicyDocument".to_string(), SOME_DOCUMENT.to_string()),
            ]
        },
    },
    Gated {
        action: "DeleteInstanceProfile",
        iam_action: "iam:DeleteInstanceProfile",
        resource: || instance_profile_arn(PROFILE),
        named_as: || format!("instance profile {PROFILE}"),
        params: || vec![("InstanceProfileName".to_string(), PROFILE.to_string())],
    },
];

/// Create the entities the rows address. Every call here is UNSIGNED, which carries no IAM
/// user's credentials and so no identity-based policies to judge: the fixture is built
/// outside the gate the rows are about.
async fn entities(router: &MultiServiceRouter, case: &str) {
    for (action, params) in [
        ("CreateGroup", vec![("GroupName", GROUP)]),
        ("CreateInstanceProfile", vec![("InstanceProfileName", PROFILE)]),
    ] {
        let (status, body) = call(router, action, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the fixture {action} must succeed, got {status}: {body}"
        );
    }
    let trust = trust_policy();
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", ROLE),
            ("AssumeRolePolicyDocument", trust.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreateRole must succeed, got {status}: {body}"
    );
    let (status, body) = call(
        router,
        "CreatePolicy",
        &[("PolicyName", POLICY), ("PolicyDocument", SOME_DOCUMENT)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture CreatePolicy must succeed, got {status}: {body}"
    );
}

/// Make the row's call, signed by `access_key_id`.
async fn invoke(
    router: &MultiServiceRouter,
    access_key_id: &str,
    row: &Gated,
) -> (u16, String) {
    let owned = (row.params)();
    let params: Vec<(&str, &str)> = owned
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    call_signed_by(router, Some(access_key_id), row.action, &params).await
}

/// RULE: a call nothing in the caller's identity-based policies allows is refused HTTP 403
/// `AccessDenied`, whichever operation it is, and the refusal names the action it asked for
/// and the entity it addressed.
#[tokio::test]
async fn an_operation_nothing_allows_is_refused() {
    for row in GATED {
        let case = format!("{} with no grant", row.action);
        let router = router();
        entities(&router, &case).await;
        // The caller holds ONE document, and it grants an action no row calls, so the row's
        // refusal is the absence of a grant for ITS action rather than the absence of any
        // document at all.
        let access_key_id = principal(
            &router,
            &case,
            CALLER,
            &[(
                GRANT_NAME.to_string(),
                statement("Allow", "iam:ListUsers", "*"),
            )],
        )
        .await;

        let (status, body) = invoke(&router, &access_key_id, row).await;
        assert_eq!(
            status, 403,
            "case {case:?}: expected the ungranted call to be refused, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        let expected = format!(
            "is not authorized to perform: {} on resource: {}",
            row.iam_action,
            (row.named_as)()
        );
        assert!(
            message.contains(&expected),
            "case {case:?}: expected the refusal to read {expected:?}, got {message:?}"
        );
        assert!(
            message.contains(&format!(
                "because no identity-based policy allows the {} action",
                row.iam_action
            )),
            "case {case:?}: expected the refusal to end at the reason, got {message:?}"
        );
    }
}

/// RULE: the same call under a statement that names ITS action on the ARN it addresses is
/// served — so the gate decides the call from the policies rather than refusing everything.
#[tokio::test]
async fn an_operation_a_statement_names_is_served() {
    for row in GATED {
        let case = format!("{} under its own grant", row.action);
        let router = router();
        entities(&router, &case).await;
        let access_key_id = principal(
            &router,
            &case,
            CALLER,
            &[(
                GRANT_NAME.to_string(),
                statement("Allow", row.iam_action, &(row.resource)()),
            )],
        )
        .await;

        let (status, body) = invoke(&router, &access_key_id, row).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the granted call to be served, got {status}: {body}"
        );
    }
}

/// RULE: because each action is decided on its own, MOVING a grant from one action to
/// another swaps both answers — the action the document no longer names is refused while
/// the one it now names is served. That is the property a policy author's sentinel probe
/// depends on, and the reason an ungated operation (always served, never deniable) breaks
/// a set-operator comparison that is observed through one.
#[tokio::test]
async fn moving_a_grant_between_two_actions_swaps_both_answers() {
    // Rows: the pair of user-addressed reads the grant moves between, in both directions,
    // so neither answer is a property of one operation.
    let probes = [
        ("iam:ListAttachedUserPolicies", "ListAttachedUserPolicies"),
        ("iam:ListGroupsForUser", "ListGroupsForUser"),
    ];
    for (granted_index, (granted_iam, _)) in probes.iter().enumerate() {
        let case = format!("the grant names {granted_iam}");
        let router = router();
        let access_key_id = principal(
            &router,
            &case,
            CALLER,
            &[(
                GRANT_NAME.to_string(),
                statement("Allow", granted_iam, &user_arn(CALLER)),
            )],
        )
        .await;

        for (index, (iam_action, query_action)) in probes.iter().enumerate() {
            let (status, body) = call_signed_by(
                &router,
                Some(&access_key_id),
                query_action,
                &[("UserName", CALLER)],
            )
            .await;
            if index == granted_index {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected {iam_action} to be served under the grant that \
                     names it, got {status}: {body}"
                );
            } else {
                assert_eq!(
                    status, 403,
                    "case {case:?}: expected {iam_action} to be refused while the grant names \
                     the other action, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "AccessDenied",
                    "case {case:?}: expected AccessDenied for {iam_action}, got {:?}",
                    error_code(&body)
                );
            }
        }
    }
}
