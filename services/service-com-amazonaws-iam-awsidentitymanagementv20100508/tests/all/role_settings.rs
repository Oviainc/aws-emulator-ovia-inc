// SPDX-License-Identifier: CC0-1.0

//! `UpdateRole` — changing a role's description and maximum session duration — end to
//! end over the awsQuery wire.
//!
//! RULE: `UpdateRole` replaces, on the account's own record of the named role, EACH
//! setting the request carries (`Description`, `MaxSessionDuration`) with the value sent,
//! leaves a setting the request omits and every other member of the record exactly as it
//! was, answers 200 with a body carrying no members, and persists the change so every
//! later read (`GetRole`) reports it; the role is matched by its case-folded name; a name
//! the account does not hold is the modelled `NoSuchEntityException` (HTTP 404, awsQuery
//! code `NoSuchEntity`); and a value outside a member's published bounds is refused
//! before the operation runs. A refused call changes no setting.
//!
//! Both settings belong to the account's own record of the role rather than to its
//! permissions or its trust relationship, so a role a service links itself to is served
//! here like any other: the protected-entity refusal guards the calls that change such a
//! role's trust policy, its permissions or its existence, which have their own suites
//! (`tests/all/role_trust_policy.rs`, `tests/all/role_deletion.rs`).
//!
//! No single recorded request is the specification, so every suite here is a TABLE whose
//! rows differ along what the rule generalises over: WHICH settings a request carries
//! (description alone, duration alone, both, neither), the length and alphabet of the
//! description, the duration across `roleMaxSessionDurationType`'s published range
//! (each bound, one inside each bound, the middle) and past each bound, the role's path,
//! the alphabet and case of the role name, and the service a role is linked to. The
//! awsQuery harness (router, call, element, elements, error_code) is the one
//! `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The trust policy every role this suite creates carries: an IAM policy document,
/// which is all `CreateRole` asks of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The description every role this suite creates starts out with, so a replacement is
/// visible as a replacement rather than as a first write.
const INITIAL_DESCRIPTION: &str = "the description the create carried";

/// The maximum session duration every role this suite creates starts out with: a value
/// inside the published range that is neither bound nor the default, so a change away
/// from it and a request that leaves it alone are told apart.
const INITIAL_MAX_SESSION_DURATION: i32 = 10_800;

/// The floor of `roleMaxSessionDurationType`'s published range, 3600..43200
/// (`smithy_query(query="operation", operation="UpdateRole")`).
const MIN_MAX_SESSION_DURATION: i32 = 3_600;

/// The ceiling of `roleMaxSessionDurationType`'s published range. See
/// [`MIN_MAX_SESSION_DURATION`].
const MAX_MAX_SESSION_DURATION: i32 = 43_200;

/// The longest description `roleDescriptionType` admits: its published length is
/// 0..1000 (`smithy_query(query="operation", operation="UpdateRole")`).
const MAX_DESCRIPTION_CHARACTERS: usize = 1000;

/// Every printable ASCII character `roleDescriptionType`'s pattern admits
/// (`\u0020` through `\u007E`), in code-point order — the XML-significant ones among
/// them included, so the response's escaping is exercised too.
fn printable_ascii() -> String {
    (' '..='~').collect()
}

/// Every Latin-1 Supplement character the description pattern admits (`\u00A1` through
/// `\u00FF`), in code-point order. `\u00A0` is NOT in the pattern's range, so it is not
/// here.
fn latin_1_supplement() -> String {
    ('\u{00A1}'..='\u{00FF}').collect()
}

/// The longest description the published length admits, built from the whole admitted
/// alphabet so the longest row is not merely one character repeated.
fn longest_description() -> String {
    let alphabet: Vec<char> = printable_ascii()
        .chars()
        .chain(latin_1_supplement().chars())
        .collect();
    (0..MAX_DESCRIPTION_CHARACTERS)
        .map(|index| alphabet[index % alphabet.len()])
        .collect()
}

/// One `UpdateRole` request to drive the rule over: which of the two settings it carries
/// and with what value.
struct UpdateCase {
    case: &'static str,
    description: Option<String>,
    max_session_duration: Option<i32>,
}

