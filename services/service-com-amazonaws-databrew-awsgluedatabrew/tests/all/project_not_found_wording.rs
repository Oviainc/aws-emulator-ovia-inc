// SPDX-License-Identifier: CC0-1.0

//! The sentence a PROJECT miss answers its `ResourceNotFoundException` with.
//!
//! The rule pinned here: DataBrew words the not-found sentence per resource KIND, and
//! the project kind's sentence carries the definite article with the kind in lower case
//! and a terminating period — `The project <Name> wasn't found.` — with the identifier
//! the request addressed the project by as the only variable part. The sentence the
//! model documents on the error shape ("One or more resources can't be found.",
//! `smithy_query(query="shape", shape="ResourceNotFoundException")`) describes the error
//! CLASS, not the message a particular lookup answers with: that shape carries a single
//! free-text `Message` member, so the message is the only detail telling a client which
//! resource of which kind was missing.
//!
//! The table spans every operation that resolves a project by name crossed with the
//! forms a project name takes (`ProjectName` is `len 1..255`, model:
//! `smithy_query(query="operation", operation="DescribeProject,DeleteProject")`), and
//! the ways a name can miss. The status (404) and the error code are asserted on every
//! row too, so a wording fix cannot quietly move either. The per-kind sentences are
//! asserted to stay DISTINCT: the kinds are not one shared string.

use serde_json::json;

use crate::databrew_harness::{
    create_project, describe_project, label, seed_dataset_and_recipe, Emulator, Reply, ROLE_ARN,
};

/// One request: its method and its URI.
type Request = (&'static str, String);

/// One operation that resolves a project by name: what to call it in an assertion, and
/// the request that addresses `name` through it.
type ProjectLookup = (&'static str, fn(&str) -> Request);

/// The sentence every project miss answers with, as a function of the name the request
/// addressed: fixed text `The project ` / ` wasn't found.` around the request's `Name`.
fn expected_project_miss(name: &str) -> String {
    format!("The project {name} wasn't found.")
}

/// Every operation that resolves a project BY NAME, with the method and URI the model
/// binds it to (`smithy_query(query="digest")`).
fn project_lookups() -> Vec<ProjectLookup> {
    vec![
        ("DescribeProject", |name| {
            ("GET", format!("/projects/{}", label(name)))
        }),
        ("DeleteProject", |name| {
            ("DELETE", format!("/projects/{}", label(name)))
        }),
    ]
}

/// Every form a project name takes: the modelled length bound at both ends, the
/// documented character set ("alphanumeric (A-Z, a-z, 0-9), hyphen (-), period (.), and
/// space", `smithy_query(query="doc", shape="CreateProjectRequest", member="Name")`),
/// and a name a create would refuse for its characters — a lookup of it is still just a
/// miss.
fn name_forms() -> Vec<(&'static str, String)> {
    vec![
        ("a plain name", "absent-project".to_string()),
        ("digits and periods", "brew.2026.9".to_string()),
        ("one character, the modelled minimum", "x".to_string()),
        (
            "255 characters, the modelled maximum",
            "n".repeat(255),
        ),
        ("a name carrying a space", "an absent project".to_string()),
        (
            "characters a create would refuse, so nothing can hold it",
            "bad_project_name".to_string(),
        ),
    ]
}

/// Every operation that resolves a project by name answers a miss with DataBrew's
/// project sentence, over every form the name can take.
#[tokio::test]
async fn a_project_miss_is_stated_in_databrews_project_sentence() {
    let emulator = Emulator::new();

    for (operation, request) in project_lookups() {
        for (form, name) in name_forms() {
            let case = format!("{operation} / {form}");
            let (method, uri) = request(&name);
            let refused = emulator.call(method, &uri, None).await;

            assert_eq!(
                refused.status, 404,
                "case {case:?}: expected 404, got {} {:?}",
                refused.status, refused.body
            );
            assert_eq!(
                refused.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected ResourceNotFoundException, got {:?}",
                refused.error_type
            );
            let expected = expected_project_miss(&name);
            assert_eq!(
                refused.error_message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                refused.error_message()
            );
        }
    }
}

/// The sentence names the project the request addressed and nothing else: a project that
/// exists resolves, the ones around it still miss, and each miss carries its own name.
#[tokio::test]
async fn the_sentence_names_the_project_the_request_addressed() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "named").await;
    assert_eq!(
        create_project(
            &emulator,
            json!({
                "Name": "held",
                "DatasetName": dataset,
                "RecipeName": recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await
        .status,
        200,
        "the project the positive control reads back must be created"
    );

    let found = describe_project(&emulator, "held").await;
    assert_eq!(
        found.status, 200,
        "case \"a project that exists\": expected 200, got {} {:?}",
        found.status, found.body
    );

    // A name a project held until it was deleted misses the same way a name nothing
    // ever held does.
    let (other_dataset, other_recipe) = seed_dataset_and_recipe(&emulator, "named-two").await;
    assert_eq!(
        create_project(
            &emulator,
            json!({
                "Name": "once-held",
                "DatasetName": other_dataset,
                "RecipeName": other_recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await
        .status,
        200,
        "the project deleted to make a once-held name must be created"
    );
    assert_eq!(
        emulator
            .call("DELETE", "/projects/once-held", None)
            .await
            .status,
        200,
        "the project deleted to make a once-held name must delete"
    );

    for (case, name) in [
        ("a name nothing ever held", "never-held"),
        ("a name a project held until it was deleted", "once-held"),
        ("a name another kind of resource holds", dataset.as_str()),
        ("the held name with one character appended", "held2"),
    ] {
        let refused = describe_project(&emulator, name).await;
        let expected = expected_project_miss(name);
        assert_eq!(
            refused.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }
}

/// The not-found sentence belongs to the resource KIND, so the kinds stay DISTINCT: one
/// identifier missing as a project, as a dataset and as a schedule answers three
/// different sentences, none of them collapsed onto a shared string.
#[tokio::test]
async fn each_resource_kind_keeps_its_own_not_found_sentence() {
    let emulator = Emulator::new();
    let shared = "shared-name";

    let as_project = describe_project(&emulator, shared).await;
    let as_dataset = emulator
        .call("GET", &format!("/datasets/{shared}"), None)
        .await;
    let as_schedule = emulator
        .call("GET", &format!("/schedules/{shared}"), None)
        .await;

    for (case, reply, expected) in [
        (
            "the project kind",
            &as_project,
            expected_project_miss(shared),
        ),
        (
            "the dataset kind",
            &as_dataset,
            format!("Dataset {shared} wasn't found."),
        ),
        (
            "the schedule kind",
            &as_schedule,
            format!("Schedule {shared} not found"),
        ),
    ] {
        let reply: &Reply = reply;
        assert_eq!(
            reply.status, 404,
            "case {case:?}: expected 404, got {} {:?}",
            reply.status, reply.body
        );
        assert_eq!(
            reply.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            reply.error_message()
        );
    }

    assert_ne!(
        as_project.error_message(),
        as_dataset.error_message(),
        "case \"project against dataset\": each kind keeps its own sentence, got {:?} twice",
        as_project.error_message()
    );
    assert_ne!(
        as_project.error_message(),
        as_schedule.error_message(),
        "case \"project against schedule\": each kind keeps its own sentence, got {:?} twice",
        as_project.error_message()
    );
}
