// SPDX-License-Identifier: CC0-1.0

//! Whether a CodeArtifact control-plane call is authorized by the RESOURCE POLICIES in force
//! on what it addressed, over the wire.
//!
//! The rule under test: a principal may perform `codeartifact:<OperationName>` only where
//! some policy in force allows it, and an explicit `Deny` in any of them refuses whatever the
//! others say. The documents in force are the caller's own identity-based policies plus the
//! resource policies of the addressed resource: for a DOMAIN-scoped call that domain's own
//! permissions policy; for a call inside a repository, that repository's policy AND the
//! policy of the domain holding it — a domain policy applies across the repositories of its
//! domain, so an explicit `Deny` in it overrides a repository-policy `Allow` for the same
//! action. A principal allowed by none of them is refused the operation's modelled
//! `AccessDeniedException` (HTTP 403;
//! `smithy_query(query="operation",operation="ListRepositoriesInDomain,ListPackages")` lists
//! it with httpError 403).
//!
//! Two further halves of the same rule:
//!
//! * a caller whose identity-based grants this emulator cannot read (the account's own
//!   published credential, which no identity policy governs) is not refused for the ABSENCE
//!   of a grant — but an explicit `Deny` in a readable resource policy refuses it, and only
//!   for the action that `Deny` names;
//! * the resource a decision is reached against, and therefore the ARN the refusal names, is
//!   the resource the action acts on: the domain for a domain-scoped action, the repository
//!   for a repository-scoped one, and the addressed package for a package-scoped one.
//!
//! It is one rule for the whole service, so every case drives it through SEVERAL operations
//! of each scope rather than through the one an issue happened to name. Each case drives the
//! production front door, and the session, the role and its policies live in the STS and IAM
//! stores of the SAME shared engine, the stores those services' own handlers write.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_as, create_domain, given_role, given_role_policy, given_session, role_arn, Answer,
    ACCOUNT, REGION,
};

/// The role every session below is assumed from, and the access key id of its session's
/// credentials: an `ASIA` key, as STS mints for a session.
const ROLE: &str = "ca-domain-reader";
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION20";
const SESSION_NAME: &str = "reader";

/// A SECOND role, the one a policy in these cases does NOT name: a sibling principal in the
/// same account, which is what shows a grant is to a principal rather than to the account.
const OTHER_ROLE: &str = "ca-domain-sibling";
const OTHER_SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION21";
const OTHER_SESSION_NAME: &str = "sibling";

/// The domain and repository every case addresses, and the package published into it.
const DOMAIN: &str = "resource-policy";
const REPOSITORY: &str = "Releases.one";
const FORMAT: &str = "generic";
const NAMESPACE: &str = "team-core";
const PACKAGE: &str = "blob-bundle";
const VERSION: &str = "1.0.0";

/// Which resource an operation's action acts on, which is the ARN its decision is reached
/// against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Domain,
    Repository,
    Package,
}

/// One request, as a client sends it: the operation whose action is being decided, the HTTP
/// method, URI and body that address it, and the scope of the resource it acts on.
#[derive(Debug, Clone, Copy)]
struct Addressed {
    operation: &'static str,
    method: &'static str,
    uri: &'static str,
    /// Whether the operation takes a body at all: its members travel in the query string, so
    /// the body a POST carries is the empty object its SDK sends.
    body: bool,
    scope: Scope,
}

