// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteProject`: the delete removes the project record, so the
//! reads that follow resolve nothing, and it releases what the project held while it
//! existed (the recipe it developed, the dataset it acted upon).
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` with a
//! restJson1 request), so what is asserted is the HTTP status, the error code and the
//! JSON body a client sees.
//!
//! The tables below span the class the implementation generalises over rather than one
//! recorded request: every shape a project can have when it is deleted (its sample, its
//! tags, the form and length of its name, whether the recipe it develops has been
//! published), and every reference a project can be the target of.

use serde_json::{json, Value};

use crate::databrew_harness::{
    create_project, describe_project, describe_working_recipe, label, seed_dataset_and_recipe,
    Emulator, Reply, ROLE_ARN,
};

/// `DeleteProject`: `DELETE /projects/{Name}` (model: `smithy_query(query="operation",
/// operation="DeleteProject")`).
async fn delete_project(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("DELETE", &format!("/projects/{}", label(name)), None)
        .await
}

/// The body that creates `name` against `dataset`/`recipe`, carrying `sample` and
/// `tags` only when the row asks for them.
fn project_body(
    name: &str,
    dataset: &str,
    recipe: &str,
    sample: Option<Value>,
    tags: Option<Value>,
) -> Value {
    let mut body = json!({
        "Name": name,
        "DatasetName": dataset,
        "RecipeName": recipe,
        "RoleArn": ROLE_ARN,
    });
    if let Some(sample) = sample {
        body["Sample"] = sample;
    }
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    body
}

/// One row of the delete table: the project to create and delete, and the state its
/// recipe is in when the delete runs.
struct DeleteCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The project name, which is also what the delete and the misses report.
    name: String,
    /// The `Sample` the create carries, or none at all.
    sample: Option<Value>,
    /// The `Tags` the create carries, or none at all.
    tags: Option<Value>,
    /// Whether the recipe the project develops is published before the delete.
    publish_recipe_first: bool,
}

/// A deleted project echoes the name it deleted and leaves nothing behind: the reads
/// that follow resolve nothing, and a second delete of the same name is a miss rather
/// than a second success.
///
/// The rows differ along the shape a project has when it is deleted — whether the
/// create named a `Sample`, whether it carried tags, the form and the length of the
/// project name (`ProjectName` is `len 1..255` over "alphanumeric (A-Z, a-z, 0-9),
/// hyphen (-), period (.), and space", model: `smithy_query(query="operation",
/// operation="DeleteProject")` and `CreateProjectRequest$Name`), and whether the recipe
/// the project develops has been published.
#[tokio::test]
async fn a_deleted_project_resolves_nothing_afterwards() {
    let cases = [
        DeleteCase {
            case: "no Sample and no tags",
            name: "plain".to_string(),
            sample: None,
            tags: None,
            publish_recipe_first: false,
        },
        DeleteCase {
            case: "an explicit Sample and tags",
            name: "sampled.project".to_string(),
            sample: Some(json!({"Size": 10, "Type": "LAST_N"})),
            tags: Some(json!({"team": "data"})),
            publish_recipe_first: false,
        },
        DeleteCase {
            case: "a name that carries a space",
            name: "my project".to_string(),
            sample: None,
            tags: Some(json!({"env": "test"})),
            publish_recipe_first: false,
        },
        DeleteCase {
            case: "a name at the longest published length",
            name: "a".repeat(255),
            sample: None,
            tags: None,
            publish_recipe_first: false,
        },
        DeleteCase {
            case: "a recipe that has been published",
            name: "published-recipe-project".to_string(),
            sample: Some(json!({"Size": 1, "Type": "FIRST_N"})),
            tags: None,
            publish_recipe_first: true,
        },
    ];

    for (index, row) in cases.into_iter().enumerate() {
        let DeleteCase {
            case,
            name,
            sample,
            tags,
            publish_recipe_first,
        } = row;
        let name = name.as_str();
        let emulator = Emulator::new();
        let (dataset, recipe) = seed_dataset_and_recipe(&emulator, &format!("del-{index}")).await;
        if publish_recipe_first {
            let published = emulator
                .call(
                    "POST",
                    &format!("/recipes/{}/publishRecipe", label(&recipe)),
                    Some(json!({})),
                )
                .await;
            assert_eq!(
                published.status, 200,
                "case {case:?}: the recipe must publish, got {} {:?}",
                published.status, published.body
            );
        }
        let created = create_project(
            &emulator,
            project_body(name, &dataset, &recipe, sample, tags),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the project must be created, got {} {:?}",
            created.status, created.body
        );

        let deleted = delete_project(&emulator, name).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("Name"),
            name,
            "case {case:?}: expected the deleted project's name, got {:?}",
            deleted.body
        );

        let gone = describe_project(&emulator, name).await;
        assert_eq!(
            gone.status, 404,
            "case {case:?}: a deleted project must resolve nothing, got {} {:?}",
            gone.status, gone.body
        );
        assert_eq!(
            gone.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            gone.error_type
        );
        assert_eq!(
            gone.error_message(),
            format!("The project {name} wasn't found."),
            "case {case:?}: the miss must name the project, got {:?}",
            gone.body
        );

        let deleted_again = delete_project(&emulator, name).await;
        assert_eq!(
            deleted_again.status, 404,
            "case {case:?}: the second delete must be a miss, got {} {:?}",
            deleted_again.status, deleted_again.body
        );
        assert_eq!(
            deleted_again.error_message(),
            format!("The project {name} wasn't found."),
            "case {case:?}: the second delete must name the project, got {:?}",
            deleted_again.body
        );

        // The recipe the project developed is a resource of its own: it outlives the
        // project, and reports no project once the one that developed it is gone
        // (model: `Recipe$ProjectName`).
        let described_recipe = describe_working_recipe(&emulator, &recipe).await;
        assert_eq!(
            described_recipe.status, 200,
            "case {case:?}: the recipe must outlive the project, got {} {:?}",
            described_recipe.status, described_recipe.body
        );
        assert!(
            described_recipe.is_absent("ProjectName"),
            "case {case:?}: expected the released recipe to report no ProjectName, got {:?}",
            described_recipe.body
        );
    }
}

