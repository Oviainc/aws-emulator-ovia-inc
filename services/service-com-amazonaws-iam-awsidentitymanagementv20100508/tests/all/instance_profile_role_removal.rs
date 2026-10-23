// SPDX-License-Identifier: CC0-1.0

//! `RemoveRoleFromInstanceProfile` and `DeleteInstanceProfile`, end to end over the
//! awsQuery wire.
//!
//! RULE under test: an IAM operation that removes an association between two entities
//! CLEARS that association in the account's state, so every later read of either
//! entity, and every delete-conflict guard that consults it, stops seeing it — while
//! every other dependency the entities carry is left exactly as it was. Applied here:
//!
//! 1. `RemoveRoleFromInstanceProfile` "removes the specified IAM role from the
//!    specified Amazon EC2 instance profile", so afterwards `GetInstanceProfile`
//!    reports no role, the role itself is unchanged, and the profile no longer counts
//!    against `DeleteRole` — "before attempting to delete a role, remove the following
//!    attached items: … instance profile (RemoveRoleFromInstanceProfile)"
//!    (`DeleteRole`'s documentation). A policy that also holds the role back keeps
//!    holding it back: only the binding was cleared.
//! 2. `DeleteInstanceProfile` reads the same binding — "the instance profile must not
//!    have an associated role" — so it is the modelled `DeleteConflict` (HTTP 409)
//!    while the binding stands and succeeds once it is cleared.
//! 3. Removing a binding the account does not hold "referenced a resource entity that
//!    does not exist": the modelled `NoSuchEntity` (HTTP 404), whether the profile, the
//!    role or the binding between them is the part that is missing.
//! 4. A service-linked role is "a protected Amazon Web Services resource. Only the
//!    service that depends on the service-linked role can modify or delete the role on
//!    your behalf" (`UnmodifiableEntityException`), so taking one out of an instance
//!    profile is refused with that modelled error (HTTP 400) — exactly as putting one in
//!    is (`tests/all/instance_profile_roles.rs`).
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over: the
//! alphabet and case of both names, the depth of both paths, HOW MANY profiles hold the
//! role at once, the casing the removal names the entities under, and what else holds
//! the role back. The awsQuery harness is the one `tests/all/user_lifecycle.rs` owns and
//! the other suites in this crate share.

use crate::role_lifecycle::{borrowed, optional, param};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The members of the `Role` shape a `GetRole` response reports
/// (`smithy_query(query="shape", shape="Role")`), which is what "the removal left the
/// role alone" is asserted over. The awsQuery envelope's per-request `RequestId` is the
/// one thing that legitimately differs between two responses, so the comparison is over
/// the reported role rather than the whole body.
const ROLE_MEMBERS: &[&str] = &[
    "RoleName",
    "RoleId",
    "Path",
    "Arn",
    "CreateDate",
    "AssumeRolePolicyDocument",
    "Description",
    "MaxSessionDuration",
];

/// A trust policy with a single statement, so `CreateRole` accepts the fixture roles
/// (the EC2 service principal is the one an instance profile's role carries).
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// An inline policy document: a JSON object carrying a `Statement`, which is what
/// `PutRolePolicy` requires of one.
const INLINE_POLICY: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:PutObject","Resource":"*"}]}"#;

/// An AWS-managed policy ARN, as this crate's `tests/all/role_deletion.rs` uses.
const MANAGED_POLICY_ARN: &str = "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess";

/// The casing a removal request spells the entity names in. IAM entity names "are not
/// distinguished by case" (`CreateUser$UserName`), so every one of these names the same
/// pair, and which one a row uses must not change the outcome.
#[derive(Debug, Clone, Copy)]
enum Casing {
    AsCreated,
    Lower,
    Upper,
}

impl Casing {
    fn apply(self, name: &str) -> String {
        match self {
            Self::AsCreated => name.to_string(),
            Self::Lower => name.to_ascii_lowercase(),
            Self::Upper => name.to_ascii_uppercase(),
        }
    }
}

