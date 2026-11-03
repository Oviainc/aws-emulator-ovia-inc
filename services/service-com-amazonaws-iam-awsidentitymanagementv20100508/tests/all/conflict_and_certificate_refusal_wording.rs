// SPDX-License-Identifier: CC0-1.0

//! The REFUSALS whose sentence AWS words per error class, end to end over the awsQuery
//! wire: a refusal reproduces AWS's text for ITS class on ITS operation — the fixed part
//! byte for byte, and the variable part taken from the request — so two operations whose
//! AWS wording differs never share one sentence.
//!
//! Five rules, each over a class rather than over one call:
//!
//! 1. a `DeletePolicy` blocked because the policy still carries a version other than its
//!    default answers AWS's three sentences for that class, whatever the number of
//!    versions and whichever of them is the default;
//! 2. where SEVERAL classes of dependent block one delete, the refusal names the one AWS
//!    names: the binding a separate resource holds (the instance profile that holds a
//!    role) before a managed policy ATTACHED to the entity, and an attachment before a
//!    policy EMBEDDED in it;
//! 3. `UploadSigningCertificate` refuses a body it cannot read as a certificate with the
//!    sentence that QUOTES the value it was sent;
//! 4. `UploadServerCertificate` refuses certificate material it cannot read with its own
//!    entirely fixed sentence, which carries nothing of the request — the two upload
//!    operations word the same defect differently, so one sentence cannot serve both;
//! 5. a value refused because it names nothing the operation can act on carries THAT
//!    operation's fixed text: `ListPoliciesGrantingServiceAccess` answers
//!    `Invalid Entity Arn: <arn> is not supported.`, and a policy AWS reserves is refused
//!    by every operation that would put it on an identity with `Cannot attach AWS
//!    reserved policy to an IAM <identity kind>.`
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over: the
//! number of policy versions and which is the default, the entity kind and the COMBINATION
//! of dependents blocking its delete, the way material fails to be a certificate, the kind
//! of ARN supplied, and the operation and identity kind a reserved policy is put to.
//! Every assertion compares the WHOLE message, so a sentence that merely contains AWS's
//! text is not enough, and every refusal is cross-read to show the request recorded
//! nothing.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does. The awsQuery harness is the
//! one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::server_certificates::material;
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A trust policy with a single statement, which every fixture role needs to exist.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// AWS's text for a `DeletePolicy` whose policy still carries a non-default version,
/// written out here rather than taken from the implementation's constant: what this suite
/// pins is the SENTENCE AWS answers, so a reworded constant has to fail here rather than
/// agree with itself.
const MORE_THAN_ONE_VERSION: &str = "This policy has more than one version. Before you \
                                     delete a policy, you must delete the policy's \
                                     versions. The default version is deleted with the \
                                     policy.";

/// AWS's text for each class of blocking dependent, written out for the same reason.
const MUST_DETACH_ALL_POLICIES: &str = "Cannot delete entity, must detach all policies first.";
const MUST_DELETE_POLICIES: &str = "Cannot delete entity, must delete policies first.";
const MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE: &str =
    "Cannot delete entity, must remove roles from instance profile first.";

/// AWS's fixed sentence for certificate material `UploadServerCertificate` cannot read.
const UNABLE_TO_PARSE_CERTIFICATE: &str =
    "Unable to parse certificate. Please ensure the certificate is in PEM format.";

/// A policy document carrying the statement `number` identifies, so successive versions of
/// one policy differ.
fn document(number: usize) -> String {
    format!(
        r#"{{"Version":"2012-10-17","Statement":[{{"Sid":"S{number}","Effect":"Allow","Action":"s3:GetObject","Resource":"*"}}]}}"#
    )
}

/// Drive one call a fixture needs, asserting it succeeded, and report the body.
async fn expect_ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(&str, &str)],
) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the fixture call {action} must succeed, got {status}: {body}"
    );
    body
}

/// A policy whose version history blocks its delete: how many versions beyond the first
/// the fixture creates, and whether the LAST of them is made the default.
struct VersionsCase {
    case: &'static str,
    extra_versions: usize,
    last_is_default: bool,
}

