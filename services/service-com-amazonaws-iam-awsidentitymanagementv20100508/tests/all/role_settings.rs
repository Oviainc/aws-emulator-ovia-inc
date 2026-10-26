// SPDX-License-Identifier: CC0-1.0

//! `UpdateRole` — changing a role's description and maximum session duration — end to end
//! over the awsQuery wire.
//!
//! RULE: `UpdateRole` replaces, on the account's own record of the named role, exactly
//! those of the two settings it takes (`Description`, `MaxSessionDuration`) that the
//! request CARRIES, leaving every member it does not carry — and every other member of the
//! record — exactly as it was; every later read of the record (`GetRole`) reports what the
//! call stored; the role is matched by its case-folded name; a name the account does not
//! hold is the modelled `NoSuchEntityException` (HTTP 404, awsQuery code `NoSuchEntity`);
//! and a service-linked role is a protected resource, so the change is
//! `UnmodifiableEntityException` (HTTP 400, `UnmodifiableEntity`). A refused call leaves
//! the held settings exactly as they were.
//!
//! No single recorded request is the specification, so each suite here is a TABLE whose
//! rows differ along what the rule generalises over: WHICH of the two settings the request
//! carries (one, the other, both), the value of each (every published bound of
//! `roleMaxSessionDurationType`'s 3600..43200 range and values inside it; the empty
//! description, a sentence, and the longest `roleDescriptionType`'s 0..1000 admits), the
//! number of successive updates, the role's path, and the alphabet and case of the role
//! name. The published range and length are read off the model
//! (`smithy_query(query="operation", operation="UpdateRole")`). The awsQuery harness
//! (router, call, element, elements, error_code) is the one `tests/all/user_lifecycle.rs`
//! owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The trust policy every role this suite creates carries: an IAM policy document, which
/// is all `CreateRole` asks of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"ec2.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The description every role this suite creates starts out with, so a change is visible
/// as a change rather than as a first write.
const INITIAL_DESCRIPTION: &str = "the description the create carried";

/// The maximum session duration every role this suite creates starts out with: a value
/// inside the published range and distinct from each one an update sets.
const INITIAL_DURATION: &str = "5400";

/// The floor and the ceiling of `roleMaxSessionDurationType`'s published range
/// (3600..43200), and one second past each — the bounds the model refuses outside.
const MIN_DURATION: i64 = 3_600;
const MAX_DURATION: i64 = 43_200;

/// The ceiling of `roleDescriptionType`'s published length (0..1000).
const MAX_DESCRIPTION_CHARACTERS: usize = 1_000;

/// One update to drive the rule over: which of the two settings the request carries, and
/// what each must read back as afterwards.
struct SettingsRow {
    case: &'static str,
    /// The `Description` the request carries, or `None` where it carries none.
    description: Option<&'static str>,
    /// The `MaxSessionDuration` the request carries, or `None` where it carries none.
    duration: Option<&'static str>,
}

/// Rows differing along which setting the update carries and the value it gives it: each
/// setting alone, both at once, every published bound of the duration range, the empty
/// description and a long one. Applied in order to the SAME role, so each row also asserts
/// that the setting the previous row stored survives an update that does not name it.
const SETTINGS_ROWS: &[SettingsRow] = &[
    SettingsRow {
        case: "the description alone",
        description: Some("assumed by the ingest pipeline"),
        duration: None,
    },
    SettingsRow {
        case: "the session duration alone, at the floor of the published range",
        description: None,
        duration: Some("3600"),
    },
    SettingsRow {
        case: "both settings at once, the duration at the ceiling of the published range",
        description: Some("assumed by the nightly compaction job"),
        duration: Some("43200"),
    },
    SettingsRow {
        case: "the empty description, the floor of the published length",
        description: Some(""),
        duration: None,
    },
    SettingsRow {
        case: "a duration inside the range, written with no leading zero",
        description: None,
        duration: Some("7200"),
    },
    SettingsRow {
        case: "a description carrying the characters the pattern admits",
        description: Some("café-münster\tline two\nline three"),
        duration: Some("10800"),
    },
];

