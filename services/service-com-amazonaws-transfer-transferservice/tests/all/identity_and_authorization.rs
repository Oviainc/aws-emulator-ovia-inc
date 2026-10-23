// SPDX-License-Identifier: CC0-1.0

//! Who may ask, and which IAM roles this service will accept.
//!
//! Four rules, each asserted over a TABLE whose rows differ along what the rule
//! generalises over — which operation, which policy shape, which account, which
//! member:
//!
//! 1. A request whose calling principal's identity policy EXPLICITLY DENIES the
//!    `transfer:<Operation>` the request performs is refused with
//!    `AccessDeniedException` (HTTP 403) and performs nothing.
//! 2. `CreateServer`/`UpdateServer` VERIFY an `API_GATEWAY` identity provider
//!    synchronously: the invocation role it names must be a role this account holds
//!    that may invoke the API the URL names, or the write is refused with
//!    `InvalidRequestException` (HTTP 400).
//! 3. A value that fails a member's ARN-shaped modelled pattern is refused with this
//!    service's OWN `InvalidRequestException`, not the framework's unmodelled
//!    `ValidationException`.
//! 4. A write that passes an IAM role validates the role's ACCOUNT before resolving
//!    any resource the request names, refusing another account's role with
//!    `AccessDeniedException`.
//!
//! Every request goes over the real wire path (`POST /` with
//! `X-Amz-Target: TransferService.<Op>`) through the shared [`crate::harness`].

use serde_json::{json, Value};

use crate::harness::{member_of, Emulator, ROLE};

/// A policy allowing every action of this service on every resource — the "the
/// caller is permitted" baseline the deny rows are added to.
const ALLOW_ALL_TRANSFER: &str = r#"{"Version":"2012-10-17","Statement":[
    {"Effect":"Allow","Action":"transfer:*","Resource":"*"}]}"#;

/// A policy allowing an API Gateway invoke on every resource: what an `API_GATEWAY`
/// identity provider's invocation role needs.
const ALLOW_INVOKE_ANY_API: &str = r#"{"Version":"2012-10-17","Statement":[
    {"Effect":"Allow","Action":"execute-api:Invoke","Resource":"*"}]}"#;

/// The invoke URL of an API Gateway REST API:
/// `https://<api-id>.execute-api.<region>.amazonaws.com/<stage>`.
const IDP_URL: &str = "https://abc123.execute-api.us-east-1.amazonaws.com/prod";

/// A `ServerId` that satisfies the modelled `^s-([0-9a-f]{17})$` without ever having
/// been minted: the rows that use it are refused before any lookup.
const UNMINTED_SERVER_ID: &str = "s-0000000000000dead";

