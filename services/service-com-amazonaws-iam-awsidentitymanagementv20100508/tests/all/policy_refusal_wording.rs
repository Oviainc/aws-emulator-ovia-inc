// SPDX-License-Identifier: CC0-1.0

//! The WORDING of two IAM refusals, end to end over the awsQuery wire: the
//! `NoSuchEntity` an operation of the policy-ATTACHMENT family answers for a
//! `PolicyArn` the account holds no managed policy under, and the `InvalidInput` an
//! operation taking a `SAMLMetadataDocument` answers for a value that is not SAML 2.0
//! metadata.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, `Message`) of a refusal reached through
//! the wire, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share; the
//! metadata documents are the ones `tests/all/saml_providers.rs` builds.
//!
//! The rules under test:
//!
//! 1. an operation whose subject is whether a managed policy can be, or already is,
//!    attached to an entity reports an absent policy with the attachment family's
//!    wording — the ARN the request supplied between the fixed text `Policy ` and
//!    ` does not exist or is not attachable.` — while an operation acting on the policy
//!    RESOURCE itself reports the same absence as ` was not found.`;
//! 2. a document that describes no SAML 2.0 identity provider is refused with ONE fixed
//!    sentence, carrying no input member name and nothing the reader of the document
//!    observed about its text.
//!
//! Each suite is a TABLE whose rows differ along what its rule generalises over — which
//! operation of the family is called, and the form of the refused ARN (path depth, name
//! alphabet, owning account, partition) or of the refused document (not XML at all,
//! unterminated, rooted elsewhere, in another namespace, naming no issuer, stating an
//! unreadable expiration) — so no single request is the specification.

use smithy_server_runtime::MultiServiceRouter;

use crate::role_lifecycle::{borrowed, param};
use crate::saml_providers::{
    METADATA_NAMESPACE, MetadataForm, expected_arn, filler, metadata_document,
};
use crate::user_lifecycle::{account_id, call, element, error_code, router};

/// A managed policy document with a single statement — the form `CreatePolicy`'s own
/// modelled example carries. The fixture policy needs one to exist at all.
const POLICY_DOCUMENT: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:GetObject","Resource":"*"}]}"#;

/// A trust policy with a single statement, which the fixture role needs.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"config.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The role every attachment-family row that acts on a role names.
const ROLE_NAME: &str = "WordingRole";

/// The user every attachment-family row that acts on a user names.
const USER_NAME: &str = "wording-user";

/// One operation addressed by a `PolicyArn`, with the entity member it ALSO requires
/// when it acts on an entity rather than on the policy alone.
struct PolicyCall {
    action: &'static str,
    entity: Option<(&'static str, &'static str)>,
}

/// The operations whose subject is the ATTACHMENT of a managed policy to an entity: two
/// of the three attach calls, the reporting half that lists the entities a policy is
/// attached to, and the listing of the policy's versions, which reports the one in force
/// for those entities. `AttachGroupPolicy` is covered, with every other operation
/// addressed by a `PolicyArn`, by `tests/all/policy_arn_absence_wording.rs`; the detach
/// calls are not here because they report an absent ATTACHMENT rather than an absent
/// policy.
const ATTACHMENT_FAMILY: &[PolicyCall] = &[
    PolicyCall {
        action: "ListEntitiesForPolicy",
        entity: None,
    },
    PolicyCall {
        action: "ListPolicyVersions",
        entity: None,
    },
    PolicyCall {
        action: "AttachRolePolicy",
        entity: Some(("RoleName", ROLE_NAME)),
    },
    PolicyCall {
        action: "AttachUserPolicy",
        entity: Some(("UserName", USER_NAME)),
    },
];

/// The operations that act on the managed policy RESOURCE itself — read it, list what it
/// holds, delete it — which report an absent policy with the plain not-found wording.
/// They are here as the CONTRAST that keeps the attachment clause sited per family.
const RESOURCE_FAMILY: &[PolicyCall] = &[
    PolicyCall {
        action: "GetPolicy",
        entity: None,
    },
    PolicyCall {
        action: "ListPolicyTags",
        entity: None,
    },
    PolicyCall {
        action: "DeletePolicy",
        entity: None,
    },
];

