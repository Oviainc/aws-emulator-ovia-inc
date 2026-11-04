// SPDX-License-Identifier: CC0-1.0

//! The `ConflictException` message `DeleteDataset` answers with while a project acts
//! upon the dataset.
//!
//! The rule pinned here: DataBrew words this refusal with its own per-resource
//! sentence, built from the request and the store — `The dataset <Name> is used in
//! project <ProjectName>. To delete this dataset, first delete the associated
//! project.` — where `<Name>` is the dataset the request addressed and `<ProjectName>`
//! is a project in the store whose `DatasetName` equals it. The sentence the model
//! documents on the error SHAPE ("Updating or deleting a resource can cause an
//! inconsistent state.", `smithy_query(query="shape", shape="ConflictException")`)
//! describes the error CLASS, and the shape carries no per-resource member, so the
//! message is the only detail a client can tell this 409 by.
//!
//! The table spans the forms the two variable parts take rather than one recorded
//! pair: the modelled length bounds of both names (`DatasetName` and `ProjectName` are
//! each `len 1..255`, `smithy_query(query="shape", shape="DatasetName,ProjectName")`),
//! names carrying a space and a period (characters DataBrew documents for a resource
//! name), and a dataset several projects act upon. Status (409) and error code are
//! asserted on every row too, so a message fix cannot quietly change either.

use serde_json::json;

use crate::databrew_harness::{label, s3_input, Emulator};

/// The role a project assumes, as `CreateProjectRequest$RoleArn` takes it (modelled
/// `len 20..2048`). The role itself is not what these cases are about.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/brew";

/// The refusal sentence DataBrew answers this delete with, as a template over the two
/// names: what the test expects is computed the same way a reader of the message is
/// expected to read it.
fn expected_message(dataset: &str, project: &str) -> String {
    format!(
        "The dataset {dataset} is used in project {project}. To delete this dataset, \
         first delete the associated project."
    )
}

/// Create the dataset a project will act upon.
async fn seed_dataset(emulator: &Emulator, case: &str, dataset: &str) {
    let created = emulator
        .call(
            "POST",
            "/datasets",
            Some(json!({"Name": dataset, "Input": s3_input("orders.csv"), "Format": "CSV"})),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the dataset must be created, got {} {:?}",
        created.status, created.body
    );
}

/// Create a recipe and a project that develops it over an existing dataset: every
/// create must succeed for the case to mean anything.
async fn seed_project_over(emulator: &Emulator, case: &str, dataset: &str, project: &str) {
    // A recipe is developed in ONE project, so each project needs its own; the name is
    // derived from the project's and clipped so it stays inside `RecipeName`'s modelled
    // `len 1..255` even for a project named at ITS maximum.
    let stem = &project[..project.len().min(200)];
    let recipe = format!("recipe-for-{stem}");
    for (what, uri, body) in [
        (
            "recipe",
            "/recipes",
            json!({
                "Name": recipe,
                "Steps": [{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}],
            }),
        ),
        (
            "project",
            "/projects",
            json!({
                "Name": project,
                "DatasetName": dataset,
                "RecipeName": recipe,
                "RoleArn": ROLE_ARN,
                "Sample": {"Size": 500, "Type": "FIRST_N"},
            }),
        ),
    ] {
        let created = emulator.call("POST", uri, Some(body)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the {what} must be created, got {} {:?}",
            created.status, created.body
        );
    }
}

/// Create a dataset and one project that acts upon it.
async fn seed_pinned_dataset(emulator: &Emulator, case: &str, dataset: &str, project: &str) {
    seed_dataset(emulator, case, dataset).await;
    seed_project_over(emulator, case, dataset, project).await;
}

/// `DELETE /datasets/{Name}`.
async fn delete_dataset(emulator: &Emulator, name: &str) -> crate::databrew_harness::Reply {
    emulator
        .call("DELETE", &format!("/datasets/{}", label(name)), None)
        .await
}

