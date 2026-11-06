// SPDX-License-Identifier: CC0-1.0

//! The `ConflictException` message `CreateProject` answers with when the recipe the
//! create names is already developed in another project.
//!
//! The rule pinned here: DataBrew words this refusal with its own per-operation
//! sentence, built from the request and the store — `The recipe <RecipeName> is already
//! associated with project <ProjectName>. Choose a recipe that isn't already part of a
//! project.` — where `<RecipeName>` is the recipe the create named and `<ProjectName>`
//! is the ONE project in the store that already develops it. A recipe belongs to at most
//! one project (model: `Recipe$ProjectName` is a single name,
//! `smithy_query(query="shape", shape="Recipe")`), so there is never more than one
//! holder to report. The sentence the model documents on the error SHAPE ("Updating or
//! deleting a resource can cause an inconsistent state.",
//! `smithy_query(query="shape", shape="ConflictException")`) describes the error CLASS,
//! and the shape carries no per-resource member, so the message is the only detail a
//! client can tell this 409 by.
//!
//! The table spans the forms the two variable parts take rather than one recorded pair:
//! the modelled length bounds of both names (`RecipeName` and `ProjectName` are each
//! `len 1..255`, `smithy_query(query="shape", shape="RecipeName,ProjectName")`), names
//! carrying spaces, periods and digits (characters DataBrew documents for a resource
//! name). Status (409) and error code are asserted on every row too, so a message fix
//! cannot quietly change either, and the refusal is shown to store nothing and to
//! re-point nothing.

use serde_json::json;

use crate::databrew_harness::{
    create_project, describe_project, describe_working_recipe, label, s3_input, Emulator, Reply,
    ROLE_ARN,
};

/// The refusal sentence DataBrew answers this create with, as a template over the two
/// names: what the test expects is computed the same way a reader of the message is
/// expected to read it.
fn expected_message(recipe: &str, project: &str) -> String {
    format!(
        "The recipe {recipe} is already associated with project {project}. Choose a \
         recipe that isn't already part of a project."
    )
}

/// Create the dataset every project in a case acts upon.
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