/// Rows spanning which settings a request carries and the values each takes: the
/// description alone (over its lengths and alphabets), the duration alone (over each
/// published bound and values inside them), both together, and neither.
fn update_cases() -> Vec<UpdateCase> {
    vec![
        UpdateCase {
            case: "the description alone, a plain sentence",
            description: Some("Assumed by the ingest pipeline.".to_string()),
            max_session_duration: None,
        },
        UpdateCase {
            case: "the description alone, the empty text its length floor admits",
            description: Some(String::new()),
            max_session_duration: None,
        },
        UpdateCase {
            case: "the description alone, a single character",
            description: Some("x".to_string()),
            max_session_duration: None,
        },
        UpdateCase {
            case: "the description alone, every printable ASCII character admitted",
            description: Some(printable_ascii()),
            max_session_duration: None,
        },
        UpdateCase {
            case: "the description alone, the tab, line feed and carriage return admitted",
            description: Some("line one\n\tindented\r\nline two".to_string()),
            max_session_duration: None,
        },
        UpdateCase {
            case: "the description alone, Latin-1 Supplement characters",
            description: Some(format!("café-münster {}", latin_1_supplement())),
            max_session_duration: None,
        },
        UpdateCase {
            case: "the description alone, the longest the published length admits",
            description: Some(longest_description()),
            max_session_duration: None,
        },
        UpdateCase {
            case: "the duration alone, at the floor of the published range",
            description: None,
            max_session_duration: Some(MIN_MAX_SESSION_DURATION),
        },
        UpdateCase {
            case: "the duration alone, one second inside the floor",
            description: None,
            max_session_duration: Some(MIN_MAX_SESSION_DURATION + 1),
        },
        UpdateCase {
            case: "the duration alone, mid-range",
            description: None,
            max_session_duration: Some(21_600),
        },
        UpdateCase {
            case: "the duration alone, one second inside the ceiling",
            description: None,
            max_session_duration: Some(MAX_MAX_SESSION_DURATION - 1),
        },
        UpdateCase {
            case: "the duration alone, at the ceiling of the published range",
            description: None,
            max_session_duration: Some(MAX_MAX_SESSION_DURATION),
        },
        UpdateCase {
            case: "both settings together",
            description: Some("read the landing bucket".to_string()),
            max_session_duration: Some(7_200),
        },
        UpdateCase {
            case: "both settings together, the other way round the range",
            description: Some(String::new()),
            max_session_duration: Some(MAX_MAX_SESSION_DURATION),
        },
        UpdateCase {
            case: "neither setting, so the record stands as it was",
            description: None,
            max_session_duration: None,
        },
    ]
}

/// One role to drive the rule over: the path it is created at, and the spelling the
/// update names it by (IAM entity names "are not distinguished by case", so a spelling
/// that differs from the created one still names the same role).
struct RoleCase {
    case: &'static str,
    role_name: &'static str,
    path: &'static str,
    /// How the update and the read spell the role's name.
    spelling: &'static str,
}

/// Rows spanning the account root, a one-segment path and a nested path; names drawn
/// from the `^[\w+=,.@-]+$` alphabet `roleNameType` admits
/// (`smithy_query(query="operation", operation="UpdateRole")`), in the created case,
/// upper-cased and lower-cased.
const ROLES: &[RoleCase] = &[
    RoleCase {
        case: "the account root, named exactly as created",
        role_name: "IngestRole",
        path: "/",
        spelling: "IngestRole",
    },
    RoleCase {
        case: "a one-segment path, named in upper case",
        role_name: "build_role+2",
        path: "/service-role/",
        spelling: "BUILD_ROLE+2",
    },
    RoleCase {
        case: "a nested path, named in lower case",
        role_name: "Team.Role=A@x-1",
        path: "/application/eu/team-1/",
        spelling: "team.role=a@x-1",
    },
];