/// RULE 1: a `DeletePolicy` refused because the policy still carries a version other than
/// its default answers AWS's three sentences for that class — the condition, the remedy
/// and what the delete does with the default version — and nothing of the policy or of how
/// many versions it has. Once those versions are gone the same delete is served, so the
/// refusal is read out of the policy's own version history.
#[tokio::test]
async fn a_policy_with_more_than_one_version_is_refused_with_awss_three_sentences() {
    let rows = &[
        VersionsCase {
            case: "one extra version, the first still default",
            extra_versions: 1,
            last_is_default: false,
        },
        VersionsCase {
            case: "one extra version, the new one made default",
            extra_versions: 1,
            last_is_default: true,
        },
        VersionsCase {
            case: "two extra versions, the first still default",
            extra_versions: 2,
            last_is_default: false,
        },
        VersionsCase {
            case: "four extra versions, the last made default",
            extra_versions: 4,
            last_is_default: true,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let created = expect_ok(
            &router,
            case,
            "CreatePolicy",
            &[("PolicyName", "versioned"), ("PolicyDocument", &document(0))],
        )
        .await;
        let arn = element(&created, "Arn");

        // Every version beyond the first, the last of them made default where the row says
        // so, so the default is not always the oldest version.
        let mut versions = vec![element(&created, "DefaultVersionId")];
        for index in 1..=row.extra_versions {
            let set_default = row.last_is_default && index == row.extra_versions;
            let body = expect_ok(
                &router,
                case,
                "CreatePolicyVersion",
                &[
                    ("PolicyArn", &arn),
                    ("PolicyDocument", &document(index)),
                    ("SetAsDefault", if set_default { "true" } else { "false" }),
                ],
            )
            .await;
            versions.push(element(&body, "VersionId"));
        }
        let default_version = if row.last_is_default {
            versions
                .last()
                .expect("the versions the fixture created")
                .clone()
        } else {
            versions.first().expect("the first version").clone()
        };

        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the delete conflict's HTTP 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {case:?}: expected DeleteConflict, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, MORE_THAN_ONE_VERSION,
            "case {case:?}: expected AWS's three sentences, got {message:?}"
        );
        // Nothing of the account's shape is in it: not the policy, not the version count.
        assert!(
            !message.contains(&arn) && !message.contains(&format!("{}", versions.len())),
            "case {case:?}: the refusal must name no policy and no count, got {message:?}"
        );

        // The refusal was about the versions: with the non-default ones gone the same
        // delete is served, and the policy is no longer readable.
        for version in versions.iter().filter(|held| **held != default_version) {
            expect_ok(
                &router,
                case,
                "DeletePolicyVersion",
                &[("PolicyArn", &arn), ("VersionId", version)],
            )
            .await;
        }
        let (status, body) = call(&router, "DeletePolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the delete must be served once the non-default versions are \
             gone, got {status}: {body}"
        );
        let (status, read) = call(&router, "GetPolicy", &[("PolicyArn", &arn)]).await;
        assert_eq!(
            status, 404,
            "case {case:?}: the deleted policy must not be readable, got {status}: {read}"
        );
    }
}

/// The identity kinds a delete can be blocked on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entity {
    Role,
    User,
    Group,
}

impl Entity {
    /// The delete that is refused, and the member it names its entity by.
    fn delete(self) -> (&'static str, &'static str) {
        match self {
            Entity::Role => ("DeleteRole", "RoleName"),
            Entity::User => ("DeleteUser", "UserName"),
            Entity::Group => ("DeleteGroup", "GroupName"),
        }
    }
}

/// One combination of dependents blocking one entity's delete, and the sentence AWS
/// answers for it.
struct Blocked {
    case: &'static str,
    entity: Entity,
    /// A policy the entity EMBEDS (`Put*Policy`).
    inline: bool,
    /// A managed policy ATTACHED to the entity (`Attach*Policy`).
    attached: bool,
    /// An instance profile holding the role (`AddRoleToInstanceProfile`), for a role.
    profile: bool,
    expected: &'static str,
}