/// The operations these cases drive: several of each scope, so no case can pass by being
/// about one handler. Every one of them addresses [`DOMAIN`] (and, where it is not
/// domain-scoped, [`REPOSITORY`] inside it) — `smithy_query(query="digest")` is where each
/// one's input members and HTTP binding come from.
const ADDRESSED: &[Addressed] = &[
    Addressed {
        operation: "ListRepositoriesInDomain",
        method: "POST",
        uri: "/v1/domain/repositories?domain=resource-policy",
        body: true,
        scope: Scope::Domain,
    },
    Addressed {
        operation: "DescribeDomain",
        method: "GET",
        uri: "/v1/domain?domain=resource-policy",
        body: false,
        scope: Scope::Domain,
    },
    Addressed {
        operation: "ListPackageGroups",
        method: "POST",
        uri: "/v1/package-groups?domain=resource-policy",
        body: true,
        scope: Scope::Domain,
    },
    Addressed {
        operation: "ListPackages",
        method: "POST",
        uri: "/v1/packages?domain=resource-policy&repository=Releases.one",
        body: true,
        scope: Scope::Repository,
    },
    Addressed {
        operation: "GetRepositoryEndpoint",
        method: "GET",
        uri: "/v1/repository/endpoint?domain=resource-policy&repository=Releases.one&format=npm",
        body: false,
        scope: Scope::Repository,
    },
    Addressed {
        operation: "DescribePackageVersion",
        method: "GET",
        uri: "/v1/package/version?domain=resource-policy&repository=Releases.one&format=generic&namespace=team-core&package=blob-bundle&version=1.0.0",
        body: false,
        scope: Scope::Package,
    },
    Addressed {
        operation: "ListPackageVersions",
        method: "POST",
        uri: "/v1/package/versions?domain=resource-policy&repository=Releases.one&format=generic&namespace=team-core&package=blob-bundle",
        body: true,
        scope: Scope::Package,
    },
];

/// The actions of the operations [`ADDRESSED`] drives, as a policy statement names them: a
/// row that denies "every action these calls ask for" names these rather than
/// `codeartifact:*`, so the policy that states it does not also deny the policy operations
/// that take it off again.
const ADDRESSED_ACTIONS: &[&str] = &[
    "codeartifact:ListRepositoriesInDomain",
    "codeartifact:DescribeDomain",
    "codeartifact:ListPackageGroups",
    "codeartifact:ListPackages",
    "codeartifact:GetRepositoryEndpoint",
    "codeartifact:DescribePackageVersion",
    "codeartifact:ListPackageVersions",
];

/// The ARN an operation of `scope` is judged against: the resource types of the IAM Service
/// Authorization Reference for CodeArtifact —
/// `domain/<name>`, `repository/<domain>/<name>` and
/// `package/<domain>/<repository>/<format>/<namespace>/<name>`.
fn judged_arn(scope: Scope) -> String {
    let prefix = format!("arn:aws:codeartifact:{REGION}:{ACCOUNT}");
    match scope {
        Scope::Domain => format!("{prefix}:domain/{DOMAIN}"),
        Scope::Repository => format!("{prefix}:repository/{DOMAIN}/{REPOSITORY}"),
        Scope::Package => {
            format!("{prefix}:package/{DOMAIN}/{REPOSITORY}/{FORMAT}/{NAMESPACE}/{PACKAGE}")
        }
    }
}

/// An emulator with this one service mounted on one shared engine: the domain, the
/// repository and one published package version created by the account's own credential, and
/// both roles held with sessions recorded.
async fn given_repository() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(
            Arc::clone(&engine),
        ),
    );
    assert_eq!(
        create_domain(&router, DOMAIN, None).await.status,
        200,
        "CreateDomain must succeed before anything is authorized"
    );
    let created = call(
        &router,
        "POST",
        &format!("/v1/repository?domain={DOMAIN}&repository={REPOSITORY}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateRepository must succeed, got {created:?}"
    );
    let published = crate::package_version_publish::publish(
        &router,
        &crate::package_version_publish::Publish {
            domain: DOMAIN.to_string(),
            repository: REPOSITORY.to_string(),
            ..crate::package_version_publish::Publish::new(
                PACKAGE,
                VERSION,
                "payload.bin",
                crate::package_version_publish::binary_content(48, 5),
            )
            .namespace(NAMESPACE)
        },
    )
    .await;
    assert_eq!(
        published.status, 200,
        "PublishPackageVersion must succeed, got {published:?}"
    );
    for (role, key, session) in [
        (ROLE, SESSION_KEY_ID, SESSION_NAME),
        (OTHER_ROLE, OTHER_SESSION_KEY_ID, OTHER_SESSION_NAME),
    ] {
        given_role(&engine, role);
        given_session(&engine, key, role, session, 3_600);
        given_role_policy(&engine, role, None);
    }
    (router, engine)
}

