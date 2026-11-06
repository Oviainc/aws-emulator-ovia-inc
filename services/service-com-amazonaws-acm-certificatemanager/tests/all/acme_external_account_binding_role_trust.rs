// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the IAM role an ACME external account binding binds, over the wire
//! (`POST /`, `X-Amz-Target: CertificateManager.CreateAcmeExternalAccountBinding`,
//! awsJson1_1).
//!
//! RULE: a request that hands this service a role to ACT AS is accepted only where the
//! account's IAM record for that role carries a trust policy granting this service's own
//! service principal every STS action it performs when it assumes the role — the assume
//! itself, the source identity it sets and the session tags it attaches — and a role whose
//! trust policy withholds any one of them (or explicitly denies it, or names another
//! service's principal) is refused with HTTP 403 `AccessDeniedException` and nothing is
//! created. Nothing is refused on evidence the emulator does not have: a `RoleArn` the IAM
//! store holds no role for, a role carrying no trust document, and a document that cannot
//! be read as a policy each show nothing about the role and let the create through.
//!
//! Every assertion goes through [`MultiServiceRouter::dispatch`], the same path the
//! emulator serves production traffic on; the roles live in the IAM store of the SAME
//! shared engine — the records `iam:CreateRole` / `iam:UpdateAssumeRolePolicy` write — and
//! every accepted binding is read back through a DIFFERENT operation
//! (`DescribeAcmeExternalAccountBinding`, `ListAcmeExternalAccountBindings`), so a response
//! that is not computed from the persisted store cannot pass.
//!
//! The table spans what the rule generalises over: how many of the actions the trust
//! document names and how they are spread over statements, the wildcards IAM admits in an
//! action's place, the forms `Action` and `Principal.Service` take (one string or a list),
//! the casing, the effect, a role held under a PATH, and each of the three cases that are
//! evidence of nothing.

use serde_json::{json, Value};
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;
use smithy_server_runtime::MultiServiceRouter;

use crate::acme_authorization::router_with_identity;
use crate::acme_domain_validation_lifecycle::endpoint_body;
use crate::acme_endpoint_lifecycle::create_endpoint;
use crate::acme_external_account_binding_lifecycle::describe_binding;
use crate::certificate_lifecycle::{call, error_code, error_message, ACCOUNT};

/// The service principal this service assumes a bound role as, as the recorded refusal
/// names it.
const SERVICE_PRINCIPAL: &str = "acm-acme.amazonaws.com";

/// How one row's `RoleArn` is backed in IAM: `Some(Some(document))` a role carrying that
/// trust document, `Some(None)` a role carrying none at all, and `None` no IAM role behind
/// the ARN the request names.
type RoleSeed = Option<Option<String>>;

/// One row: what it exercises, how the role is backed in IAM, whether the role is held
/// under a path, and whether the create must be accepted.
type RoleCase = (&'static str, RoleSeed, &'static str, bool);

/// A trust policy with one statement, written as `iam:CreateRole` would be handed it.
fn statement(effect: &str, action: &str, service: &str) -> String {
    format!(
        "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"{effect}\",\
         \"Principal\":{{\"Service\":{service}}},\"Action\":{action}}}]}}"
    )
}

