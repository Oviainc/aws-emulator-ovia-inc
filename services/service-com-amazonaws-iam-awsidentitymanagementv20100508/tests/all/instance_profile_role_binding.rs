// SPDX-License-Identifier: CC0-1.0

//! What `AddRoleToInstanceProfile` writes is a BINDING, end to end over the awsQuery wire.
//!
//! The rule under test: binding a role to an instance profile records which role the
//! profile holds, not a copy of that role, so
//!
//! 1. every later read of the profile reports the role as the account holds it AT THE
//!    TIME OF THE READ — `InstanceProfile$Roles` is "the role associated with the
//!    instance profile", and the role itself lives in one place, which `GetRole` reports
//!    from too; and
//! 2. the bound role is a subordinate of the profile, so it cannot be deleted while the
//!    binding holds: "before you delete a role, remove the following attached items:
//!    inline policies (DeleteRolePolicy), attached managed policies (DetachRolePolicy),
//!    instance profile (RemoveRoleFromInstanceProfile)" (`DeleteRole`'s documentation),
//!    and a delete that still has one is the modelled `DeleteConflict` (HTTP 409).
//!
//! Each suite is a TABLE whose rows differ along what the rule generalises over — which
//! optional members the role carries, its path's depth, the alphabet and case of both
//! names, the casing the binding was written under, how many tags the role gains after
//! it was bound, and what else holds the role back — so no single request is the
//! specification. The profiles here carry NO tags of their own, which is what lets a
//! `Tags` list in a `GetInstanceProfile` response be read as the bound role's.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the
//! HTTP behaviour a client on the socket sees. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.

use std::collections::BTreeMap;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::user_lifecycle::{call, element, error_code, router};

/// A trust policy with a single statement, so `CreateRole` accepts the fixture roles (the
/// EC2 service principal is the one an instance profile's role carries).
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// An inline policy document: a JSON object carrying a `Statement`, which is what
/// `PutRolePolicy` requires of one.
const INLINE_POLICY: &str =
    r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:PutObject","Resource":"*"}]}"#;

/// One row: a role, a profile, and the binding between them.
struct BindingCase {
    /// What the row exercises, named in every assertion.
    case: &'static str,
    /// The role name, spanning the `^[\w+=,.@-]+$` alphabet
    /// (`smithy_query(query="operation", operation="CreateRole")`).
    role: &'static str,
    /// `Path` on the role, or `None` to leave it out.
    role_path: Option<&'static str>,
    /// `Description` on the role, or `None` to leave it out.
    description: Option<&'static str>,
    /// `MaxSessionDuration`, or `None` for the documented one-hour default.
    /// `roleMaxSessionDurationType`'s range is 3600..43200
    /// (`smithy_query(query="shape", shape="Role", member="MaxSessionDuration")`), so the
    /// rows span its floor, a value between and its ceiling.
    max_session_duration: Option<&'static str>,
    /// The tags the role is created with.
    create_tags: &'static [(&'static str, &'static str)],
    /// The tags a `TagRole` applies AFTER the role was bound to the profile: this is the
    /// change a stale copy of the role would hide.
    later_tags: &'static [(&'static str, &'static str)],
    /// The instance profile name.
    profile: &'static str,
    /// `Path` on the profile, or `None` to leave it out.
    profile_path: Option<&'static str>,
    /// The casing of the role name the binding is written under: IAM entity names "are
    /// not distinguished by case", so any of them names the same role.
    bind_role_as: &'static str,
}

/// The table both suites are written against.
fn binding_cases() -> Vec<BindingCase> {
    vec![
        BindingCase {
            case: "no optional member, bound under the created casing",
            role: "asg-instance-role",
            role_path: None,
            description: None,
            max_session_duration: None,
            create_tags: &[],
            later_tags: &[("env", "prod")],
            profile: "asg-instance-profile",
            profile_path: None,
            bind_role_as: "asg-instance-role",
        },
        BindingCase {
            case: "nested paths, every optional member, bound lowercased",
            role: "Fleet_Role",
            role_path: Some("/eng/tools/"),
            description: Some("the fleet's boot role"),
            max_session_duration: Some("7200"),
            create_tags: &[("team", "platform")],
            later_tags: &[("team", "platform"), ("env", "dev"), ("owner", "ops")],
            profile: "Fleet_Profile",
            profile_path: Some("/service-role/"),
            bind_role_as: "fleet_role",
        },
        BindingCase {
            case: "punctuation across the whole name alphabet, bound uppercased",
            role: "web+edge=1@x",
            role_path: Some("/eng/"),
            description: Some("edge role"),
            max_session_duration: Some("43200"),
            create_tags: &[("a", "1"), ("b", "2"), ("c", "3")],
            later_tags: &[("a", "1"), ("b", "two"), ("c", "3"), ("d", "4")],
            profile: "ops@3,x",
            profile_path: Some("/eng/tools/deep/"),
            bind_role_as: "WEB+EDGE=1@X",
        },
        BindingCase {
            case: "session duration at its floor, tags dropped to none",
            role: "batch.role-2",
            role_path: Some("/"),
            description: None,
            max_session_duration: Some("3600"),
            create_tags: &[("stage", "canary")],
            later_tags: &[("stage", "ga")],
            profile: "batch.profile-2",
            profile_path: Some("/"),
            bind_role_as: "batch.Role-2",
        },
    ]
}