/// RULE 2: a blocked delete carries AWS's fixed sentence for the class of dependent AWS
/// names, and where several classes block one delete it names exactly one of them, in
/// AWS's own precedence — the instance profile that holds a role, then the policies
/// attached to the entity, then the policies embedded in it.
///
/// The rows span the identity kinds and the COMBINATIONS: each class alone, each pair, and
/// all three together.
#[tokio::test]
async fn a_delete_blocked_by_several_dependents_names_the_one_aws_names() {
    let rows = &[
        Blocked {
            case: "a role embedding an inline policy",
            entity: Entity::Role,
            inline: true,
            attached: false,
            profile: false,
            expected: MUST_DELETE_POLICIES,
        },
        Blocked {
            case: "a role carrying an attached managed policy",
            entity: Entity::Role,
            inline: false,
            attached: true,
            profile: false,
            expected: MUST_DETACH_ALL_POLICIES,
        },
        Blocked {
            case: "a role held by an instance profile",
            entity: Entity::Role,
            inline: false,
            attached: false,
            profile: true,
            expected: MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE,
        },
        Blocked {
            case: "a role embedding and carrying a policy",
            entity: Entity::Role,
            inline: true,
            attached: true,
            profile: false,
            expected: MUST_DETACH_ALL_POLICIES,
        },
        Blocked {
            case: "a role embedding a policy and held by an instance profile",
            entity: Entity::Role,
            inline: true,
            attached: false,
            profile: true,
            expected: MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE,
        },
        Blocked {
            case: "a role carrying an attachment and held by an instance profile",
            entity: Entity::Role,
            inline: false,
            attached: true,
            profile: true,
            expected: MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE,
        },
        Blocked {
            case: "a role blocked by all three classes",
            entity: Entity::Role,
            inline: true,
            attached: true,
            profile: true,
            expected: MUST_REMOVE_ROLES_FROM_INSTANCE_PROFILE,
        },
        Blocked {
            case: "a user embedding an inline policy",
            entity: Entity::User,
            inline: true,
            attached: false,
            profile: false,
            expected: MUST_DELETE_POLICIES,
        },
        Blocked {
            case: "a user carrying an attached managed policy",
            entity: Entity::User,
            inline: false,
            attached: true,
            profile: false,
            expected: MUST_DETACH_ALL_POLICIES,
        },
        Blocked {
            case: "a user embedding and carrying a policy",
            entity: Entity::User,
            inline: true,
            attached: true,
            profile: false,
            expected: MUST_DETACH_ALL_POLICIES,
        },
        Blocked {
            case: "a group embedding an inline policy",
            entity: Entity::Group,
            inline: true,
            attached: false,
            profile: false,
            expected: MUST_DELETE_POLICIES,
        },
        Blocked {
            case: "a group carrying an attached managed policy",
            entity: Entity::Group,
            inline: false,
            attached: true,
            profile: false,
            expected: MUST_DETACH_ALL_POLICIES,
        },
        Blocked {
            case: "a group embedding and carrying a policy",
            entity: Entity::Group,
            inline: true,
            attached: true,
            profile: false,
            expected: MUST_DETACH_ALL_POLICIES,
        },
    ];

    for row in rows {
        let case = row.case;
        let router = router();
        let name = "blocked";

        // One managed policy per row, so nothing but the row's own dependents can block
        // its delete.
        let created = expect_ok(
            &router,
            case,
            "CreatePolicy",
            &[("PolicyName", "blocking"), ("PolicyDocument", &document(0))],
        )
        .await;
        let policy_arn = element(&created, "Arn");

        match row.entity {
            Entity::Role => {
                expect_ok(
                    &router,
                    case,
                    "CreateRole",
                    &[
                        ("RoleName", name),
                        ("AssumeRolePolicyDocument", TRUST_POLICY),
                    ],
                )
                .await;
            }
            Entity::User => {
                expect_ok(&router, case, "CreateUser", &[("UserName", name)]).await;
            }
            Entity::Group => {
                expect_ok(&router, case, "CreateGroup", &[("GroupName", name)]).await;
            }
        }

        if row.inline {
            let (action, member) = match row.entity {
                Entity::Role => ("PutRolePolicy", "RoleName"),
                Entity::User => ("PutUserPolicy", "UserName"),
                Entity::Group => ("PutGroupPolicy", "GroupName"),
            };
            expect_ok(
                &router,
                case,
                action,
                &[
                    (member, name),
                    ("PolicyName", "embedded"),
                    ("PolicyDocument", &document(1)),
                ],
            )
            .await;
        }
        if row.attached {
            let (action, member) = match row.entity {
                Entity::Role => ("AttachRolePolicy", "RoleName"),
                Entity::User => ("AttachUserPolicy", "UserName"),
                Entity::Group => ("AttachGroupPolicy", "GroupName"),
            };
            expect_ok(
                &router,
                case,
                action,
                &[(member, name), ("PolicyArn", &policy_arn)],
            )
            .await;
        }
        if row.profile {
            expect_ok(
                &router,
                case,
                "CreateInstanceProfile",
                &[("InstanceProfileName", "holder")],
            )
            .await;
            expect_ok(
                &router,
                case,
                "AddRoleToInstanceProfile",
                &[("InstanceProfileName", "holder"), ("RoleName", name)],
            )
            .await;
        }

        let (action, member) = row.entity.delete();
        let (status, body) = call(&router, action, &[(member, name)]).await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected the delete conflict's HTTP 409, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "DeleteConflict",
            "case {case:?}: expected DeleteConflict, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, row.expected,
            "case {case:?}: expected AWS's sentence for the class it names first, got \
             {message:?}"
        );
        assert!(
            !message.contains(name) && !message.contains(&policy_arn),
            "case {case:?}: the refusal must name no entity of this account, got {message:?}"
        );
    }
}

