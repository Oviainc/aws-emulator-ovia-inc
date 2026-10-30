// SPDX-License-Identifier: CC0-1.0

//! The `<RequestId>` an awsQuery error document carries, held across the SERVICE SURFACE:
//! many operations, many resource families and every error class this emulator answers.
//!
//! The rule under test: an awsQuery error response is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId>…</RequestId></ErrorResponse>`
//! — the `<RequestId>` is a SIBLING of `<Error>` and a property of the RESPONSE rather than
//! of any error shape, so it is present, exactly once and after `</Error>`, on every
//! failure this service answers whatever the operation, the resource family, the HTTP
//! status or the error code. The id's VALUE is minted per request, so it is not part of the
//! requirement: what a row reads back is its presence, its position, its 8-4-4-4-12 form
//! and that no two calls share one.
//!
//! `tests/all/error_envelope_request_id.rs` pins the same rule along the other axis — which
//! LAYER rendered the failure (a handler's modelled error, a refusal the centralized walk
//! raised, the scaffold's 501, a request naming no modelled action) — and owns the form
//! check [`is_request_id`] this suite shares. The axis here is the surface: a per-operation
//! or per-family regression would show up as one row failing while the layer suite still
//! passes.
//!
//! The rows differ along what the rule generalises over: the operation called (17 of them),
//! the resource family it addresses (access keys, the account alias, the account password
//! policy, users, groups, roles, instance profiles, server and signing certificates,
//! managed and inline policies, login profiles, identity providers), the HTTP status (400,
//! 404, 409) and the error code (`ValidationError`, `NoSuchEntity`, `EntityAlreadyExists`,
//! `InvalidInput`, `MalformedCertificate`). No single row is the specification.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is what a
//! client on the socket sees. The awsQuery harness is the one `tests/all/user_lifecycle.rs`
//! owns and every suite in this crate shares.

use std::collections::BTreeSet;

use crate::error_envelope_request_id::is_request_id;
use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A trust policy with a single statement, which the fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// An inline policy document with a single statement, which the `PutUserPolicy` row needs
/// so that only the absent USER can refuse it.
const INLINE_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// The user the account holds, so a row asking for it again is refused as a conflict and a
/// row addressing one of its credentials is refused by the CREDENTIAL rather than by the
/// user.
const HELD_USER: &str = "surface-held-user";

/// The role the account holds, for the conflict row on the role family.
const HELD_ROLE: &str = "SurfaceHeldRole";

/// The group the account holds, for the conflict row on the group family.
const HELD_GROUP: &str = "SurfaceHeldGroup";

/// An access key id of the modelled shape this emulator never issued, so the rows naming it
/// resolve no key of the held user.
const ABSENT_ACCESS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";

/// One row: the call to make, and the status and error code the operation answers it with.
struct Case {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
}

