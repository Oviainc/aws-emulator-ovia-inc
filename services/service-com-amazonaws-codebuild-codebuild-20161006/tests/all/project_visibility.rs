// SPDX-License-Identifier: CC0-1.0

//! `UpdateProjectVisibility`, over the wire the router serves.
//!
//! RULE: `UpdateProjectVisibility` writes the requested `projectVisibility` onto the
//! build project its `projectArn` identifies, so that call and every later read of the
//! project report it; a project whose builds are public carries the
//! `publicProjectAlias` the public build APIs address it by and a private one carries
//! none; a named `resourceAccessRole` replaces the stored one and an omitted one leaves
//! it; and a `projectArn` that is not a project ARN, a `resourceAccessRole` that is not
//! an IAM role ARN, or a visibility outside `ProjectVisibilityType`, is refused as
//! invalid input while an ARN that resolves no project of this account, region and
//! partition is refused as a missing resource — in every case persisting nothing.
//!
//! The harness (`call`, `create_project`, `emulator`, `error_type`, `expected_arn`) is
//! the one `project_build_lifecycle` already drives this service with.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::project_build_lifecycle::{
    call, create_project, emulator, error_type, expected_arn, REGION,
};

/// An IAM role ARN a caller may hand `UpdateProjectVisibility`: one with an IAM path
/// and one without, the two forms a role ARN takes.
const ROLE_A: &str = "arn:aws:iam::000000000000:role/service-role/codebuild-public-a";
const ROLE_B: &str = "arn:aws:iam::000000000000:role/codebuild-public-b";

/// Call `UpdateProjectVisibility` for `project_arn`.
async fn update_visibility(
    router: &MultiServiceRouter,
    project_arn: &str,
    visibility: &str,
    role: Option<&str>,
) -> (u16, Value) {
    let mut body = json!({"projectArn": project_arn, "projectVisibility": visibility});
    if let (Value::Object(target), Some(role)) = (&mut body, role) {
        target.insert("resourceAccessRole".to_string(), json!(role));
    }
    call(router, "UpdateProjectVisibility", body).await
}

/// The project `name` as `BatchGetProjects` reports it — the implemented reader this
/// write has to be visible through.
async fn stored_project(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "BatchGetProjects", json!({"names": [name]})).await;
    assert_eq!(
        status, 200,
        "BatchGetProjects {name:?}: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["projects"].as_array().map(Vec::len),
        Some(1),
        "BatchGetProjects {name:?}: expected exactly one project, got {body}"
    );
    body["projects"][0].clone()
}

/// One row of the visibility table: a sequence of updates applied to one project.
struct Case {
    case: &'static str,
    steps: Vec<Step>,
}

/// One `UpdateProjectVisibility` call of a row: the visibility it asks for, and the
/// `resourceAccessRole` it names, if any.
struct Step {
    visibility: &'static str,
    role: Option<&'static str>,
}

fn step(visibility: &'static str, role: Option<&'static str>) -> Step {
    Step { visibility, role }
}

/// `project` without `keys` — so an assertion can say "everything else is untouched".
fn without(project: &Value, keys: &[&str]) -> Value {
    let mut copy = project.clone();
    if let Some(object) = copy.as_object_mut() {
        for key in keys {
            object.remove(*key);
        }
    }
    copy
}