/// Create one role carrying [`TRUST_POLICY`], [`INITIAL_DESCRIPTION`] and
/// [`INITIAL_MAX_SESSION_DURATION`].
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let duration = INITIAL_MAX_SESSION_DURATION.to_string();
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
            ("Description", INITIAL_DESCRIPTION),
            ("MaxSessionDuration", duration.as_str()),
        ],
    )
    .await;
    assert!(
        status == 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// One `UpdateRole` call carrying only the settings the case names.
async fn update(
    router: &MultiServiceRouter,
    role_name: &str,
    description: Option<&str>,
    max_session_duration: Option<i32>,
) -> (u16, String) {
    let duration = max_session_duration.map(|seconds| seconds.to_string());
    let mut params: Vec<(&str, &str)> = vec![("RoleName", role_name)];
    if let Some(description) = description {
        params.push(("Description", description));
    }
    if let Some(duration) = duration.as_deref() {
        params.push(("MaxSessionDuration", duration));
    }
    call(router, "UpdateRole", &params).await
}

/// The two settings `GetRole` reports for a role the account holds.
///
/// An empty description and no `Description` element at all say the same thing on the
/// wire — the role carries no text — so both read as the empty string here.
async fn read_settings(router: &MultiServiceRouter, role_name: &str) -> (String, String) {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert!(
        status == 200,
        "case {role_name:?}: GetRole must succeed, got {status}: {body}"
    );
    let described = elements(&body, "Description");
    assert!(
        described.len() <= 1,
        "expected at most one <Description> in the response, got {}: {body}",
        described.len()
    );
    (
        described.into_iter().next().unwrap_or_default(),
        element(&body, "MaxSessionDuration"),
    )
}

/// Every member `GetRole` reports for a role apart from the two settings `UpdateRole`
/// writes — the part of the record an update must leave alone.
async fn role_identity(
    router: &MultiServiceRouter,
    role_name: &str,
) -> Vec<(&'static str, String)> {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert!(
        status == 200,
        "case {role_name:?}: GetRole must succeed, got {status}: {body}"
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

/// RULE: each setting the request carries replaces the one the role held, a setting the
/// request omits is left as it was, the rest of the record is untouched, and a later
/// `GetRole` reports the result — over a sequence of updates, whichever settings each
/// one carries.
#[tokio::test]
async fn an_update_replaces_only_the_settings_the_request_carries() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
        let identity_before = role_identity(&router, role.role_name).await;

        // What the role holds as the suite walks the table: the create's own settings to
        // begin with, then whatever each accepted update left behind.
        let mut held_description = INITIAL_DESCRIPTION.to_string();
        let mut held_duration = INITIAL_MAX_SESSION_DURATION.to_string();
        let (read_description, read_duration) = read_settings(&router, role.role_name).await;
        assert_eq!(
            (read_description, read_duration),
            (held_description.clone(), held_duration.clone()),
            "case {:?}: the created role must report the settings the create carried",
            role.case
        );

        for update_case in update_cases() {
            let case = format!("{} / {}", role.case, update_case.case);
            let (status, body) = update(
                &router,
                role.spelling,
                update_case.description.as_deref(),
                update_case.max_session_duration,
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateRole to answer 200, got {status}: {body}"
            );
            assert!(
                elements(&body, "Code").is_empty(),
                "case {case:?}: a successful update must carry no error code: {body}"
            );
            // `UpdateRoleResponse` models no members, so the updated record is reported
            // by the readers of it, not by this response.
            assert!(
                elements(&body, "Role").is_empty()
                    && elements(&body, "Description").is_empty()
                    && elements(&body, "MaxSessionDuration").is_empty(),
                "case {case:?}: the response models no members, got {body}"
            );

            if let Some(description) = &update_case.description {
                held_description = description.clone();
            }
            if let Some(duration) = update_case.max_session_duration {
                held_duration = duration.to_string();
            }

            // The change is persisted, and the setting the request did not carry still
            // holds what it held.
            let (read_description, read_duration) = read_settings(&router, role.spelling).await;
            assert_eq!(
                read_description, held_description,
                "case {case:?}: expected GetRole to report the description \
                 {held_description:?}, got {read_description:?}"
            );
            assert_eq!(
                read_duration, held_duration,
                "case {case:?}: expected GetRole to report the duration {held_duration:?}, \
                 got {read_duration:?}"
            );

            // Nothing else about the role moved: the update writes those settings alone.
            let identity_now = role_identity(&router, role.spelling).await;
            assert_eq!(
                identity_now, identity_before,
                "case {case:?}: expected the rest of the role to be untouched, got \
                 {identity_now:?}"
            );
        }
    }
}