/// Create the resources the conflict and credential rows need.
async fn with_held_resources(router: &smithy_server_runtime::MultiServiceRouter) {
    let fixtures: Vec<(&str, Vec<(String, String)>)> = vec![
        ("CreateUser", vec![param("UserName", HELD_USER)]),
        (
            "CreateRole",
            vec![
                param("RoleName", HELD_ROLE),
                param("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        ),
        ("CreateGroup", vec![param("GroupName", HELD_GROUP)]),
    ];
    for (action, params) in fixtures {
        let (status, body) = call(router, action, &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "the fixture {action} must succeed, got {status}: {body}"
        );
    }
}

/// The rows: one per operation, spanning the resource families this service serves and the
/// error classes it answers.
fn surface_cases() -> Vec<Case> {
    let absent_policy_arn = format!("arn:aws:iam::{}:policy/surface-absent-policy", account_id());
    vec![
        // 404 `NoSuchEntity`, reached from a handler on nine different families.
        Case {
            case: "an access-key status update naming a key the user holds none of",
            action: "UpdateAccessKey",
            params: vec![
                param("UserName", HELD_USER),
                param("AccessKeyId", ABSENT_ACCESS_KEY),
                param("Status", "Inactive"),
            ],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "an access-key deletion naming a key the user holds none of",
            action: "DeleteAccessKey",
            params: vec![
                param("UserName", HELD_USER),
                param("AccessKeyId", ABSENT_ACCESS_KEY),
            ],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "an account-alias deletion naming an alias the account does not hold",
            action: "DeleteAccountAlias",
            params: vec![param("AccountAlias", "surface-absent-alias")],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "a role read naming no stored role",
            action: "GetRole",
            params: vec![param("RoleName", "SurfaceAbsentRole")],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "an instance-profile read naming no stored profile",
            action: "GetInstanceProfile",
            params: vec![param("InstanceProfileName", "SurfaceAbsentProfile")],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "a server-certificate read naming no stored certificate",
            action: "GetServerCertificate",
            params: vec![param("ServerCertificateName", "surface-absent-cert")],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "a login-profile read for a user holding no password",
            action: "GetLoginProfile",
            params: vec![param("UserName", HELD_USER)],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "a tag listing naming no stored user",
            action: "ListUserTags",
            params: vec![param("UserName", "surface-absent-user")],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "an inline-policy write naming no stored user",
            action: "PutUserPolicy",
            params: vec![
                param("UserName", "surface-absent-user"),
                param("PolicyName", "surface-inline"),
                param("PolicyDocument", INLINE_POLICY),
            ],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "a managed-policy read naming an ARN the account holds nothing under",
            action: "GetPolicy",
            params: vec![param("PolicyArn", &absent_policy_arn)],
            status: 404,
            code: "NoSuchEntity",
        },
        Case {
            case: "a group membership removal naming no stored user",
            action: "RemoveUserFromGroup",
            params: vec![
                param("GroupName", HELD_GROUP),
                param("UserName", "surface-absent-user"),
            ],
            status: 404,
            code: "NoSuchEntity",
        },
        // 409 `EntityAlreadyExists`, on each family that mints a named resource.
        Case {
            case: "a user the account already holds",
            action: "CreateUser",
            params: vec![param("UserName", HELD_USER)],
            status: 409,
            code: "EntityAlreadyExists",
        },
        Case {
            case: "a role the account already holds",
            action: "CreateRole",
            params: vec![
                param("RoleName", HELD_ROLE),
                param("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
            status: 409,
            code: "EntityAlreadyExists",
        },
        // 400, from the centralized walk's bound check and from a handler's own rule.
        Case {
            case: "a page size one above the listing maximum",
            action: "ListUsers",
            params: vec![param("MaxItems", "1001")],
            status: 400,
            code: "ValidationError",
        },
        Case {
            case: "a minimum password length below the modelled floor",
            action: "UpdateAccountPasswordPolicy",
            params: vec![param("MinimumPasswordLength", "5")],
            status: 400,
            code: "ValidationError",
        },
        Case {
            case: "a signing certificate whose body is not a certificate",
            action: "UploadSigningCertificate",
            params: vec![
                param("UserName", HELD_USER),
                param("CertificateBody", "not-a-pem-encoded-certificate"),
            ],
            status: 400,
            code: "MalformedCertificate",
        },
        Case {
            case: "an OIDC provider URL outside the documented form",
            action: "CreateOpenIDConnectProvider",
            params: vec![param("Url", "http://server.example.org")],
            status: 400,
            code: "InvalidInput",
        },
    ]
}

/// RULE: every awsQuery error document this service answers carries one `<RequestId>` beside
/// its `<Error>`, whatever the operation, the resource family, the status or the code — and
/// a distinct one per call.
#[tokio::test]
async fn every_operations_error_document_names_the_request_it_answers() {
    let router = router();
    with_held_resources(&router).await;

    let mut seen: BTreeSet<String> = BTreeSet::new();
    for Case {
        case,
        action,
        params,
        status: expected_status,
        code,
    } in surface_cases()
    {
        let (status, body) = call(&router, action, &borrowed(&params)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?} ({action}): expected HTTP {expected_status}, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?} ({action}): expected the code {code:?}, got {body}"
        );
        // `element` is itself the "exactly one" assertion: an error envelope names the
        // request once, never twice.
        let request_id = element(&body, "RequestId");
        assert!(
            is_request_id(&request_id),
            "case {case:?} ({action}): expected the document to name the request, got \
             {request_id:?} in {body}"
        );
        let error_end = body
            .find("</Error>")
            .unwrap_or_else(|| panic!("case {case:?} ({action}): no <Error> element in {body}"));
        let named = body
            .find("<RequestId>")
            .unwrap_or_else(|| panic!("case {case:?} ({action}): no <RequestId> element in {body}"));
        assert!(
            named > error_end,
            "case {case:?} ({action}): expected <RequestId> beside <Error> rather than inside \
             it, got {body}"
        );
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?} ({action}): expected a fresh id per call, got {request_id:?} again"
        );
    }
    assert_eq!(
        seen.len(),
        surface_cases().len(),
        "expected one id per row, got {seen:?}"
    );
}
