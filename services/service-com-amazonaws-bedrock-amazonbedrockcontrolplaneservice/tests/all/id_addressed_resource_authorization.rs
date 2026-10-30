// SPDX-License-Identifier: CC0-1.0

//! WHICH RESOURCE the identity-based decision in front of this service is taken about, when
//! the request carries that resource's own ARN resource id in its URI rather than the whole
//! ARN — driven over the real wire path (a restJson1 request through the multi-service
//! router into the mounted service beside IAM and STS on one engine), so what is asserted
//! is what a client sees.
//!
//! THE RULE UNDER TEST: IAM evaluates an action against the ARN of the resource the request
//! ADDRESSES. A request that carries one resource's own ARN resource id in its URI — the
//! bare id the identifier member's modelled form admits beside the whole ARN
//! (`GuardrailIdentifier`'s `^(([a-z0-9]+)|(arn:…:guardrail/[a-z0-9]+))$`,
//! `ModelInvocationJobIdentifier`'s and `AdvancedPromptOptimizationJobIdentifier`'s
//! `^((arn:…/)?[a-z0-9]{12})$`:
//! `smithy_query(query="operation",operation="GetGuardrail,GetModelInvocationJob,GetAdvancedPromptOptimizationJob")`)
//! — addresses THAT resource, so the action is judged against that resource's ARN and the
//! refusal names it. The family's collection ARN belongs to a request that addresses no
//! single member of it: a create, a listing, or a read whose label is a resource NAME,
//! whose id this service minted and which the decision — taken before anything is read —
//! cannot resolve.
//!
//! Two consequences a client depends on, each asserted over a table rather than over one
//! request:
//!
//! 1. A principal an explicit `Deny` of the action covers is refused with the modelled
//!    `AccessDeniedException` (`@httpError(403)`) carrying IAM's sentence about the ARN of
//!    the resource the request named by id, and no part of the resource is disclosed —
//!    while a principal whose policy allows the action is served the very same call.
//! 2. A policy scoped to exactly that resource's ARN serves the call, one scoped to the
//!    family's collection serves it too, and one scoped to ANOTHER member of the same
//!    family does not — which is what separates per-resource least privilege from
//!    per-family.
//!
//! The decision is taken before the store is read, so these rows need no resource to exist:
//! the ids they address are spelled in each family's own id form and the verdict must not
//! depend on whether a resource carries them. The one row that DOES create a guardrail is
//! the disclosure row, which can only be asserted against a guardrail there is a body to
//! disclose.
//!
//! The emulator, the signed call, the seeded session and the policy documents are the
//! harness `crate::custom_model_authorization` owns, reused rather than copied: one rule,
//! one dispatch path.

use serde_json::{json, Value};

use crate::custom_model_authorization::{
    allow, allow_all_but, emulator, given_role_policy, given_session, session_arn, Verdict,
    ACCOUNT_KEY_ID, SESSION_KEY_ID,
};
use crate::support::{label, Reply, ACCOUNT, REGION};

/// One request that addresses a single resource by that resource's own ARN resource id:
/// which operation IAM is asked about, how the call is made, and the resource type and id
/// the URI names.
struct IdAddressed {
    case: &'static str,
    operation: &'static str,
    method: &'static str,
    /// The URI, with `{id}` standing for the label the row addresses.
    path: &'static str,
    body: Option<Value>,
    resource_type: &'static str,
    resource_id: &'static str,
}

impl IdAddressed {
    /// The request target this row sends, with the id written into the path label.
    fn target(&self) -> String {
        self.path.replace("{id}", &label(self.resource_id))
    }

    /// The ARN of the resource this request addresses — what the decision is taken about
    /// and what the refusal must name.
    fn addressed(&self) -> String {
        arn(self.resource_type, self.resource_id)
    }
}

/// The ARN of `resource_id` of `resource_type` in this account and Region, written the way
/// this service writes the ARNs of the resources it mints.
fn arn(resource_type: &str, resource_id: &str) -> String {
    format!("arn:aws:bedrock:{REGION}:{ACCOUNT}:{resource_type}/{resource_id}")
}

/// The collection of `resource_type` in this account and Region: the ARN a request that
/// addresses no single member of the family is judged against.
fn collection(resource_type: &str) -> String {
    arn(resource_type, "*")
}

/// A policy allowing every action of this service on `resource` ALONE — the shape a
/// least-privilege policy scoped to one resource, or to one family, takes.
fn allow_on(resource: &str) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": "Allow", "Action": "bedrock:*", "Resource": resource }]
    })
}

