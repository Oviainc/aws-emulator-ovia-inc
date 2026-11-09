// SPDX-License-Identifier: CC0-1.0

//! The identities a cluster's resource-based policy GRANTS TO, resolved against this
//! account's IAM before the document is stored.
//!
//! RULE: a resource-based policy document states WHO a permission is granted to, so
//! every operation that writes a cluster's policy resolves each identity the
//! statements' `Principal` elements name against this account's IAM before the
//! document is stored — a grant to an identity that does not exist is a grant to
//! nobody — and refuses the whole request with the modelled `ValidationException`
//! (HTTP 400) when IAM answers that one of them is absent, leaving the document the
//! cluster already carries and the version it carries it under exactly as they were.
//! A refusal has to be grounded in what IAM ANSWERED, so a principal that names no
//! IAM role or user of this account — the wildcard, an account or its root, a
//! service or federated principal, an identity of ANOTHER account, whose IAM this
//! emulator is not the one to ask — is resolved by nothing and refuses nothing.
//!
//! Both entry points that write a document go through the rule: `PutClusterPolicy`
//! and `CreateCluster`'s inline `policy`.
//!
//! The emulator here holds Aurora DSQL beside IAM, which holds the identities a
//! document names, on one engine — the shape `core/router/src/lib.rs` mounts every
//! service in — so each row is resolved against the very roles and users the row
//! created through IAM's own API. Every call goes the whole way round through
//! `MultiServiceRouter::dispatch`, through the shared `Dsql` harness.
//!
//! The table spans what the rule generalises over: the IAM entity type the ARN names
//! (role, user, an account's root), the path the entity lives under, the form the
//! `Principal` element takes (a bare string, a typed object, a list), the principal
//! type key (`AWS`, `Service`, `Federated`), which account the ARN names, how many
//! statements the document carries, and whether the entity was ever created.

use serde_json::{json, Value};
use service_com_amazonaws_dsql_dsql::state::policy_principals;

use crate::harness::{xml_text, Dsql};

/// The Region every call below is signed for.
const REGION: &str = "us-east-1";

/// An account that is NOT the one this emulator serves, so its identities are held by
/// an IAM this emulator is not the one to ask. Twelve digits, as every ARN's account
/// field is.
const ANOTHER_ACCOUNT: &str = "210987654321";

/// The role every accepted row grants to, and the one the baseline document names.
const HELD_ROLE: &str = "dsql-reader";

/// A role IAM holds under a path, the form the model documents for a console-created
/// role (`:role/service-role/<name>`).
const HELD_PATH_ROLE: &str = "dsql-path-reader";

/// The IAM path [`HELD_PATH_ROLE`] lives under.
const ROLE_PATH: &str = "/service-role/";

/// The user every accepted row that names a user grants to.
const HELD_USER: &str = "dsql-operator";

/// A role name nothing ever created.
const ABSENT_ROLE: &str = "dsql-never-created";

/// A user name nothing ever created.
const ABSENT_USER: &str = "dsql-never-created-user";

/// The account this emulator serves — the one every cluster it mints belongs to.
fn own_account() -> String {
    engine::EMULATOR_ACCOUNT_ID.to_string()
}

/// An IAM ARN of `account`: `arn:aws:iam::<account>:<resource>`.
fn iam_arn(account: &str, resource: &str) -> String {
    format!("arn:aws:iam::{account}:{resource}")
}

/// The sentence a document Aurora DSQL cannot apply is refused with, in full: it
/// names the member the document arrived in (`smithy_query(query="operation",
/// operation="CreateCluster,PutClusterPolicy")`: `policy`) and the elements a usable
/// document carries.
const INAPPLICABLE_POLICY: &str = "Invalid input for 'policy'. Aurora DSQL can't apply the policy \
     because the policy document isn't valid. Make sure your policy includes all required \
     elements (Version, Statement, Principal, Action, and Resource) in valid JSON format and try \
     again.";

/// A document granting one read action on every resource to `principal`, whatever
/// form that element takes. `Allow` throughout, so no row is about the lockout safety
/// check — that rule is `tests/all/policy_principal_set.rs`.
fn policy(principal: Value) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Sid": "AllowConnect",
            "Effect": "Allow",
            "Principal": principal,
            "Action": "dsql:DbConnect",
            "Resource": "*",
        }]
    })
    .to_string()
}

/// A document whose statements are the ones given, for the rows that span how many
/// statements a document carries.
fn document_of(statements: Vec<Value>) -> String {
    json!({"Version": "2012-10-17", "Statement": statements}).to_string()
}