/// A resource policy stating `effect` for `principal` on `actions`, the shape
/// `PutDomainPermissionsPolicy`/`PutRepositoryPermissionsPolicy` take.
fn policy(effect: &str, principal: &str, actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": effect,
            "Principal": { "AWS": principal },
            "Action": actions,
            "Resource": "*"
        }]
    })
}

/// Attach `document` to the DOMAIN, or detach whatever it carries when `None`, as the
/// account's own credential.
async fn given_domain_policy(router: &MultiServiceRouter, document: Option<&Value>) {
    let answer = match document {
        Some(document) => {
            call(
                router,
                "PUT",
                "/v1/domain/permissions/policy",
                Some(json!({ "domain": DOMAIN, "policyDocument": document.to_string() })),
            )
            .await
        }
        None => {
            call(
                router,
                "DELETE",
                &format!("/v1/domain/permissions/policy?domain={DOMAIN}"),
                None,
            )
            .await
        }
    };
    assert!(
        answer.status == 200,
        "seeding the domain policy must succeed, got {answer:?}"
    );
}

/// Attach `document` to the REPOSITORY, or detach whatever it carries when `None`.
async fn given_repository_policy(router: &MultiServiceRouter, document: Option<&Value>) {
    let query = format!("domain={DOMAIN}&repository={REPOSITORY}");
    let answer = match document {
        Some(document) => {
            call(
                router,
                "PUT",
                &format!("/v1/repository/permissions/policy?{query}"),
                Some(json!({ "policyDocument": document.to_string() })),
            )
            .await
        }
        // `DeleteRepositoryPermissionsPolicy` is bound to the PLURAL path.
        None => {
            call(
                router,
                "DELETE",
                &format!("/v1/repository/permissions/policies?{query}"),
                None,
            )
            .await
        }
    };
    assert_eq!(
        answer.status, 200,
        "seeding the repository policy must succeed, got {answer:?}"
    );
}

/// Send one addressed request as the session `access_key_id` speaks for.
async fn as_session(
    router: &MultiServiceRouter,
    access_key_id: &str,
    addressed: &Addressed,
) -> Answer {
    let body = addressed.body.then(|| json!({}));
    call_as(router, access_key_id, addressed.method, addressed.uri, body).await
}

/// Send one addressed request as the ACCOUNT's own published credential — the caller whose
/// identity-based grants this emulator holds nothing to read.
async fn as_account(router: &MultiServiceRouter, addressed: &Addressed) -> Answer {
    let body = addressed.body.then(|| json!({}));
    call(router, addressed.method, addressed.uri, body).await
}

/// Whether an answer is the modelled authorization refusal.
fn refused(answer: &Answer) -> bool {
    (answer.status, answer.code().as_str()) == (403, "AccessDeniedException")
}

