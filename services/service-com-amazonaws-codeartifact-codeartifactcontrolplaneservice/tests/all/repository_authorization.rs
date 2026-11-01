// SPDX-License-Identifier: CC0-1.0

//! Whether a CodeArtifact control-plane call on a REPOSITORY is authorized, over the wire.
//!
//! The rule under test: a principal may perform `codeartifact:<OperationName>` on a
//! repository only where some policy in force allows it — the caller's own identity-based
//! policies, or that repository's own permissions policy naming the caller as `Principal` —
//! an explicit `Deny` in either refuses whatever the other says, and a principal allowed by
//! neither is refused `AccessDeniedException` (HTTP 403) instead of being served.
//!
//! It is one rule for the whole class, so every case drives it through SEVERAL operations
//! that address a repository — a read, a listing, a policy read and an update — rather than
//! through the one an issue happened to name. Each case drives the production front door, and
//! the session, the role and its policies live in the STS and IAM stores of the SAME shared
//! engine, the stores those services' own handlers write.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{
    call, call_as, create_domain, given_role, given_role_policy, given_session, role_arn,
    session_arn, Answer, ACCOUNT, REGION,
};

/// The role every session below is assumed from, and the access key id of its session's
/// credentials: an `ASIA` key, as STS mints for a session.
const ROLE: &str = "ca-reader";
const SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION02";
const SESSION_NAME: &str = "reader";

/// A SECOND role, the one a repository policy in these cases does NOT name: a sibling
/// principal in the same account, which is what shows a grant is to a principal rather than
/// to the account.
const OTHER_ROLE: &str = "ca-other";
const OTHER_SESSION_KEY_ID: &str = "ASIAEXAMPLESESSION03";
const OTHER_SESSION_NAME: &str = "other";

/// The domain and repository every case addresses.
const DOMAIN: &str = "authz";
const REPOSITORY: &str = "releases";

/// Which RESOURCE an addressed action is judged against: a CodeArtifact action acts on one
/// resource type — `ListPackages` on the repository, `ListPackageVersions` on one package
/// inside it — and the ARN of that resource is both what the decision is reached against and
/// what the refusal names.
#[derive(Debug, Clone, Copy)]
enum Judged {
    Repository,
    /// The package the request addresses, as `(format, namespace, name)`. The namespace is a
    /// SEGMENT of the package ARN's path, so a request that carries none leaves it empty.
    Package(&'static str, &'static str, &'static str),
}

/// One request on the repository, as a client sends it: the operation whose action is being
/// decided, and the HTTP method, URI and body that address it.
#[derive(Debug, Clone, Copy)]
struct Addressed {
    operation: &'static str,
    method: &'static str,
    uri: &'static str,
    /// Whether the operation takes a body at all: its members travel in the query string,
    /// so the body a POST carries is the empty object its SDK sends.
    body: bool,
    /// The resource this operation's action is judged against.
    judged: Judged,
}

/// The repository-addressed operations these cases drive. Each is an operation whose input
/// names a repository within a domain (`smithy_query(query="digest")`), so the resource the
/// decision is reached against is that repository and the resource-based input is its own
/// permissions policy.
const ADDRESSED: &[Addressed] = &[
    Addressed {
        operation: "GetRepositoryEndpoint",
        method: "GET",
        uri: "/v1/repository/endpoint?domain=authz&repository=releases&format=npm",
        body: false,
        judged: Judged::Repository,
    },
    Addressed {
        operation: "DescribeRepository",
        method: "GET",
        uri: "/v1/repository?domain=authz&repository=releases",
        body: false,
        judged: Judged::Repository,
    },
    Addressed {
        operation: "ListPackages",
        method: "POST",
        uri: "/v1/packages?domain=authz&repository=releases",
        body: true,
        judged: Judged::Repository,
    },
    Addressed {
        operation: "GetRepositoryPermissionsPolicy",
        method: "GET",
        uri: "/v1/repository/permissions/policy?domain=authz&repository=releases",
        body: false,
        judged: Judged::Repository,
    },
    Addressed {
        operation: "ListPackageVersions",
        method: "POST",
        uri: "/v1/package/versions?domain=authz&repository=releases&format=npm&package=left-pad",
        body: true,
        judged: Judged::Package("npm", "", "left-pad"),
    },
];

/// An emulator with this one service mounted on one shared engine, a domain and a repository
/// created by the account's own credential, and both roles held with sessions recorded.
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
    for (role, key, session) in [
        (ROLE, SESSION_KEY_ID, SESSION_NAME),
        (OTHER_ROLE, OTHER_SESSION_KEY_ID, OTHER_SESSION_NAME),
    ] {
        given_role(&engine, role);
        given_session(&engine, key, role, session, 3_600);
    }
    (router, engine)
}

