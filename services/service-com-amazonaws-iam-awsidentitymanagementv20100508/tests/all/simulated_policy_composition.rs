// SPDX-License-Identifier: CC0-1.0

//! Which of the documents a simulation was given produced the allow it reports, when
//! ONE resource-based policy is supplied for several principals at once.
//!
//! The rule under test: `MatchedStatements` names every document that determined the
//! reported decision and of which kind it is — an identity-based policy in force on the
//! simulated principal as `SourcePolicyType` "IAM Policy", the policy the request
//! supplied in `ResourcePolicy` as "Resource Policy"
//! (`EvaluationResult$MatchedStatements`, `Statement$SourcePolicyType`) — so an allow
//! two independent grants could each have produced is attributed to the one that did,
//! and a decision no statement stated names none. The response is therefore enough to
//! answer "would removing this document change the answer for this principal?", which
//! the decision alone is not.
//!
//! This suite holds the SUPPLIED document fixed and varies the principal: the same
//! resource-based policy is simulated for a principal whose own identity policy grants
//! the action, for one the supplied policy itself names, for one both grant, and for
//! one neither does — the composition axis
//! `tests/all/simulated_resource_policy.rs` holds fixed by tailoring the document to
//! each row. The rows differ along the service the action belongs to, its ARN form, how
//! the deciding documents scope the resource (the ARN itself, a wildcard run, every
//! resource), and the name the identity policy is stored under.
//!
//! The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns, the simulation
//! request spelling the one `tests/all/policy_simulation.rs` owns, and the attribution
//! readers are `tests/all/simulated_resource_policy.rs`'s, so every simulation suite in
//! this crate reads a response the same way.

use crate::policy_simulation::{principal, simulate, ALLOWED, IMPLICIT_DENY};
use crate::simulated_decision_attribution::allow;
use crate::simulated_resource_policy::{
    attribution, parts, user_inline_policy_id, IDENTITY_POLICY, RESOURCE_POLICY,
    RESOURCE_POLICY_ID,
};
use crate::user_lifecycle::{element, router};

/// One row: one action, on one resource, with the documents in force scoped `scope`.
struct Case {
    case: &'static str,
    action: &'static str,
    /// The resource the simulation asks about. "You can simulate resources that don't
    /// exist in your account" (`SimulatePrincipalPolicy`), so none is created anywhere.
    resource: &'static str,
    /// What the `Resource` element of both deciding documents names: the resource
    /// itself, a wildcard run covering it, or every resource.
    scope: &'static str,
    /// The name the granting identity policy is embedded under, which is what a
    /// `MatchedStatements` entry identifies it by.
    policy_name: &'static str,
}

/// Rows spanning the service, the ARN form and the resource scoping, with the identity
/// policy names spanning the `^[\w+=,.@-]+$` alphabet and both name cases.
const CASES: &[Case] = &[
    Case {
        case: "a queue a send is simulated against",
        action: "sqs:SendMessage",
        resource: "arn:aws:sqs:us-west-2:000000000000:orders",
        scope: "arn:aws:sqs:us-west-2:000000000000:orders",
        policy_name: "send-to-orders",
    },
    Case {
        case: "an object under a wildcard run",
        action: "s3:GetObject",
        resource: "arn:aws:s3:::quarterly-reports/2026/q1.csv",
        scope: "arn:aws:s3:::quarterly-reports/*",
        policy_name: "Read.Reports+2026",
    },
    Case {
        case: "a key with both documents scoped to every resource",
        action: "kms:Encrypt",
        resource: "arn:aws:kms:eu-west-1:000000000000:key/3f6c7a4e-3333-4444-8888-cccccccccccc",
        scope: "*",
        policy_name: "ENCRYPT",
    },
    Case {
        case: "a table a read is simulated against",
        action: "dynamodb:GetItem",
        resource: "arn:aws:dynamodb:ap-southeast-2:000000000000:table/ledger",
        scope: "arn:aws:dynamodb:ap-southeast-2:000000000000:table/ledger",
        policy_name: "ledger_reads",
    },
];

/// What one principal of a row holds: whether its own identity policy grants the
/// action, and whether the supplied resource-based policy names it.
struct Holding {
    case: &'static str,
    /// The user name's suffix, so each principal of a row is a distinct identity.
    at: &'static str,
    identity_grants: bool,
    named_by_resource_policy: bool,
}