/// One role to drive the rule over: the path it is created at, and the spelling the update
/// names it by (IAM entity names "are not distinguished by case", so a spelling that
/// differs from the created one still names the same role).
struct RoleCase {
    case: &'static str,
    role_name: &'static str,
    path: &'static str,
    spelling: &'static str,
}

/// Rows spanning the account root, a one-segment path and a nested path; names drawn from
/// the `^[\w+=,.@-]+$` alphabet `roleNameType` admits, in the created case, upper-cased and
/// lower-cased.
const ROLES: &[RoleCase] = &[
    RoleCase {
        case: "the account root, named exactly as created",
        role_name: "SettingsRole",
        path: "/",
        spelling: "SettingsRole",
    },
    RoleCase {
        case: "a one-segment path, named in upper case",
        role_name: "settings_role+2",
        path: "/service-role/",
        spelling: "SETTINGS_ROLE+2",
    },
    RoleCase {
        case: "a nested path, named in lower case",
        role_name: "Team.Settings=A@x-1",
        path: "/application/eu/team-1/",
        spelling: "team.settings=a@x-1",
    },
];

/// Create one role carrying [`TRUST_POLICY`], [`INITIAL_DESCRIPTION`] and
/// [`INITIAL_DURATION`].
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
            ("Description", INITIAL_DESCRIPTION),
            ("MaxSessionDuration", INITIAL_DURATION),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// One `UpdateRole` call carrying only the members the row states.
async fn update(
    router: &MultiServiceRouter,
    role_name: &str,
    description: Option<&str>,
    duration: Option<&str>,
) -> (u16, String) {
    let mut params: Vec<(&str, &str)> = vec![("RoleName", role_name)];
    if let Some(description) = description {
        params.push(("Description", description));
    }
    if let Some(duration) = duration {
        params.push(("MaxSessionDuration", duration));
    }
    call(router, "UpdateRole", &params).await
}

/// The description and the maximum session duration `GetRole` reports for a role the
/// account holds.
///
/// An empty description and no `Description` element at all say the same thing on the wire
/// — the role carries no text — so both read as the empty string here.
async fn read_back(router: &MultiServiceRouter, case: &str, role_name: &str) -> (String, String) {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetRole must succeed, got {status}: {body}"
    );
    let described = elements(&body, "Description");
    assert!(
        described.len() <= 1,
        "case {case:?}: expected at most one <Description>, got {}: {body}",
        described.len()
    );
    (
        described.into_iter().next().unwrap_or_default(),
        element(&body, "MaxSessionDuration"),
    )
}

/// Every member `GetRole` reports for a role apart from the two settings — the record an
/// update must leave alone.
async fn role_identity(
    router: &MultiServiceRouter,
    case: &str,
    role_name: &str,
) -> Vec<(&'static str, String)> {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: GetRole must succeed, got {status}: {body}"
    );
    [
        "RoleName",
        "RoleId",
        "Arn",
        "Path",
        "CreateDate",
        "AssumeRolePolicyDocument",
    ]
    .into_iter()
    .map(|tag| (tag, element(&body, tag)))
    .collect()
}