/// Every class of request that addresses one resource by that resource's own id, one row
/// per thing the rule generalises over: the FAMILY whose space it is (the three whose
/// identifier member admits the bare ARN id), the METHOD and operation reaching it (a read,
/// an update, a delete, a create of a sub-resource, a stop), whether a sub-resource segment
/// follows the id, and the id's own spelling inside its family's form.
fn rows() -> Vec<IdAddressed> {
    vec![
        IdAddressed {
            case: "a guardrail read addressed by its id",
            operation: "GetGuardrail",
            method: "GET",
            path: "/guardrails/{id}",
            body: None,
            resource_type: "guardrail",
            resource_id: "abcd1234wxyz",
        },
        IdAddressed {
            case: "a guardrail read of a one-character id",
            operation: "GetGuardrail",
            method: "GET",
            path: "/guardrails/{id}",
            body: None,
            resource_type: "guardrail",
            resource_id: "a",
        },
        IdAddressed {
            case: "a guardrail read of a digits-only id",
            operation: "GetGuardrail",
            method: "GET",
            path: "/guardrails/{id}",
            body: None,
            resource_type: "guardrail",
            resource_id: "0123456789",
        },
        IdAddressed {
            case: "a guardrail update addressed by its id",
            operation: "UpdateGuardrail",
            method: "PUT",
            path: "/guardrails/{id}",
            body: Some(json!({
                "name": "idaddressed",
                "blockedInputMessaging": "blocked in",
                "blockedOutputsMessaging": "blocked out",
            })),
            resource_type: "guardrail",
            resource_id: "abcd1234wxyz",
        },
        IdAddressed {
            case: "a guardrail delete addressed by its id",
            operation: "DeleteGuardrail",
            method: "DELETE",
            path: "/guardrails/{id}",
            body: None,
            resource_type: "guardrail",
            resource_id: "abcd1234wxyz",
        },
        IdAddressed {
            case: "a version published on the guardrail its id names",
            operation: "CreateGuardrailVersion",
            method: "POST",
            path: "/guardrails/{id}",
            body: Some(json!({ "description": "published by the id-addressed suite" })),
            resource_type: "guardrail",
            resource_id: "abcd1234wxyz",
        },
        IdAddressed {
            case: "a batch inference job read addressed by its id",
            operation: "GetModelInvocationJob",
            method: "GET",
            path: "/model-invocation-job/{id}",
            body: None,
            resource_type: "model-invocation-job",
            resource_id: "0123456789ab",
        },
        IdAddressed {
            case: "a stop reaching that same job through a sub-resource segment",
            operation: "StopModelInvocationJob",
            method: "POST",
            path: "/model-invocation-job/{id}/stop",
            body: None,
            resource_type: "model-invocation-job",
            resource_id: "0123456789ab",
        },
        IdAddressed {
            case: "an optimization job read addressed by its id",
            operation: "GetAdvancedPromptOptimizationJob",
            method: "GET",
            path: "/advanced-prompt-optimization-jobs/{id}",
            body: None,
            resource_type: "advanced-prompt-optimization-job",
            resource_id: "0123456789ab",
        },
        IdAddressed {
            case: "a stop reaching that same optimization job",
            operation: "StopAdvancedPromptOptimizationJob",
            method: "POST",
            path: "/advanced-prompt-optimization-jobs/{id}/stop",
            body: None,
            resource_type: "advanced-prompt-optimization-job",
            resource_id: "0123456789ab",
        },
    ]
}

/// RULE, the refusal half: a request that names one resource by its own id is judged
/// against THAT resource, so an explicit `Deny` of the action refuses it with 403
/// `AccessDeniedException` and IAM's sentence about that resource's ARN, while a policy
/// allowing the action serves the identical call.
#[tokio::test]
async fn a_request_naming_a_resource_by_its_id_is_judged_against_that_resource() {
    let emu = emulator();
    given_session(&emu.engine);

    for row in rows() {
        let action = format!("bedrock:{}", row.operation);
        let target = row.target();

        // The explicit deny of exactly this action, beside an allow of everything else.
        given_role_policy(&emu.engine, Some(&allow_all_but(&action)));
        let refused = emu
            .call_as(SESSION_KEY_ID, row.method, &target, row.body.clone())
            .await;
        assert_refused(
            &format!("{:?} under an explicit deny of {action}", row.case),
            &refused,
            &action,
            &row.addressed(),
            Verdict::ExplicitDeny,
        );

        // The positive control: the same call, from a principal whose policy allows the
        // action, is NOT refused — whatever the operation then answers about a resource
        // this suite never created.
        given_role_policy(&emu.engine, Some(&allow(json!(action.clone()))));
        let served = emu
            .call_as(SESSION_KEY_ID, row.method, &target, row.body.clone())
            .await;
        assert_ne!(
            served.error.as_deref(),
            Some("AccessDeniedException"),
            "case {:?}: a policy allowing {action} must not draw AccessDeniedException, got \
             {} {:?}",
            row.case,
            served.status,
            served.body
        );
    }
}