/// One row: a role, the instance profiles that hold it, and what else holds it back.
struct RemovalCase {
    /// What the row exercises, named in every assertion.
    case: &'static str,
    /// The role name, spanning the `^[\w+=,.@-]+$` alphabet
    /// (`smithy_query(query="operation", operation="RemoveRoleFromInstanceProfile")`).
    role: &'static str,
    /// `Path` on the role, or `None` to leave it out.
    role_path: Option<&'static str>,
    /// The profiles the role is bound to, each with its `Path` or `None`: one row binds
    /// the role into several at once, so the rule is asserted over the COUNT of holders
    /// and not only over a single binding.
    profiles: &'static [(&'static str, Option<&'static str>)],
    /// The casing every removal and delete in this row names the entities under.
    casing: Casing,
    /// An inline policy put on the role, which must go on holding the role back after
    /// the binding is cleared.
    inline_policy: Option<&'static str>,
    /// Whether a managed policy is attached to the role, for the same reason.
    managed_policy: bool,
}

fn removal_cases() -> Vec<RemovalCase> {
    vec![
        RemovalCase {
            case: "one profile, nothing else holds the role, named as created",
            role: "asg-instance-role",
            role_path: None,
            profiles: &[("asg-instance-profile", None)],
            casing: Casing::AsCreated,
            inline_policy: None,
            managed_policy: false,
        },
        RemovalCase {
            case: "two profiles hold the role, an inline policy too, named lowercased",
            role: "Fleet_Role",
            role_path: Some("/eng/tools/"),
            profiles: &[
                ("Fleet_Profile", Some("/service-role/")),
                ("Fleet_Profile_Blue", None),
            ],
            casing: Casing::Lower,
            inline_policy: Some("writes-its-own-prefix"),
            managed_policy: false,
        },
        RemovalCase {
            case: "three profiles, punctuation across the name alphabet, named uppercased",
            role: "web+edge=1@x",
            role_path: Some("/eng/"),
            profiles: &[
                ("ops@3,x", Some("/eng/tools/deep/")),
                ("ops.b-2", None),
                ("ops_c=3", Some("/")),
            ],
            casing: Casing::Upper,
            inline_policy: None,
            managed_policy: true,
        },
        RemovalCase {
            case: "one profile, both an inline and a managed policy also hold the role",
            role: "batch.Role-2",
            role_path: Some("/"),
            profiles: &[("batch.profile-2", Some("/"))],
            casing: Casing::Lower,
            inline_policy: Some("batch.writes"),
            managed_policy: true,
        },
    ]
}

/// Seed one row — the role, its policies and every profile bound to it — answering the
/// router they live in.
async fn bound(case: &RemovalCase) -> smithy_server_runtime::MultiServiceRouter {
    let router = router();

    let mut params = vec![
        param("RoleName", case.role),
        param("AssumeRolePolicyDocument", TRUST_POLICY),
    ];
    if let Some(path) = case.role_path {
        params.push(param("Path", path));
    }
    let (status, body) = call(&router, "CreateRole", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreateRole must succeed, got {status}: {body}",
        case.case
    );

    if let Some(policy_name) = case.inline_policy {
        let (status, body) = call(
            &router,
            "PutRolePolicy",
            &[
                ("RoleName", case.role),
                ("PolicyName", policy_name),
                ("PolicyDocument", INLINE_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture PutRolePolicy must succeed, got {status}: {body}",
            case.case
        );
    }
    if case.managed_policy {
        let (status, body) = call(
            &router,
            "AttachRolePolicy",
            &[
                ("RoleName", case.role),
                ("PolicyArn", MANAGED_POLICY_ARN),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture AttachRolePolicy must succeed, got {status}: {body}",
            case.case
        );
    }

    for (profile, path) in case.profiles {
        let mut params = vec![param("InstanceProfileName", profile)];
        if let Some(path) = path {
            params.push(param("Path", path));
        }
        let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}/{profile}: the fixture CreateInstanceProfile must succeed, got \
             {status}: {body}",
            case.case
        );
        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[("InstanceProfileName", profile), ("RoleName", case.role)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}/{profile}: the fixture AddRoleToInstanceProfile must succeed, got \
             {status}: {body}",
            case.case
        );
    }

    router
}