/// RULE: the update replaces exactly the settings the request carries, leaves the one it
/// does not alone, and a later `GetRole` reports both — however many times the role is
/// updated, and whatever value each setting is given.
#[tokio::test]
async fn an_update_replaces_only_the_settings_the_request_carries() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
        let identity_before = role_identity(&router, role.case, role.role_name).await;
        let mut expected = (
            INITIAL_DESCRIPTION.to_string(),
            INITIAL_DURATION.to_string(),
        );
        let read = read_back(&router, role.case, role.role_name).await;
        assert_eq!(
            read, expected,
            "case {:?}: the created role must report what the create carried, got {read:?}",
            role.case
        );

        for row in SETTINGS_ROWS {
            let case = format!("{} / {}", role.case, row.case);
            let (status, body) =
                update(&router, role.spelling, row.description, row.duration).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateRole to answer 200, got {status}: {body}"
            );
            assert!(
                elements(&body, "Code").is_empty(),
                "case {case:?}: a successful update must carry no error code: {body}"
            );

            // Only what the request carried moved.
            if let Some(description) = row.description {
                expected.0 = description.to_string();
            }
            if let Some(duration) = row.duration {
                expected.1 = duration.to_string();
            }
            let read = read_back(&router, &case, role.spelling).await;
            assert_eq!(
                read, expected,
                "case {case:?}: expected GetRole to report {expected:?}, got {read:?}"
            );

            // Nothing else about the role moved: the update changes two settings and
            // nothing else, so the role keeps its identity.
            let identity_now = role_identity(&router, &case, role.spelling).await;
            assert_eq!(
                identity_now, identity_before,
                "case {case:?}: expected the rest of the role to be untouched, got \
                 {identity_now:?}"
            );
        }
    }
}

/// RULE: the settings of one role are that role's alone — an update changes no other
/// role's settings.
#[tokio::test]
async fn an_update_changes_no_other_roles_settings() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
    }

    for (index, role) in ROLES.iter().enumerate() {
        let row = &SETTINGS_ROWS[index];
        let case = format!("{} / {}", role.case, row.case);
        let (status, body) = update(&router, role.spelling, row.description, row.duration).await;
        assert_eq!(status, 200, "case {case:?}: the update failed: {body}");

        for (other_index, other) in ROLES.iter().enumerate() {
            let other_row = &SETTINGS_ROWS[other_index];
            let expected = if other_index <= index {
                (
                    other_row
                        .description
                        .unwrap_or(INITIAL_DESCRIPTION)
                        .to_string(),
                    other_row.duration.unwrap_or(INITIAL_DURATION).to_string(),
                )
            } else {
                (
                    INITIAL_DESCRIPTION.to_string(),
                    INITIAL_DURATION.to_string(),
                )
            };
            let read = read_back(&router, &case, other.role_name).await;
            assert_eq!(
                read, expected,
                "case {case:?}: expected {:?} to report {expected:?}, got {read:?}",
                other.role_name
            );
        }
    }
}

/// RULE: a name the ACCOUNT does not hold "referenced a resource entity that does not
/// exist" — the modelled `NoSuchEntityException` (HTTP 404) — and the call persists
/// nothing, including for a role name AWS publishes for a service but this account has
/// never created.
#[tokio::test]
async fn updating_a_role_the_account_does_not_hold_is_not_found() {
    let router = router();
    create_role(&router, "HeldSettingsRole", "/").await;

    // Rows: a plain absent name, names one character either side of a held one, the whole
    // admitted alphabet, and a name AWS PUBLISHES for a service's own service-linked role,
    // which this account has not created (`state::service_provisioned_role`'s
    // documentation: an update "keeps answering from the store alone", so such a name is
    // NoSuchEntity here).
    let absent = [
        "absent",
        "HeldSettingsRol",
        "HeldSettingsRoles",
        "role_with+every=allowed.char@-",
        "AWSServiceRoleForBackup",
    ];
    for role_name in absent {
        let (status, body) = update(&router, role_name, Some("a description"), Some("7200")).await;
        assert_eq!(
            status, 404,
            "case {role_name:?}: expected 404 for an unheld role, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {role_name:?}: expected the modelled awsQuery code, got {body}"
        );
    }

    // The role the account does hold kept the settings it was created with.
    let read = read_back(&router, "the held role", "HeldSettingsRole").await;
    assert_eq!(
        read,
        (
            INITIAL_DESCRIPTION.to_string(),
            INITIAL_DURATION.to_string()
        ),
        "the held role's settings must be untouched by the refused updates, got {read:?}"
    );
}