/// Create a recipe: every create must succeed for the case to mean anything.
async fn seed_recipe(emulator: &Emulator, case: &str, recipe: &str) {
    let created = emulator
        .call(
            "POST",
            "/recipes",
            Some(json!({
                "Name": recipe,
                "Steps": [{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}],
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the recipe must be created, got {} {:?}",
        created.status, created.body
    );
}

/// Create the project that attaches `recipe` to itself.
async fn seed_owning_project(
    emulator: &Emulator,
    case: &str,
    project: &str,
    dataset: &str,
    recipe: &str,
) {
    let created = create_project(
        emulator,
        json!({
            "Name": project,
            "DatasetName": dataset,
            "RecipeName": recipe,
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case:?}: the owning project must be created, got {} {:?}",
        created.status, created.body
    );
}

/// `DeleteProject`: `DELETE /projects/{Name}`.
async fn delete_project(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("DELETE", &format!("/projects/{}", label(name)), None)
        .await
}

/// Assert one refused create: the modelled status and code, the per-operation sentence
/// over this row's two names, and that nothing moved.
async fn assert_refused(
    emulator: &Emulator,
    case: &str,
    attempted: &str,
    recipe: &str,
    owner: &str,
) {
    let refused = create_project(
        emulator,
        json!({
            "Name": attempted,
            "DatasetName": "shared-dataset",
            "RecipeName": recipe,
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
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
    let expected = expected_message(recipe, owner);
    assert_eq!(
        refused.error_message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        refused.error_message()
    );
    assert_eq!(
        describe_working_recipe(emulator, recipe)
            .await
            .text("ProjectName"),
        owner,
        "case {case:?}: the refused create must not re-point the recipe"
    );
}

/// Every (recipe name, owning project name, attempted project name) triple the rule has
/// to carry through: the message is built from the request and the store, so each row
/// answers its OWN sentence.
#[tokio::test]
async fn the_refusal_names_the_recipe_and_the_project_that_holds_it() {
    // Both names are at most 255 characters and may carry alphanumerics, hyphens,
    // periods and spaces, so the rows span the bounds and those characters.
    let cases: [(&str, String, String, String); 7] = [
        (
            "plain names",
            "uppercase-dept".to_string(),
            "orders-cleanup".to_string(),
            "orders-cleanup-two".to_string(),
        ),
        (
            "one character each, the modelled minimum",
            "r".to_string(),
            "p".to_string(),
            "q".to_string(),
        ),
        (
            "a 255-character recipe name, the modelled maximum",
            "r".repeat(255),
            "long-recipe-project".to_string(),
            "long-recipe-late-comer".to_string(),
        ),
        (
            "a 255-character owning project name, the modelled maximum",
            "recipe-of-a-long-project".to_string(),
            "p".repeat(255),
            "late-comer".to_string(),
        ),
        (
            "a 255-character attempted project name, the modelled maximum",
            "recipe-of-a-long-late-comer".to_string(),
            "short-owner".to_string(),
            "l".repeat(255),
        ),
        (
            "names carrying spaces",
            "upper case dept".to_string(),
            "quarterly cleanup".to_string(),
            "quarterly cleanup two".to_string(),
        ),
        (
            "names carrying periods and digits",
            "sales.2026.q1".to_string(),
            "cleanup.v2".to_string(),
            "cleanup.v3".to_string(),
        ),
    ];

    for (case, recipe, owner, attempted) in cases {
        let emulator = Emulator::new();
        seed_dataset(&emulator, case, "shared-dataset").await;
        seed_recipe(&emulator, case, &recipe).await;
        seed_owning_project(&emulator, case, &owner, "shared-dataset", &recipe).await;

        assert_refused(&emulator, case, &attempted, &recipe, &owner).await;
        assert_eq!(
            describe_project(&emulator, &attempted).await.status,
            404,
            "case {case:?}: the refused create must store no project"
        );
    }
}

/// The project the message names is the one that actually develops the recipe
/// addressed: two recipes, each attached to its own project, answer two different
/// sentences, and the project of the OTHER recipe is named in neither.
#[tokio::test]
async fn each_recipe_reports_its_own_project() {
    let emulator = Emulator::new();
    seed_dataset(&emulator, "two pairs", "shared-dataset").await;
    for (recipe, owner) in [("rc-one", "project-one"), ("rc-two", "project-two")] {
        seed_recipe(&emulator, owner, recipe).await;
        seed_owning_project(&emulator, owner, owner, "shared-dataset", recipe).await;
    }

    for (recipe, owner, other_owner) in [
        ("rc-one", "project-one", "project-two"),
        ("rc-two", "project-two", "project-one"),
    ] {
        assert_refused(&emulator, recipe, "late-comer", recipe, owner).await;
        let refused = create_project(
            &emulator,
            json!({
                "Name": "late-comer",
                "DatasetName": "shared-dataset",
                "RecipeName": recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await;
        assert!(
            !refused.error_message().contains(other_owner),
            "case {recipe:?}: the refusal must not name {other_owner:?}, got {:?}",
            refused.error_message()
        );
    }
}

/// The remediation the sentence prints is the one that works: a recipe no project holds
/// is accepted, and freeing the named project's recipe (by deleting that project) makes
/// the very create that was refused go through.
#[tokio::test]
async fn an_unattached_recipe_is_what_clears_the_refusal() {
    let emulator = Emulator::new();
    let case = "the remediation the sentence names";
    seed_dataset(&emulator, case, "shared-dataset").await;
    seed_recipe(&emulator, case, "held-recipe").await;
    seed_recipe(&emulator, case, "free-recipe").await;
    seed_owning_project(&emulator, case, "holder", "shared-dataset", "held-recipe").await;

    assert_refused(&emulator, case, "late-comer", "held-recipe", "holder").await;

    // Another recipe, which no project holds, is accepted for the same project name:
    // what is refused is the recipe, not the create.
    let accepted = create_project(
        &emulator,
        json!({
            "Name": "late-comer",
            "DatasetName": "shared-dataset",
            "RecipeName": "free-recipe",
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: a recipe no project holds must be accepted, got {} {:?}",
        accepted.status, accepted.body
    );

    // Deleting the project the sentence named releases its recipe, so the originally
    // refused pairing is accepted afterwards.
    let deleted = delete_project(&emulator, "holder").await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: the named project must delete, got {} {:?}",
        deleted.status, deleted.body
    );
    let accepted = create_project(
        &emulator,
        json!({
            "Name": "second-chance",
            "DatasetName": "shared-dataset",
            "RecipeName": "held-recipe",
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case {case:?}: the released recipe must be accepted, got {} {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        describe_working_recipe(&emulator, "held-recipe")
            .await
            .text("ProjectName"),
        "second-chance",
        "case {case:?}: the released recipe must report the project that took it"
    );
}