/// RULE: the policies in force on what the call addressed decide it — the domain's own for a
/// domain-scoped call, the repository's AND its domain's for a call inside the repository —
/// with an `Allow` in any of them granting and a `Deny` in any of them refusing.
#[tokio::test]
async fn the_policies_in_force_on_the_addressed_resource_decide_the_call() {
    let (router, _engine) = given_repository().await;
    let allows_the_role = policy("Allow", &role_arn(ROLE), json!(["codeartifact:*"]));
    let allows_the_sibling = policy("Allow", &role_arn(OTHER_ROLE), json!(["codeartifact:*"]));
    let denies_the_role = policy("Deny", &role_arn(ROLE), json!("codeartifact:*"));

    // (case, the domain's policy, the repository's policy, the scopes the call is served in)
    type Row<'a> = (&'a str, Option<&'a Value>, Option<&'a Value>, &'a [Scope]);
    let rows: &[Row<'_>] = &[
        ("no policy anywhere", None, None, &[]),
        (
            "the domain policy names the role",
            Some(&allows_the_role),
            None,
            // A domain policy grants across the domain: the repositories inside it too.
            &[Scope::Domain, Scope::Repository, Scope::Package],
        ),
        (
            "the domain policy names a sibling role",
            Some(&allows_the_sibling),
            None,
            &[],
        ),
        (
            "the repository policy names the role",
            None,
            Some(&allows_the_role),
            // A repository policy reaches the calls INSIDE that repository, and no further:
            // a domain-scoped call is not a call on the repository.
            &[Scope::Repository, Scope::Package],
        ),
        (
            "both policies name the role",
            Some(&allows_the_role),
            Some(&allows_the_role),
            &[Scope::Domain, Scope::Repository, Scope::Package],
        ),
        (
            "the domain policy denies what the repository policy allows",
            Some(&denies_the_role),
            Some(&allows_the_role),
            &[],
        ),
        (
            "the repository policy denies what the domain policy allows",
            Some(&allows_the_role),
            Some(&denies_the_role),
            // The deny is stated by the repository's policy, so it refuses the calls inside
            // that repository while the domain-scoped grant stands.
            &[Scope::Domain],
        ),
    ];

    for (case, domain_policy, repository_policy, served_scopes) in rows {
        given_domain_policy(&router, *domain_policy).await;
        given_repository_policy(&router, *repository_policy).await;
        for addressed in ADDRESSED {
            let answer = as_session(&router, SESSION_KEY_ID, addressed).await;
            let operation = addressed.operation;
            let expected_served = served_scopes.contains(&addressed.scope);
            assert_eq!(
                !refused(&answer),
                expected_served,
                "case {case:?} on {operation}: expected served={expected_served}, got {answer:?}"
            );
        }
    }
}

/// RULE: a grant is to the PRINCIPAL and the ACTION a statement names, so a domain-policy
/// grant of one action serves that action alone and reaches no sibling principal.
#[tokio::test]
async fn a_domain_policy_grant_reaches_the_principal_and_the_action_it_names_only() {
    let (router, _engine) = given_repository().await;
    let one_action = policy(
        "Allow",
        &role_arn(ROLE),
        json!(["codeartifact:ListRepositoriesInDomain"]),
    );
    given_domain_policy(&router, Some(&one_action)).await;
    given_repository_policy(&router, None).await;

    // (case, the session signing, the operation, whether the policy grants it)
    for (case, key, addressed, granted) in [
        (
            "the named role on the named action",
            SESSION_KEY_ID,
            &ADDRESSED[0],
            true,
        ),
        (
            "the named role on an action the policy does not name",
            SESSION_KEY_ID,
            &ADDRESSED[1],
            false,
        ),
        (
            "a sibling role on the named action",
            OTHER_SESSION_KEY_ID,
            &ADDRESSED[0],
            false,
        ),
    ] {
        let answer = as_session(&router, key, addressed).await;
        if granted {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected the grant to serve the call, got {answer:?}"
            );
            assert!(
                answer.body["repositories"].is_array(),
                "case {case:?}: expected the repository listing, got {answer:?}"
            );
        } else {
            assert!(
                refused(&answer),
                "case {case:?}: expected the modelled refusal, got {answer:?}"
            );
        }
    }
}