/// Assert `answer` is the modelled 403 `AccessDeniedException` whose message names
/// each fragment in `named`.
fn assert_access_denied(case: &str, answer: &crate::harness::Answer, named: &[&str]) {
    assert_eq!(
        answer.status, 403,
        "case {case:?}: expected HTTP 403, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "AccessDeniedException",
        "case {case:?}: expected AccessDeniedException, got body {}",
        answer.body
    );
    let message = member_of(&answer.body, "Message")
        .as_str()
        .unwrap_or_default()
        .to_string();
    for fragment in named {
        assert!(
            message.contains(fragment),
            "case {case:?}: expected the refusal to name {fragment:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 1. The explicit-deny gate, over every implemented operation class.
// ---------------------------------------------------------------------------

/// RULE: a request whose calling principal's identity policy explicitly denies its
/// `transfer:<Operation>` is refused with `AccessDeniedException` (403), and the same
/// principal's other actions are unaffected.
///
/// The rows differ along the operation (a create, a read, a list, an update, a
/// delete), and the table is driven twice: once against a principal whose policy
/// allows everything and denies ONE action, and once against a principal with no
/// policy at all, which may do all of them.
#[tokio::test]
async fn an_explicit_deny_on_one_action_refuses_only_that_action() {
    // (case, operation, request)
    let rows: Vec<(&str, &str, Value)> = vec![
        ("a create", "CreateServer", json!({})),
        ("a read", "DescribeServer", json!({})),
        ("a list", "ListServers", json!({})),
        ("an update", "UpdateServer", json!({})),
        ("a delete", "DeleteServer", json!({})),
        ("a user read", "DescribeUser", json!({"UserName": "u-gate"})),
        ("a user list", "ListUsers", json!({})),
        (
            "a user update",
            "UpdateUser",
            json!({"UserName": "u-gate", "HomeDirectory": "/bucket/u-gate"}),
        ),
        (
            "a key import",
            "ImportSshPublicKey",
            json!({"UserName": "u-gate", "SshPublicKeyBody": crate::harness::SSH_KEY}),
        ),
        ("a user delete", "DeleteUser", json!({"UserName": "u-gate"})),
        (
            "an access create",
            "CreateAccess",
            json!({
                "ExternalId": "S-1-1-12-1234567890-123456789-1234567890-1234",
                "Role": ROLE,
            }),
        ),
    ];

    for (case, operation, body) in rows {
        // The server and user the row addresses are created BEFORE the principal is
        // seeded, so the setup is never the thing the gate refuses: the policy under
        // test is in place only for the one request below.
        let denied = Emulator::new();
        let server_id = denied.create_server(json!({})).await;
        denied.create_user(&server_id, "u-gate").await;
        // The principal is denied exactly this operation and allowed the rest, so a
        // refusal can only come from the deny this row names.
        denied.seed_calling_principal(
            "gated-user",
            &format!(
                r#"{{"Version":"2012-10-17","Statement":[
                    {{"Effect":"Allow","Action":"transfer:*","Resource":"*"}},
                    {{"Effect":"Deny","Action":"transfer:{operation}","Resource":"*"}}]}}"#
            ),
        );

        let mut request = body.clone();
        if operation != "CreateServer" && operation != "ListServers" {
            request["ServerId"] = json!(&server_id);
        }
        let answer = denied.call(operation, request).await;
        assert_access_denied(
            case,
            &answer,
            &[&format!("transfer:{operation}"), "explicit deny"],
        );

        // The same request from a principal whose policies deny nothing is served:
        // the gate refuses on the stored deny, not on being gated at all.
        let permitted = Emulator::new();
        let server_id = permitted.create_server(json!({})).await;
        permitted.create_user(&server_id, "u-gate").await;
        permitted.seed_calling_principal("ungated-user", ALLOW_ALL_TRANSFER);
        let mut request = body;
        if operation != "CreateServer" && operation != "ListServers" {
            request["ServerId"] = json!(&server_id);
        }
        let answer = permitted.call(operation, request).await;
        assert_ne!(
            answer.status, 403,
            "case {case:?}: an undenied principal must not be refused, got body {}",
            answer.body
        );
    }
}

