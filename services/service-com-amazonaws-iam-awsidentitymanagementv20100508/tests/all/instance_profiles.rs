// SPDX-License-Identifier: CC0-1.0

//! `CreateInstanceProfile`, its reporting half `GetInstanceProfile`, and
//! `AddRoleToInstanceProfile`, end to end over the awsQuery wire.
//!
//! Every assertion drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is under test is the
//! HTTP behaviour (status, awsQuery error code, XML members) of an instance profile that
//! was PERSISTED, never a handler called directly. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share.
//!
//! The rule under test is that a create PERSISTS the profile and the read REPORTS what was
//! persisted: the name and path the request chose, a minted id and an ARN derived from
//! them, an empty role list, and the tags the request carried; and that
//! `AddRoleToInstanceProfile` binds the ONE role a profile may hold onto that same record.
//! Each suite is a TABLE whose rows differ along what that rule generalises over — the
//! path's depth, which optional members are present, the tag count, the alphabet and case
//! of the name, how many roles the profile already holds — so no single request is the
//! specification.
//!
//! `tests/all/instance_profile_roles.rs` covers the one refusal no suite here does:
//! a service-linked role cannot be bound into an instance profile.

use std::collections::BTreeMap;

use crate::role_lifecycle::{borrowed, optional, param, reported_tags};
use crate::user_lifecycle::{account_id, call, element, elements, error_code, router};

/// A trust policy with a single statement, so `CreateRole` accepts the fixture roles the
/// role-binding suites attach (the EC2 service principal is the one an instance profile's
/// role carries).
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// One `CreateInstanceProfile` request, with the members the row exercises.
struct CreateCase {
    /// The profile name, spanning the `^[\w+=,.@-]+$` alphabet
    /// (`smithy_query(query="operation", operation="CreateInstanceProfile")`).
    name: &'static str,
    /// `Path`, or `None` to leave it out.
    path: Option<&'static str>,
    /// The path the profile must REPORT: the request's, or `/` when it named none.
    expected_path: &'static str,
    /// The tags the request carries.
    tags: Vec<(&'static str, &'static str)>,
}

impl CreateCase {
    fn params(&self) -> Vec<(String, String)> {
        let mut params = vec![param("InstanceProfileName", self.name)];
        if let Some(path) = self.path {
            params.push(param("Path", path));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            let position = index + 1;
            params.push(param(&format!("Tags.member.{position}.Key"), key));
            params.push(param(&format!("Tags.member.{position}.Value"), value));
        }
        params
    }

    fn expected_tags(&self) -> BTreeMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect()
    }
}

/// The table every suite here is written against: paths at three depths, tag counts from
/// none to several, and names spanning the whole accepted alphabet.
fn create_cases() -> Vec<CreateCase> {
    vec![
        CreateCase {
            name: "asg-instance-profile",
            path: None,
            expected_path: "/",
            tags: Vec::new(),
        },
        CreateCase {
            name: "Fleet_Profile",
            path: Some("/"),
            expected_path: "/",
            tags: vec![("env", "dev")],
        },
        CreateCase {
            name: "batch.profile-2",
            path: Some("/service-role/"),
            expected_path: "/service-role/",
            tags: vec![("env", "prod"), ("team", "platform")],
        },
        CreateCase {
            name: "web+edge=1",
            path: Some("/eng/tools/"),
            expected_path: "/eng/tools/",
            tags: vec![("a", "1"), ("b", "2"), ("c", "3")],
        },
        CreateCase {
            name: "ops@3,x",
            path: Some("/eng/"),
            expected_path: "/eng/",
            tags: Vec::new(),
        },
    ]
}

/// An instance profile ARN is `arn:aws:iam::<account>:instance-profile<path><name>` — IAM
/// is global, so the region segment is EMPTY.
fn expected_arn(path: &str, name: &str) -> String {
    format!(
        "arn:aws:iam::{}:instance-profile{path}{name}",
        account_id()
    )
}

/// `InstanceProfile$InstanceProfileId` targets `idType` (length 16..128, pattern
/// `^[\w]+$`) — the modelled constraints on the unique id the service MINTS.
fn assert_well_formed_id(case: &str, id: &str) {
    assert!(
        id.len() >= 16 && id.len() <= 128,
        "case {case:?}: InstanceProfileId must be 16..128 characters, got {id:?}"
    );
    assert!(
        id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'),
        "case {case:?}: InstanceProfileId must match ^[\\w]+$, got {id:?}"
    );
}