/// The table. The actions named are the ones the refusal below lists, which is what this
/// service performs on a role it assumes.
fn role_cases() -> Vec<RoleCase> {
    vec![
        (
            "every action in one statement, the principal as a string",
            Some(Some(statement(
                "Allow",
                "[\"sts:AssumeRole\",\"sts:SetSourceIdentity\",\"sts:TagSession\"]",
                &format!("\"{SERVICE_PRINCIPAL}\""),
            ))),
            "/",
            true,
        ),
        (
            "the three actions spread over three statements",
            Some(Some(format!(
                "{{\"Version\":\"2012-10-17\",\"Statement\":[{}]}}",
                ["sts:AssumeRole", "sts:SetSourceIdentity", "sts:TagSession"]
                    .iter()
                    .map(|action| format!(
                        "{{\"Effect\":\"Allow\",\
                         \"Principal\":{{\"Service\":\"{SERVICE_PRINCIPAL}\"}},\
                         \"Action\":\"{action}\"}}"
                    ))
                    .collect::<Vec<String>>()
                    .join(",")
            ))),
            "/",
            true,
        ),
        (
            "a service wildcard covering every action",
            Some(Some(statement(
                "Allow",
                "\"sts:*\"",
                &format!("\"{SERVICE_PRINCIPAL}\""),
            ))),
            "/",
            true,
        ),
        (
            "every action, the principal in a list beside another service's",
            Some(Some(statement(
                "Allow",
                "[\"sts:AssumeRole\",\"sts:TagSession\",\"sts:SetSourceIdentity\"]",
                &format!("[\"ec2.amazonaws.com\",\"{SERVICE_PRINCIPAL}\"]"),
            ))),
            "/",
            true,
        ),
        (
            "every action and the principal named in another casing",
            Some(Some(statement(
                "Allow",
                "[\"STS:ASSUMEROLE\",\"sts:setsourceidentity\",\"Sts:TagSession\"]",
                "\"ACM-Acme.AmazonAWS.com\"",
            ))),
            "/",
            true,
        ),
        (
            "a role held under a path, whose trust policy admits the principal",
            Some(Some(statement(
                "Allow",
                "\"*\"",
                &format!("\"{SERVICE_PRINCIPAL}\""),
            ))),
            "/service-role/",
            true,
        ),
        (
            "another service's principal only",
            Some(Some(statement(
                "Allow",
                "\"sts:AssumeRole\"",
                "\"ec2.amazonaws.com\"",
            ))),
            "/",
            false,
        ),
        (
            "the assume alone, without the session actions",
            Some(Some(statement(
                "Allow",
                "\"sts:AssumeRole\"",
                &format!("\"{SERVICE_PRINCIPAL}\""),
            ))),
            "/",
            false,
        ),
        (
            "the session actions without the assume",
            Some(Some(statement(
                "Allow",
                "[\"sts:TagSession\",\"sts:SetSourceIdentity\"]",
                &format!("\"{SERVICE_PRINCIPAL}\""),
            ))),
            "/",
            false,
        ),
        (
            "every action, explicitly denied",
            Some(Some(statement(
                "Deny",
                "[\"sts:AssumeRole\",\"sts:SetSourceIdentity\",\"sts:TagSession\"]",
                &format!("\"{SERVICE_PRINCIPAL}\""),
            ))),
            "/",
            false,
        ),
        (
            "a document that carries no statement at all",
            Some(Some("{\"Version\":\"2012-10-17\"}".to_string())),
            "/",
            false,
        ),
        (
            "no IAM role behind the ARN: evidence of nothing",
            None,
            "/",
            true,
        ),
        (
            "a stored role carrying no trust document: evidence of nothing",
            Some(None),
            "/",
            true,
        ),
        (
            "a trust document that is not a policy: evidence of nothing",
            Some(Some("not a policy at all".to_string())),
            "/",
            true,
        ),
    ]
}

/// The sentence this service refuses a role it cannot assume with: fixed text apart from
/// the `RoleArn` echoed from the request.
fn unable_to_assume(role_arn: &str) -> String {
    format!(
        "Unable to assume role for {role_arn}. Verify that the role exists and its trust \
         policy grants {SERVICE_PRINCIPAL} access to sts:AssumeRole, sts:SetSourceIdentity, \
         and sts:TagSession."
    )
}

/// The ARN the account holds a role under, as `iam:CreateRole` mints it.
fn role_arn(path: &str, role_name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role{path}{role_name}")
}

/// Record a role in the IAM store of the shared engine, carrying `trust` as its
/// `AssumeRolePolicyDocument` — the record `iam:CreateRole` writes — and answer the ARN it
/// is held under. A row seeding no role still gets an ARN, which no IAM record answers to.
fn given_role(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    index: usize,
    seed: &RoleSeed,
    path: &str,
) -> String {
    let role_name = format!("acme-eab-role-{index}");
    let arn = role_arn(path, &role_name);
    let Some(trust) = seed else {
        return arn;
    };
    let store = iam::service_state(&**engine).expect("the IAM store");
    store.lock().roles.insert(
        iam::name_key(&role_name),
        ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::Role {
            path: path.to_string(),
            role_name,
            role_id: format!("AROAEXAMPLEACMEEAB{index:02}"),
            arn: arn.clone(),
            create_date: ::aws_smithy_types::DateTime::from_secs(0),
            assume_role_policy_document: trust.clone(),
            description: None,
            max_session_duration: None,
            permissions_boundary: None,
            tags: None,
            role_last_used: None,
            source_role_template: None,
        },
    );
    arn
}

