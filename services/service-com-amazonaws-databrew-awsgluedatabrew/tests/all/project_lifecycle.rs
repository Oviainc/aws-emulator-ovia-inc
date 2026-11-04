// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for DataBrew projects: `CreateProject` -> `DescribeProject`, the
//! recipe a project attaches itself to, and the dataset it pins while it exists.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` with a
//! restJson1 request), so what is asserted is the HTTP status, the error code and the
//! JSON body a client sees.
//!
//! The tables below span the classes the implementation generalises over rather than
//! one recorded request: every shape a `Sample` can take (absent, each `SampleType`,
//! each published size bound and one past it, a type with no size), every form a
//! project name can take, every way a referenced resource can be missing, and both
//! sides of the delete rule (a dataset a project pins, and one nothing pins).

use serde_json::{json, Value};

use crate::databrew_harness::{label, s3_input, Emulator, Reply, ARN_PREFIX, CALLER_PRINCIPAL};

/// The role a project assumes, as `CreateProjectRequest$RoleArn` takes it (modelled
/// `len 20..2048`).
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/brew";

/// `CreateProject`: `POST /projects` (model: `smithy_query(query="operation",
/// operation="CreateProject")`).
async fn create_project(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("POST", "/projects", Some(body)).await
}

/// `DescribeProject`: `GET /projects/{Name}`.
async fn describe_project(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call("GET", &format!("/projects/{}", label(name)), None)
        .await
}

/// `DescribeRecipe` of a recipe's working draft: a created recipe holds only that
/// draft, which the `LATEST_WORKING` alias addresses.
async fn describe_working_recipe(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/recipes/{}?recipeVersion=LATEST_WORKING", label(name)),
            None,
        )
        .await
}

/// The `Message` of a modelled DataBrew error, whichever casing the protocol put it
/// on the wire with.
fn error_message(reply: &Reply) -> String {
    reply
        .body
        .get("Message")
        .or_else(|| reply.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Seed the dataset and recipe a project is created against, and answer their names.
async fn seed_dataset_and_recipe(emulator: &Emulator, suffix: &str) -> (String, String) {
    let dataset = format!("ds-{suffix}");
    let recipe = format!("rc-{suffix}");
    let created_dataset = emulator
        .call(
            "POST",
            "/datasets",
            Some(json!({"Name": dataset, "Input": s3_input("orders.csv"), "Format": "CSV"})),
        )
        .await;
    assert_eq!(
        created_dataset.status, 200,
        "the dataset a project acts upon must be created, got {} {:?}",
        created_dataset.status, created_dataset.body
    );
    let created_recipe = emulator
        .call(
            "POST",
            "/recipes",
            Some(json!({
                "Name": recipe,
                "Steps": [{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}}}],
            })),
        )
        .await;
    assert_eq!(
        created_recipe.status, 200,
        "the recipe a project develops must be created, got {} {:?}",
        created_recipe.status, created_recipe.body
    );
    (dataset, recipe)
}