/// RULE 3: `UploadSigningCertificate` refuses a `CertificateBody` it cannot read as an
/// X.509 certificate with AWS's sentence for this operation, which QUOTES the value it was
/// sent — whatever that value is — and stores nothing.
#[tokio::test]
async fn an_unreadable_signing_certificate_quotes_the_value_it_refuses() {
    let router = router();
    let user = "cert-holder";
    expect_ok(&router, user, "CreateUser", &[("UserName", user)]).await;

    // Rows spanning text that is not PEM at all, a single character, PEM framing around
    // something that is not a certificate, PEM framing around nothing, and a body that is
    // a private key rather than a certificate.
    let bodies: Vec<(&str, String)> = vec![
        ("plain text", "not a certificate".to_string()),
        ("a single character", "x".to_string()),
        (
            "PEM framing around a payload that is not DER",
            "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n".to_string(),
        ),
        (
            "framing only",
            "-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n".to_string(),
        ),
        (
            "a private key where a certificate belongs",
            material(&rcgen::PKCS_ECDSA_P256_SHA256, (2024, 2034), "key.example.com").private_key,
        ),
        (
            // The value is quoted back into an XML envelope, so a body carrying the
            // metacharacters of that envelope has to come back as the body that was sent,
            // escaped rather than breaking the document around it.
            "a body carrying XML metacharacters",
            "<certificate>&\"not one\"</certificate>".to_string(),
        ),
    ];

    for (case, body) in &bodies {
        let params = vec![param("UserName", user), param("CertificateBody", body)];
        let (status, refused) =
            call(&router, "UploadSigningCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "MalformedCertificate",
            "case {case:?}: expected MalformedCertificate, got {:?}: {refused}",
            error_code(&refused)
        );
        let message = element(&refused, "Message");
        assert_eq!(
            message,
            format!("Certificate {body} is malformed."),
            "case {case:?}: expected AWS's sentence quoting the submitted body, got \
             {message:?}"
        );
    }

    let (_, listed) = call(&router, "ListSigningCertificates", &[("UserName", user)]).await;
    assert!(
        !listed.contains("<CertificateId>"),
        "a refused upload must store nothing, got {listed}"
    );
}