// ---------------------------------------------------------------------------
// Create, then read back
// ---------------------------------------------------------------------------

/// Every row creates a profile that `GetInstanceProfile` then reports identically: the
/// name and path the request chose, the minted id, the derived ARN, no role, and the tags
/// the request carried.
#[tokio::test]
async fn a_created_instance_profile_round_trips_through_get_instance_profile() {
    let router = router();
    let mut minted_ids: Vec<String> = Vec::new();

    for case in create_cases() {
        let params = case.params();
        let (status, created) =
            call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200, got {status} with body {created}",
            case.name
        );

        let id = element(&created, "InstanceProfileId");
        assert_well_formed_id(case.name, &id);
        assert!(
            !minted_ids.contains(&id),
            "case {:?}: InstanceProfileId {id:?} was already minted for another profile",
            case.name
        );
        minted_ids.push(id.clone());

        let expected_members = [
            ("InstanceProfileName", case.name.to_string()),
            ("Path", case.expected_path.to_string()),
            ("Arn", expected_arn(case.expected_path, case.name)),
        ];
        for (member, expected) in &expected_members {
            let actual = element(&created, member);
            assert_eq!(
                &actual, expected,
                "case {:?}: expected <{member}>{expected}</{member}>, got {actual:?}",
                case.name
            );
        }
        // "The role associated with the instance profile" (`InstanceProfile$Roles`): a
        // freshly created profile has none, so no role is reported.
        assert!(
            elements(&created, "RoleName").is_empty(),
            "case {:?}: a new instance profile carries no role, got {created}",
            case.name
        );
        assert_eq!(
            reported_tags(&created),
            case.expected_tags(),
            "case {:?}: the create must report the tags it was given, got {created}",
            case.name
        );

        // The read reports the SAME record: every member, including the minted id.
        let read_params = vec![param("InstanceProfileName", case.name)];
        let (status, read) =
            call(&router, "GetInstanceProfile", &borrowed(&read_params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 from GetInstanceProfile, got {status} with body \
             {read}",
            case.name
        );
        assert_eq!(
            element(&read, "InstanceProfileId"),
            id,
            "case {:?}: the read must report the id the create minted, got {read}",
            case.name
        );
        for (member, expected) in &expected_members {
            let actual = element(&read, member);
            assert_eq!(
                &actual, expected,
                "case {:?}: the read must report <{member}>{expected}</{member}>, got \
                 {actual:?}",
                case.name
            );
        }
        assert_eq!(
            reported_tags(&read),
            case.expected_tags(),
            "case {:?}: the read must report the stored tags, got {read}",
            case.name
        );
        // `CreateDate` is minted once and reported unchanged by every later read.
        assert_eq!(
            element(&read, "CreateDate"),
            element(&created, "CreateDate"),
            "case {:?}: CreateDate must be stable across reads",
            case.name
        );
    }
}

/// Reading is a READ: two `GetInstanceProfile` calls report the same record, and the
/// second one is not affected by the first.
#[tokio::test]
async fn reading_an_instance_profile_changes_nothing_a_later_caller_sees() {
    let router = router();
    let params = vec![param("InstanceProfileName", "stable-profile")];
    let (status, _) = call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
    assert_eq!(status, 200, "the fixture profile must be created");

    let (_, first) = call(&router, "GetInstanceProfile", &borrowed(&params)).await;
    let (_, second) = call(&router, "GetInstanceProfile", &borrowed(&params)).await;
    for member in ["InstanceProfileId", "Arn", "Path", "CreateDate"] {
        assert_eq!(
            element(&first, member),
            element(&second, member),
            "member {member}: two reads must report the same value"
        );
    }
}

// ---------------------------------------------------------------------------
// Names are unique within the account, and not distinguished by case
// ---------------------------------------------------------------------------