/// A project nobody created resolves nothing, and the refused delete removes nothing:
/// every other project this account holds is still there.
///
/// The rows differ along the way a name can miss — a name nothing ever held, a name
/// another KIND of resource holds (a dataset and a recipe of that name exist), and a
/// name a project held until it was deleted.
#[tokio::test]
async fn deleting_a_project_that_does_not_exist_removes_nothing() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "miss").await;
    let (_, other_recipe) = seed_dataset_and_recipe(&emulator, "miss-two").await;

    for (name, recipe_name) in [("kept-one", &recipe), ("kept-two", &other_recipe)] {
        assert_eq!(
            create_project(
                &emulator,
                project_body(name, &dataset, recipe_name, None, None)
            )
            .await
            .status,
            200,
            "the project {name} must be created"
        );
    }
    assert_eq!(
        delete_project(&emulator, "kept-two").await.status,
        200,
        "the project deleted to make a once-held name must delete"
    );

    for (case, name) in [
        ("a name nothing ever held", "never-held"),
        ("a name another kind of resource holds", dataset.as_str()),
        ("a name a project held until it was deleted", "kept-two"),
    ] {
        let refused = delete_project(&emulator, name).await;
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
        assert_eq!(
            refused.error_message(),
            format!("The project {name} wasn't found."),
            "case {case:?}: the miss must name the project, got {:?}",
            refused.body
        );
        let kept = describe_project(&emulator, "kept-one").await;
        assert_eq!(
            kept.status, 200,
            "case {case:?}: the refused delete must remove nothing, got {} {:?}",
            kept.status, kept.body
        );
    }
}