/// RULE 4: `UploadServerCertificate` refuses certificate material it cannot read with its
/// OWN entirely fixed sentence — carrying nothing of the request, and not the sentence
/// `UploadSigningCertificate` answers for the same body — and stores nothing.
#[tokio::test]
async fn an_unreadable_server_certificate_answers_awss_fixed_sentence() {
    let router = router();
    let valid = material(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2020, 2040),
        "valid.example.com",
    );

    // Rows spanning the two members that carry certificates and the ways each fails to be
    // one: a body that is not PEM, a body whose PEM frames no certificate, a body carrying
    // the private key instead, a chain that is not PEM, and a chain framing nothing.
    let rows: Vec<(&str, String, String, Option<String>)> = vec![
        (
            "a CertificateBody that is not PEM",
            "not a certificate at all".to_string(),
            valid.private_key.clone(),
            None,
        ),
        (
            "a CertificateBody whose PEM frames no certificate",
            "-----BEGIN CERTIFICATE-----\nZm9v\n-----END CERTIFICATE-----\n".to_string(),
            valid.private_key.clone(),
            None,
        ),
        (
            "a CertificateBody carrying the private key instead",
            valid.private_key.clone(),
            valid.private_key.clone(),
            None,
        ),
        (
            "a CertificateChain that is not PEM",
            valid.certificate.clone(),
            valid.private_key.clone(),
            Some("not a chain".to_string()),
        ),
        (
            "a CertificateChain framing nothing",
            valid.certificate.clone(),
            valid.private_key.clone(),
            Some("-----BEGIN CERTIFICATE-----\n-----END CERTIFICATE-----\n".to_string()),
        ),
    ];

    for (index, (case, body, key, chain)) in rows.iter().enumerate() {
        let name = format!("refused-{index}");
        let mut params = vec![
            param("ServerCertificateName", &name),
            param("CertificateBody", body),
            param("PrivateKey", key),
        ];
        if let Some(chain) = chain {
            params.push(param("CertificateChain", chain));
        }
        let (status, refused) = call(&router, "UploadServerCertificate", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {refused}"
        );
        assert_eq!(
            error_code(&refused),
            "MalformedCertificate",
            "case {case:?}: expected MalformedCertificate, got {:?}: {refused}",
            error_code(&refused)
        );
        let message = element(&refused, "Message");
        assert_eq!(
            message, UNABLE_TO_PARSE_CERTIFICATE,
            "case {case:?}: expected AWS's fixed sentence for this operation, got {message:?}"
        );
        assert!(
            !message.contains(body.trim()),
            "case {case:?}: the sentence must carry nothing of the request, got {message:?}"
        );

        let read_params = vec![param("ServerCertificateName", &name)];
        let (status, read) = call(&router, "GetServerCertificate", &borrowed(&read_params)).await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused upload must store nothing, got {status}: {read}"
        );
    }

    // The two upload operations word the same defect differently, so neither sentence can
    // serve both: the signing upload names the value, the server upload does not.
    let user = "sentence-holder";
    expect_ok(&router, user, "CreateUser", &[("UserName", user)]).await;
    let shared_body = "not a certificate at all";
    let params = vec![
        param("UserName", user),
        param("CertificateBody", shared_body),
    ];
    let (_, signing) = call(&router, "UploadSigningCertificate", &borrowed(&params)).await;
    let signing_message = element(&signing, "Message");
    assert_ne!(
        signing_message, UNABLE_TO_PARSE_CERTIFICATE,
        "the signing upload must not answer the server upload's sentence, got \
         {signing_message:?}"
    );
    assert_eq!(
        signing_message,
        format!("Certificate {shared_body} is malformed."),
        "the signing upload must quote the body it refused, got {signing_message:?}"
    );
}