/// One `Allow` statement granting a read action to `principal`.
fn statement(sid: &str, principal: Value) -> Value {
    json!({
        "Sid": sid,
        "Effect": "Allow",
        "Principal": principal,
        "Action": "dsql:DbConnect",
        "Resource": "*",
    })
}

/// A trust policy admitting one service, so the fixture roles are ordinary roles:
/// what this suite asks IAM is whether the entity EXISTS, not what may assume it.
fn trust_policy() -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": {"Service": "dsql.amazonaws.com"},
            "Action": "sts:AssumeRole",
        }]
    })
    .to_string()
}

/// Create the identities the rows name through IAM's own API — two roles (one under
/// the default path, one under [`ROLE_PATH`]) and one user — and answer the ARNs IAM
/// itself minted for them, so no row asserts against an ARN this suite spelled.
async fn given_identities(dsql: &Dsql) -> (String, String, String) {
    let trust = trust_policy();
    let mut role_arns = Vec::new();
    for (name, path) in [(HELD_ROLE, "/"), (HELD_PATH_ROLE, ROLE_PATH)] {
        let (status, document) = dsql
            .iam(
                "CreateRole",
                &[
                    ("RoleName", name),
                    ("Path", path),
                    ("AssumeRolePolicyDocument", &trust),
                ],
            )
            .await;
        assert_eq!(
            status, 200,
            "setup: the role {name} must be created in IAM first, got {status} {document}"
        );
        role_arns.push(
            xml_text(&document, "Arn")
                .unwrap_or_else(|| panic!("CreateRole must answer the role's ARN, got {document}")),
        );
    }
    let (status, document) = dsql.iam("CreateUser", &[("UserName", HELD_USER)]).await;
    assert_eq!(
        status, 200,
        "setup: the user {HELD_USER} must be created in IAM first, got {status} {document}"
    );
    let user_arn = xml_text(&document, "Arn")
        .unwrap_or_else(|| panic!("CreateUser must answer the user's ARN, got {document}"));
    (role_arns[0].clone(), role_arns[1].clone(), user_arn)
}

/// One row: the document a write carries, and whether IAM resolves every identity it
/// grants to — so whether the document must be stored.
struct PrincipalCase {
    name: &'static str,
    document: String,
    stored: bool,
}

/// The rows, given the ARNs IAM minted for the identities it holds.
fn principal_cases(role: &str, path_role: &str, user: &str) -> Vec<PrincipalCase> {
    let own = own_account();
    let row = |name: &'static str, document: String, stored: bool| PrincipalCase {
        name,
        document,
        stored,
    };
    vec![
        // --- identities IAM holds, which resolve ------------------------------
        row(
            "a role of this account that exists",
            policy(json!({"AWS": role})),
            true,
        ),
        row(
            "a role of this account under a path",
            policy(json!({"AWS": path_role})),
            true,
        ),
        row(
            "a user of this account that exists",
            policy(json!({"AWS": user})),
            true,
        ),
        row(
            "a list whose identities all exist",
            policy(json!({"AWS": [role, user]})),
            true,
        ),
        row(
            "two statements, each naming an identity that exists",
            document_of(vec![
                statement("one", json!({"AWS": role})),
                statement("two", json!({"AWS": user})),
            ]),
            true,
        ),
        // --- values that name no IAM role or user of this account -------------
        row(
            "the wildcard, as a typed object",
            policy(json!({"AWS": "*"})),
            true,
        ),
        row("the wildcard, as a bare string", policy(json!("*")), true),
        row(
            "this account's root, which names no role or user",
            policy(json!({"AWS": iam_arn(&own, "root")})),
            true,
        ),
        row(
            "this account by its bare id",
            policy(json!({"AWS": own.clone()})),
            true,
        ),
        row(
            "a service principal",
            policy(json!({"Service": "dsql.amazonaws.com"})),
            true,
        ),
        row(
            "a federated principal, which is no IAM role or user",
            policy(json!({"Federated": iam_arn(&own, "saml-provider/corp")})),
            true,
        ),
        row(
            // Another account's IAM is not this emulator's to ask, so nothing is
            // known about the identity and the document is not refused on an
            // assumption.
            "a role of another account that was never created",
            policy(json!({"AWS": iam_arn(ANOTHER_ACCOUNT, &format!("role/{ABSENT_ROLE}"))})),
            true,
        ),
        row(
            "another account's root",
            policy(json!({"AWS": iam_arn(ANOTHER_ACCOUNT, "root")})),
            true,
        ),
        row(
            "a statement naming no principal at all",
            document_of(vec![json!({
                "Sid": "NoPrincipal",
                "Effect": "Allow",
                "Action": "dsql:DbConnect",
                "Resource": "*",
            })]),
            true,
        ),
        // --- identities of this account IAM holds nothing under ---------------
        row(
            "a role of this account that was never created",
            policy(json!({"AWS": iam_arn(&own, &format!("role/{ABSENT_ROLE}"))})),
            false,
        ),
        row(
            "a role under a path that was never created",
            policy(json!({"AWS": iam_arn(&own, &format!("role/service-role/{ABSENT_ROLE}"))})),
            false,
        ),
        row(
            "a user of this account that was never created",
            policy(json!({"AWS": iam_arn(&own, &format!("user/{ABSENT_USER}"))})),
            false,
        ),
        row(
            "a role named as a bare string beside nothing else",
            policy(json!(iam_arn(&own, &format!("role/{ABSENT_ROLE}")))),
            false,
        ),
        row(
            "a list in which one identity is absent",
            policy(json!({"AWS": [role, iam_arn(&own, &format!("role/{ABSENT_ROLE}"))]})),
            false,
        ),
        row(
            "two principal types together, one of them absent",
            policy(json!({
                "Service": "dsql.amazonaws.com",
                "AWS": iam_arn(&own, &format!("user/{ABSENT_USER}")),
            })),
            false,
        ),
        row(
            "the absent identity in the second of two statements",
            document_of(vec![
                statement("one", json!({"AWS": role})),
                statement(
                    "two",
                    json!({"AWS": iam_arn(&own, &format!("role/{ABSENT_ROLE}"))}),
                ),
            ]),
            false,
        ),
    ]
}