/// A second create under any casing of a held name is `EntityAlreadyExists` (HTTP 409),
/// because IAM entity names "are not distinguished by case".
#[tokio::test]
async fn a_second_create_under_any_casing_of_a_held_name_is_refused() {
    let router = router();
    let created = vec![param("InstanceProfileName", "Shared-Profile")];
    let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&created)).await;
    assert_eq!(status, 200, "the first create must succeed, got {body}");

    // Rows: the same casing, all lower, all upper, and a mixed casing.
    for name in ["Shared-Profile", "shared-profile", "SHARED-PROFILE", "sHaReD-pRoFiLe"] {
        let params = vec![param("InstanceProfileName", name)];
        let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 409,
            "case {name:?}: expected HTTP 409, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "EntityAlreadyExists",
            "case {name:?}: expected EntityAlreadyExists, got {body}"
        );
    }

    // A different name is still accepted, so the refusal is about the name and not about
    // the account already holding a profile.
    let other = vec![param("InstanceProfileName", "another-profile")];
    let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&other)).await;
    assert_eq!(
        status, 200,
        "a distinct name must still be accepted, got {status} with body {body}"
    );
}

/// The read resolves a name under any casing too — the same identity rule the create
/// enforces uniqueness under.
#[tokio::test]
async fn the_read_resolves_a_name_under_any_casing() {
    let router = router();
    let created = vec![param("InstanceProfileName", "Mixed_Case_Profile")];
    let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&created)).await;
    assert_eq!(status, 200, "the fixture profile must be created, got {body}");
    let arn = element(&body, "Arn");

    for name in [
        "Mixed_Case_Profile",
        "mixed_case_profile",
        "MIXED_CASE_PROFILE",
    ] {
        let params = vec![param("InstanceProfileName", name)];
        let (status, read) = call(&router, "GetInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200, got {status} with body {read}"
        );
        // The ARN keeps the case the CREATOR chose, whatever casing the read asked under.
        assert_eq!(
            element(&read, "Arn"),
            arn,
            "case {name:?}: the read must report the stored ARN, got {read}"
        );
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A name the account holds no profile for is `NoSuchEntity` (HTTP 404), whatever shape
/// the name takes — never an empty success.
#[tokio::test]
async fn reading_what_was_never_created_is_no_such_entity() {
    let router = router();
    let seed = vec![param("InstanceProfileName", "held-profile")];
    let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&seed)).await;
    assert_eq!(status, 200, "the fixture profile must be created, got {body}");

    // Rows: a name nothing holds, a near-miss of the held one, a name in the alphabet's
    // punctuation, and a long name.
    let long_name = "p".repeat(128);
    let cases = [
        "never-created",
        "held-profile-2",
        "held@profile",
        long_name.as_str(),
    ];
    for name in cases {
        let params = vec![param("InstanceProfileName", name)];
        let (status, body) = call(&router, "GetInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 404,
            "case {name:?}: expected HTTP 404, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {name:?}: expected NoSuchEntity, got {body}"
        );
        assert!(
            optional(&body, "InstanceProfileId").is_none(),
            "case {name:?}: a refused read reports no profile, got {body}"
        );
    }
}