/// RULE 5a: `ListPoliciesGrantingServiceAccess` refuses an `Arn` that names no identity
/// whose policies it could list with its own fixed text, around the value the request
/// supplied — and still serves an ARN that does name one, so the refusal is about the kind
/// of ARN and not about the store.
#[tokio::test]
async fn an_arn_that_names_no_identity_is_refused_with_this_operations_sentence() {
    let router = router();
    let account = account_id();
    let held = expect_ok(&router, "held", "CreateUser", &[("UserName", "Asker")]).await;
    let held_arn = element(&held, "Arn");

    // Rows spanning the kinds of ARN that are not an identity's: a customer managed
    // policy, a policy in AWS's own namespace, an instance profile, a server certificate,
    // and a resource of another service.
    let rows: Vec<(&str, String)> = vec![
        (
            "a customer managed policy",
            format!("arn:aws:iam::{account}:policy/Readers"),
        ),
        (
            "a policy in AWS's namespace",
            "arn:aws:iam::aws:policy/ReadOnlyAccess".to_string(),
        ),
        (
            "an instance profile",
            format!("arn:aws:iam::{account}:instance-profile/Build"),
        ),
        (
            "a server certificate",
            format!("arn:aws:iam::{account}:server-certificate/Edge"),
        ),
        (
            "a resource of another service",
            "arn:aws:s3:::reports-bucket-of-this-account".to_string(),
        ),
    ];

    for (case, arn) in &rows {
        let params = vec![
            param("Arn", arn),
            param("ServiceNamespaces.member.1", "s3"),
        ];
        let (status, body) = call(
            &router,
            "ListPoliciesGrantingServiceAccess",
            &borrowed(&params),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {case:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message,
            format!("Invalid Entity Arn: {arn} is not supported."),
            "case {case:?}: expected this operation's fixed text around the supplied ARN, \
             got {message:?}"
        );
    }

    let params = vec![
        param("Arn", &held_arn),
        param("ServiceNamespaces.member.1", "s3"),
    ];
    let (status, body) = call(
        &router,
        "ListPoliciesGrantingServiceAccess",
        &borrowed(&params),
    )
    .await;
    assert_eq!(
        status, 200,
        "an ARN that does name an identity must still be served, got {status}: {body}"
    );
}

/// One way of putting a managed policy on an identity, and the identity kind AWS names in
/// the refusal of a policy it reserves.
struct Put {
    case: &'static str,
    action: &'static str,
    /// The member naming the identity, and the member naming the policy.
    name_member: &'static str,
    policy_member: &'static str,
    identity_kind: &'static str,
}

/// RULE 5b: a managed policy AWS reserves — one published in AWS's namespace under the
/// path reserved for service-linked roles — is refused by every operation that would
/// attach it to an identity or make it one's permissions boundary, with AWS's fixed
/// sentence naming the KIND of identity the call addressed and nothing of the policy; a
/// policy AWS does not reserve is accepted by the same operations.
#[tokio::test]
async fn a_policy_aws_reserves_is_refused_by_every_way_of_putting_it_on_an_identity() {
    let puts = &[
        Put {
            case: "attached to a role",
            action: "AttachRolePolicy",
            name_member: "RoleName",
            policy_member: "PolicyArn",
            identity_kind: "role",
        },
        Put {
            case: "attached to a user",
            action: "AttachUserPolicy",
            name_member: "UserName",
            policy_member: "PolicyArn",
            identity_kind: "user",
        },
        Put {
            case: "attached to a group",
            action: "AttachGroupPolicy",
            name_member: "GroupName",
            policy_member: "PolicyArn",
            identity_kind: "group",
        },
        Put {
            case: "made a role's permissions boundary",
            action: "PutRolePermissionsBoundary",
            name_member: "RoleName",
            policy_member: "PermissionsBoundary",
            identity_kind: "role",
        },
        Put {
            case: "made a user's permissions boundary",
            action: "PutUserPermissionsBoundary",
            name_member: "UserName",
            policy_member: "PermissionsBoundary",
            identity_kind: "user",
        },
    ];

    // The reserved ARNs: AWS's own namespace under the path it reserves for service-linked
    // roles (`state::SERVICE_LINKED_ROLE_PATH_PREFIX`), both directly under it and one
    // level deeper, neither of them an entry this emulator's catalog carries — the
    // reservation is about the path, not about which entries it holds.
    let reserved = [
        format!(
            "arn:aws:iam::aws:policy{}AWSLambdaRole",
            state::SERVICE_LINKED_ROLE_PATH_PREFIX
        ),
        format!(
            "arn:aws:iam::aws:policy{}elasticbeanstalk/ServicePolicy",
            state::SERVICE_LINKED_ROLE_PATH_PREFIX
        ),
    ];

    for (index, put) in puts.iter().enumerate() {
        for (position, policy_arn) in reserved.iter().enumerate() {
            let case = format!("{} ({policy_arn})", put.case);
            let router = router();
            let identity = format!("reserved-{index}-{position}");
            create_identity(&router, put.name_member, &identity).await;

            let params = vec![
                param(put.name_member, &identity),
                param(put.policy_member, policy_arn),
            ];
            let (status, body) = call(&router, put.action, &borrowed(&params)).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "PolicyNotAttachable",
                "case {case:?}: expected PolicyNotAttachable, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                format!(
                    "Cannot attach AWS reserved policy to an IAM {}.",
                    put.identity_kind
                ),
                "case {case:?}: expected AWS's sentence for the reservation, got {message:?}"
            );
            assert!(
                !message.contains(policy_arn) && !message.contains(&identity),
                "case {case:?}: the sentence must carry nothing of the request, got \
                 {message:?}"
            );

            // The refusal is about the reservation: a policy in the same namespace that is
            // NOT under the reserved path is accepted by the very same call.
            let params = vec![
                param(put.name_member, &identity),
                param(put.policy_member, "arn:aws:iam::aws:policy/ReadOnlyAccess"),
            ];
            let (status, body) = call(&router, put.action, &borrowed(&params)).await;
            assert_eq!(
                status, 200,
                "case {case:?}: a policy AWS does not reserve must be accepted, got \
                 {status}: {body}"
            );
        }
    }
}

/// Create the identity a row addresses, named by the member it is addressed through.
async fn create_identity(router: &MultiServiceRouter, name_member: &str, identity: &str) {
    match name_member {
        "RoleName" => {
            expect_ok(
                router,
                identity,
                "CreateRole",
                &[
                    ("RoleName", identity),
                    ("AssumeRolePolicyDocument", TRUST_POLICY),
                ],
            )
            .await;
        }
        "GroupName" => {
            expect_ok(router, identity, "CreateGroup", &[("GroupName", identity)]).await;
        }
        _ => {
            expect_ok(router, identity, "CreateUser", &[("UserName", identity)]).await;
        }
    }
}