/// A policy allowing `actions` on every resource, as an identity policy states one (no
/// `Principal` element).
fn identity(effect: &str, actions: Value) -> Value {
    json!({
        "Version": "2012-10-17",
        "Statement": [{ "Effect": effect, "Action": actions, "Resource": "*" }]
    })
}

/// A repository permissions policy stating `effect` for `principal` on `actions`, the shape
/// `PutRepositoryPermissionsPolicy` takes.
fn resource_policy(effect: &str, principal: &str, actions: Value) -> Value {
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

/// Attach `document` to the repository with `PutRepositoryPermissionsPolicy`, as the
/// account's own credential (the caller this gate does not bound), or detach whatever it
/// carries when `None`.
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
        // `DeleteRepositoryPermissionsPolicy` is bound to the PLURAL path
        // (`smithy_query(query="operation",operation="DeleteRepositoryPermissionsPolicy")`:
        // DELETE /v1/repository/permissions/policies).
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
    assert!(
        answer.status == 200 || answer.status == 404,
        "seeding the repository policy must succeed or find none to remove, got {answer:?}"
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

/// The sentence IAM refuses a request with that no identity-based policy allows: its fixed
/// text, with the principal, the action and the repository ARN taken from the request and
/// from state.
fn no_identity_grant(session: &str, action: &str, repository: &str) -> String {
    format!(
        "User: {session} is not authorized to perform: {action} on resource: {repository} \
         because no identity-based policy allows the {action} action"
    )
}

/// The ARN of the resource an addressed action is judged against, given the repository's own
/// ARN: the repository itself, or the package inside it the request addressed
/// (`package/<domain>/<repository>/<format>/<namespace>/<name>`, the package resource type of
/// the IAM Service Authorization Reference, with the namespace segment empty where the
/// request carries none).
fn judged_arn(repository_arn: &str, judged: Judged) -> String {
    match judged {
        Judged::Repository => repository_arn.to_string(),
        Judged::Package(format, namespace, name) => format!(
            "arn:aws:codeartifact:{REGION}:{ACCOUNT}:package/{DOMAIN}/{REPOSITORY}/{format}/\
             {namespace}/{name}"
        ),
    }
}

/// The repository ARN a decision about this repository is reached against — the ARN the
/// repository itself reports, read back through `DescribeRepository` rather than spelled
/// twice.
async fn repository_arn(router: &MultiServiceRouter) -> String {
    let described = call(
        router,
        "GET",
        &format!("/v1/repository?domain={DOMAIN}&repository={REPOSITORY}"),
        None,
    )
    .await;
    described
        .body
        .get("repository")
        .and_then(|repository| repository.get("arn"))
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("no repository ARN in {described:?}"))
        .to_string()
}

/// RULE: either document may grant, either may deny, and a principal neither allows is
/// refused — over every repository-addressed operation, not one of them.
#[tokio::test]
async fn a_repository_call_is_served_only_where_some_policy_allows_its_action() {
    let (router, engine) = given_repository().await;
    let names_the_role = resource_policy("Allow", &role_arn(ROLE), json!(["codeartifact:*"]));
    let names_the_session = resource_policy(
        "Allow",
        &session_arn(ROLE, SESSION_NAME),
        json!(["codeartifact:*"]),
    );
    let names_the_other_role =
        resource_policy("Allow", &role_arn(OTHER_ROLE), json!(["codeartifact:*"]));
    let names_the_account = resource_policy(
        "Allow",
        &format!("arn:aws:iam::{ACCOUNT}:root"),
        json!(["codeartifact:*"]),
    );
    let denies_the_role = resource_policy("Deny", &role_arn(ROLE), json!("codeartifact:*"));
    let allows_everything = identity("Allow", json!("*"));
    let allows_this_service = identity("Allow", json!("codeartifact:*"));
    let denies_this_service = identity("Deny", json!("codeartifact:*"));

    // Rows differ along WHICH document grants (neither, the identity policy, the repository
    // policy, both), along WHO the repository policy names (the role, one of its sessions,
    // a sibling role, the whole account) and along whether a readable document DENIES.
    let rows: &[(&str, Option<&Value>, Option<&Value>, bool)] = &[
        ("no policy anywhere", None, None, false),
        (
            "the identity policy alone allows this service",
            Some(&allows_this_service),
            None,
            true,
        ),
        (
            "the identity policy allows every action of every service",
            Some(&allows_everything),
            None,
            true,
        ),
        (
            "the repository policy alone names the role",
            None,
            Some(&names_the_role),
            true,
        ),
        (
            "the repository policy alone names the session",
            None,
            Some(&names_the_session),
            true,
        ),
        (
            "the repository policy names a sibling role",
            None,
            Some(&names_the_other_role),
            false,
        ),
        (
            "the repository policy names the account root",
            None,
            Some(&names_the_account),
            false,
        ),
        (
            "the repository policy denies the role its identity policy allows",
            Some(&allows_everything),
            Some(&denies_the_role),
            false,
        ),
        (
            "the identity policy denies the role the repository policy allows",
            Some(&denies_this_service),
            Some(&names_the_role),
            false,
        ),
        (
            "both documents allow it",
            Some(&allows_this_service),
            Some(&names_the_role),
            true,
        ),
    ];

    for (case, identity_policy, policy, allowed) in rows {
        given_role_policy(&engine, ROLE, *identity_policy);
        given_repository_policy(&router, *policy).await;
        for addressed in ADDRESSED {
            let answer = as_session(&router, SESSION_KEY_ID, addressed).await;
            let operation = addressed.operation;
            if *allowed {
                assert_ne!(
                    (answer.status, answer.code()),
                    (403, "AccessDeniedException".to_string()),
                    "case {case:?} on {operation}: expected the call to be authorized, got \
                     {answer:?}"
                );
            } else {
                assert_eq!(
                    (answer.status, answer.code()),
                    (403, "AccessDeniedException".to_string()),
                    "case {case:?} on {operation}: expected the modelled refusal, got {answer:?}"
                );
            }
        }
    }
}

/// RULE: a grant is to the PRINCIPAL the policy names, so a sibling session in the same
/// account is refused the call the named one is served — and the grant is per ACTION, so an
/// operation the policy does not name is refused for the named principal too.
#[tokio::test]
async fn a_resource_grant_reaches_the_principal_and_the_action_it_names_only() {
    let (router, engine) = given_repository().await;
    // Neither role holds any identity-based CodeArtifact permission: the repository's own
    // policy is the whole of the grant.
    for role in [ROLE, OTHER_ROLE] {
        given_role_policy(&engine, role, None);
    }
    let one_action = resource_policy(
        "Allow",
        &role_arn(ROLE),
        json!(["codeartifact:GetRepositoryEndpoint"]),
    );
    given_repository_policy(&router, Some(&one_action)).await;

    // (case, the session signing, the operation, whether the policy grants it)
    for (case, key, addressed, granted) in [
        (
            "the named role on the granted action",
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
            "a sibling role on the granted action",
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
                answer
                    .body
                    .get("repositoryEndpoint")
                    .and_then(Value::as_str)
                    .is_some_and(|endpoint| endpoint.starts_with("https://")),
                "case {case:?}: expected an https repositoryEndpoint, got {answer:?}"
            );
        } else {
            assert_eq!(
                (answer.status, answer.code()),
                (403, "AccessDeniedException".to_string()),
                "case {case:?}: expected the modelled refusal, got {answer:?}"
            );
            assert!(
                answer.body.get("repositoryEndpoint").is_none(),
                "case {case:?}: a refused call must carry no endpoint, got {answer:?}"
            );
        }
    }
}