/// RULE (the same rule, its resource dimension): a deny scoped to ONE resource
/// refuses the request that addresses that resource and no other.
///
/// The rows differ along which server the request addresses and whether the deny
/// names it.
#[tokio::test]
async fn a_resource_scoped_deny_refuses_only_the_resource_it_names() {
    let emulator = Emulator::new();
    let denied_server = emulator.create_server(json!({})).await;
    let other_server = emulator.create_server(json!({})).await;
    emulator.seed_calling_principal(
        "scoped-user",
        &format!(
            r#"{{"Version":"2012-10-17","Statement":[
                {{"Effect":"Allow","Action":"transfer:*","Resource":"*"}},
                {{"Effect":"Deny","Action":"transfer:DescribeServer",
                  "Resource":"arn:aws:transfer:us-east-1:000000000000:server/{denied_server}"}}]}}"#
        ),
    );

    for (case, server_id, expect_denied) in [
        ("the server the deny names", denied_server.as_str(), true),
        ("another server", other_server.as_str(), false),
    ] {
        let answer = emulator
            .call("DescribeServer", json!({"ServerId": server_id}))
            .await;
        if expect_denied {
            assert_access_denied(case, &answer, &["transfer:DescribeServer"]);
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the read to be served, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

/// RULE (the same rule, how a statement is matched): the gate reads IAM's own
/// wildcard and condition semantics out of the stored document.
///
/// The rows differ along the SHAPE of the statement: a wildcard action, a
/// differently-cased action, a `NotAction`, a single statement rather than a list, a
/// statement whose `Condition` cannot be evaluated, and a document that does not
/// parse.
#[tokio::test]
async fn the_deny_decision_follows_the_stored_policy_document() {
    // (case, policy document, is the create denied)
    let rows: Vec<(&str, &str, bool)> = vec![
        (
            "a wildcard action",
            r#"{"Statement":[{"Effect":"Deny","Action":"transfer:Create*","Resource":"*"}]}"#,
            true,
        ),
        (
            "an action in another case",
            r#"{"Statement":[{"Effect":"Deny","Action":"TRANSFER:createserver","Resource":"*"}]}"#,
            true,
        ),
        (
            "one statement rather than a list",
            r#"{"Statement":{"Effect":"Deny","Action":"*","Resource":"*"}}"#,
            true,
        ),
        (
            "a NotAction that excludes this action",
            r#"{"Statement":[{"Effect":"Deny","NotAction":"transfer:DeleteServer","Resource":"*"}]}"#,
            true,
        ),
        (
            "a NotAction that spares this action",
            r#"{"Statement":[{"Effect":"Deny","NotAction":"transfer:CreateServer","Resource":"*"}]}"#,
            false,
        ),
        (
            "a deny on another service's action",
            r#"{"Statement":[{"Effect":"Deny","Action":"s3:*","Resource":"*"}]}"#,
            false,
        ),
        (
            // The shared evaluator's fail-closed reading of a condition it cannot
            // decide: a `Deny` that MAY hold is never weakened into permission.
            "a deny whose condition cannot be evaluated",
            r#"{"Statement":[{"Effect":"Deny","Action":"transfer:*","Resource":"*",
                "Condition":{"Bool":{"aws:SecureTransport":"false"}}}]}"#,
            true,
        ),
        (
            "a deny scoped to one server, on a create that names none",
            r#"{"Statement":[{"Effect":"Deny","Action":"transfer:CreateServer",
                "Resource":"arn:aws:transfer:us-east-1:000000000000:server/s-1"}]}"#,
            false,
        ),
        ("a document that is not JSON", "{not json", false),
    ];

    for (case, policy, expect_denied) in rows {
        let emulator = Emulator::new();
        emulator.seed_calling_principal("documented-user", policy);
        let answer = emulator.call("CreateServer", json!({})).await;
        if expect_denied {
            assert_access_denied(case, &answer, &["transfer:CreateServer"]);
        } else {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 2. The API_GATEWAY identity provider is verified before the server is stored.
// ---------------------------------------------------------------------------

/// One row of the identity-provider verification table: the case name, the role to
/// seed into IAM with its inline policy (or `None` to seed none), the invocation
/// role the request names, and whether the write is accepted.
type IdpRow<'a> = (&'a str, Option<(&'a str, String)>, &'a str, bool);

/// RULE: a server whose `IdentityProviderType` is `API_GATEWAY` is created only when
/// the `InvocationRole` it names is a role this account holds that may invoke the API
/// the `Url` names; otherwise the write is refused with `InvalidRequestException`
/// (400) and no server is persisted.
///
/// The rows differ along what makes the role unable to invoke: the account does not
/// hold it, it holds no invoke permission, its permission is scoped to a different
/// API, or the ARN is in another account — crossed with the two ways the permission
/// can be granted (every resource, or exactly this API).
#[tokio::test]
async fn an_api_gateway_identity_provider_is_verified_against_the_roles_policies() {
    // (case, the role to seed and its policy, the invocation role ARN, accepted)
    let rows: Vec<IdpRow<'_>> = vec![
        (
            "a role the account does not hold",
            None,
            "arn:aws:iam::000000000000:role/absent-idp",
            false,
        ),
        (
            "a role with no invoke permission",
            Some((
                "arn:aws:iam::000000000000:role/no-invoke",
                r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#
                    .to_string(),
            )),
            "arn:aws:iam::000000000000:role/no-invoke",
            false,
        ),
        (
            "a role allowed to invoke every API",
            Some((
                "arn:aws:iam::000000000000:role/invoke-any",
                ALLOW_INVOKE_ANY_API.to_string(),
            )),
            "arn:aws:iam::000000000000:role/invoke-any",
            true,
        ),
        (
            "a role allowed to invoke exactly this API",
            Some((
                "arn:aws:iam::000000000000:role/invoke-this",
                r#"{"Statement":[{"Effect":"Allow","Action":"execute-api:Invoke",
                    "Resource":"arn:aws:execute-api:us-east-1:000000000000:abc123/prod/*"}]}"#
                    .to_string(),
            )),
            "arn:aws:iam::000000000000:role/invoke-this",
            true,
        ),
        (
            "a role allowed to invoke a different API",
            Some((
                "arn:aws:iam::000000000000:role/invoke-other",
                r#"{"Statement":[{"Effect":"Allow","Action":"execute-api:Invoke",
                    "Resource":"arn:aws:execute-api:us-east-1:000000000000:zzz999/prod/*"}]}"#
                    .to_string(),
            )),
            "arn:aws:iam::000000000000:role/invoke-other",
            false,
        ),
        (
            "a role in another account",
            None,
            "arn:aws:iam::111122223333:role/invoke-any",
            false,
        ),
        (
            "a role whose invoke permission is a wildcard action",
            Some((
                "arn:aws:iam::000000000000:role/invoke-wildcard",
                r#"{"Statement":[{"Effect":"Allow","Action":"execute-api:*","Resource":"*"}]}"#
                    .to_string(),
            )),
            "arn:aws:iam::000000000000:role/invoke-wildcard",
            true,
        ),
        (
            "a role allowed to invoke every API and denied this one",
            Some((
                "arn:aws:iam::000000000000:role/invoke-denied",
                r#"{"Statement":[
                    {"Effect":"Allow","Action":"execute-api:Invoke","Resource":"*"},
                    {"Effect":"Deny","Action":"execute-api:Invoke",
                     "Resource":"arn:aws:execute-api:us-east-1:000000000000:abc123/prod/*"}]}"#
                    .to_string(),
            )),
            "arn:aws:iam::000000000000:role/invoke-denied",
            false,
        ),
        (
            "a role allowed to invoke every API and denied a different one",
            Some((
                "arn:aws:iam::000000000000:role/invoke-denied-other",
                r#"{"Statement":[
                    {"Effect":"Allow","Action":"execute-api:Invoke","Resource":"*"},
                    {"Effect":"Deny","Action":"execute-api:Invoke",
                     "Resource":"arn:aws:execute-api:us-east-1:000000000000:zzz999/prod/*"}]}"#
                    .to_string(),
            )),
            "arn:aws:iam::000000000000:role/invoke-denied-other",
            true,
        ),
    ];

    for (case, seed, invocation_role, accepted) in rows {
        let emulator = Emulator::new();
        if let Some((role_arn, policy)) = seed {
            emulator.seed_role(role_arn, &policy);
        }
        let request = json!({
            "IdentityProviderType": "API_GATEWAY",
            "IdentityProviderDetails": {"Url": IDP_URL, "InvocationRole": invocation_role},
        });
        let answer = emulator.call("CreateServer", request).await;

        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
            let server_id = answer
                .get("ServerId")
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a ServerId"))
                .to_string();
            let described = emulator
                .call("DescribeServer", json!({"ServerId": server_id}))
                .await;
            let details = member_of(described.get("Server"), "IdentityProviderDetails").clone();
            assert_eq!(
                member_of(&details, "InvocationRole"),
                &json!(invocation_role),
                "case {case:?}: the stored provider must report the verified role, got {details}"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got body {}",
                answer.body
            );
            let message = member_of(&answer.body, "Message")
                .as_str()
                .unwrap_or_default()
                .to_string();
            assert_eq!(
                message,
                format!("Unable to verify access to API {IDP_URL}"),
                "case {case:?}: the refusal must name the API it could not verify"
            );
            assert!(
                answer.body.get("ServerId").is_none(),
                "case {case:?}: a refused create must answer no ServerId, got {}",
                answer.body
            );
            let listed = emulator.call("ListServers", json!({})).await;
            let servers = listed
                .get("Servers")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                servers.is_empty(),
                "case {case:?}: a refused create must persist no server, got {}",
                listed.body
            );
        }
    }
}