/// RULE: "service-linked roles are protected Amazon Web Services resources. Only the
/// service that depends on the service-linked role can modify or delete the role on your
/// behalf" — so a caller's update of such a role is refused with the modelled
/// `UnmodifiableEntityException` (HTTP 400) and the role keeps the settings the service's
/// own role carries.
#[tokio::test]
async fn updating_a_service_linked_role_is_refused() {
    let router = router();

    // Rows spanning the service the role is linked to.
    let principals = [
        "lambda.amazonaws.com",
        "elasticbeanstalk.amazonaws.com",
        "autoscaling.amazonaws.com",
    ];
    for principal in principals {
        let (status, created) = call(
            &router,
            "CreateServiceLinkedRole",
            &[("AWSServiceName", principal)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {principal:?}: CreateServiceLinkedRole must succeed, got {status}: {created}"
        );
        let role_name = element(&created, "RoleName");
        let held = read_back(&router, principal, &role_name).await;

        for row in SETTINGS_ROWS.iter().take(3) {
            let case = format!("{principal} / {}", row.case);
            let (status, body) = update(&router, &role_name, row.description, row.duration).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for a service-linked role, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnmodifiableEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            let read = read_back(&router, &case, &role_name).await;
            assert_eq!(
                read, held,
                "case {case:?}: a refused update must leave the service's settings alone, got \
                 {read:?}"
            );
        }
    }
}

/// RULE: a value outside a member's published constraint is refused before the operation
/// runs, so nothing the model forbids reaches the store, while each bound itself is
/// accepted.
#[tokio::test]
async fn a_setting_outside_its_published_constraint_is_refused() {
    let router = router();
    create_role(&router, "BoundedSettingsRole", "/").await;

    // One second either side of the published duration range, and far outside it.
    for past_bound in [
        MIN_DURATION - 1,
        MAX_DURATION + 1,
        0,
        MAX_DURATION * 2,
    ] {
        let duration = past_bound.to_string();
        let (status, body) = update(&router, "BoundedSettingsRole", None, Some(&duration)).await;
        assert_eq!(
            status, 400,
            "case {past_bound}: expected 400 outside the published range, got {status}: {body}"
        );
        let read = read_back(&router, "a refused duration", "BoundedSettingsRole").await;
        assert_eq!(
            read.1, INITIAL_DURATION,
            "case {past_bound}: a refused update must leave the held duration alone, got {read:?}"
        );
    }

    // One character past the ceiling of the published description length.
    for past_bound in [
        MAX_DESCRIPTION_CHARACTERS + 1,
        MAX_DESCRIPTION_CHARACTERS * 2,
    ] {
        let description = "d".repeat(past_bound);
        let (status, body) =
            update(&router, "BoundedSettingsRole", Some(&description), None).await;
        assert_eq!(
            status, 400,
            "case {past_bound}: expected 400 past the published length, got {status}: {body}"
        );
        let read = read_back(&router, "a refused description", "BoundedSettingsRole").await;
        assert_eq!(
            read.0, INITIAL_DESCRIPTION,
            "case {past_bound}: a refused update must leave the held description alone, got \
             {read:?}"
        );
    }

    // Each bound itself is a legal value and is stored.
    for at_bound in [MIN_DURATION, MAX_DURATION] {
        let duration = at_bound.to_string();
        let (status, body) = update(&router, "BoundedSettingsRole", None, Some(&duration)).await;
        assert_eq!(
            status, 200,
            "case {at_bound}: expected 200 at the published bound, got {status}: {body}"
        );
        let read = read_back(&router, "a bound duration", "BoundedSettingsRole").await;
        assert_eq!(
            read.1, duration,
            "case {at_bound}: expected the bound duration to be stored, got {read:?}"
        );
    }
    for at_bound in [0, MAX_DESCRIPTION_CHARACTERS] {
        let description = "d".repeat(at_bound);
        let (status, body) =
            update(&router, "BoundedSettingsRole", Some(&description), None).await;
        assert_eq!(
            status, 200,
            "case {at_bound}: expected 200 at the published bound, got {status}: {body}"
        );
        let read = read_back(&router, "a bound description", "BoundedSettingsRole").await;
        assert_eq!(
            read.0, description,
            "case {at_bound}: expected the bound-length description to be stored, got {read:?}"
        );
    }
}