/// RULE: the settings of one role are the settings of that role alone — an update
/// changes no other role's.
#[tokio::test]
async fn an_update_changes_no_other_roles_settings() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
    }

    // One row per role, each carrying a different pair of settings, paired with the
    // roles themselves so the table and [`ROLES`] cannot drift apart.
    let rows: [(&str, Option<&str>, Option<i32>); 3] = [
        ("a sentence and the range floor", Some("the first role's own text"), Some(MIN_MAX_SESSION_DURATION)),
        ("the empty description and the range ceiling", Some(""), Some(MAX_MAX_SESSION_DURATION)),
        ("the duration alone", None, Some(9_000)),
    ];
    assert_eq!(
        rows.len(),
        ROLES.len(),
        "the table must carry one row per role, got {} row(s) for {} role(s)",
        rows.len(),
        ROLES.len()
    );

    // What each role is expected to report as the walk applies one row at a time.
    let mut expected: Vec<(String, String)> = ROLES
        .iter()
        .map(|_| {
            (
                INITIAL_DESCRIPTION.to_string(),
                INITIAL_MAX_SESSION_DURATION.to_string(),
            )
        })
        .collect();

    for (index, (role, (row, description, duration))) in ROLES.iter().zip(rows).enumerate() {
        let case = format!("{} / {row}", role.case);
        let (status, body) = update(&router, role.spelling, description, duration).await;
        assert_eq!(status, 200, "case {case:?}: update failed: {body}");

        if let Some(description) = description {
            expected[index].0 = description.to_string();
        }
        if let Some(duration) = duration {
            expected[index].1 = duration.to_string();
        }

        for (other, want) in ROLES.iter().zip(&expected) {
            let read = read_settings(&router, other.role_name).await;
            assert_eq!(
                &read, want,
                "case {case:?}: expected {:?} to report {want:?}, got {read:?}",
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
    create_role(&router, "HeldRole", "/").await;

    // Rows: a plain absent name, names one character either side of a held one, the
    // whole admitted alphabet, and a name AWS PUBLISHES for a service's own
    // service-linked role, which this account has not created.
    let absent = [
        "absent",
        "HeldRol",
        "HeldRoles",
        "role_with+every=allowed.char@-",
        "AWSServiceRoleForBackup",
    ];
    for role_name in absent {
        let (status, body) = update(&router, role_name, Some("for nothing"), Some(7_200)).await;
        assert_eq!(
            status, 404,
            "case {role_name:?}: expected 404 for an unheld role, got {status}: {body}"
        );
        assert_eq!(
            error_code(&body),
            "NoSuchEntity",
            "case {role_name:?}: expected the modelled awsQuery code, got {body}"
        );

        // The refused update minted nothing: the name still resolves to no role of the
        // account's.
        let (get_status, got) = call(&router, "GetRole", &[("RoleName", role_name)]).await;
        assert!(
            get_status == 404 || element(&got, "Path").starts_with("/aws-service-role/"),
            "case {role_name:?}: a refused update must persist no role, got {get_status}: {got}"
        );
    }

    // The role the account does hold kept the settings it was created with.
    let read = read_settings(&router, "HeldRole").await;
    assert_eq!(
        read,
        (
            INITIAL_DESCRIPTION.to_string(),
            INITIAL_MAX_SESSION_DURATION.to_string()
        ),
        "the held role's settings must be untouched by the refused updates, got {read:?}"
    );
}

/// RULE: a role's DESCRIPTION is a setting of the account's own record of it, not one of
/// the properties a service-linked role protects — so an update carrying only a description
/// is served for such a role exactly as for any other, and a later `GetRole` reports what
/// it wrote. Its MAXIMUM SESSION DURATION is the owning service's, so an update carrying
/// one is refused `UnmodifiableEntity` and writes neither setting.
#[tokio::test]
async fn updating_a_service_linked_roles_settings_is_served() {
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
        // The settings the owning service's create left on the role: the duration no row
        // below may move, and the description the served rows replace.
        let (mut held_description, created_duration) = read_settings(&router, &role_name).await;

        for update_case in update_cases() {
            let case = format!("{principal} / {}", update_case.case);
            let (status, body) = update(
                &router,
                &role_name,
                update_case.description.as_deref(),
                update_case.max_session_duration,
            )
            .await;
            // The duration belongs to the service the role is linked to: a request asking
            // to move it is refused whatever else it carries, and a request that asks only
            // about the description is served.
            if update_case.max_session_duration.is_some() {
                assert_eq!(
                    status, 400,
                    "case {case:?}: expected a duration change on a service-linked role to \
                     be refused, got {status}: {body}"
                );
                assert_eq!(
                    error_code(&body),
                    "UnmodifiableEntity",
                    "case {case:?}: expected the modelled awsQuery code, got {body}"
                );
                let message = element(&body, "Message");
                assert_eq!(
                    message,
                    format!(
                        "{role_name} is an AWS service linked role, max session duration \
                         cannot be changed."
                    ),
                    "case {case:?}: expected AWS's UnmodifiableEntity wording, got {message:?}"
                );
            } else {
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 for a service-linked role, got {status}: \
                     {body}"
                );
                assert!(
                    elements(&body, "Code").is_empty(),
                    "case {case:?}: a successful update must carry no error code: {body}"
                );
                if let Some(description) = &update_case.description {
                    held_description = description.clone();
                }
            }
            // A refused update writes NEITHER setting, so the expected pair is the one the
            // served rows left behind — with the duration still the one the create set.
            let read = read_settings(&router, &role_name).await;
            assert_eq!(
                read,
                (held_description.clone(), created_duration.clone()),
                "case {case:?}: expected GetRole to report the settings the update wrote, \
                 got {read:?}"
            );
        }

        // The role is still the service's own: what makes it a service-linked role is
        // its path, which this update does not touch.
        let (status, body) = call(&router, "GetRole", &[("RoleName", &role_name)]).await;
        assert_eq!(status, 200, "case {principal:?}: GetRole failed: {body}");
        let path = element(&body, "Path");
        assert_eq!(
            path,
            format!("/aws-service-role/{principal}/"),
            "case {principal:?}: expected the service-linked path to be untouched, got \
             {path:?}"
        );
    }
}