/// RULE: the refusal is IAM's whole sentence — the signing session's ARN, the
/// service-qualified action of the operation refused, and the ARN of the resource that
/// action acts on: the repository for a repository-scoped action, the addressed package for
/// a package-scoped one.
#[tokio::test]
async fn the_refusal_names_the_session_the_action_and_the_judged_resource() {
    let (router, engine) = given_repository().await;
    given_role_policy(&engine, ROLE, None);
    // A repository policy that names a DIFFERENT principal: the refusal is still the
    // implicit-deny sentence, because a resource policy that does not name the caller
    // withholds nothing of its own — the permission the caller is missing is an
    // identity-based one.
    let names_the_other_role =
        resource_policy("Allow", &role_arn(OTHER_ROLE), json!(["codeartifact:*"]));
    let arn = repository_arn(&router).await;

    for policy in [None, Some(&names_the_other_role)] {
        given_repository_policy(&router, policy).await;
        for addressed in ADDRESSED {
            let answer = as_session(&router, SESSION_KEY_ID, addressed).await;
            let case = addressed.operation;
            let expected = no_identity_grant(
                &session_arn(ROLE, SESSION_NAME),
                &format!("codeartifact:{case}"),
                &judged_arn(&arn, addressed.judged),
            );
            assert_eq!(
                (answer.status, answer.code()),
                (403, "AccessDeniedException".to_string()),
                "case {case:?}: expected the modelled refusal, got {answer:?}"
            );
            assert_eq!(
                answer.message(),
                expected,
                "case {case:?}: expected IAM's whole sentence, got {:?}",
                answer.message()
            );
        }
    }
}