/// A project reads back exactly the definition it was created with, for every shape a
/// `Sample` takes and whether or not the create carried tags.
///
/// The rows differ along the sample's type, its size and whether the create named a
/// sample at all. `SampleType` is one of `FIRST_N`, `LAST_N`, `RANDOM` and `SampleSize`
/// ranges 1..5000 (model: `smithy_query(query="shape", shape="Sample,SampleType,
/// SampleSize")`), and an unspecified sample "consists of the first 500 rows from the
/// dataset" (model: `Project$Sample`) — which is what the first two rows assert.
#[tokio::test]
async fn a_project_round_trips_every_shape_of_sample() {
    let cases: [(&str, Option<Value>, Value, Option<Value>); 7] = [
        (
            "no Sample member: the documented default",
            None,
            json!({"Size": 500, "Type": "FIRST_N"}),
            Some(json!({"team": "data"})),
        ),
        (
            "a Type with no Size: the documented default size",
            Some(json!({"Type": "RANDOM"})),
            json!({"Size": 500, "Type": "RANDOM"}),
            None,
        ),
        (
            "FIRST_N at the smallest published size",
            Some(json!({"Size": 1, "Type": "FIRST_N"})),
            json!({"Size": 1, "Type": "FIRST_N"}),
            None,
        ),
        (
            "LAST_N at the largest published size",
            Some(json!({"Size": 5000, "Type": "LAST_N"})),
            json!({"Size": 5000, "Type": "LAST_N"}),
            Some(json!({"team": "data", "env": "test"})),
        ),
        (
            "RANDOM inside the published range",
            Some(json!({"Size": 250, "Type": "RANDOM"})),
            json!({"Size": 250, "Type": "RANDOM"}),
            None,
        ),
        (
            "LAST_N at the smallest published size",
            Some(json!({"Size": 1, "Type": "LAST_N"})),
            json!({"Size": 1, "Type": "LAST_N"}),
            None,
        ),
        (
            "RANDOM at the largest published size",
            Some(json!({"Size": 5000, "Type": "RANDOM"})),
            json!({"Size": 5000, "Type": "RANDOM"}),
            None,
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, sample, expected_sample, tags)) in cases.into_iter().enumerate() {
        let (dataset, recipe) = seed_dataset_and_recipe(&emulator, &index.to_string()).await;
        let name = format!("project-{index}");
        let mut body = json!({
            "Name": name,
            "DatasetName": dataset,
            "RecipeName": recipe,
            "RoleArn": ROLE_ARN,
        });
        if let Some(sample) = sample {
            body["Sample"] = sample;
        }
        if let Some(tags) = tags.clone() {
            body["Tags"] = tags;
        }

        let created = create_project(&emulator, body).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateProject to answer 200, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("Name"),
            name,
            "case {case:?}: CreateProject must echo the name it stored, got {:?}",
            created.body
        );

        let described = describe_project(&emulator, &name).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeProject to answer 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.member("Sample"),
            expected_sample,
            "case {case:?}: expected Sample {expected_sample:?}, got {:?}",
            described.body["Sample"]
        );
        assert_eq!(
            described.text("DatasetName"),
            dataset,
            "case {case:?}: expected the stored DatasetName back, got {:?}",
            described.body["DatasetName"]
        );
        assert_eq!(
            described.text("RecipeName"),
            recipe,
            "case {case:?}: expected the stored RecipeName back, got {:?}",
            described.body["RecipeName"]
        );
        assert_eq!(
            described.text("RoleArn"),
            ROLE_ARN,
            "case {case:?}: expected the stored RoleArn back, got {:?}",
            described.body["RoleArn"]
        );
        assert_eq!(
            described.member("Tags"),
            tags.unwrap_or(Value::Null),
            "case {case:?}: expected the stored Tags back, got {:?}",
            described.body["Tags"]
        );
        assert_eq!(
            described.text("ResourceArn"),
            format!("{ARN_PREFIX}project/{name}"),
            "case {case:?}: unexpected ResourceArn, got {:?}",
            described.body["ResourceArn"]
        );
        assert!(
            described.body["CreateDate"].as_f64().unwrap_or_default() > 0.0,
            "case {case:?}: expected a server-generated CreateDate, got {:?}",
            described.body["CreateDate"]
        );
        assert_eq!(
            described.text("CreatedBy"),
            CALLER_PRINCIPAL,
            "case {case:?}: expected CreatedBy to name the calling principal, got {:?}",
            described.body["CreatedBy"]
        );
        // A project that has only been created has no modification to report.
        for member in ["LastModifiedDate", "LastModifiedBy"] {
            assert!(
                described.is_absent(member),
                "case {case:?}: an unmodified project must report no {member}, got {:?}",
                described.body[member]
            );
        }
        // Creating a project opens no interactive session for it, so none of the three
        // session members has anything to report.
        for member in ["SessionStatus", "OpenedBy", "OpenDate"] {
            assert!(
                described.is_absent(member),
                "case {case:?}: a project with no session must report no {member}, got {:?}",
                described.body[member]
            );
        }
    }
}

/// A sample size outside the published range is refused, and the refused create stores
/// nothing.
///
/// Source: `SampleSize` ranges 1..5000 (model: `smithy_query(query="shape",
/// shape="SampleSize")`), so each bound is tried one value past it.
#[tokio::test]
async fn a_sample_size_outside_the_published_range_is_refused() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "sample-bounds").await;

    for (case, size) in [("below the minimum", 0), ("above the maximum", 5001)] {
        let name = format!("size-{size}");
        let refused = create_project(
            &emulator,
            json!({
                "Name": name,
                "DatasetName": dataset,
                "RecipeName": recipe,
                "RoleArn": ROLE_ARN,
                "Sample": {"Size": size, "Type": "FIRST_N"},
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} (Size {size}): expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            describe_project(&emulator, &name).await.status,
            404,
            "case {case:?}: the refused create must store nothing"
        );
    }
}