/// RULE: the requested visibility is what the call and every later read of the project
/// report; the public alias is present exactly while the project is public; a named
/// `resourceAccessRole` replaces the stored one and an omitted one leaves it.
#[tokio::test]
async fn the_requested_visibility_is_what_every_later_read_reports() {
    let router = emulator();

    // The rows differ along the two values of `ProjectVisibilityType` (from
    // `smithy_query(query="shape", shape="ProjectVisibilityType")`: PUBLIC_READ,
    // PRIVATE), whether the request names a `resourceAccessRole`, and how many updates
    // the project has taken. A single `PUBLIC_READ` is one row of this table.
    let cases = [
        Case {
            case: "private on a fresh project",
            steps: vec![step("PRIVATE", None)],
        },
        Case {
            case: "public with no access role",
            steps: vec![step("PUBLIC_READ", None)],
        },
        Case {
            case: "public with an access role",
            steps: vec![step("PUBLIC_READ", Some(ROLE_A))],
        },
        Case {
            case: "public then private",
            steps: vec![step("PUBLIC_READ", Some(ROLE_A)), step("PRIVATE", None)],
        },
        Case {
            case: "public twice, the second naming another role",
            steps: vec![
                step("PUBLIC_READ", Some(ROLE_A)),
                step("PUBLIC_READ", Some(ROLE_B)),
            ],
        },
        Case {
            case: "alternating four times",
            steps: vec![
                step("PRIVATE", None),
                step("PUBLIC_READ", None),
                step("PRIVATE", Some(ROLE_B)),
                step("PUBLIC_READ", None),
            ],
        },
    ];

    for (index, Case { case, steps }) in cases.iter().enumerate() {
        let name = format!("visibility-case-{index}");
        let created = create_project(&router, &name, json!({})).await;
        let arn = expected_arn(&format!("project/{name}"));
        assert_eq!(
            created["projectVisibility"].as_str(),
            Some("PRIVATE"),
            "case {case:?}: a new project starts PRIVATE, got {created}"
        );
        // The alias this project reports the first time it is published; every later
        // public step must report the same one. It is READ from the service, never
        // recomputed here, so the assertion can fail.
        let mut first_alias: Option<String> = None;
        let mut expected_role: Option<&str> = None;

        for (step, Step { visibility, role }) in steps.iter().enumerate() {
            let (status, body) = update_visibility(&router, &arn, visibility, *role).await;
            assert_eq!(
                status, 200,
                "case {case:?} step {step} ({visibility}, role {role:?}): \
                 expected 200, got {status} with {body}"
            );
            if role.is_some() {
                expected_role = *role;
            }
            let reported_alias = body["publicProjectAlias"].as_str().map(str::to_string);

            assert_eq!(
                body["projectArn"].as_str(),
                Some(arn.as_str()),
                "case {case:?} step {step}: expected the project's ARN {arn:?}, got {body}"
            );
            assert_eq!(
                body["projectVisibility"].as_str(),
                Some(*visibility),
                "case {case:?} step {step}: expected {visibility}, got {body}"
            );
            if *visibility == "PUBLIC_READ" {
                let alias = reported_alias.clone().unwrap_or_default();
                assert!(
                    !alias.is_empty(),
                    "case {case:?} step {step}: a public project must report an alias, got {body}"
                );
                match &first_alias {
                    Some(first) => assert_eq!(
                        &alias, first,
                        "case {case:?} step {step}: the same project must keep one alias \
                         (first {first:?}), got {body}"
                    ),
                    None => first_alias = Some(alias),
                }
            } else {
                assert_eq!(
                    reported_alias, None,
                    "case {case:?} step {step}: a private project must report no alias, \
                     got {body}"
                );
            }

            let project = stored_project(&router, &name).await;
            assert_eq!(
                project["projectVisibility"].as_str(),
                Some(*visibility),
                "case {case:?} step {step}: the project must report {visibility}, got {project}"
            );
            assert_eq!(
                project["publicProjectAlias"].as_str().map(str::to_string),
                reported_alias,
                "case {case:?} step {step}: the project must report the alias the call \
                 answered ({reported_alias:?}), got {project}"
            );
            assert_eq!(
                project["resourceAccessRole"].as_str(),
                expected_role,
                "case {case:?} step {step}: expected resourceAccessRole \
                 {expected_role:?}, got {project}"
            );
        }
    }
}