impl BindingCase {
    fn create_role_params(&self) -> Vec<(String, String)> {
        let mut params = vec![
            param("RoleName", self.role),
            param("AssumeRolePolicyDocument", TRUST_POLICY),
        ];
        if let Some(path) = self.role_path {
            params.push(param("Path", path));
        }
        if let Some(description) = self.description {
            params.push(param("Description", description));
        }
        if let Some(duration) = self.max_session_duration {
            params.push(param("MaxSessionDuration", duration));
        }
        params.extend(tag_params(self.create_tags));
        params
    }

    fn create_profile_params(&self) -> Vec<(String, String)> {
        let mut params = vec![param("InstanceProfileName", self.profile)];
        if let Some(path) = self.profile_path {
            params.push(param("Path", path));
        }
        params
    }

    fn bind_params(&self) -> Vec<(String, String)> {
        vec![
            param("InstanceProfileName", self.profile),
            param("RoleName", self.bind_role_as),
        ]
    }

    fn role_params(&self) -> Vec<(String, String)> {
        vec![param("RoleName", self.role)]
    }

    fn profile_params(&self) -> Vec<(String, String)> {
        vec![param("InstanceProfileName", self.profile)]
    }
}

/// `Tags.member.N.Key`/`.Value` for an awsQuery tag list.
fn tag_params(tags: &[(&str, &str)]) -> Vec<(String, String)> {
    let mut params = Vec::new();
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        params.push(param(&format!("Tags.member.{position}.Key"), key));
        params.push(param(&format!("Tags.member.{position}.Value"), value));
    }
    params
}

fn expected_tags(tags: &[(&str, &str)]) -> BTreeMap<String, String> {
    tags.iter()
        .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
        .collect()
}

/// The members of the bound role that a `GetInstanceProfile` response carries exactly
/// once, so they can be read out of it as the role's: the profile shape declares none of
/// them (`smithy_query(query="shape", shape="InstanceProfile")` lists `Path`,
/// `InstanceProfileName`, `InstanceProfileId`, `Arn`, `CreateDate`, `Roles`, `Tags`).
const ROLE_ONLY_MEMBERS: &[&str] = &[
    "RoleName",
    "RoleId",
    "Description",
    "MaxSessionDuration",
    "AssumeRolePolicyDocument",
];

/// Seed one row's role and profile and bind them, answering the router they live in.
async fn bound(case: &BindingCase) -> smithy_server_runtime::MultiServiceRouter {
    let router = router();
    let params = case.create_role_params();
    let (status, body) = call(&router, "CreateRole", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreateRole must succeed, got {status}: {body}",
        case.case
    );
    let params = case.create_profile_params();
    let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: the fixture CreateInstanceProfile must succeed, got {status}: {body}",
        case.case
    );
    let params = case.bind_params();
    let (status, body) = call(&router, "AddRoleToInstanceProfile", &borrowed(&params)).await;
    assert_eq!(
        status, 200,
        "case {:?}: AddRoleToInstanceProfile must answer 200, got {status}: {body}",
        case.case
    );
    router
}

// ---------------------------------------------------------------------------
// The binding names the role; the report resolves it
// ---------------------------------------------------------------------------