/// RULE over `PutClusterPolicy`: a document whose identities IAM all resolves is
/// attached and reads back; one granting to an identity IAM holds nothing under is
/// refused 400 `ValidationException` and leaves the attached document AND its
/// `policyVersion` exactly as they were.
#[tokio::test]
async fn a_cluster_policy_is_attached_only_when_iam_resolves_every_identity_it_grants_to() {
    let dsql = Dsql::with_delivery_services(REGION);
    let (role, path_role, user) = given_identities(&dsql).await;
    let cases = principal_cases(&role, &path_role, &user);
    let clusters = dsql.create_settled_many(cases.len()).await;

    // Every row starts from a document that IS attached — one granting to the role
    // IAM holds — so a refusal below is visible as the document the cluster keeps and
    // as the version it keeps it under.
    let baseline = policy(json!({"AWS": role.clone()}));

    for (case, identifier) in cases.iter().zip(&clusters) {
        let attached = dsql
            .put_policy(identifier, json!({"policy": baseline.clone()}))
            .await;
        assert_eq!(
            attached.status, 200,
            "case {:?}: the baseline document must be attached, got {} {:?}",
            case.name, attached.status, attached.body
        );
        let baseline_version = attached.str("policyVersion");

        let answer = dsql
            .put_policy(
                identifier,
                json!({
                    "policy": case.document.clone(),
                    "expectedPolicyVersion": baseline_version.clone(),
                }),
            )
            .await;

        let (expected_document, expected_version) = if case.stored {
            assert_eq!(
                answer.status, 200,
                "case {:?}: a document IAM resolves every identity of must be attached, got {} \
                 {:?}",
                case.name, answer.status, answer.body
            );
            assert_ne!(
                answer.str("policyVersion"),
                baseline_version,
                "case {:?}: an attached document must answer a new version, got {:?}",
                case.name,
                answer.body
            );
            (case.document.clone(), answer.str("policyVersion"))
        } else {
            assert_eq!(
                (answer.status, answer.error.as_deref()),
                (400, Some("ValidationException")),
                "case {:?}: an identity IAM does not hold must be refused 400 \
                 ValidationException, got {} {:?} {:?}",
                case.name,
                answer.status,
                answer.error,
                answer.body
            );
            assert_eq!(
                answer.message(),
                INAPPLICABLE_POLICY,
                "case {:?}: expected {INAPPLICABLE_POLICY:?}, got {:?}",
                case.name,
                answer.message()
            );
            assert!(
                !answer.body.to_string().contains(ABSENT_ROLE)
                    && !answer.body.to_string().contains(ABSENT_USER),
                "case {:?}: the refusal must echo nothing of the document, got {:?}",
                case.name,
                answer.body
            );
            (baseline.clone(), baseline_version.clone())
        };

        let read = dsql.get_policy(identifier).await;
        assert_eq!(
            (read.status, read.str("policy"), read.str("policyVersion")),
            (200, expected_document.clone(), expected_version.clone()),
            "case {:?}: the cluster must hold {expected_document} at version \
             {expected_version}, got {} {:?}",
            case.name,
            read.status,
            read.body
        );

        // And the version counter did not move either: a write that still claims the
        // version the refused call was built against is served.
        if !case.stored {
            let claimed = dsql
                .put_policy(
                    identifier,
                    json!({
                        "policy": baseline.clone(),
                        "expectedPolicyVersion": baseline_version.clone(),
                    }),
                )
                .await;
            assert_eq!(
                claimed.status, 200,
                "case {:?}: a refusal must mint no version, so {baseline_version} is still the \
                 current one, got {} {:?}",
                case.name,
                claimed.status,
                claimed.body
            );
        }
    }
}

