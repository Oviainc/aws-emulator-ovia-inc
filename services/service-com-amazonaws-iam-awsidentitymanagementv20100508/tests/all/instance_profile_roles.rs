// SPDX-License-Identifier: CC0-1.0

//! The one `AddRoleToInstanceProfile` refusal no other suite in this crate covers:
//! a SERVICE-LINKED role cannot be bound into an instance profile.
//!
//! The rest of the operation's surface — the successful bind read back through
//! `GetInstanceProfile`, the case-folded re-add, the one-role quota and the absent
//! entities — is covered by `tests/all/instance_profiles.rs` and
//! `tests/all/instance_profile_role_binding.rs`, so this file asserts only what
//! they do not and reuses the awsQuery harness `tests/all/user_lifecycle.rs` owns.
//!
//! RULE: a service-linked role is "a protected Amazon Web Services resource. Only
//! the service that depends on the service-linked role can modify or delete the role
//! on your behalf" (`UnmodifiableEntityException`), so binding one into an instance
//! profile — a modification of that role on the caller's behalf — is refused with
//! that modelled error for EVERY service that owns such a role, and the profile is
//! left holding nothing.

use crate::role_lifecycle::{borrowed, param};
use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The service principals whose service-linked role this suite tries to bind: three
/// different owners, so the refusal is asserted on the class and not on one role.
const PRINCIPALS: &[&str] = &[
    "autoscaling.amazonaws.com",
    "elasticbeanstalk.amazonaws.com",
    "networkmonitor.amazonaws.com",
];

#[tokio::test]
async fn a_service_linked_role_cannot_be_bound_into_an_instance_profile() {
    let router = router();

    for (index, principal) in PRINCIPALS.iter().enumerate() {
        // One profile per row, so the "nothing was bound" assertion is about this
        // row's refusal rather than about a profile a previous row left alone.
        let profile_name = format!("profile-for-slr-{index}");
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
            "AddRoleToInstanceProfile",
            &[
                ("InstanceProfileName", profile_name.as_str()),
                ("RoleName", role_name.as_str()),
            ],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {principal:?}: expected 400 binding a service-linked role, got \
             {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "UnmodifiableEntity",
            "case {principal:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refusal bound nothing: the profile still reports no role.
        let read = vec![param("InstanceProfileName", profile_name.as_str())];
        let (status, read_body) = call(&router, "GetInstanceProfile", &borrowed(&read)).await;
        assert_eq!(
            status, 200,
            "case {principal:?}: GetInstanceProfile must report the profile, got \
             {status}: {read_body}"
        );
        assert!(
            elements(&read_body, "RoleName").is_empty(),
            "case {principal:?}: a refused bind must leave the profile without a role, \
             got {read_body}"
        );
    }
}