/// RULE (the same rule, the other place a role's permissions can live): the
/// verification reads the policies ATTACHED to the invocation role as well as its
/// inline ones, resolving each attached ARN to the default version of the policy the
/// account holds — and an AWS-MANAGED policy, whose document AWS owns rather than
/// this account, is evidence of nothing, so it cannot be the ground for a refusal. An
/// attachment naming a CUSTOMER-managed policy the account does not hold is not that
/// case: the account is the authority on its own policies, so it grants nothing.
///
/// The rows differ along what the attached policy is and who owns it: one the account
/// holds that allows the invoke, one it holds that allows something else, one it holds
/// that denies the invoke (which no allow can outvote), an AWS-managed one the account
/// cannot read, and a customer-managed one the account does not hold.
#[tokio::test]
async fn an_attached_managed_policy_is_read_when_the_account_holds_it() {
    // An AWS-managed policy ARN carries IAM's own `aws` account segment, which is how
    // a document this account does not own is told apart from one it simply lacks.
    const AWS_MANAGED: &str = "arn:aws:iam::aws:policy/AmazonAPIGatewayInvokeFullAccess";
    const CUSTOMER_MANAGED: &str = "arn:aws:iam::000000000000:policy/InvokeTheIdpApi";

    // (case, the attached policy's ARN, its document when the account holds it, accepted)
    let rows: Vec<(&str, &str, Option<&str>, bool)> = vec![
        (
            "an attached policy the account holds that allows the invoke",
            CUSTOMER_MANAGED,
            Some(ALLOW_INVOKE_ANY_API),
            true,
        ),
        (
            "an attached policy the account holds that allows something else",
            CUSTOMER_MANAGED,
            Some(r#"{"Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#),
            false,
        ),
        (
            "an attached policy the account holds that denies the invoke",
            CUSTOMER_MANAGED,
            Some(
                r#"{"Statement":[
                    {"Effect":"Allow","Action":"execute-api:Invoke","Resource":"*"},
                    {"Effect":"Deny","Action":"execute-api:Invoke","Resource":"*"}]}"#,
            ),
            false,
        ),
        (
            "an attached AWS-managed policy, whose document this account cannot read",
            AWS_MANAGED,
            None,
            true,
        ),
        (
            "an attached customer-managed policy the account does not hold",
            CUSTOMER_MANAGED,
            None,
            false,
        ),
    ];

    let role_arn = "arn:aws:iam::000000000000:role/attached-idp";

    for (case, policy_arn, document, accepted) in rows {
        let emulator = Emulator::new();
        emulator.seed_role_with_attached_policy(role_arn, policy_arn, document);
        let answer = emulator
            .call(
                "CreateServer",
                json!({
                    "IdentityProviderType": "API_GATEWAY",
                    "IdentityProviderDetails": {"Url": IDP_URL, "InvocationRole": role_arn},
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got body {}",
                answer.body
            );
            let listed = emulator.call("ListServers", json!({})).await;
            let servers = listed
                .get("Servers")
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                servers.is_empty(),
                "case {case:?}: a refused create must persist no server, got {}",
                listed.body
            );
        }
    }
}

