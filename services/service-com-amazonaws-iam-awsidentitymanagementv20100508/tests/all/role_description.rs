// SPDX-License-Identifier: CC0-1.0

//! `UpdateRoleDescription` — replacing a role's description — end to end over the
//! awsQuery wire.
//!
//! RULE: `UpdateRoleDescription` REPLACES the description held on the account's own
//! record of the named role with the text the request carried, changing no other member
//! of that record, and reports the modified role; every later read of the record
//! (`GetRole`) reports the new description; the role is matched by its case-folded name;
//! a name the account does not hold is the modelled `NoSuchEntityException` (HTTP 404,
//! awsQuery code `NoSuchEntity`); and a service-linked role is a protected resource, so
//! the update is `UnmodifiableEntityException` (HTTP 400, `UnmodifiableEntity`). A
//! refused call leaves the held description exactly as it was.
//!
//! No single recorded request is the specification, so every suite here is a TABLE whose
//! rows differ along what the rule generalises over: the length of the description (zero
//! characters, one, a sentence, and the longest `roleDescriptionType` admits) and its
//! alphabet (every printable ASCII character, the tab/line-feed/carriage-return and the
//! Latin-1 Supplement characters the member's pattern admits), the number of successive
//! updates, the role's path, the alphabet and case of the role name, and the service a
//! role is linked to. The awsQuery harness (router, call, element, elements, error_code)
//! is the one `tests/all/user_lifecycle.rs` owns and every suite in this crate shares.

use smithy_server_runtime::MultiServiceRouter;

use crate::user_lifecycle::{call, element, elements, error_code, router};

/// The trust policy every role this suite creates carries: an IAM policy document,
/// which is all `CreateRole` asks of one.
const TRUST_POLICY: &str = r#"{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"Service":"dlm.amazonaws.com"},"Action":"sts:AssumeRole"}]}"#;

/// The description every role this suite creates starts out with, so a replacement is
/// visible as a replacement rather than as a first write.
const INITIAL_DESCRIPTION: &str = "the description the create carried";

/// The longest description `roleDescriptionType` admits: its published length is
/// 0..1000 (`smithy_query(query="operation", operation="UpdateRoleDescription")`).
const MAX_DESCRIPTION_CHARACTERS: usize = 1000;

/// Every printable ASCII character `roleDescriptionType`'s pattern admits
/// (`\u0020` through `\u007E`), in code-point order — the XML-significant ones among
/// them included, so the response's escaping is exercised too.
fn printable_ascii() -> String {
    (' '..='~').collect()
}

/// Every Latin-1 Supplement character the pattern admits (`\u00A1` through `\u00FF`),
/// in code-point order. `\u00A0` is NOT in the pattern's range, so it is not here.
fn latin_1_supplement() -> String {
    ('\u{00A1}'..='\u{00FF}').collect()
}