/// RULE: a caller whose identity-based grants this emulator cannot read is served while
/// nothing denies it, and an explicit `Deny` in a readable resource policy refuses it — for
/// the action that `Deny` names, and only that one.
#[tokio::test]
async fn an_explicit_deny_refuses_the_account_credential_for_the_action_it_names() {
    let (router, _engine) = given_repository().await;

    // Served with no policy anywhere: there is no identity to read policies for, so the
    // absence of a grant refuses nothing.
    for addressed in ADDRESSED {
        let answer = as_account(&router, addressed).await;
        assert!(
            !refused(&answer),
            "case \"no policy anywhere\" on {}: expected the call to be served, got {answer:?}",
            addressed.operation
        );
    }

    // Rows differ along WHICH document states the deny and which action it names: the deny
    // refuses exactly the calls that ask for that action on the resources the document
    // governs, and leaves every other call served.
    let everyone = format!("arn:aws:iam::{ACCOUNT}:root");
    for (case, on_domain, action, denied_operations) in [
        (
            "the domain policy denies one domain-scoped action",
            true,
            json!("codeartifact:ListRepositoriesInDomain"),
            &["ListRepositoriesInDomain"][..],
        ),
        (
            "the domain policy denies one package-scoped action",
            true,
            json!("codeartifact:DescribePackageVersion"),
            &["DescribePackageVersion"][..],
        ),
        (
            "the repository policy denies one repository-scoped action",
            false,
            json!("codeartifact:ListPackages"),
            &["ListPackages"][..],
        ),
        (
            "the domain policy denies every action these calls ask for",
            true,
            json!(ADDRESSED_ACTIONS),
            &[
                "ListRepositoriesInDomain",
                "DescribeDomain",
                "ListPackageGroups",
                "ListPackages",
                "GetRepositoryEndpoint",
                "DescribePackageVersion",
                "ListPackageVersions",
            ][..],
        ),
    ] {
        let document = policy("Deny", &everyone, action);
        if on_domain {
            given_domain_policy(&router, Some(&document)).await;
            given_repository_policy(&router, None).await;
        } else {
            given_domain_policy(&router, None).await;
            given_repository_policy(&router, Some(&document)).await;
        }
        for addressed in ADDRESSED {
            let answer = as_account(&router, addressed).await;
            let operation = addressed.operation;
            let expected_refusal = denied_operations.contains(&operation);
            assert_eq!(
                refused(&answer),
                expected_refusal,
                "case {case:?} on {operation}: expected refused={expected_refusal}, got {answer:?}"
            );
        }
    }

    // Withdrawn: the identical calls are served again, so the decision is read from the
    // stores on every call rather than remembered.
    given_domain_policy(&router, None).await;
    given_repository_policy(&router, None).await;
    for addressed in ADDRESSED {
        let answer = as_account(&router, addressed).await;
        assert!(
            !refused(&answer),
            "case \"after the deny is withdrawn\" on {}: expected the call to be served, got \
             {answer:?}",
            addressed.operation
        );
    }
}

/// RULE: the refusal is IAM's whole sentence, and the resource it names is the one the
/// action acts on — the domain, the repository or the addressed package.
#[tokio::test]
async fn the_refusal_names_the_resource_the_action_acts_on() {
    let (router, _engine) = given_repository().await;
    // A deny on every action these calls ask for, stated by the DOMAIN's policy, so one
    // document refuses every scope: what differs between the rows is the resource each action
    // is judged against.
    let denies_everyone = policy(
        "Deny",
        &format!("arn:aws:iam::{ACCOUNT}:root"),
        json!(ADDRESSED_ACTIONS),
    );
    given_domain_policy(&router, Some(&denies_everyone)).await;
    given_repository_policy(&router, None).await;

    for addressed in ADDRESSED {
        let answer = as_account(&router, addressed).await;
        let operation = addressed.operation;
        assert!(
            refused(&answer),
            "case {operation:?}: expected the modelled refusal, got {answer:?}"
        );
        let expected = format!(
            "User: {} is not authorized to perform: codeartifact:{operation} on resource: {} \
             with an explicit deny in a resource-based policy",
            service_common::caller_principal::ambient_user_arn(),
            judged_arn(addressed.scope)
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {operation:?}: expected IAM's whole sentence naming the {:?} ARN, got {:?}",
            addressed.scope,
            answer.message()
        );
    }
}