/// RULE over `CreateCluster`'s inline `policy`: the verdict belongs to the DOCUMENT,
/// so the same identities are resolved there — and a refused create mints no cluster
/// at all.
#[tokio::test]
async fn create_cluster_resolves_an_inline_documents_identities_the_same_way() {
    let dsql = Dsql::with_delivery_services(REGION);
    let (role, path_role, user) = given_identities(&dsql).await;

    let mut held = 0usize;
    for case in principal_cases(&role, &path_role, &user) {
        let created = dsql
            .create(json!({
                "deletionProtectionEnabled": false,
                "policy": case.document.clone(),
            }))
            .await;

        if case.stored {
            assert_eq!(
                created.status, 200,
                "case {:?}: a document IAM resolves every identity of must be attached, got {} \
                 {:?}",
                case.name, created.status, created.body
            );
            held += 1;
            let read = dsql.get_policy(&created.str("identifier")).await;
            assert_eq!(
                read.str("policy"),
                case.document,
                "case {:?}: the inline document must read back verbatim, got {:?}",
                case.name,
                read.body
            );
        } else {
            assert_eq!(
                (created.status, created.error.as_deref()),
                (400, Some("ValidationException")),
                "case {:?}: an identity IAM does not hold must be refused 400 \
                 ValidationException, got {} {:?}",
                case.name,
                created.status,
                created.body
            );
            assert_eq!(
                created.message(),
                INAPPLICABLE_POLICY,
                "case {:?}: expected {INAPPLICABLE_POLICY:?}, got {:?}",
                case.name,
                created.message()
            );
        }

        // A refused create mints nothing, so the store holds exactly the clusters the
        // accepted rows created.
        let listed = dsql.list("").await;
        let count = listed.get("clusters").as_array().map_or(0, Vec::len);
        assert_eq!(
            count, held,
            "case {:?}: the store must hold {held} clusters, got {count} ({:?})",
            case.name,
            listed.get("clusters")
        );
    }
}

/// The identities a document grants to, as they are READ out of it: every form the
/// policy language writes a `Principal` in, flattened in the order the document names
/// them and without repeats — and `NotPrincipal`, which states who a statement
/// EXEMPTS rather than who it grants to, naming nobody to resolve.
#[test]
fn the_identities_a_document_grants_to_are_read_out_of_every_principal_form() {
    let own = own_account();
    let role = iam_arn(&own, &format!("role/{HELD_ROLE}"));
    let user = iam_arn(&own, &format!("user/{HELD_USER}"));
    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "the bare-string shorthand",
            policy(json!("*")),
            vec!["*".to_string()],
        ),
        (
            "one AWS identity",
            policy(json!({"AWS": role.clone()})),
            vec![role.clone()],
        ),
        (
            "a service principal",
            policy(json!({"Service": "dsql.amazonaws.com"})),
            vec!["dsql.amazonaws.com".to_string()],
        ),
        (
            "a list of AWS identities",
            policy(json!({"AWS": [role.clone(), user.clone()]})),
            vec![role.clone(), user.clone()],
        ),
        (
            "one statement written as the object itself",
            json!({
                "Version": "2012-10-17",
                "Statement": statement("one", json!({"AWS": role.clone()})),
            })
            .to_string(),
            vec![role.clone()],
        ),
        (
            "two statements, each naming its own identity",
            document_of(vec![
                statement("one", json!({"AWS": role.clone()})),
                statement("two", json!({"AWS": user.clone()})),
            ]),
            vec![role.clone(), user.clone()],
        ),
        (
            // The same identity named twice is ONE identity to resolve.
            "one identity named by two statements",
            document_of(vec![
                statement("one", json!({"AWS": role.clone()})),
                statement("two", json!({"AWS": role.clone()})),
            ]),
            vec![role.clone()],
        ),
        (
            // An exemption grants to nobody, so it names nobody to resolve.
            "a statement naming NotPrincipal only",
            document_of(vec![json!({
                "Effect": "Deny",
                "NotPrincipal": {"AWS": role.clone()},
                "Action": "dsql:DbConnect",
                "Resource": "*",
            })]),
            Vec::new(),
        ),
        (
            "a document with no statement, which names nobody",
            json!({"Version": "2012-10-17"}).to_string(),
            Vec::new(),
        ),
        (
            "text that is no policy document at all",
            "not a policy".to_string(),
            Vec::new(),
        ),
    ];

    for (case, document, expected) in cases {
        let read = policy_principals(&document);
        assert_eq!(
            read, expected,
            "case {case:?}: expected {expected:?}, got {read:?} for {document}"
        );
    }
}