/// RULE: the alias is a function of the project alone — the same project yields the
/// same alias whenever it is public, and two projects never share one.
#[tokio::test]
async fn the_public_alias_identifies_the_project_and_nothing_else() {
    let router = emulator();

    let mut aliases: Vec<(String, String)> = Vec::new();
    for name in ["alias-one", "alias-two", "alias-three"] {
        create_project(&router, name, json!({})).await;
        let arn = expected_arn(&format!("project/{name}"));
        let (status, body) = update_visibility(&router, &arn, "PUBLIC_READ", None).await;
        assert_eq!(
            status, 200,
            "{name}: expected 200, got {status} with {body}"
        );
        let alias = body["publicProjectAlias"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert!(
            !alias.is_empty() && alias.len() == 32 && alias.chars().all(|c| c.is_ascii_hexdigit()),
            "{name}: expected a non-empty opaque alias token, got {alias:?}"
        );
        assert_ne!(
            alias, arn,
            "{name}: the alias must be an identifier of its own, not the ARN echoed back"
        );

        // Taking the project private and public again yields the same alias: it is not
        // re-drawn per call.
        let (_, back_to_private) = update_visibility(&router, &arn, "PRIVATE", None).await;
        assert!(
            back_to_private["publicProjectAlias"].is_null(),
            "{name}: a private project carries no alias, got {back_to_private}"
        );
        let (_, public_again) = update_visibility(&router, &arn, "PUBLIC_READ", Some(ROLE_A)).await;
        assert_eq!(
            public_again["publicProjectAlias"].as_str(),
            Some(alias.as_str()),
            "{name}: re-publishing must report the same alias, got {public_again}"
        );

        aliases.push((name.to_string(), alias));
    }

    for (left, right) in aliases.iter().zip(aliases.iter().skip(1)) {
        assert_ne!(
            left.1, right.1,
            "{} and {}: distinct projects must not share an alias",
            left.0, right.0
        );
    }
}

/// RULE: the call changes the project's visibility members and its `lastModified`, and
/// nothing else about the project.
#[tokio::test]
async fn an_update_touches_only_the_visibility_members_and_last_modified() {
    let router = emulator();

    create_project(&router, "untouched", json!({})).await;
    create_project(
        &router,
        "published",
        json!({"description": "kept", "badgeEnabled": true, "timeoutInMinutes": 15}),
    )
    .await;
    let before = stored_project(&router, "published").await;

    // No sleep: the service stamps a successful update at the later of now and one
    // millisecond past the stored stamp, so `lastModified` moves even when both writes
    // land inside one millisecond.

    let arn = expected_arn("project/published");
    let (status, body) = update_visibility(&router, &arn, "PUBLIC_READ", Some(ROLE_A)).await;
    assert_eq!(
        status, 200,
        "publishing: expected 200, got {status} with {body}"
    );

    let after = stored_project(&router, "published").await;
    let volatile = [
        "projectVisibility",
        "publicProjectAlias",
        "resourceAccessRole",
        "lastModified",
    ];
    assert_eq!(
        without(&after, &volatile),
        without(&before, &volatile),
        "every other member of the project must survive the update: before {before}, after {after}"
    );
    assert_eq!(
        after["created"], before["created"],
        "created must not move: before {before}, after {after}"
    );
    assert!(
        after["lastModified"].as_f64() > before["lastModified"].as_f64(),
        "lastModified must move forward: before {before}, after {after}"
    );

    // And the move is observable where the service sorts on it.
    let (status, listed) = call(
        &router,
        "ListProjects",
        json!({"sortBy": "LAST_MODIFIED_TIME", "sortOrder": "ASCENDING"}),
    )
    .await;
    assert_eq!(
        status, 200,
        "ListProjects: expected 200, got {status} with {listed}"
    );
    assert_eq!(
        listed["projects"],
        json!(["untouched", "published"]),
        "the updated project must sort last by modified time, got {listed}"
    );
}

/// RULE: a `projectArn` that is not a CodeBuild project ARN, a `resourceAccessRole`
/// that is not an IAM role ARN, and a visibility outside `ProjectVisibilityType`, are
/// refused as invalid input; an ARN that resolves no project of this account, region
/// and partition is refused as a missing resource; and a refused call persists nothing.
#[tokio::test]
async fn a_request_that_names_no_project_or_no_modeled_visibility_is_refused() {
    let router = emulator();
    create_project(&router, "refusal-probe", json!({})).await;
    let arn = expected_arn("project/refusal-probe");

    let refused: Vec<(&str, Value, &str)> = vec![
        (
            "an ARN naming a project that does not exist",
            json!({"projectArn": expected_arn("project/no-such-project"),
                   "projectVisibility": "PUBLIC_READ"}),
            "ResourceNotFoundException",
        ),
        (
            "not an ARN at all",
            json!({"projectArn": "refusal-probe", "projectVisibility": "PUBLIC_READ"}),
            "InvalidInputException",
        ),
        (
            "an ARN of another service",
            json!({"projectArn": "arn:aws:s3:::project/refusal-probe",
                   "projectVisibility": "PUBLIC_READ"}),
            "InvalidInputException",
        ),
        (
            "a CodeBuild ARN of another resource type",
            json!({"projectArn": expected_arn("build/refusal-probe:1"),
                   "projectVisibility": "PUBLIC_READ"}),
            "InvalidInputException",
        ),
        (
            "an ARN with a field missing",
            json!({"projectArn": format!("arn:aws:codebuild:{REGION}:project/refusal-probe"),
                   "projectVisibility": "PUBLIC_READ"}),
            "InvalidInputException",
        ),
        (
            "an ARN with an empty project name",
            json!({"projectArn": expected_arn("project/"), "projectVisibility": "PUBLIC_READ"}),
            "InvalidInputException",
        ),
        (
            // Below `NonEmptyString`'s `@length(min: 1)`, so the boundary refuses it.
            "an empty ARN",
            json!({"projectArn": "", "projectVisibility": "PUBLIC_READ"}),
            "InvalidInputException",
        ),
        (
            "a visibility outside ProjectVisibilityType",
            json!({"projectArn": arn.clone(), "projectVisibility": "PUBLIC"}),
            "InvalidInputException",
        ),
        (
            "a lowercase spelling of a modeled visibility",
            json!({"projectArn": arn.clone(), "projectVisibility": "public_read"}),
            "InvalidInputException",
        ),
        (
            "an empty visibility",
            json!({"projectArn": arn.clone(), "projectVisibility": ""}),
            "InvalidInputException",
        ),
        (
            // The name resolves an entry, but the ARN is not the one that entry was
            // stored with, so it addresses another account's project.
            "a project ARN of this name in another region",
            json!({"projectArn": format!(
                       "arn:aws:codebuild:eu-west-1:{}:project/refusal-probe",
                       engine::EMULATOR_ACCOUNT_ID),
                   "projectVisibility": "PUBLIC_READ"}),
            "ResourceNotFoundException",
        ),
        (
            "a project ARN of this name in another partition and account",
            json!({"projectArn":
                       "arn:aws-us-gov:codebuild:us-gov-west-1:000000000000:project/refusal-probe",
                   "projectVisibility": "PUBLIC_READ"}),
            "ResourceNotFoundException",
        ),
        (
            // `resourceAccessRole` is documented as the ARN of an IAM role, and the
            // model types it only as `NonEmptyString`, so the service checks it.
            "a resourceAccessRole that is not an ARN",
            json!({"projectArn": arn.clone(), "projectVisibility": "PUBLIC_READ",
                   "resourceAccessRole": "codebuild-public-role"}),
            "InvalidInputException",
        ),
        (
            "a resourceAccessRole naming another IAM resource type",
            json!({"projectArn": arn.clone(), "projectVisibility": "PUBLIC_READ",
                   "resourceAccessRole": "arn:aws:iam::000000000000:user/someone"}),
            "InvalidInputException",
        ),
        (
            "a resourceAccessRole that is an ARN of another service",
            json!({"projectArn": arn.clone(), "projectVisibility": "PUBLIC_READ",
                   "resourceAccessRole": expected_arn("project/refusal-probe")}),
            "InvalidInputException",
        ),
    ];

    for (case, body, expected_code) in refused {
        let (status, response) = call(&router, "UpdateProjectVisibility", body.clone()).await;
        assert_eq!(
            status, 400,
            "{case} ({body}): expected 400, got {status} with {response}"
        );
        assert_eq!(
            error_type(&response),
            expected_code,
            "{case} ({body}): expected {expected_code}, got {response}"
        );

        let project = stored_project(&router, "refusal-probe").await;
        assert_eq!(
            project["projectVisibility"].as_str(),
            Some("PRIVATE"),
            "{case}: a refused call must persist nothing, got {project}"
        );
        assert!(
            project["publicProjectAlias"].is_null(),
            "{case}: a refused call must persist no alias, got {project}"
        );
    }

    // The accepted counterpart of those rows: the ARN the project was stored with
    // resolves it, and that ARN is what the response reports.
    let (status, body) = update_visibility(&router, &arn, "PUBLIC_READ", None).await;
    assert_eq!(
        status, 200,
        "the project's own ARN: expected 200, got {status} with {body}"
    );
    assert_eq!(
        body["projectArn"].as_str(),
        Some(arn.as_str()),
        "the project's own ARN is what identifies it, got {body}"
    );
}