/// Deleting a project releases what it held: the dataset it acted upon can be deleted
/// afterwards, and the recipe it developed can be taken up by a new project.
#[tokio::test]
async fn a_deleted_project_releases_its_dataset_and_its_recipe() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "release").await;
    assert_eq!(
        create_project(
            &emulator,
            project_body("holder", &dataset, &recipe, None, None)
        )
        .await
        .status,
        200,
        "the project that holds the dataset and the recipe must be created"
    );

    // While the project exists it pins both: the dataset cannot be deleted and the
    // recipe cannot be taken over.
    let refused_dataset = emulator
        .call("DELETE", &format!("/datasets/{}", label(&dataset)), None)
        .await;
    assert_eq!(
        refused_dataset.status, 409,
        "case \"the dataset of a live project\": expected 409, got {} {:?}",
        refused_dataset.status, refused_dataset.body
    );
    let refused_takeover = create_project(
        &emulator,
        project_body("taker", &dataset, &recipe, None, None),
    )
    .await;
    assert_eq!(
        refused_takeover.status, 409,
        "case \"the recipe of a live project\": expected 409, got {} {:?}",
        refused_takeover.status, refused_takeover.body
    );

    assert_eq!(
        delete_project(&emulator, "holder").await.status,
        200,
        "the project must delete"
    );

    let taken_over = create_project(
        &emulator,
        project_body("taker", &dataset, &recipe, None, None),
    )
    .await;
    assert_eq!(
        taken_over.status, 200,
        "case \"the recipe of a deleted project\": expected 200, got {} {:?}",
        taken_over.status, taken_over.body
    );
    let described_recipe = describe_working_recipe(&emulator, &recipe).await;
    assert_eq!(
        described_recipe.member("ProjectName"),
        json!("taker"),
        "case \"the recipe of a deleted project\": expected the new project to be \
         reported, got {:?}",
        described_recipe.body
    );

    assert_eq!(
        delete_project(&emulator, "taker").await.status,
        200,
        "the project that took the recipe over must delete"
    );
    let deleted_dataset = emulator
        .call("DELETE", &format!("/datasets/{}", label(&dataset)), None)
        .await;
    assert_eq!(
        deleted_dataset.status, 200,
        "case \"the dataset of a deleted project\": expected 200, got {} {:?}",
        deleted_dataset.status, deleted_dataset.body
    );
}

/// Every recipe that names the deleted project is released, not only the one the
/// project record points back at: no recipe is left reporting a project that resolves
/// nothing (model: `Recipe$ProjectName`).
///
/// The second attachment is seeded straight into the shared store, because the wire
/// path that attaches a recipe (`CreateProject`) attaches exactly one.
#[tokio::test]
async fn deleting_a_project_releases_every_recipe_that_names_it() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "release-all").await;
    let (_, second_recipe) = seed_dataset_and_recipe(&emulator, "release-all-two").await;
    assert_eq!(
        create_project(
            &emulator,
            project_body("many-recipes", &dataset, &recipe, None, None)
        )
        .await
        .status,
        200,
        "the project the recipes name must be created"
    );
    {
        let state = emulator
            .engine()
            .state::<service_com_amazonaws_databrew_awsgluedatabrew::ComAmazonawsDatabrewAwsgluedatabrewState>();
        let mut store = state.lock();
        store
            .recipes
            .get_mut(&second_recipe)
            .expect("the seeded recipe is stored")
            .project_name = Some("many-recipes".to_string());
    }

    assert_eq!(
        delete_project(&emulator, "many-recipes").await.status,
        200,
        "the project every recipe names must delete"
    );

    for (case, recipe_name) in [
        ("the recipe the project record named", &recipe),
        ("a recipe the project record did not name", &second_recipe),
    ] {
        let described = describe_working_recipe(&emulator, recipe_name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: the recipe must outlive the project, got {} {:?}",
            described.status, described.body
        );
        assert!(
            described.is_absent("ProjectName"),
            "case {case:?}: expected no ProjectName once the project is gone, got {:?}",
            described.body
        );
    }
}

/// A job that names a project resolves it when it RUNS, so it does not hold the
/// project: the delete goes through, and the job's own definition is untouched by it.
#[tokio::test]
async fn a_job_that_names_a_project_does_not_hold_it() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "job-ref").await;
    assert_eq!(
        create_project(
            &emulator,
            project_body("job-project", &dataset, &recipe, None, None)
        )
        .await
        .status,
        200,
        "the project a job names must be created"
    );
    let created_job = emulator
        .call(
            "POST",
            "/recipeJobs",
            Some(json!({
                "Name": "project-job",
                "ProjectName": "job-project",
                "RoleArn": ROLE_ARN,
                "Outputs": [{"Location": {"Bucket": "brew-bucket", "Key": "out/"}}],
            })),
        )
        .await;
    assert_eq!(
        created_job.status, 200,
        "the job that names the project must be created, got {} {:?}",
        created_job.status, created_job.body
    );

    let deleted = delete_project(&emulator, "job-project").await;
    assert_eq!(
        deleted.status, 200,
        "case \"a project only a job names\": expected 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let described_job = emulator.call("GET", "/jobs/project-job", None).await;
    assert_eq!(
        described_job.status, 200,
        "case \"a project only a job names\": the job must outlive the project, got {} {:?}",
        described_job.status, described_job.body
    );
    assert_eq!(
        described_job.member("ProjectName"),
        json!("job-project"),
        "case \"a project only a job names\": the job must report the definition it was \
         created with, got {:?}",
        described_job.body
    );
}