#[tokio::test]
async fn add_role_to_instance_profile_puts_the_single_role_on_the_profile() {
    let router = router();
    // Two roles, so the one-role quota can be taken on a second, different role.
    for role in ["EMR_EC2_DefaultRole", "EMR_EC2_OtherRole"] {
        let (status, body) = call(
            &router,
            "CreateRole",
            &[
                ("RoleName", role),
                ("AssumeRolePolicyDocument", TRUST_POLICY),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {role:?}: the fixture CreateRole must succeed, got {status}: {body}"
        );
    }

    for case in create_cases() {
        let params = case.params();
        let (status, body) =
            call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 200,
            "case {:?}: the fixture create must succeed, got {status}: {body}",
            case.name
        );

        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[
                ("InstanceProfileName", case.name),
                ("RoleName", "EMR_EC2_DefaultRole"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: AddRoleToInstanceProfile must answer 200, got {status}: {body}",
            case.name
        );

        // The association is visible through the profile's own reader, which is what
        // makes it persisted rather than reported back from the request.
        let (status, body) = call(
            &router,
            "GetInstanceProfile",
            &[("InstanceProfileName", case.name)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: GetInstanceProfile must answer 200, got {status}: {body}",
            case.name
        );
        let roles = elements(&body, "RoleName");
        assert_eq!(
            roles,
            vec!["EMR_EC2_DefaultRole".to_string()],
            "case {:?}: the profile must report the one role it holds, got {roles:?}",
            case.name
        );

        // "An instance profile can contain only one role, and this quota cannot be
        // increased." A repeat of the SAME role is a conflict; a DIFFERENT role is
        // past the quota. Either way the profile still holds exactly the first role.
        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[
                ("InstanceProfileName", case.name),
                ("RoleName", "emr_ec2_defaultrole"),
            ],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: re-adding the held role must answer 409, got {status}: {body}",
            case.name
        );
        let code = error_code(&body);
        assert_eq!(
            code, "EntityAlreadyExists",
            "case {:?}: expected EntityAlreadyExists, got {code:?}: {body}",
            case.name
        );

        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[
                ("InstanceProfileName", case.name),
                ("RoleName", "EMR_EC2_OtherRole"),
            ],
        )
        .await;
        assert_eq!(
            status, 409,
            "case {:?}: a second role must answer 409, got {status}: {body}",
            case.name
        );
        let code = error_code(&body);
        assert_eq!(
            code, "LimitExceeded",
            "case {:?}: expected LimitExceeded, got {code:?}: {body}",
            case.name
        );

        let (_, body) = call(
            &router,
            "GetInstanceProfile",
            &[("InstanceProfileName", case.name)],
        )
        .await;
        let roles = elements(&body, "RoleName");
        assert_eq!(
            roles,
            vec!["EMR_EC2_DefaultRole".to_string()],
            "case {:?}: a refused add must leave the profile's role alone, got {roles:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn add_role_to_instance_profile_resolves_both_entities() {
    let router = router();
    let (status, body) = call(
        &router,
        "CreateRole",
        &[
            ("RoleName", "EMR_EC2_DefaultRole"),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
        ],
    )
    .await;
    assert_eq!(status, 200, "the fixture CreateRole must succeed: {body}");
    let cases = create_cases();
    let first = &cases[0];
    let params = first.params();
    let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
    assert_eq!(status, 200, "the fixture create must succeed: {body}");

    // Either named entity may be the absent one, and either way nothing is created
    // to satisfy the call.
    for (profile, role) in [
        ("no-such-profile", "EMR_EC2_DefaultRole"),
        (first.name, "NoSuchRole"),
        ("no-such-profile", "NoSuchRole"),
    ] {
        let (status, body) = call(
            &router,
            "AddRoleToInstanceProfile",
            &[("InstanceProfileName", profile), ("RoleName", role)],
        )
        .await;
        assert_eq!(
            status, 404,
            "case {profile:?}/{role:?}: expected 404, got {status}: {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, "NoSuchEntity",
            "case {profile:?}/{role:?}: expected NoSuchEntity, got {code:?}: {body}"
        );
    }

    let (status, body) = call(
        &router,
        "GetInstanceProfile",
        &[("InstanceProfileName", first.name)],
    )
    .await;
    assert_eq!(status, 200, "the held profile must still read back: {body}");
    assert!(
        elements(&body, "RoleName").is_empty(),
        "a refused add must associate nothing, got {body}"
    );
}

/// A tag key in AWS's reserved `aws:` namespace is `InvalidInput` (HTTP 400), and — "if
/// any one of the tags is invalid … the entire request fails and the resource is not
/// created" — nothing is persisted.
#[tokio::test]
async fn a_reserved_tag_key_is_refused_and_creates_nothing() {
    let router = router();

    // Rows: the reserved key alone, and beside a valid one, in either order.
    let cases: Vec<(&str, Vec<(&str, &str)>)> = vec![
        ("reserved-only", vec![("aws:cloudformation:stack", "s")]),
        (
            "reserved-after-valid",
            vec![("env", "dev"), ("aws:created-by", "me")],
        ),
        (
            "reserved-before-valid",
            vec![("aws:created-by", "me"), ("env", "dev")],
        ),
    ];
    for (name, tags) in cases {
        let case = CreateCase {
            name: "refused-profile",
            path: None,
            expected_path: "/",
            tags,
        };
        let params = case.params();
        let (status, body) = call(&router, "CreateInstanceProfile", &borrowed(&params)).await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidInput",
            "case {name:?}: expected InvalidInput, got {body}"
        );

        let read = vec![param("InstanceProfileName", "refused-profile")];
        let (status, body) = call(&router, "GetInstanceProfile", &borrowed(&read)).await;
        assert_eq!(
            status, 404,
            "case {name:?}: a refused create must persist nothing, got {status} with body \
             {body}"
        );
    }
}