/// RULE 1: a profile reports the bound role as the account holds it when the profile is
/// READ — identical to what `GetRole` reports, before and after a later change to that
/// role.
#[tokio::test]
async fn an_instance_profile_reports_the_bound_role_as_the_account_holds_it() {
    for case in binding_cases() {
        let router = bound(&case).await;
        let profile_params = case.profile_params();
        let role_params = case.role_params();

        // Right after the binding: every role member the profile reports is the one the
        // role's own reader reports.
        let (status, profile) =
            call(&router, "GetInstanceProfile", &borrowed(&profile_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetInstanceProfile must answer 200, got {status}: {profile}",
            case.case
        );
        let (status, role) = call(&router, "GetRole", &borrowed(&role_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetRole must answer 200, got {status}: {role}",
            case.case
        );
        for member in ROLE_ONLY_MEMBERS {
            let reported = optional(&profile, member);
            assert_eq!(
                reported,
                optional(&role, member),
                "case {:?}: the profile must report the role's {member}, got {reported:?} in \
                 {profile}",
                case.case
            );
        }
        assert_eq!(
            element(&profile, "RoleName"),
            case.role,
            "case {:?}: the bound role is reported under the case it was CREATED with, \
             whatever casing the binding named it in, got {profile}",
            case.case
        );
        assert_eq!(
            reported_tags(&profile),
            expected_tags(case.create_tags),
            "case {:?}: the profile carries no tags of its own, so it must report the bound \
             role's, got {profile}",
            case.case
        );

        // Now the ROLE changes. The profile holds a binding, not a copy, so the next read
        // of the profile reports the changed role.
        let mut tag_role = vec![param("RoleName", case.role)];
        tag_role.extend(tag_params(case.later_tags));
        let (status, body) = call(&router, "TagRole", &borrowed(&tag_role)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture TagRole must succeed, got {status}: {body}",
            case.case
        );

        let (status, profile) =
            call(&router, "GetInstanceProfile", &borrowed(&profile_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: GetInstanceProfile must still answer 200, got {status}: {profile}",
            case.case
        );
        assert_eq!(
            reported_tags(&profile),
            expected_tags(case.later_tags),
            "case {:?}: the profile must report the role's tags as they stand NOW, got \
             {profile}",
            case.case
        );
        let (_, role) = call(&router, "GetRole", &borrowed(&role_params)).await;
        for member in ROLE_ONLY_MEMBERS {
            let reported = optional(&profile, member);
            assert_eq!(
                reported,
                optional(&role, member),
                "case {:?}: after the change the profile must still report the role's \
                 {member}, got {reported:?} in {profile}",
                case.case
            );
        }

        // Reading the profile is a READ: a second read reports the same thing.
        let (_, again) = call(&router, "GetInstanceProfile", &borrowed(&profile_params)).await;
        assert_eq!(
            reported_tags(&again),
            reported_tags(&profile),
            "case {:?}: two reads of the profile must report the same role, got {again}",
            case.case
        );
        assert_eq!(
            element(&again, "RoleId"),
            element(&profile, "RoleId"),
            "case {:?}: two reads of the profile must report the same role, got {again}",
            case.case
        );
    }
}

// ---------------------------------------------------------------------------
// The binding holds the role back from deletion
// ---------------------------------------------------------------------------

/// RULE 2: while an instance profile holds a role, deleting that role is `DeleteConflict`
/// (HTTP 409) and nothing is released — whatever casing the delete names it in, and
/// whether or not a policy holds it back as well.
#[tokio::test]
async fn a_role_bound_to_an_instance_profile_is_not_deleted() {
    for case in binding_cases() {
        let router = bound(&case).await;

        // Rows within the row: the binding alone, and the binding beside an inline policy
        // and an attached managed one — each kind holds the role back on its own, so the
        // refusal must not depend on which of them is present.
        //
        // An AWS-managed policy ARN, as this crate's `tests/all/role_deletion.rs` uses.
        for extra in ["nothing else", "an inline policy", "a managed policy"] {
            match extra {
                "an inline policy" => {
                    let (status, body) = call(
                        &router,
                        "PutRolePolicy",
                        &[
                            ("RoleName", case.role),
                            ("PolicyName", "writes-its-own-prefix"),
                            ("PolicyDocument", INLINE_POLICY),
                        ],
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {:?}/{extra}: the fixture PutRolePolicy must succeed, got \
                         {status}: {body}",
                        case.case
                    );
                }
                "a managed policy" => {
                    let (status, body) = call(
                        &router,
                        "AttachRolePolicy",
                        &[
                            ("RoleName", case.role),
                            (
                                "PolicyArn",
                                "arn:aws:iam::aws:policy/AmazonS3ReadOnlyAccess",
                            ),
                        ],
                    )
                    .await;
                    assert_eq!(
                        status, 200,
                        "case {:?}/{extra}: the fixture AttachRolePolicy must succeed, got \
                         {status}: {body}",
                        case.case
                    );
                }
                _ => {}
            }

            // The delete names the role under the casing the BINDING used, which is not
            // the casing the role was created with in every row.
            for named_as in [case.role, case.bind_role_as] {
                let (status, body) = call(&router, "DeleteRole", &[("RoleName", named_as)]).await;
                assert_eq!(
                    status, 409,
                    "case {:?}/{extra}: DeleteRole {named_as:?} must answer 409 while a \
                     profile holds the role, got {status}: {body}",
                    case.case
                );
                assert_eq!(
                    error_code(&body),
                    "DeleteConflict",
                    "case {:?}/{extra}: expected the modelled awsQuery code, got {body}",
                    case.case
                );
            }

            // Nothing was released: the role still reads back, and the profile still
            // reports it.
            let role_params = case.role_params();
            let (status, body) = call(&router, "GetRole", &borrowed(&role_params)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{extra}: the role must survive the refused delete, got {status}: \
                 {body}",
                case.case
            );
            let profile_params = case.profile_params();
            let (status, body) =
                call(&router, "GetInstanceProfile", &borrowed(&profile_params)).await;
            assert_eq!(
                status, 200,
                "case {:?}/{extra}: the profile must survive the refused delete, got \
                 {status}: {body}",
                case.case
            );
            assert_eq!(
                element(&body, "RoleName"),
                case.role,
                "case {:?}/{extra}: the profile must still report the bound role, got {body}",
                case.case
            );
        }

        // A role NO profile holds, in the same account, is released as usual: it is the
        // BINDING that blocks the delete, not the account's state in general.
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", "unbound-role"),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: the unbound fixture create must succeed, got {status}: {body}",
            case.case
        );
        let (status, body) = call(&router, "DeleteRole", &[("RoleName", "unbound-role")]).await;
        assert_eq!(
            status, 200,
            "case {:?}: a role no profile holds must be deleted, got {status}: {body}",
            case.case
        );
    }
}