/// Create the fixture role, user and one held policy, answering the router and the held
/// policy's ARN.
async fn fixture() -> (MultiServiceRouter, String) {
    let router = router();

    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", ROLE_NAME),
            ("Path", "/"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateRole must succeed for the fixture, got {status}: {body}"
    );

    let (status, body) = call(
        &router,
        "CreateUser",
        &[("UserName", USER_NAME), ("Path", "/")],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateUser must succeed for the fixture, got {status}: {body}"
    );

    let (status, body) = call(
        &router,
        "CreatePolicy",
        &[
            ("PolicyName", "HeldPolicy"),
            ("Path", "/held/"),
            ("PolicyDocument", POLICY_DOCUMENT),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreatePolicy must succeed for the fixture, got {status}: {body}"
    );
    (router, element(&body, "Arn"))
}

/// Drive one `PolicyCall` against `policy_arn`.
async fn policy_call(
    router: &MultiServiceRouter,
    call_under_test: &PolicyCall,
    policy_arn: &str,
) -> (u16, String) {
    let mut params = vec![param("PolicyArn", policy_arn)];
    if let Some((member, value)) = call_under_test.entity {
        params.push(param(member, value));
    }
    call(router, call_under_test.action, &borrowed(&params)).await
}

/// RULE: an operation of the policy-attachment family refuses a well-formed `PolicyArn`
/// the account holds no managed policy under with 404 `NoSuchEntity` whose message is the
/// request's ARN between the fixed text `Policy ` and ` does not exist or is not
/// attachable.`, while an operation acting on the policy resource itself words the same
/// absence ` was not found.`
#[tokio::test]
async fn an_unheld_policy_arn_is_worded_per_operation_family() {
    let (router, held) = fixture().await;
    let account = account_id();

    // Rows spanning the forms a well-formed managed-policy ARN takes that the account
    // still holds no policy under: a name nothing was created under, the held name at
    // another path, the held name in another account, the held name in another
    // partition, a deeply nested path, and a name spelling the punctuation
    // `policyNameType`'s pattern admits (`^[\w+=,.@-]+$`,
    // `smithy_query(query="shape", shape="policyNameType")`).
    let unheld: Vec<(&str, String)> = vec![
        (
            "a name nothing was created under",
            format!("arn:aws:iam::{account}:policy/NeverCreated"),
        ),
        (
            "the held name at another path",
            format!("arn:aws:iam::{account}:policy/elsewhere/HeldPolicy"),
        ),
        (
            "the held name in another account",
            "arn:aws:iam::210987654321:policy/held/HeldPolicy".to_string(),
        ),
        (
            "the held name in another partition",
            format!("arn:aws-us-gov:iam::{account}:policy/held/HeldPolicy"),
        ),
        (
            "a deeply nested path",
            format!("arn:aws:iam::{account}:policy/app/audit/2026/Ghost"),
        ),
        (
            "a name of the punctuation the name alphabet admits",
            format!("arn:aws:iam::{account}:policy/ghost.name+2=x@y_-"),
        ),
    ];

    for (what, arn) in &unheld {
        for call_under_test in ATTACHMENT_FAMILY {
            let action = call_under_test.action;
            let case = format!("{action} on {what}");
            let (status, body) = policy_call(&router, call_under_test, arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                format!("Policy {arn} does not exist or is not attachable."),
                "case {case:?}: expected the attachment family's wording, got {message:?}"
            );
        }

        for call_under_test in RESOURCE_FAMILY {
            let action = call_under_test.action;
            let case = format!("{action} on {what}");
            let (status, body) = policy_call(&router, call_under_test, arn).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected HTTP 404, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {case:?}: expected NoSuchEntity, got {:?}: {body}",
                error_code(&body)
            );
            let message = element(&body, "Message");
            assert_eq!(
                message,
                format!("Policy {arn} was not found."),
                "case {case:?}: expected the policy resource's wording, got {message:?}"
            );
        }
    }

    // The refusals are about the policy the ARN names, not about the family: the HELD
    // policy is served by every attachment-family call, and the attachment it writes is
    // what the reporting half then relates.
    for call_under_test in ATTACHMENT_FAMILY {
        let action = call_under_test.action;
        let (status, body) = policy_call(&router, call_under_test, &held).await;
        assert_eq!(
            status, 200,
            "case {action:?}: the held policy must be served, got {status}: {body}"
        );
    }
    let (status, body) = call(&router, "ListEntitiesForPolicy", &[("PolicyArn", &held)]).await;
    assert_eq!(
        status, 200,
        "the held policy must be served after the attachments, got {status}: {body}"
    );
    assert_eq!(
        element(&body, "RoleName"),
        ROLE_NAME,
        "expected the attached role to be related, got {body}"
    );
    assert_eq!(
        element(&body, "UserName"),
        USER_NAME,
        "expected the attached user to be related, got {body}"
    );
}

/// IAM's fixed wording for a `SAMLMetadataDocument` that describes no SAML 2.0 identity
/// provider: ONE sentence, with no input member name, no property of the document and no
/// position in it.
const INVALID_SAML_METADATA_DOCUMENT: &str = "Invalid SAML metadata document";