/// Descriptions spanning the lengths and alphabets the rule generalises over. Each is
/// a legal `roleDescriptionType` value, so each must be stored and reported as the exact
/// text sent.
fn descriptions() -> Vec<(String, String)> {
    let mut rows = vec![
        (
            "the empty description, the floor of the published length".to_string(),
            String::new(),
        ),
        ("a single character".to_string(), "x".to_string()),
        (
            "a plain sentence".to_string(),
            "Assumed by the ingest pipeline to read the landing bucket.".to_string(),
        ),
        (
            "every printable ASCII character the pattern admits".to_string(),
            printable_ascii(),
        ),
        (
            "the tab, line feed and carriage return the pattern admits".to_string(),
            "line one\n\tindented\r\nline two".to_string(),
        ),
        (
            "Latin-1 Supplement characters".to_string(),
            format!("café-münster {}", latin_1_supplement()),
        ),
    ];
    // The ceiling of the published length, built from the whole admitted alphabet so the
    // longest row is not merely one character repeated.
    let alphabet: Vec<char> = printable_ascii()
        .chars()
        .chain(latin_1_supplement().chars())
        .collect();
    let longest: String = (0..MAX_DESCRIPTION_CHARACTERS)
        .map(|index| alphabet[index % alphabet.len()])
        .collect();
    rows.push((
        "the longest description the published length admits".to_string(),
        longest,
    ));
    rows
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
/// (`smithy_query(query="operation", operation="UpdateRoleDescription")`), in the
/// created case, upper-cased and lower-cased.
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

/// Create one role carrying [`TRUST_POLICY`] and [`INITIAL_DESCRIPTION`].
async fn create_role(router: &MultiServiceRouter, role_name: &str, path: &str) {
    let (status, body) = call(
        router,
        "CreateRole",
        &[
            ("RoleName", role_name),
            ("Path", path),
            ("AssumeRolePolicyDocument", TRUST_POLICY),
            ("Description", INITIAL_DESCRIPTION),
        ],
    )
    .await;
    assert!(
        status == 200,
        "case {role_name:?}: CreateRole must succeed, got {status}: {body}"
    );
}

/// One `UpdateRoleDescription` call.
async fn update(
    router: &MultiServiceRouter,
    role_name: &str,
    description: &str,
) -> (u16, String) {
    call(
        router,
        "UpdateRoleDescription",
        &[("RoleName", role_name), ("Description", description)],
    )
    .await
}

/// The description a response reports for the single role it carries.
///
/// An empty description and no `Description` element at all say the same thing on the
/// wire — the role carries no text — so both read as the empty string here.
fn reported(body: &str) -> String {
    let found = elements(body, "Description");
    assert!(
        found.len() <= 1,
        "expected at most one <Description> in the response, got {}: {body}",
        found.len()
    );
    found.into_iter().next().unwrap_or_default()
}

/// The description `GetRole` reports for a role the account holds.
async fn read_back(router: &MultiServiceRouter, role_name: &str) -> String {
    let (status, body) = call(router, "GetRole", &[("RoleName", role_name)]).await;
    assert!(
        status == 200,
        "case {role_name:?}: GetRole must succeed, got {status}: {body}"
    );
    reported(&body)
}

/// Every member `GetRole` reports for a role, apart from its description — the record
/// an update must leave alone.
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
        "MaxSessionDuration",
        "AssumeRolePolicyDocument",
    ]
    .into_iter()
    .map(|tag| (tag, element(&body, tag)))
    .collect()
}

/// RULE: the update replaces the role's description with the text the request carried,
/// reports the modified role, and a later `GetRole` reports the same text — however many
/// times the description is replaced, and whatever length and alphabet each text has.
#[tokio::test]
async fn an_update_replaces_the_description_and_get_role_reads_it_back() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
        let identity_before = role_identity(&router, role.role_name).await;
        assert_eq!(
            read_back(&router, role.role_name).await,
            INITIAL_DESCRIPTION,
            "case {:?}: the created role must report the description the create carried",
            role.case
        );

        // Each row is a further replacement of the SAME role's description, so the rule
        // is asserted over a sequence of updates rather than one.
        for (description_case, description) in descriptions() {
            let case = format!("{} / {description_case}", role.case);
            let (status, body) = update(&router, role.spelling, &description).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected UpdateRoleDescription to answer 200, got {status}: \
                 {body}"
            );
            assert!(
                elements(&body, "Code").is_empty(),
                "case {case:?}: a successful update must carry no error code: {body}"
            );

            // The response "contains details about the modified role": the new
            // description, under the role the request named.
            let answered = reported(&body);
            assert_eq!(
                answered, description,
                "case {case:?}: expected the response to report the new description, got \
                 {answered:?}"
            );
            let answered_name = element(&body, "RoleName");
            assert_eq!(
                answered_name, role.role_name,
                "case {case:?}: expected the response to name the modified role, got \
                 {answered_name:?}"
            );

            // And the change is persisted: the implemented reader sees it too.
            let read = read_back(&router, role.spelling).await;
            assert_eq!(
                read, description,
                "case {case:?}: expected GetRole to read the new description back, got {read:?}"
            );

            // Nothing else about the role moved: the update changes one member.
            let identity_now = role_identity(&router, role.spelling).await;
            assert_eq!(
                identity_now, identity_before,
                "case {case:?}: expected the rest of the role to be untouched, got \
                 {identity_now:?}"
            );
        }
    }
}