// ---------------------------------------------------------------------------
// RULES 1 and 2: the removal clears the binding, and only the binding
// ---------------------------------------------------------------------------

#[tokio::test]
async fn removing_a_role_from_an_instance_profile_clears_the_binding_and_nothing_else() {
    for case in removal_cases() {
        let router = bound(&case).await;
        let role_as_named = case.casing.apply(case.role);
        let read_role = vec![param("RoleName", case.role)];

        // What the role is before any removal: a removal must not change ANY of it.
        let (status, role_before) = call(&router, "GetRole", &borrowed(&read_role)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture GetRole must succeed, got {status}: {role_before}",
            case.case
        );

        let holders = case.profiles.len();
        for (removed, (profile, _)) in case.profiles.iter().enumerate() {
            let profile_as_named = case.casing.apply(profile);
            let read_profile = vec![param("InstanceProfileName", profile)];

            // RULE 2, while the binding stands: the profile "must not have an
            // associated role", so deleting it is refused and nothing is released.
            let delete_profile = vec![param("InstanceProfileName", &profile_as_named)];
            let (status, body) =
                call(&router, "DeleteInstanceProfile", &borrowed(&delete_profile)).await;
            assert_eq!(
                status, 409,
                "case {:?}/{profile}: DeleteInstanceProfile must answer 409 while the profile \
                 holds a role, got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "DeleteConflict",
                "case {:?}/{profile}: expected the modelled awsQuery code, got {body}",
                case.case
            );
            let (status, body) = call(&router, "GetInstanceProfile", &borrowed(&read_profile)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{profile}: the refused delete must leave the profile, got {status}: \
                 {body}",
                case.case
            );
            assert_eq!(
                element(&body, "RoleName"),
                case.role,
                "case {:?}/{profile}: the refused delete must leave the binding, got {body}",
                case.case
            );

            // RULE 1: the removal answers 200 and the profile reports no role after it.
            let remove = vec![
                param("InstanceProfileName", &profile_as_named),
                param("RoleName", &role_as_named),
            ];
            let (status, body) =
                call(&router, "RemoveRoleFromInstanceProfile", &borrowed(&remove)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{profile}: RemoveRoleFromInstanceProfile must answer 200, got \
                 {status}: {body}",
                case.case
            );
            let (status, body) = call(&router, "GetInstanceProfile", &borrowed(&read_profile)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{profile}: the profile itself survives the removal, got {status}: \
                 {body}",
                case.case
            );
            assert!(
                elements(&body, "RoleName").is_empty(),
                "case {:?}/{profile}: the profile must report no role after the removal, got \
                 {body}",
                case.case
            );

            // The ROLE is untouched: every member it reported before the removal is the
            // member it reports after it.
            let (status, role_after) = call(&router, "GetRole", &borrowed(&read_role)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{profile}: the role must survive the removal, got {status}: \
                 {role_after}",
                case.case
            );
            for member in ROLE_MEMBERS {
                assert_eq!(
                    optional(&role_after, member),
                    optional(&role_before, member),
                    "case {:?}/{profile}: the removal must change nothing about the role's \
                     {member}, got {role_after}",
                    case.case
                );
            }

            // RULE 2, now that this profile holds nothing: the delete succeeds and the
            // profile stops resolving.
            let (status, body) =
                call(&router, "DeleteInstanceProfile", &borrowed(&delete_profile)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{profile}: DeleteInstanceProfile must answer 200 once the profile \
                 holds no role, got {status}: {body}",
                case.case
            );
            let (status, body) = call(&router, "GetInstanceProfile", &borrowed(&read_profile)).await;
            assert_eq!(
                status, 404,
                "case {:?}/{profile}: the deleted profile must stop resolving, got {status}: \
                 {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {:?}/{profile}: expected the modelled awsQuery code, got {body}",
                case.case
            );

            // While ANOTHER profile still holds the role, the role is still held back:
            // what releases it is every binding going, not the first one.
            if removed + 1 < holders {
                let (status, body) =
                    call(&router, "DeleteRole", &[("RoleName", role_as_named.as_str())]).await;
                assert_eq!(
                    status, 409,
                    "case {:?}: DeleteRole must still answer 409 while {} of {holders} profiles \
                     hold the role, got {status}: {body}",
                    case.case,
                    holders - removed - 1
                );
                assert_eq!(
                    error_code(&body),
                    "DeleteConflict",
                    "case {:?}: expected the modelled awsQuery code, got {body}",
                    case.case
                );
            }
        }

        // Every binding is gone. A POLICY that also holds the role back still does — the
        // removal cleared the binding and nothing else — so the delete is released only
        // once those are removed too.
        if case.inline_policy.is_some() || case.managed_policy {
            let (status, body) =
                call(&router, "DeleteRole", &[("RoleName", role_as_named.as_str())]).await;
            assert_eq!(
                status, 409,
                "case {:?}: a policy must go on holding the role back after the binding was \
                 cleared, got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "DeleteConflict",
                "case {:?}: expected the modelled awsQuery code, got {body}",
                case.case
            );

            if let Some(policy_name) = case.inline_policy {
                let (status, body) = call(
                    &router,
                    "DeleteRolePolicy",
                    &[("RoleName", case.role), ("PolicyName", policy_name)],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the fixture DeleteRolePolicy must succeed, got {status}: {body}",
                    case.case
                );
            }
            if case.managed_policy {
                let (status, body) = call(
                    &router,
                    "DetachRolePolicy",
                    &[
                        ("RoleName", case.role),
                        ("PolicyArn", MANAGED_POLICY_ARN),
                    ],
                )
                .await;
                assert_eq!(
                    status, 200,
                    "case {:?}: the fixture DetachRolePolicy must succeed, got {status}: {body}",
                    case.case
                );
            }
        }

        // The teardown the rule is about: a role nothing holds any more is deleted.
        let (status, body) =
            call(&router, "DeleteRole", &[("RoleName", role_as_named.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {:?}: DeleteRole must answer 200 once no profile and no policy hold the \
             role, got {status}: {body}",
            case.case
        );
        let (status, body) = call(&router, "GetRole", &borrowed(&read_role)).await;
        assert_eq!(
            status, 404,
            "case {:?}: the deleted role must stop resolving, got {status}: {body}",
            case.case
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {:?}: expected the modelled awsQuery code, got {body}",
            case.case
        );

        // And the whole lifecycle runs again under the same names, which is what a leaked
        // role would prevent.
        let mut params = vec![
            param("RoleName", case.role),
            param("AssumeRolePolicyDocument", TRUST_POLICY),
        ];
        if let Some(path) = case.role_path {
            params.push(param("Path", path));
        }
        let (status, body) = call(&router, "CreateRole", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the same names must be free after the teardown, got {status}: {body}",
            case.case
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: a binding the account does not hold is NoSuchEntity
// ---------------------------------------------------------------------------

/// Which part of the binding a refusal row leaves missing.
#[derive(Debug, Clone, Copy)]
enum Missing {
    /// No profile of that name.
    Profile,
    /// No role of that name.
    Role,
    /// Both entities exist, but this profile never held this role.
    Binding,
    /// The binding existed and was already removed, so the removal is not idempotent:
    /// the second call finds nothing to remove.
    AlreadyRemoved,
}

#[tokio::test]
async fn removing_a_binding_the_account_does_not_hold_is_no_such_entity() {
    for case in removal_cases() {
        let router = bound(&case).await;
        let (profile, _) = case.profiles[0];
        let role_as_named = case.casing.apply(case.role);
        let profile_as_named = case.casing.apply(profile);

        // A second role and a second profile that are NOT bound to each other, so the
        // "never bound" row names two entities the account really holds.
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", "role-bound-to-nothing"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture CreateRole must succeed, got {status}: {body}",
            case.case
        );

        for missing in [
            Missing::Profile,
            Missing::Role,
            Missing::Binding,
            Missing::AlreadyRemoved,
        ] {
            let (profile_param, role_param) = match missing {
                Missing::Profile => (
                    "profile-the-account-does-not-hold".to_string(),
                    role_as_named.clone(),
                ),
                Missing::Role => (
                    profile_as_named.clone(),
                    "role-the-account-does-not-hold".to_string(),
                ),
                Missing::Binding => (
                    profile_as_named.clone(),
                    "role-bound-to-nothing".to_string(),
                ),
                Missing::AlreadyRemoved => {
                    let remove = vec![
                        param("InstanceProfileName", &profile_as_named),
                        param("RoleName", &role_as_named),
                    ];
                    let (status, body) =
                        call(&router, "RemoveRoleFromInstanceProfile", &borrowed(&remove)).await;
                    assert_eq!(
                        status, 200,
                        "case {:?}/{missing:?}: the first removal must succeed, got {status}: \
                         {body}",
                        case.case
                    );
                    (profile_as_named.clone(), role_as_named.clone())
                }
            };

            let remove = vec![
                param("InstanceProfileName", &profile_param),
                param("RoleName", &role_param),
            ];
            let (status, body) =
                call(&router, "RemoveRoleFromInstanceProfile", &borrowed(&remove)).await;
            assert_eq!(
                status, 404,
                "case {:?}/{missing:?}: expected 404 for a binding the account does not hold, \
                 got {status}: {body}",
                case.case
            );
            assert_eq!(
                error_code(&body),
                "NoSuchEntity",
                "case {:?}/{missing:?}: expected the modelled awsQuery code, got {body}",
                case.case
            );
        }

        // A refused removal released nothing that was there: the role the account holds
        // still reads back, and so does the second profile's binding when the row has
        // one.
        let read_role = vec![param("RoleName", case.role)];
        let (status, body) = call(&router, "GetRole", &borrowed(&read_role)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the role must survive the refused removals, got {status}: {body}",
            case.case
        );
        if let Some((other, _)) = case.profiles.get(1) {
            let read_profile = vec![param("InstanceProfileName", other)];
            let (status, body) = call(&router, "GetInstanceProfile", &borrowed(&read_profile)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{other}: the other profile must survive, got {status}: {body}",
                case.case
            );
            assert_eq!(
                element(&body, "RoleName"),
                case.role,
                "case {:?}/{other}: removing one binding must leave the others, got {body}",
                case.case
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE 4: a service-linked role is not modified on the caller's behalf
// ---------------------------------------------------------------------------

/// The service principals whose service-linked role this suite tries to remove: three
/// different owners, so the refusal is asserted on the class and not on one role. The
/// same principals `tests/all/instance_profile_roles.rs` asserts the bind refusal over.
const PRINCIPALS: &[&str] = &[
    "autoscaling.amazonaws.com",
    "elasticbeanstalk.amazonaws.com",
    "networkmonitor.amazonaws.com",
];

#[tokio::test]
async fn a_service_linked_role_cannot_be_removed_from_an_instance_profile() {
    let router = router();

    for (index, principal) in PRINCIPALS.iter().enumerate() {
        let profile_name = format!("profile-for-slr-removal-{index}");
        let created = vec![param("InstanceProfileName", profile_name.as_str())];
        let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&created)).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the fixture CreateInstanceProfile must succeed, got \
             {status}: {body}"
        );

        let (status, created_role) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the fixture CreateServiceLinkedRole must succeed, got \
             {status}: {created_role}"
        );
        let role_name = element(&created_role, "RoleName");

        let (status, body) = call(
            &router,
            "RemoveRoleFromInstanceProfile",
            &[
                ("InstanceProfileName", profile_name.as_str()),
                ("RoleName", role_name.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {principal:?}: expected 400 removing a service-linked role, got {status}: \
             {body}"
        );
        assert_eq!(
            error_code(&body),
            "UnmodifiableEntity",
            "case {principal:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refusal changed nothing: the profile still holds no role, and the
        // service-linked role still reads back.
        let (status, read_body) = call(&router, "GetInstanceProfile", &borrowed(&created)).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the profile must survive the refusal, got {status}: {read_body}"
        );
        assert!(
            elements(&read_body, "RoleName").is_empty(),
            "case {principal:?}: the profile must still hold no role, got {read_body}"
        );
        let (status, read_body) = call(&router, "GetRole", &[("RoleName", role_name.as_str())]).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: the service-linked role must survive the refusal, got \
             {status}: {read_body}"
        );
    }
}