/// Every binding `ListAcmeExternalAccountBindings` reports for `endpoint_arn`.
async fn listed_bindings(
    router: &MultiServiceRouter,
    case: &str,
    endpoint_arn: &str,
) -> Vec<Value> {
    let (status, response) = call(
        router,
        "ListAcmeExternalAccountBindings",
        &json!({ "AcmeEndpointArn": endpoint_arn }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListAcmeExternalAccountBindings must succeed, got {status} {response}"
    );
    response["ExternalAccountBindings"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: no ExternalAccountBindings list in {response}"))
        .clone()
}

/// RULE: the create is accepted exactly where the role's trust policy admits this
/// service's principal for every action it performs on the role; a role it withholds is
/// refused 403 `AccessDeniedException` with the sentence above, and the store gains
/// nothing.
#[tokio::test]
async fn a_bound_role_is_accepted_only_where_its_trust_policy_admits_this_service() {
    let (router, engine) = router_with_identity();
    let endpoint_arn = create_endpoint(&router, "binding host", &endpoint_body()).await;

    let mut accepted: Vec<String> = Vec::new();
    for (index, (what, seed, path, must_accept)) in role_cases().into_iter().enumerate() {
        let role = given_role(&engine, index, &seed, path);
        let (status, response) = call(
            &router,
            "CreateAcmeExternalAccountBinding",
            &json!({ "AcmeEndpointArn": endpoint_arn, "RoleArn": role }),
        )
        .await;

        if must_accept {
            assert_eq!(
                status, 200,
                "case {what:?}: the create must be accepted for {role}, got {status} {response}"
            );
            let arn = response["ExternalAccountBinding"]["AcmeExternalAccountBindingArn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {what:?}: the create reported no ARN: {response}"))
                .to_string();
            // Read the stored record back through a different operation: the binding is
            // persisted, carrying the role the request bound.
            let described = describe_binding(&router, what, &arn).await;
            assert_eq!(
                described["RoleArn"].as_str(),
                Some(role.as_str()),
                "case {what:?}: expected the stored RoleArn {role}, got {described}"
            );
            accepted.push(arn);
        } else {
            assert_eq!(
                status, 403,
                "case {what:?}: the create must be refused for {role}, got {status} {response}"
            );
            let code = error_code(&response);
            assert_eq!(
                code, "AccessDeniedException",
                "case {what:?}: expected AccessDeniedException, got {code:?} in {response}"
            );
            let message = error_message(&response);
            assert_eq!(
                message,
                unable_to_assume(&role),
                "case {what:?}: expected ACM's wording, got {message:?}"
            );
        }

        // Nothing but the accepted creates is stored: a refused create leaves the endpoint
        // holding exactly the bindings the accepted rows made. The listing has its own
        // order, so what is compared is the SET.
        let listed = listed_bindings(&router, what, &endpoint_arn).await;
        let mut reported: Vec<String> = listed
            .iter()
            .map(|binding| {
                binding["AcmeExternalAccountBindingArn"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        reported.sort();
        let mut expected = accepted.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {what:?}: expected exactly the accepted bindings {expected:?}, got {reported:?}"
        );
        let bound_roles: Vec<&str> = listed
            .iter()
            .map(|binding| binding["RoleArn"].as_str().unwrap_or_default())
            .collect();
        assert_eq!(
            bound_roles.contains(&role.as_str()),
            must_accept,
            "case {what:?}: expected {role} to be listed only where the create was accepted, \
             got {bound_roles:?}"
        );
    }
}

/// RULE (the same one, on the state a refused create must not leave behind): a create
/// refused for its role records nothing at all — not even the `IdempotencyToken` it
/// carried, so the same token is free to create a binding once the role is one this
/// service can assume.
#[tokio::test]
async fn a_create_refused_for_its_role_records_no_idempotency_token() {
    let (router, engine) = router_with_identity();
    let endpoint_arn = create_endpoint(&router, "token host", &endpoint_body()).await;
    let token = "acme-eab-token-01";

    // A role that trusts another service only, so the create is refused.
    let untrusted = given_role(
        &engine,
        0,
        &Some(Some(statement(
            "Allow",
            "\"sts:AssumeRole\"",
            "\"ec2.amazonaws.com\"",
        ))),
        "/",
    );
    let (status, response) = call(
        &router,
        "CreateAcmeExternalAccountBinding",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "RoleArn": untrusted,
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 403,
        "case \"refused create\": expected 403, got {status} {response}"
    );

    // The SAME token, now with a role this service can assume: a token the refused call
    // had recorded would make this a conflict.
    let trusted = given_role(
        &engine,
        1,
        &Some(Some(statement(
            "Allow",
            "[\"sts:AssumeRole\",\"sts:SetSourceIdentity\",\"sts:TagSession\"]",
            &format!("\"{SERVICE_PRINCIPAL}\""),
        ))),
        "/",
    );
    let (status, response) = call(
        &router,
        "CreateAcmeExternalAccountBinding",
        &json!({
            "AcmeEndpointArn": endpoint_arn,
            "RoleArn": trusted,
            "IdempotencyToken": token,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"the token after a refusal\": the create must be accepted, \
         got {status} {response}"
    );
    assert_eq!(
        response["ExternalAccountBinding"]["RoleArn"].as_str(),
        Some(trusted.as_str()),
        "case \"the token after a refusal\": expected the binding to carry {trusted}, \
         got {response}"
    );
}