/// Every (dataset name, project name) pair the rule has to carry through: the message
/// is built from the request and the store, so each row answers its OWN sentence.
#[tokio::test]
async fn the_refusal_names_the_dataset_and_the_project_that_holds_it() {
    // Both names are at most 255 characters and may carry alphanumerics, hyphens,
    // periods and spaces, so the rows span the bounds and those characters.
    let cases: [(&str, String, String); 6] = [
        (
            "plain names",
            "sales-orders".to_string(),
            "orders-cleanup".to_string(),
        ),
        (
            "one character each, the modelled minimum",
            "d".to_string(),
            "p".to_string(),
        ),
        (
            "a 255-character dataset name, the modelled maximum",
            "d".repeat(255),
            "long-dataset-project".to_string(),
        ),
        (
            "a 255-character project name, the modelled maximum",
            "dataset-of-a-long-project".to_string(),
            "p".repeat(255),
        ),
        (
            "names carrying a space",
            "quarterly sales".to_string(),
            "quarterly cleanup".to_string(),
        ),
        (
            "names carrying periods and digits",
            "sales.2026.q1".to_string(),
            "cleanup.v2".to_string(),
        ),
    ];

    for (case, dataset, project) in cases {
        let emulator = Emulator::new();
        seed_pinned_dataset(&emulator, case, &dataset, &project).await;

        let refused = delete_dataset(&emulator, &dataset).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ConflictException"),
            "case {case:?}: expected ConflictException, got {:?}",
            refused.error_type
        );
        let expected = expected_message(&dataset, &project);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
        assert_eq!(
            emulator
                .call("GET", &format!("/datasets/{}", label(&dataset)), None)
                .await
                .status,
            200,
            "case {case:?}: the refused delete must remove nothing"
        );
    }
}

/// The project the message names is the one that actually pins the dataset addressed:
/// two datasets, each with its own project, answer two different sentences, and the
/// project of the OTHER dataset is named in neither.
#[tokio::test]
async fn each_dataset_reports_its_own_project() {
    let emulator = Emulator::new();
    seed_pinned_dataset(&emulator, "first pair", "ds-one", "project-one").await;
    seed_pinned_dataset(&emulator, "second pair", "ds-two", "project-two").await;

    for (dataset, project, other_project) in [
        ("ds-one", "project-one", "project-two"),
        ("ds-two", "project-two", "project-one"),
    ] {
        let refused = delete_dataset(&emulator, dataset).await;
        let expected = expected_message(dataset, project);
        assert_eq!(
            refused.error_message(),
            expected,
            "case {dataset:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
        assert!(
            !refused.error_message().contains(other_project),
            "case {dataset:?}: the refusal must not name {other_project:?}, got {:?}",
            refused.error_message()
        );
    }
}

/// A dataset several projects act upon is refused with the singular sentence naming a
/// project that really holds it, and the remediation it prints is the one that works:
/// removing the project it names gets the caller one step further, and with the last
/// project gone the delete goes through.
///
/// `DeleteProject` is still scaffolded in this crate, so the project is removed from
/// the one shared store the handlers serve from — the same state a delete would clear.
#[tokio::test]
async fn removing_the_named_project_is_what_clears_the_refusal() {
    let emulator = Emulator::new();
    let dataset = "shared-dataset";
    let projects = ["beta-cleanup", "alpha-cleanup", "gamma-cleanup"];
    seed_dataset(&emulator, "three projects over one dataset", dataset).await;
    for project in projects {
        seed_project_over(&emulator, project, dataset, project).await;
    }

    let mut remaining: Vec<&str> = projects.to_vec();
    while !remaining.is_empty() {
        let case = format!("{} projects left", remaining.len());
        let refused = delete_dataset(&emulator, dataset).await;
        assert_eq!(
            refused.status, 409,
            "case {case:?}: expected 409, got {} {:?}",
            refused.status, refused.body
        );
        // The sentence is singular, so exactly one of the projects still holding the
        // dataset is named — whichever it is, it is a project the store really pins by.
        let named = remaining
            .iter()
            .copied()
            .find(|project| refused.error_message() == expected_message(dataset, project));
        let named = named.unwrap_or_else(|| {
            panic!(
                "case {case:?}: the refusal must name one of {remaining:?}, got {:?}",
                refused.error_message()
            )
        });

        {
            let state = emulator
                .engine()
                .state::<service_com_amazonaws_databrew_awsgluedatabrew::ComAmazonawsDatabrewAwsgluedatabrewState>();
            assert!(
                state.lock().projects.remove(named).is_some(),
                "case {case:?}: the refusal named {named:?}, which the store must hold"
            );
        }
        remaining.retain(|project| *project != named);
    }

    let deleted = delete_dataset(&emulator, dataset).await;
    assert_eq!(
        deleted.status, 200,
        "case \"every project gone\": expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        deleted.text("Name"),
        dataset,
        "case \"every project gone\": DeleteDataset must name the dataset it removed, got {:?}",
        deleted.body
    );
    assert_eq!(
        emulator
            .call("GET", &format!("/datasets/{dataset}"), None)
            .await
            .status,
        404,
        "case \"every project gone\": the deleted dataset must be gone"
    );
}