/// RULE (the same rule, the other write that sets an identity provider): switching a
/// server to `API_GATEWAY` through `UpdateServer` verifies the invocation role the
/// server would end up with, and leaves the stored provider alone when it cannot.
#[tokio::test]
async fn an_update_to_an_api_gateway_provider_is_verified_too() {
    let emulator = Emulator::new();
    emulator.seed_role(
        "arn:aws:iam::000000000000:role/invoke-any",
        ALLOW_INVOKE_ANY_API,
    );
    let server_id = emulator.create_server(json!({})).await;

    for (case, invocation_role, accepted) in [
        (
            "a role that cannot invoke",
            "arn:aws:iam::000000000000:role/absent-idp",
            false,
        ),
        (
            "a role that can invoke",
            "arn:aws:iam::000000000000:role/invoke-any",
            true,
        ),
    ] {
        let answer = emulator
            .call(
                "UpdateServer",
                json!({
                    "ServerId": &server_id,
                    "IdentityProviderType": "API_GATEWAY",
                    "IdentityProviderDetails": {
                        "Url": IDP_URL,
                        "InvocationRole": invocation_role,
                    },
                }),
            )
            .await;
        let described = emulator
            .call("DescribeServer", json!({"ServerId": &server_id}))
            .await;
        let stored = member_of(described.get("Server"), "IdentityProviderType").clone();
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the update to be served, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                stored,
                json!("API_GATEWAY"),
                "case {case:?}: the verified provider must be stored"
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                stored,
                json!("SERVICE_MANAGED"),
                "case {case:?}: a refused update must leave the stored provider alone"
            );
        }
    }
}