/// `bypassPolicyLockoutSafetyCheck` accepts exactly one risk — a document nobody
/// could replace — and says nothing about whether the identities it grants to exist:
/// a document naming an identity IAM holds nothing under is refused with the flag set
/// as it is without it.
#[tokio::test]
async fn the_lockout_bypass_does_not_accept_an_unresolved_identity() {
    let dsql = Dsql::with_delivery_services(REGION);
    let (role, _, _) = given_identities(&dsql).await;
    let cluster = dsql.create_settled().await;

    let held = policy(json!({"AWS": role}));
    let attached = dsql
        .put_policy(&cluster, json!({"policy": held.clone()}))
        .await;
    assert_eq!(
        attached.status, 200,
        "the baseline document must be attached, got {} {:?}",
        attached.status, attached.body
    );
    let version = attached.str("policyVersion");

    let absent = policy(json!({"AWS": iam_arn(&own_account(), &format!("role/{ABSENT_ROLE}"))}));
    for (case, bypass) in [
        ("with the lockout bypass set", true),
        ("without it", false),
    ] {
        let answer = dsql
            .put_policy(
                &cluster,
                json!({"policy": absent.clone(), "bypassPolicyLockoutSafetyCheck": bypass}),
            )
            .await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("ValidationException")),
            "case {case:?}: an identity IAM does not hold must be refused, got {} {:?}",
            answer.status,
            answer.body
        );
        let read = dsql.get_policy(&cluster).await;
        assert_eq!(
            (read.str("policy"), read.str("policyVersion")),
            (held.clone(), version.clone()),
            "case {case:?}: the refusal must leave the attached document and its version alone, \
             got {:?}",
            read.body
        );
    }

    // Control: with the identity CREATED, the very same document lands — so the
    // refusals above are about what IAM answered and not about the document's form.
    let (status, document) = dsql
        .iam(
            "CreateRole",
            &[
                ("RoleName", ABSENT_ROLE),
                ("Path", "/"),
                ("AssumeRolePolicyDocument", &trust_policy()),
            ],
        )
        .await;
    assert_eq!(
        status, 200,
        "the control role must be created in IAM, got {status} {document}"
    );
    let answer = dsql
        .put_policy(&cluster, json!({"policy": absent.clone()}))
        .await;
    assert_eq!(
        answer.status, 200,
        "case the identity now exists: the document must be attached, got {} {:?}",
        answer.status, answer.body
    );
    let read = dsql.get_policy(&cluster).await;
    assert_eq!(
        read.str("policy"),
        absent,
        "case the identity now exists: the cluster must hold the document, got {:?}",
        read.body
    );
}

/// With NO IAM mounted there is nothing to ask, so a refusal has nothing to stand on:
/// every document is attached as submitted rather than refused on an assumption.
#[tokio::test]
async fn with_no_iam_to_ask_a_document_is_attached_as_submitted() {
    let dsql = Dsql::in_region(REGION);
    let cluster = dsql.create_settled().await;

    let document = policy(json!({"AWS": iam_arn(&own_account(), &format!("role/{ABSENT_ROLE}"))}));
    let answer = dsql
        .put_policy(&cluster, json!({"policy": document.clone()}))
        .await;
    assert_eq!(
        answer.status, 200,
        "with no IAM to ask the document must be attached, got {} {:?}",
        answer.status, answer.body
    );
    let read = dsql.get_policy(&cluster).await;
    assert_eq!(
        read.str("policy"),
        document,
        "with no IAM to ask the cluster must hold the submitted document, got {:?}",
        read.body
    );
}