/// RULE: the description of one role is the description of that role alone — an update
/// changes no other role's description.
#[tokio::test]
async fn an_update_changes_no_other_roles_description() {
    let router = router();
    for role in ROLES {
        create_role(&router, role.role_name, role.path).await;
    }

    let rows = descriptions();
    for (index, role) in ROLES.iter().enumerate() {
        let (description_case, description) = &rows[index];
        let case = format!("{} / {description_case}", role.case);
        let (status, body) = update(&router, role.spelling, description).await;
        assert_eq!(status, 200, "case {case:?}: update failed: {body}");

        for (other_index, other) in ROLES.iter().enumerate() {
            let expected = if other_index <= index {
                rows[other_index].1.clone()
            } else {
                INITIAL_DESCRIPTION.to_string()
            };
            let read = read_back(&router, other.role_name).await;
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
    create_role(&router, "HeldRole", "/").await;

    // Rows: a plain absent name, names one character either side of a held one, the
    // whole admitted alphabet, and a name AWS PUBLISHES for a service's own
    // service-linked role, which this account has not created
    // (`state::service_provisioned_role`'s documentation: an update "keeps answering
    // from the store alone", so such a name is NoSuchEntity here).
    let absent = [
        "absent",
        "HeldRol",
        "HeldRoles",
        "role_with+every=allowed.char@-",
        "AWSServiceRoleForBackup",
    ];
    for role_name in absent {
        let (status, body) = update(&router, role_name, "a description for nothing").await;
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

    // The role the account does hold kept the description it was created with.
    assert_eq!(
        read_back(&router, "HeldRole").await,
        INITIAL_DESCRIPTION,
        "the held role's description must be untouched by the refused updates"
    );
}

/// RULE: "service-linked roles are protected Amazon Web Services resources. Only the
/// service that depends on the service-linked role can modify or delete the role on your
/// behalf" — so a caller's update of such a role's description is refused with the
/// modelled `UnmodifiableEntityException` (HTTP 400) and the role keeps the description
/// the service's own role carries.
#[tokio::test]
async fn updating_a_service_linked_roles_description_is_refused() {
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
        let held = read_back(&router, &role_name).await;

        for (description_case, description) in descriptions().into_iter().take(3) {
            let case = format!("{principal} / {description_case}");
            let (status, body) = update(&router, &role_name, &description).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for a service-linked role, got {status}: {body}"
            );
            assert_eq!(
                error_code(&body),
                "UnmodifiableEntity",
                "case {case:?}: expected the modelled awsQuery code, got {body}"
            );
            let read = read_back(&router, &role_name).await;
            assert_eq!(
                read, held,
                "case {case:?}: a refused update must leave the service's description alone, \
                 got {read:?}"
            );
        }
    }
}

/// RULE: a description outside `roleDescriptionType`'s published length is refused
/// before the operation runs, so text the model forbids never reaches the store, while
/// each bound itself is accepted.
#[tokio::test]
async fn a_description_outside_the_published_length_is_refused() {
    let router = router();
    create_role(&router, "Bounded", "/").await;

    // One character past the ceiling of the published length (0..1000), and far past it.
    for past_bound in [
        MAX_DESCRIPTION_CHARACTERS + 1,
        MAX_DESCRIPTION_CHARACTERS * 2,
    ] {
        let description = "d".repeat(past_bound);
        let (status, body) = update(&router, "Bounded", &description).await;
        assert_eq!(
            status, 400,
            "case {past_bound}: expected 400 past the published length, got {status}: {body}"
        );
        let read = read_back(&router, "Bounded").await;
        assert_eq!(
            read, INITIAL_DESCRIPTION,
            "case {past_bound}: a refused update must leave the held description alone, got \
             {read:?}"
        );
    }

    // Each bound itself is a legal description and is stored.
    for at_bound in [0, MAX_DESCRIPTION_CHARACTERS] {
        let description = "d".repeat(at_bound);
        let (status, body) = update(&router, "Bounded", &description).await;
        assert_eq!(
            status, 200,
            "case {at_bound}: expected 200 at the published bound, got {status}: {body}"
        );
        let read = read_back(&router, "Bounded").await;
        assert_eq!(
            read, description,
            "case {at_bound}: expected the bound-length description to be stored, got {read:?}"
        );
    }
}