/// Every combination of the two independent grants: each alone, both, and neither.
const HOLDINGS: &[Holding] = &[
    Holding {
        case: "the identity policy alone grants",
        at: "ident",
        identity_grants: true,
        named_by_resource_policy: false,
    },
    Holding {
        case: "the supplied resource policy alone grants",
        at: "named",
        identity_grants: false,
        named_by_resource_policy: true,
    },
    Holding {
        case: "both documents grant",
        at: "both",
        identity_grants: true,
        named_by_resource_policy: true,
    },
    Holding {
        case: "neither document grants",
        at: "neither",
        identity_grants: false,
        named_by_resource_policy: false,
    },
];

/// The rule over every (row, principal) pair: the decision is the allow exactly when
/// some document grants, and the statements reported are exactly the documents that did.
#[tokio::test]
async fn an_allow_is_attributed_to_the_documents_that_granted_it() {
    let router = router();

    for (row, case) in CASES.iter().enumerate() {
        let identity_document = allow(case.action, case.scope);
        // Every principal of the row, created BEFORE the resource-based policy, since
        // that document names the ones it grants by the ARN the account holds them
        // under.
        let mut principals: Vec<String> = Vec::new();
        for holding in HOLDINGS {
            let policies: Vec<(&str, &str)> = if holding.identity_grants {
                vec![(case.policy_name, identity_document.as_str())]
            } else {
                Vec::new()
            };
            principals.push(
                principal(
                    &router,
                    case.case,
                    &format!("{}{row}", holding.at),
                    &policies,
                )
                .await,
            );
        }
        // ONE document, supplied unchanged for every principal below: it grants the two
        // the row names and says nothing about the other two.
        let granted: Vec<String> = HOLDINGS
            .iter()
            .zip(&principals)
            .filter(|(holding, _)| holding.named_by_resource_policy)
            .map(|(_, arn)| arn.clone())
            .collect();
        let resource_policy = allow_principals(&granted, case.action, case.scope);
        let extra = vec![("ResourcePolicy".to_string(), resource_policy)];

        for (holding, arn) in HOLDINGS.iter().zip(&principals) {
            let case_name = format!("{}: {}", case.case, holding.case);
            let (status, body) =
                simulate(&router, arn, &[case.action], &[case.resource], &extra).await;
            assert_eq!(
                status, 200,
                "case {case_name:?}: expected 200, got {status}: {body}"
            );

            // A grant from either document suffices; nothing denies in any row, so a
            // principal neither grants is the decision nothing stated.
            let expected_decision = if holding.identity_grants || holding.named_by_resource_policy {
                ALLOWED
            } else {
                IMPLICIT_DENY
            };
            assert_eq!(
                element(&body, "EvalDecision"),
                expected_decision,
                "case {case_name:?}: expected the action to be decided \
                 {expected_decision:?}, got {body}"
            );
            assert_eq!(
                element(&body, "EvalResourceDecision"),
                expected_decision,
                "case {case_name:?}: expected the resource to be decided \
                 {expected_decision:?}, got {body}"
            );

            // The documents that granted, in the order the response reports them: the
            // identity-based policies first, then the resource-based one.
            let mut expected: Vec<(String, String)> = Vec::new();
            if holding.identity_grants {
                expected.push((
                    // An inline policy of the principal this row created, so it is named
                    // by that embedding.
                    user_inline_policy_id(&format!("{}{row}", holding.at), case.policy_name),
                    IDENTITY_POLICY.to_string(),
                ));
            }
            if holding.named_by_resource_policy {
                expected.push((
                    RESOURCE_POLICY_ID.to_string(),
                    RESOURCE_POLICY.to_string(),
                ));
            }
            let (top, per_resource) = parts(&body);
            assert_eq!(
                attribution(top),
                expected,
                "case {case_name:?}: expected the action's result to name {expected:?}, \
                 got {body}"
            );
            assert_eq!(
                attribution(per_resource),
                expected,
                "case {case_name:?}: expected the resource's result to name \
                 {expected:?}, got {body}"
            );
        }
    }
}

/// A resource-based `Allow` of one action on one resource, granted to the principals
/// `principals` names and to nobody else: the statement's principal element is what
/// makes it a resource-based grant, and the list spelling is what lets ONE document be
/// supplied for several simulated principals.
fn allow_principals(principals: &[String], action: &str, resource: &str) -> String {
    let listed: Vec<String> = principals.iter().map(|arn| format!("\"{arn}\"")).collect();
    let listed = listed.join(",");
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"grant","Effect":"Allow","Principal":{{"AWS":[{listed}]}},"Action":"{action}","Resource":"{resource}"}}]}}"#
    )
}