/// Every way a value can satisfy `SAMLMetadataDocumentType`'s length bound and still
/// describe no SAML 2.0 identity provider: `(what the row is, the document)`.
///
/// The rows span the whole class the rule generalises over — not XML at all, XML that is
/// unterminated, XML rooted at another element, a descriptor in another namespace, a
/// descriptor naming no issuer or an empty one, a group describing no entity, and an
/// expiration that is not an `xs:dateTime` — so no one malformed document is the
/// specification.
fn documents_that_are_not_saml_metadata() -> Vec<(&'static str, String)> {
    let filler = filler();
    vec![
        ("not XML at all", format!("this is not a document {filler}")),
        (
            "an unterminated element",
            format!("<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"x\">{filler}"),
        ),
        (
            "rooted at another element",
            format!("<NotADescriptor xmlns=\"{METADATA_NAMESPACE}\">{filler}</NotADescriptor>"),
        ),
        (
            "a descriptor in another namespace",
            format!(
                "<EntityDescriptor xmlns=\"urn:example:not-saml\" entityID=\"https://i.example\">\
                 {filler}</EntityDescriptor>"
            ),
        ),
        (
            "no entityID",
            format!("<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\">{filler}</EntityDescriptor>"),
        ),
        (
            "an empty entityID",
            format!(
                "<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"\">{filler}\
                 </EntityDescriptor>"
            ),
        ),
        (
            "a group describing no entity",
            format!(
                "<EntitiesDescriptor xmlns=\"{METADATA_NAMESPACE}\">{filler}\
                 </EntitiesDescriptor>"
            ),
        ),
        (
            "a validUntil that is not a date-time",
            format!(
                "<EntityDescriptor xmlns=\"{METADATA_NAMESPACE}\" entityID=\"https://i.example\" \
                 validUntil=\"next tuesday\">{filler}</EntityDescriptor>"
            ),
        ),
    ]
}

/// RULE: every operation taking a `SAMLMetadataDocument` refuses a value that describes
/// no SAML 2.0 identity provider with 400 `InvalidInput` and IAM's ONE fixed sentence,
/// and persists nothing — whichever way the document fails to be metadata, and whether
/// the call would create a provider or replace a held one's document.
#[tokio::test]
async fn a_document_that_is_not_saml_metadata_is_refused_with_one_fixed_sentence() {
    let router = router();
    let good = metadata_document(MetadataForm::Entity, "https://idp.example.com/saml", None);

    // The provider whose document a refused update must leave exactly as it is.
    let held = "WordingIdP";
    let (status, body) = call(
        &router,
        "CreateSAMLProvider",
        &[("Name", held), ("SAMLMetadataDocument", &good)],
    )
    .await;
    assert_eq!(
        status, 200,
        "a document that IS SAML metadata must be accepted, got {status}: {body}"
    );
    let held_arn = element(&body, "SAMLProviderArn");

    for (index, (what, document)) in documents_that_are_not_saml_metadata().iter().enumerate() {
        // The create half: a name no provider is held under, so only the document can
        // refuse the call.
        let name = format!("Wording{index}");
        let create = vec![param("Name", &name), param("SAMLMetadataDocument", document)];
        let (status, body) = call(&router, "CreateSAMLProvider", &borrowed(&create)).await;
        assert_eq!(
            status, 400,
            "case {what:?}: expected CreateSAMLProvider HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {what:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, INVALID_SAML_METADATA_DOCUMENT,
            "case {what:?}: expected IAM's fixed wording, got {message:?}"
        );

        // Nothing was persisted: the ARN the create would have minted names no provider.
        let (status, read) = call(
            &router,
            "GetSAMLProvider",
            &[("SAMLProviderArn", &expected_arn(&name))],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {what:?}: a refused create must persist nothing, got {status}: {read}"
        );

        // The update half: the same document, refused the same way against a provider
        // the account DOES hold.
        let update = vec![
            param("SAMLProviderArn", &held_arn),
            param("SAMLMetadataDocument", document),
        ];
        let (status, body) = call(&router, "UpdateSAMLProvider", &borrowed(&update)).await;
        assert_eq!(
            status, 400,
            "case {what:?}: expected UpdateSAMLProvider HTTP 400, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {what:?}: expected InvalidInput, got {:?}: {body}",
            error_code(&body)
        );
        let message = element(&body, "Message");
        assert_eq!(
            message, INVALID_SAML_METADATA_DOCUMENT,
            "case {what:?}: expected IAM's fixed wording, got {message:?}"
        );

        // The refused update changed nothing: the held provider still reports the
        // document it was created with.
        let (status, read) =
            call(&router, "GetSAMLProvider", &[("SAMLProviderArn", &held_arn)]).await;
        assert_eq!(
            status, 200,
            "case {what:?}: the held provider must survive a refused update, got {status}: \
             {read}"
        );
        let reported = element(&read, "SAMLMetadataDocument");
        assert_eq!(
            reported, good,
            "case {what:?}: a refused update must leave the held document alone, got \
             {reported:?}"
        );
    }
}