/// RULE, the scope half: the resource a request addresses by id is that ONE resource, so a
/// policy scoped to its ARN serves the call and so does one scoped to the family's
/// collection, while one scoped to another member of the SAME family refuses it — and the
/// refusal names the resource the request addressed rather than the one the policy names.
#[tokio::test]
async fn a_policy_scoped_to_the_addressed_resource_serves_it_and_one_scoped_elsewhere_does_not() {
    let emu = emulator();
    given_session(&emu.engine);

    for row in rows() {
        let action = format!("bedrock:{}", row.operation);
        let target = row.target();
        let addressed = row.addressed();
        // Another member of the same family: the same id form, a different id.
        let sibling = arn(row.resource_type, "zzzzzzzzzzzz");

        // (case, the resource the policy is scoped to, whether the call is served)
        let scopes: &[(&str, String, bool)] = &[
            ("scoped to the resource the request addressed", addressed.clone(), true),
            ("scoped to the family's collection", collection(row.resource_type), true),
            ("scoped to another member of the same family", sibling, false),
            (
                "scoped to the collection of another family",
                collection("imported-model"),
                false,
            ),
        ];

        for (scope_case, scope, served) in scopes {
            given_role_policy(&emu.engine, Some(&allow_on(scope)));
            let reply = emu
                .call_as(SESSION_KEY_ID, row.method, &target, row.body.clone())
                .await;
            let case = format!("{:?} under a policy {scope_case}", row.case);
            if *served {
                assert_ne!(
                    reply.error.as_deref(),
                    Some("AccessDeniedException"),
                    "case {case}: a policy covering {scope} must serve the call, got {} {:?}",
                    reply.status,
                    reply.body
                );
            } else {
                assert_refused(&case, &reply, &action, &addressed, Verdict::DefaultDeny);
            }
        }
    }
}

/// RULE, the disclosure half: the decision stands in FRONT of the handler, so a read a
/// `Deny` covers answers 403 and nothing of the guardrail — not its id, not its ARN, and
/// not the blocked-messaging strings the model marks sensitive — while the account's own
/// credential still reads it back whole.
#[tokio::test]
async fn a_denied_read_addressed_by_id_discloses_nothing_of_the_resource() {
    let emu = emulator();
    given_session(&emu.engine);

    // One guardrail, created with the account's own credential, so there IS a body to
    // withhold.
    let created = emu
        .call_as(
            ACCOUNT_KEY_ID,
            "POST",
            "/guardrails",
            Some(json!({
                "name": "id-addressed-disclosure",
                "blockedInputMessaging": "Input blocked by the id-addressed suite.",
                "blockedOutputsMessaging": "Output blocked by the id-addressed suite.",
                "wordPolicyConfig": { "wordsConfig": [{ "text": "forbidden" }] },
            })),
        )
        .await;
    assert_eq!(
        created.status, 202,
        "the account's own credential must be served the create, got {} {:?}",
        created.status, created.body
    );
    let guardrail_id = created.text("guardrailId");
    let guardrail_arn = created.text("guardrailArn");

    // Addressed by the bare id, exactly as a client that holds only the id does.
    let target = format!("/guardrails/{}", label(&guardrail_id));
    given_role_policy(
        &emu.engine,
        Some(&allow_all_but("bedrock:GetGuardrail")),
    );
    let refused = emu.call_as(SESSION_KEY_ID, "GET", &target, None).await;
    assert_refused(
        "\"a denied read of a guardrail that exists\"",
        &refused,
        "bedrock:GetGuardrail",
        &guardrail_arn,
        Verdict::ExplicitDeny,
    );
    for withheld in [
        "guardrailId",
        "guardrailArn",
        "name",
        "status",
        "wordPolicy",
        "blockedInputMessaging",
        "blockedOutputsMessaging",
    ] {
        assert_eq!(
            refused.get(withheld),
            Value::Null,
            "case \"a denied read of a guardrail that exists\": the refusal must disclose no \
             {withheld}, got {:?}",
            refused.body
        );
    }

    // And the refusal left the guardrail readable to a credential IAM permits.
    let read = emu.call_as(ACCOUNT_KEY_ID, "GET", &target, None).await;
    assert_eq!(
        read.status, 200,
        "case \"a denied read of a guardrail that exists\": the account must still read it, \
         got {} {:?}",
        read.status, read.body
    );
    assert_eq!(
        read.text("guardrailArn"),
        guardrail_arn,
        "case \"a denied read of a guardrail that exists\": expected the same guardrail back, \
         got {:?}",
        read.body
    );
}