/// RULE: the decision is read from the stores on EVERY call, so a grant takes effect on the
/// next request and withdrawing it refuses the identical call again — and the resource the
/// decision is reached against is the repository the request names, so a grant on one
/// repository carries nothing to its neighbour.
#[tokio::test]
async fn a_grant_takes_effect_and_is_withdrawn_on_the_next_call() {
    let (router, engine) = given_repository().await;
    given_role_policy(&engine, ROLE, None);
    let neighbour = "staging";
    let created = call(
        &router,
        "POST",
        &format!("/v1/repository?domain={DOMAIN}&repository={neighbour}"),
        Some(json!({})),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "CreateRepository for the neighbour must succeed, got {created:?}"
    );
    let on_neighbour = Addressed {
        operation: "DescribeRepository",
        method: "GET",
        uri: "/v1/repository?domain=authz&repository=staging",
        body: false,
        judged: Judged::Repository,
    };
    let names_the_role = resource_policy("Allow", &role_arn(ROLE), json!(["codeartifact:*"]));

    // Before the grant: refused.
    let before = as_session(&router, SESSION_KEY_ID, &ADDRESSED[1]).await;
    assert_eq!(
        (before.status, before.code()),
        (403, "AccessDeniedException".to_string()),
        "case \"before the grant\": expected the modelled refusal, got {before:?}"
    );

    // Granted on this repository: served here, still refused on the neighbour, which the
    // grant does not cover.
    given_repository_policy(&router, Some(&names_the_role)).await;
    let during = as_session(&router, SESSION_KEY_ID, &ADDRESSED[1]).await;
    assert_eq!(
        during.status, 200,
        "case \"while granted\": expected the call to be served, got {during:?}"
    );
    let sibling = as_session(&router, SESSION_KEY_ID, &on_neighbour).await;
    assert_eq!(
        (sibling.status, sibling.code()),
        (403, "AccessDeniedException".to_string()),
        "case \"a neighbouring repository the grant does not cover\": expected the modelled \
         refusal, got {sibling:?}"
    );

    // Withdrawn: the identical call is refused again.
    given_repository_policy(&router, None).await;
    let after = as_session(&router, SESSION_KEY_ID, &ADDRESSED[1]).await;
    assert_eq!(
        (after.status, after.code()),
        (403, "AccessDeniedException".to_string()),
        "case \"after the grant is withdrawn\": expected the modelled refusal, got {after:?}"
    );
}

/// RULE: a caller this gate does not bound is served — the account's own published
/// credential, which no identity policy governs, and a session whose role this account does
/// not hold.
#[tokio::test]
async fn a_caller_with_no_identity_policies_to_read_is_served() {
    let (router, engine) = given_repository().await;
    given_role_policy(&engine, ROLE, None);
    given_repository_policy(&router, None).await;

    // (case, how the request is signed)
    for (case, key) in [
        ("the account's own published credential", "AKIDEXAMPLE"),
        ("a session this emulator holds no record of", "ASIAUNKNOWN000000001"),
    ] {
        for addressed in ADDRESSED {
            let answer = as_session(&router, key, addressed).await;
            let operation = addressed.operation;
            assert_ne!(
                (answer.status, answer.code()),
                (403, "AccessDeniedException".to_string()),
                "case {case:?} on {operation}: expected the call to be served, got {answer:?}"
            );
        }
    }

    // A session of a role the ACCOUNT does not hold: there is no identity whose policies
    // could be read, so it is not refused on evidence this emulator does not have.
    given_session(&engine, "ASIAEXAMPLESESSION04", "not-held", "ghost", 3_600);
    let answer = as_session(&router, "ASIAEXAMPLESESSION04", &ADDRESSED[0]).await;
    assert_eq!(
        answer.status, 200,
        "case \"a session of a role the account does not hold\": expected the call to be served, \
         got {answer:?}"
    );
}