/// RULE (the same rule, where the `Url` names no API): a URL that is not an API
/// Gateway invoke endpoint leaves no API ARN to compare, so only a role allowed to
/// invoke EVERY resource verifies the provider; a role allowed to invoke one
/// particular API does not.
#[tokio::test]
async fn a_provider_url_naming_no_api_is_verified_only_by_an_unscoped_allow() {
    let scoped_to_one_api = r#"{"Statement":[{"Effect":"Allow","Action":"execute-api:Invoke",
        "Resource":"arn:aws:execute-api:us-east-1:000000000000:abc123/prod/*"}]}"#;

    for (case, policy, accepted) in [
        ("an allow covering every resource", ALLOW_INVOKE_ANY_API, true),
        ("an allow scoped to one API", scoped_to_one_api, false),
    ] {
        let emulator = Emulator::new();
        let role = "arn:aws:iam::000000000000:role/idp-elsewhere";
        emulator.seed_role(role, policy);
        let answer = emulator
            .call(
                "CreateServer",
                json!({
                    "IdentityProviderType": "API_GATEWAY",
                    "IdentityProviderDetails": {
                        "Url": "https://auth.example.invalid/authenticate",
                        "InvocationRole": role,
                    },
                }),
            )
            .await;
        if accepted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the create to be served, got {} with body {}",
                answer.status, answer.body
            );
        } else {
            assert_eq!(
                answer.status, 400,
                "case {case:?}: expected HTTP 400, got {} with body {}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidRequestException",
                "case {case:?}: expected InvalidRequestException, got body {}",
                answer.body
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 3. An ARN-shaped pattern is this service's own client error.
// ---------------------------------------------------------------------------

/// RULE: a value that fails a member's ARN-shaped modelled pattern is refused with
/// this service's modelled `InvalidRequestException` (HTTP 400) worded
/// `Invalid <Member> ARN` — never with the framework's `ValidationException`, which
/// no Transfer operation models.
///
/// The rows differ along the operation, the member (`Role` on three operations, the
/// optional `LoggingRole` on a fourth), and how the value fails the pattern: not an
/// ARN at all, an ARN of another resource type, and an ARN with no role name. A
/// pattern violation on a member that is NOT an ARN is the last row: it keeps the
/// framework's `ValidationException`, so the rewording cannot swallow every pattern.
#[tokio::test]
async fn a_malformed_arn_is_refused_with_the_services_own_client_error() {
    let emulator = Emulator::new();

    // (case, operation, request, expected code, the message fragment)
    let rows: Vec<(&str, &str, Value, &str, &str)> = vec![
        (
            "CreateAccess Role that is not an ARN",
            "CreateAccess",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "ExternalId": "S-1-1-12-1234567890-123456789-1234567890-1234",
                "Role": "not-an-iam-role-arn-at-all",
            }),
            "InvalidRequestException",
            "Invalid Role ARN",
        ),
        (
            "UpdateUser Role that is not an ARN",
            "UpdateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "u-arn",
                "Role": "not-a-valid-role-arn",
            }),
            "InvalidRequestException",
            "Invalid Role ARN",
        ),
        (
            "CreateUser Role naming another resource type",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "u-arn",
                "Role": "arn:aws:iam::000000000000:user/not-a-role",
            }),
            "InvalidRequestException",
            "Invalid Role ARN",
        ),
        (
            "CreateUser Role with no role name",
            "CreateUser",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "UserName": "u-arn",
                "Role": "arn:aws:iam::000000000000:role/",
            }),
            "InvalidRequestException",
            "Invalid Role ARN",
        ),
        (
            "CreateServer LoggingRole that is not an ARN",
            "CreateServer",
            json!({"LoggingRole": "transfer-logs"}),
            "InvalidRequestException",
            "Invalid LoggingRole ARN",
        ),
        (
            "a pattern violation on a member that is not an ARN",
            "CreateUser",
            json!({"ServerId": UNMINTED_SERVER_ID, "UserName": "a b", "Role": ROLE}),
            "ValidationException",
            "userName",
        ),
    ];

    for (case, operation, request, code, fragment) in rows {
        let answer = emulator.call(operation, request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {}",
            answer.status, answer.body
        );
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got body {}",
            answer.body
        );
        // The modelled `InvalidRequestException` spells its member `Message`; the
        // framework's synthetic shape spells it `message`.
        let message = member_of(&answer.body, "Message")
            .as_str()
            .or_else(|| member_of(&answer.body, "message").as_str())
            .unwrap_or_default()
            .to_string();
        assert!(
            message.contains(fragment),
            "case {case:?}: expected the refusal to name {fragment:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// 4. A passed role's account is checked before the request's resources.
// ---------------------------------------------------------------------------

/// RULE: a write that passes an IAM role validates the role ARN's ACCOUNT before it
/// resolves any resource the request names, refusing another account's role with
/// `AccessDeniedException` (HTTP 403); a role in THIS account that does not exist is
/// not this refusal, so the resource lookup is what answers then.
///
/// The rows differ along the account the role names (another account, this one, an
/// ARN with no account), the partition, and whether the server the request names
/// exists — which is how the ordering is observable: the cross-account refusal must
/// win over the not-found.
#[tokio::test]
async fn a_cross_account_role_is_refused_before_the_resource_is_resolved() {
    let emulator = Emulator::new();
    let server_id = emulator.create_server(json!({})).await;

    // (case, server id, role, expected error code)
    let rows: Vec<(&str, &str, &str, &str)> = vec![
        (
            "another account's role on a server that does not exist",
            UNMINTED_SERVER_ID,
            "arn:aws:iam::111122223333:role/sim2real-nonexistent-role",
            "AccessDeniedException",
        ),
        (
            "another account's role on a server that does exist",
            &server_id,
            "arn:aws:iam::111122223333:role/transfer-s3",
            "AccessDeniedException",
        ),
        (
            "another account's role in another partition",
            UNMINTED_SERVER_ID,
            "arn:aws-us-gov:iam::444455556666:role/transfer-s3",
            "AccessDeniedException",
        ),
        (
            "this account's role on a server that does not exist",
            UNMINTED_SERVER_ID,
            ROLE,
            "ResourceNotFoundException",
        ),
        (
            "this account's role naming a role that was never created",
            UNMINTED_SERVER_ID,
            "arn:aws:iam::000000000000:role/never-created",
            "ResourceNotFoundException",
        ),
    ];

    for (case, server, role, code) in rows {
        let answer = emulator
            .call(
                "CreateUser",
                json!({"ServerId": server, "UserName": "u-pass-role", "Role": role}),
            )
            .await;
        assert_eq!(
            answer.error_code(),
            code,
            "case {case:?}: expected {code}, got {} with body {}",
            answer.status,
            answer.body
        );
        if code == "AccessDeniedException" {
            assert_access_denied(case, &answer, &["Cross-account pass role is not allowed."]);
        }
        assert!(
            answer.body.get("UserName").is_none(),
            "case {case:?}: a refused create must answer no UserName, got {}",
            answer.body
        );
    }

    // The same rule on the other write that passes a role: an update naming another
    // account's role is refused and changes nothing.
    emulator.create_user(&server_id, "u-pass-role").await;
    let answer = emulator
        .call(
            "UpdateUser",
            json!({
                "ServerId": &server_id,
                "UserName": "u-pass-role",
                "Role": "arn:aws:iam::111122223333:role/transfer-s3",
            }),
        )
        .await;
    assert_access_denied(
        "UpdateUser with another account's role",
        &answer,
        &["Cross-account pass role is not allowed."],
    );
    let described = emulator
        .call(
            "DescribeUser",
            json!({"ServerId": &server_id, "UserName": "u-pass-role"}),
        )
        .await;
    assert_eq!(
        member_of(described.get("User"), "Role"),
        &json!(ROLE),
        "a refused update must leave the stored role alone, got {}",
        described.body
    );

    // And on the third write that passes a role: `CreateAccess` names a `Role` the
    // service would assume too, so the same account check runs before it resolves the
    // server — observable here because the server the row names was never minted.
    let answer = emulator
        .call(
            "CreateAccess",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "ExternalId": "S-1-1-12-1234567890-123456789-1234567890-1234",
                "Role": "arn:aws:iam::111122223333:role/transfer-s3",
            }),
        )
        .await;
    assert_access_denied(
        "CreateAccess with another account's role",
        &answer,
        &["Cross-account pass role is not allowed."],
    );
    // The same request with THIS account's role gets past the account check and is
    // answered by the resource lookup, which is what makes the ordering observable.
    let answer = emulator
        .call(
            "CreateAccess",
            json!({
                "ServerId": UNMINTED_SERVER_ID,
                "ExternalId": "S-1-1-12-1234567890-123456789-1234567890-1234",
                "Role": ROLE,
            }),
        )
        .await;
    assert_eq!(
        answer.error_code(),
        "ResourceNotFoundException",
        "CreateAccess with this account's role: expected the lookup to answer, got {} with body {}",
        answer.status,
        answer.body
    );
}