/// RULE, the contrast: a label that is NOT that family's own ARN resource id — a resource
/// NAME, or a value of no id form at all — names no single member the decision can resolve
/// without reading the store, so such a request is judged against the family's COLLECTION
/// and a policy scoped to an ARN built from the label grants it nothing.
#[tokio::test]
async fn a_request_naming_a_resource_by_name_is_judged_against_the_family() {
    let emu = emulator();
    given_session(&emu.engine);

    // (case, the operation, the request target's family space, the label, the ARN resource
    // type the family's collection is written with) — every row an identifier member whose
    // modelled form admits a resource NAME beside the ARN rather than the ARN's own id
    // (`smithy_query(query="operation",operation="GetCustomModel,GetImportedModel,GetProvisionedModelThroughput,GetCustomModelDeployment")`),
    // plus one label of a form no resource id of ITS family takes.
    let rows: &[(&str, &str, String, &str, &str)] = &[
        (
            "a custom-model read addressed by the model's name",
            "GetCustomModel",
            "/custom-models/{label}".to_string(),
            "my-custom-model",
            "custom-model",
        ),
        (
            "an imported-model read addressed by the model's name",
            "GetImportedModel",
            "/imported-models/{label}".to_string(),
            "my-imported-model",
            "imported-model",
        ),
        (
            "a throughput read addressed by the provisioned model's name",
            "GetProvisionedModelThroughput",
            "/provisioned-model-throughput/{label}".to_string(),
            "my-throughput",
            "provisioned-model",
        ),
        (
            "a deployment read addressed by the deployment's name",
            "GetCustomModelDeployment",
            "/model-customization/custom-model-deployments/{label}".to_string(),
            "my-deployment",
            "custom-model-deployment",
        ),
        (
            // The guardrails space DOES address its members by id, but only a label of the
            // family's own id form is one: `GuardrailId` is `^[a-z0-9]+$`.
            "a guardrail read whose label is of no resource-id form",
            "GetGuardrail",
            "/guardrails/{label}".to_string(),
            "Not-An-Id",
            "guardrail",
        ),
    ];

    for (case, operation, path, name, resource_type) in rows {
        let action = format!("bedrock:{operation}");
        let target = path.replace("{label}", &label(name));

        // An ARN built from the LABEL grants nothing, because the label is not the id.
        given_role_policy(&emu.engine, Some(&allow_on(&arn(resource_type, name))));
        let refused = emu.call_as(SESSION_KEY_ID, "GET", &target, None).await;
        assert_refused(
            &format!("{case:?} under a policy scoped to an ARN built from the name"),
            &refused,
            &action,
            &collection(resource_type),
            Verdict::DefaultDeny,
        );

        // The family's collection is what such a request addresses, so that serves it.
        given_role_policy(&emu.engine, Some(&allow_on(&collection(resource_type))));
        let served = emu.call_as(SESSION_KEY_ID, "GET", &target, None).await;
        assert_ne!(
            served.error.as_deref(),
            Some("AccessDeniedException"),
            "case {case:?}: a policy scoped to the family must serve the call, got {} {:?}",
            served.status,
            served.body
        );
    }
}

/// The refusal one row expects: 403, the modelled error code, and IAM's own sentence with
/// every part — principal, action, resource, reason — taken from the request and the
/// stored policy.
fn assert_refused(case: &str, reply: &Reply, action: &str, resource: &str, verdict: Verdict) {
    assert_eq!(
        reply.status, 403,
        "case {case}: expected the modelled 403, got {} {:?}",
        reply.status, reply.body
    );
    assert_eq!(
        reply.error.as_deref(),
        Some("AccessDeniedException"),
        "case {case}: expected AccessDeniedException, got {:?}",
        reply.error
    );
    let expected = format!(
        "User: {} is not authorized to perform: {action} on resource: {resource} {}",
        session_arn(),
        verdict.reason(action)
    );
    assert_eq!(
        reply.get("message"),
        json!(expected),
        "case {case}: expected IAM's wording {expected:?}, got {:?}",
        reply.body
    );
}