/// A project acts upon the dataset it names and develops the recipe it names, so it can
/// only be created against resources this account already holds: a name that resolves
/// to nothing is refused with the modelled `ValidationException` (`CreateProject` models
/// no `ResourceNotFoundException`), and the refused create stores nothing.
#[tokio::test]
async fn a_project_can_only_name_a_dataset_and_a_recipe_that_exist() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "existing").await;

    let cases = [
        ("an unknown dataset", "no-such-dataset".to_string(), recipe.clone()),
        ("an unknown recipe", dataset.clone(), "no-such-recipe".to_string()),
        (
            "neither resource known",
            "no-such-dataset".to_string(),
            "no-such-recipe".to_string(),
        ),
        // A recipe name is not a dataset name and the other way round: each member is
        // looked up in its own store.
        ("the recipe's name as the dataset", recipe.clone(), recipe.clone()),
        ("the dataset's name as the recipe", dataset.clone(), dataset.clone()),
    ];

    for (index, (case, dataset_name, recipe_name)) in cases.into_iter().enumerate() {
        let name = format!("refused-{index}");
        let refused = create_project(
            &emulator,
            json!({
                "Name": name,
                "DatasetName": dataset_name,
                "RecipeName": recipe_name,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
        assert_eq!(
            describe_project(&emulator, &name).await.status,
            404,
            "case {case:?}: the refused create must store nothing"
        );
    }

    // The positive control: the same request against resources that do exist.
    let created = create_project(
        &emulator,
        json!({
            "Name": "accepted",
            "DatasetName": dataset,
            "RecipeName": recipe,
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"both resources known\": expected 200, got {} {:?}",
        created.status, created.body
    );
}

/// A project name is unique and comes from the documented character set, and a project
/// nobody created resolves nothing.
#[tokio::test]
async fn a_project_name_is_unique_and_from_the_documented_character_set() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "names").await;

    let absent = describe_project(&emulator, "ghost").await;
    assert_eq!(
        absent.status, 404,
        "case \"a project nobody created\": expected 404, got {} {:?}",
        absent.status, absent.body
    );
    assert_eq!(
        absent.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "case \"a project nobody created\": expected ResourceNotFoundException, got {:?}",
        absent.error_type
    );
    assert_eq!(
        error_message(&absent),
        "Project ghost wasn't found.",
        "case \"a project nobody created\": unexpected message, got {:?}",
        absent.body
    );

    // The documented character set: "Valid characters are alphanumeric (A-Z, a-z,
    // 0-9), hyphen (-), period (.), and space"
    // (`smithy_query(query="doc", shape="CreateProjectRequest", member="Name")`).
    // Each accepted project develops a recipe of its own, since a recipe belongs to one
    // project.
    for (index, name) in ["plain", "with-hyphen", "with.period", "with space", "MiXeD123"]
        .into_iter()
        .enumerate()
    {
        let (_, own_recipe) = seed_dataset_and_recipe(&emulator, &format!("name-{index}")).await;
        let created = create_project(
            &emulator,
            json!({
                "Name": name,
                "DatasetName": dataset,
                "RecipeName": own_recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a name in the documented set must be accepted, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            describe_project(&emulator, name).await.text("Name"),
            name,
            "case {name:?}: the project must read back under the name it was created with"
        );
    }
    for name in ["under_score", "with/slash", "with$dollar", "   "] {
        let refused = create_project(
            &emulator,
            json!({
                "Name": name,
                "DatasetName": dataset,
                "RecipeName": recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: a name outside the documented set must be refused, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {name:?}: expected ValidationException, got {:?}",
            refused.error_type
        );
    }

    // A second create of the same name conflicts, and leaves the stored project as the
    // first create wrote it.
    let (other_dataset, other_recipe) = seed_dataset_and_recipe(&emulator, "second").await;
    let duplicate = create_project(
        &emulator,
        json!({
            "Name": "plain",
            "DatasetName": other_dataset,
            "RecipeName": other_recipe,
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        duplicate.status, 409,
        "case \"duplicate project name\": expected 409, got {} {:?}",
        duplicate.status, duplicate.body
    );
    assert_eq!(
        duplicate.error_type.as_deref(),
        Some("ConflictException"),
        "case \"duplicate project name\": expected ConflictException, got {:?}",
        duplicate.error_type
    );
    assert_eq!(
        error_message(&duplicate),
        "Project plain already exists.",
        "case \"duplicate project name\": unexpected message, got {:?}",
        duplicate.body
    );
    assert_eq!(
        describe_project(&emulator, "plain").await.text("DatasetName"),
        dataset,
        "case \"duplicate project name\": the refused create must not overwrite the stored one"
    );
}

/// The recipe a project develops is attached to that project, so the recipe reports the
/// project it belongs to — and reports none before any project names it (model:
/// `Recipe$ProjectName`). A recipe belongs to ONE project, so a later project naming the
/// same recipe is refused rather than taking it over.
#[tokio::test]
async fn the_recipe_a_project_develops_reports_that_project() {
    let emulator = Emulator::new();
    let (dataset, recipe) = seed_dataset_and_recipe(&emulator, "attach").await;

    let before = describe_working_recipe(&emulator, &recipe).await;
    assert_eq!(
        before.status, 200,
        "expected DescribeRecipe to answer 200, got {} {:?}",
        before.status, before.body
    );
    assert!(
        before.is_absent("ProjectName"),
        "case \"a recipe no project names\": expected no ProjectName, got {:?}",
        before.body["ProjectName"]
    );

    let created = create_project(
        &emulator,
        json!({
            "Name": "attached",
            "DatasetName": dataset,
            "RecipeName": recipe,
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case \"the first project to name it\": expected 200, got {} {:?}",
        created.status, created.body
    );
    assert_eq!(
        describe_working_recipe(&emulator, &recipe)
            .await
            .text("ProjectName"),
        "attached",
        "case \"the first project to name it\": expected the recipe to report that project"
    );

    // Each later create that would take the recipe over is refused, whether the project
    // naming it is new or already exists, and the recipe keeps the project it belongs to.
    for (case, project) in [
        ("another project naming the same recipe", "late-comer"),
        ("the owning project's own name again", "attached"),
    ] {
        let refused = create_project(
            &emulator,
            json!({
                "Name": project,
                "DatasetName": dataset,
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
        assert_eq!(
            describe_working_recipe(&emulator, &recipe)
                .await
                .text("ProjectName"),
            "attached",
            "case {case:?}: the refused create must not re-point the recipe"
        );
        assert_eq!(
            describe_project(&emulator, "late-comer").await.status,
            404,
            "case {case:?}: the refused create must store no project"
        );
    }

    // The refusal names the recipe and the project that holds it.
    let refused = create_project(
        &emulator,
        json!({
            "Name": "late-comer",
            "DatasetName": dataset,
            "RecipeName": recipe,
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        error_message(&refused),
        format!("Recipe {recipe} is currently in use by the following projects: attached."),
        "case \"a recipe another project develops\": unexpected message, got {:?}",
        refused.body
    );

    // A project of its own recipe is accepted: it is the recipe that is taken, not the
    // dataset.
    let (_, free_recipe) = seed_dataset_and_recipe(&emulator, "attach-second").await;
    let accepted = create_project(
        &emulator,
        json!({
            "Name": "late-comer",
            "DatasetName": dataset,
            "RecipeName": free_recipe,
            "RoleArn": ROLE_ARN,
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a project of its own recipe\": expected 200, got {} {:?}",
        accepted.status, accepted.body
    );
    assert_eq!(
        describe_working_recipe(&emulator, &free_recipe)
            .await
            .text("ProjectName"),
        "late-comer",
        "case \"a project of its own recipe\": expected the recipe to report that project"
    );
}

/// A project acts upon its dataset for as long as the project exists, so that dataset
/// cannot be deleted while the project pins it: the delete answers the modelled
/// `ConflictException` (HTTP 409) and removes nothing. A dataset no project pins — even
/// one a JOB names, which resolves its dataset per run — deletes as before.
#[tokio::test]
async fn a_dataset_a_project_acts_upon_cannot_be_deleted() {
    let emulator = Emulator::new();
    let (pinned, recipe) = seed_dataset_and_recipe(&emulator, "pinned").await;
    assert_eq!(
        create_project(
            &emulator,
            json!({
                "Name": "pinning-project",
                "DatasetName": pinned,
                "RecipeName": recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await
        .status,
        200,
        "the project that pins the dataset must be created"
    );

    let refused = emulator
        .call("DELETE", &format!("/datasets/{}", label(&pinned)), None)
        .await;
    assert_eq!(
        refused.status, 409,
        "case \"a dataset a project acts upon\": expected 409, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_type.as_deref(),
        Some("ConflictException"),
        "case \"a dataset a project acts upon\": expected ConflictException, got {:?}",
        refused.error_type
    );
    assert_eq!(
        error_message(&refused),
        format!(
            "The dataset {pinned} is used in project pinning-project. To delete this \
             dataset, first delete the associated project."
        ),
        "case \"a dataset a project acts upon\": unexpected message, got {:?}",
        refused.body
    );
    assert_eq!(
        emulator
            .call("GET", &format!("/datasets/{}", label(&pinned)), None)
            .await
            .status,
        200,
        "case \"a dataset a project acts upon\": the refused delete must remove nothing"
    );

    // Several projects can act upon one dataset, and every one of them has to go before
    // the dataset can: the refusal names one of them — the first in name order — and is
    // answered again for the next until none is left.
    let (_, second_recipe) = seed_dataset_and_recipe(&emulator, "second-pinning").await;
    assert_eq!(
        create_project(
            &emulator,
            json!({
                "Name": "another-pinning-project",
                "DatasetName": pinned,
                "RecipeName": second_recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await
        .status,
        200,
        "the second project that pins the dataset must be created"
    );
    let refused_again = emulator
        .call("DELETE", &format!("/datasets/{}", label(&pinned)), None)
        .await;
    assert_eq!(
        refused_again.status, 409,
        "case \"two projects acting upon one dataset\": expected 409, got {} {:?}",
        refused_again.status, refused_again.body
    );
    assert_eq!(
        error_message(&refused_again),
        format!(
            "The dataset {pinned} is used in project another-pinning-project. To delete \
             this dataset, first delete the associated project."
        ),
        "case \"two projects acting upon one dataset\": the refusal must name a pinning \
         project, got {:?}",
        refused_again.body
    );

    // A dataset nothing references deletes, and so does one only a JOB names: a job
    // resolves its dataset when it runs, so it does not pin it.
    let (free, _) = seed_dataset_and_recipe(&emulator, "free").await;
    let (job_dataset, _) = seed_dataset_and_recipe(&emulator, "job").await;
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": "reader-job",
                    "DatasetName": job_dataset,
                    "RoleArn": ROLE_ARN,
                })),
            )
            .await
            .status,
        200,
        "the job that names a dataset must be created"
    );

    for (case, dataset) in [
        ("a dataset nothing references", &free),
        ("a dataset only a job names", &job_dataset),
    ] {
        let deleted = emulator
            .call("DELETE", &format!("/datasets/{}", label(dataset)), None)
            .await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            deleted.status, deleted.body
        );
        assert_eq!(
            deleted.text("Name"),
            *dataset,
            "case {case:?}: DeleteDataset must name the dataset it removed, got {:?}",
            deleted.body
        );
        assert_eq!(
            emulator
                .call("GET", &format!("/datasets/{}", label(dataset)), None)
                .await
                .status,
            404,
            "case {case:?}: the deleted dataset must be gone"
        );
    }

    // With every project that pinned it gone from the store, the same delete goes
    // through: it is the projects' existence that refuses it, not the dataset.
    {
        let state = emulator
            .engine()
            .state::<service_com_amazonaws_databrew_awsgluedatabrew::ComAmazonawsDatabrewAwsgluedatabrewState>();
        let mut store = state.lock();
        store.projects.remove("pinning-project");
        store.projects.remove("another-pinning-project");
    }
    let deleted = emulator
        .call("DELETE", &format!("/datasets/{}", label(&pinned)), None)
        .await;
    assert_eq!(
        deleted.status, 200,
        "case \"the pinning projects are gone\": expected 200, got {} {:?}",
        deleted.status, deleted.body
    );
}