/// RULE: a value outside a member's published bounds is refused before the operation
/// runs, so it never reaches the store, while each bound itself is accepted.
#[tokio::test]
async fn a_setting_outside_its_published_bounds_is_refused() {
    let router = router();
    create_role(&router, "Bounded", "/").await;
    let held = (
        INITIAL_DESCRIPTION.to_string(),
        INITIAL_MAX_SESSION_DURATION.to_string(),
    );

    // One second either side of the published range (3600..43200), and far past each.
    for past_bound in [
        MIN_MAX_SESSION_DURATION - 1,
        0,
        MAX_MAX_SESSION_DURATION + 1,
        MAX_MAX_SESSION_DURATION * 2,
    ] {
        let (status, body) = update(&router, "Bounded", None, Some(past_bound)).await;
        assert_eq!(
            status, 400,
            "case {past_bound}: expected 400 outside the published range, got {status}: {body}"
        );
        let read = read_settings(&router, "Bounded").await;
        assert_eq!(
            read, held,
            "case {past_bound}: a refused update must leave the held settings alone, got \
             {read:?}"
        );
    }

    // One character past the description's published length (0..1000), and far past it.
    for past_bound in [
        MAX_DESCRIPTION_CHARACTERS + 1,
        MAX_DESCRIPTION_CHARACTERS * 2,
    ] {
        let description = "d".repeat(past_bound);
        let (status, body) = update(&router, "Bounded", Some(&description), None).await;
        assert_eq!(
            status, 400,
            "case {past_bound}: expected 400 past the published length, got {status}: {body}"
        );
        let read = read_settings(&router, "Bounded").await;
        assert_eq!(
            read, held,
            "case {past_bound}: a refused update must leave the held settings alone, got \
             {read:?}"
        );
    }

    // Each bound itself is a legal value and is stored.
    for at_bound in [MIN_MAX_SESSION_DURATION, MAX_MAX_SESSION_DURATION] {
        let description = "d".repeat(MAX_DESCRIPTION_CHARACTERS);
        let (status, body) = update(&router, "Bounded", Some(&description), Some(at_bound)).await;
        assert_eq!(
            status, 200,
            "case {at_bound}: expected 200 at the published bound, got {status}: {body}"
        );
        let read = read_settings(&router, "Bounded").await;
        assert_eq!(
            read,
            (description, at_bound.to_string()),
            "case {at_bound}: expected the bound value to be stored, got {read:?}"
        );
    }
}
