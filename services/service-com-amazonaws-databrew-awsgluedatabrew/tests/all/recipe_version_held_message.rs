// SPDX-License-Identifier: CC0-1.0

//! The ONE sentence every "this recipe version is still held" refusal states.
//!
//! A recipe version a project develops or a job runs cannot be deleted, and the refusal
//! names the pair it is about and the resource holding it:
//!
//! ```text
//! Recipe <recipe-name> version <version-identifier> are used by <project|job> <holder>.
//! ```
//!
//! Its fixed text is `Recipe `, ` version `, ` are used by <holder kind> ` and the
//! terminating period; its variable parts are the recipe name the request addressed, the
//! version identifier that request named (a number or the `LATEST_WORKING` alias, exactly
//! as sent) and the name of the holder, read from emulator state.
//!
//! Both operations that delete a recipe version state it: `BatchDeleteRecipeVersion`
//! reports it as the `ErrorMessage` of the version's `Errors` item beside `ErrorCode`
//! "409" on a 200 response (model: `RecipeVersionErrorDetail`, whose `ErrorCode` is
//! `pattern ^[1-5][0-9][0-9]$`), and `DeleteRecipeVersion` reports it as the `Message` of
//! its 409 `ConflictException`.
//!
//! The table spans what the sentence generalises over: the recipe name (including a name
//! carrying a space, which `RecipeName` — `length 1..255`, no pattern — allows), the kind
//! of holder, the holder's name, how many versions the recipe holds, and which identifier
//! form the request addressed the held version with.

use serde_json::{json, Value};

use crate::databrew_harness::{create_project, label, Emulator, Reply, ROLE_ARN};

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// What holds the version a row asks to delete, and under which name.
enum Holder {
    /// A project developing the recipe: it holds the recipe's working version.
    Project(&'static str),
    /// A job whose recipe reference names this version, or, when the reference carries
    /// no version, the recipe's latest published one.
    Job {
        name: &'static str,
        reference: Option<&'static str>,
    },
}

impl Holder {
    /// The holder kind the sentence names, which is the resource kind itself.
    fn kind(&self) -> &'static str {
        match self {
            Holder::Project(_) => "project",
            Holder::Job { .. } => "job",
        }
    }

    /// The holder name the sentence names.
    fn name(&self) -> &'static str {
        match self {
            Holder::Project(name) => name,
            Holder::Job { name, .. } => name,
        }
    }
}

/// One row: a recipe, what holds one of its versions, and the identifier the delete
/// request addresses that version with.
struct HeldCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The recipe the request addresses.
    recipe: &'static str,
    /// How many publishes the recipe has had, so a row can address a published version
    /// by number and vary the draft's own number.
    publishes: usize,
    /// What holds the version.
    holder: Holder,
    /// The `RecipeVersion` the delete request names, as sent.
    requested: &'static str,
}

/// `CreateRecipe`: `POST /recipes`.
async fn create_recipe(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "POST",
            "/recipes",
            Some(json!({
                "Name": name,
                "Steps": [{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}],
            })),
        )
        .await
}

/// `PublishRecipe`: `POST /recipes/{Name}/publishRecipe`.
async fn publish_recipe(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/recipes/{}/publishRecipe", label(name)),
            Some(json!({})),
        )
        .await
}

/// `BatchDeleteRecipeVersion`: `POST /recipes/{Name}/batchDeleteRecipeVersion`.
async fn batch_delete(emulator: &Emulator, name: &str, versions: &[&str]) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/recipes/{}/batchDeleteRecipeVersion", label(name)),
            Some(json!({"RecipeVersions": versions})),
        )
        .await
}

/// `DeleteRecipeVersion`: `DELETE /recipes/{Name}/recipeVersion/{RecipeVersion}`.
async fn delete_version(emulator: &Emulator, name: &str, version: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!(
                "/recipes/{}/recipeVersion/{}",
                label(name),
                label(version)
            ),
            None,
        )
        .await
}

/// `DescribeRecipe`: `GET /recipes/{Name}?recipeVersion=<version>`.
async fn describe_version(emulator: &Emulator, name: &str, version: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/recipes/{}?recipeVersion={}",
                label(name),
                label(version)
            ),
            None,
        )
        .await
}

/// Seed one row: the recipe with its publishes, and the project or job that holds one of
/// its versions.
async fn seed(row: &HeldCase) -> Emulator {
    let HeldCase {
        case,
        recipe,
        publishes,
        holder,
        ..
    } = row;
    let emulator = Emulator::new();
    assert_eq!(
        create_recipe(&emulator, recipe).await.status,
        200,
        "case {case:?}: the recipe {recipe} must be created"
    );
    for round in 0..*publishes {
        let published = publish_recipe(&emulator, recipe).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: publish {} must succeed, got {} {:?}",
            round + 1,
            published.status,
            published.body
        );
    }
    match holder {
        Holder::Project(name) => {
            let dataset = "held-ds";
            assert_eq!(
                emulator
                    .call(
                        "POST",
                        "/datasets",
                        Some(json!({
                            "Name": dataset,
                            "Format": "CSV",
                            "Input": {
                                "S3InputDefinition": {"Bucket": "brew-bucket", "Key": "in.csv"},
                            },
                        })),
                    )
                    .await
                    .status,
                200,
                "case {case:?}: the dataset the project acts upon must be created"
            );
            let created = create_project(
                &emulator,
                json!({
                    "Name": name,
                    "DatasetName": dataset,
                    "RecipeName": recipe,
                    "RoleArn": ROLE_ARN,
                    "Sample": {"Size": 500, "Type": "FIRST_N"},
                }),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the project {name} must be created, got {} {:?}",
                created.status, created.body
            );
        }
        Holder::Job { name, reference } => {
            let mut recipe_reference = json!({"Name": recipe});
            if let Some(version) = reference {
                recipe_reference["RecipeVersion"] = json!(version);
            }
            let created = emulator
                .call(
                    "POST",
                    "/recipeJobs",
                    Some(json!({
                        "Name": name,
                        "RoleArn": ROLE_ARN,
                        "RecipeReference": recipe_reference,
                    })),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case:?}: the job {name} must be created, got {} {:?}",
                created.status, created.body
            );
        }
    }
    emulator
}

/// Every row of the table, each stating the sentence its refusal must carry.
fn cases() -> Vec<HeldCase> {
    vec![
        HeldCase {
            case: "a project holding the draft, addressed by its alias",
            recipe: "developed",
            publishes: 0,
            holder: Holder::Project("holder-prj"),
            requested: LATEST_WORKING,
        },
        HeldCase {
            case: "a project holding the draft, addressed by the draft's own number",
            recipe: "developed",
            publishes: 0,
            holder: Holder::Project("holder-prj"),
            requested: "0.1",
        },
        HeldCase {
            case: "a project on a recipe with two publishes, draft addressed by its alias",
            recipe: "published-and-developed",
            publishes: 2,
            holder: Holder::Project("second-prj"),
            requested: LATEST_WORKING,
        },
        HeldCase {
            case: "a project on a recipe with two publishes, draft addressed by its number",
            recipe: "published-and-developed",
            publishes: 2,
            holder: Holder::Project("second-prj"),
            requested: "2.1",
        },
        HeldCase {
            case: "a recipe name carrying a space, held by a project",
            recipe: "sales recipe",
            publishes: 0,
            holder: Holder::Project("space prj"),
            requested: LATEST_WORKING,
        },
        HeldCase {
            case: "a job naming a published version by number",
            recipe: "run-by-a-job",
            publishes: 2,
            holder: Holder::Job {
                name: "job-on-1-0",
                reference: Some("1.0"),
            },
            requested: "1.0",
        },
        HeldCase {
            case: "a job naming no version, so it runs the latest published one",
            recipe: "run-by-a-job",
            publishes: 2,
            holder: Holder::Job {
                name: "job-on-latest",
                reference: None,
            },
            requested: "2.0",
        },
        HeldCase {
            case: "a job running the draft, addressed by its alias",
            recipe: "draft-run-by-a-job",
            publishes: 0,
            holder: Holder::Job {
                name: "job-on-draft",
                reference: Some(LATEST_WORKING),
            },
            requested: LATEST_WORKING,
        },
        HeldCase {
            case: "a job running the draft, addressed by the draft's own number",
            recipe: "draft-run-by-a-job",
            publishes: 0,
            holder: Holder::Job {
                name: "job-on-draft",
                reference: Some(LATEST_WORKING),
            },
            requested: "0.1",
        },
    ]
}

/// The sentence the refusal must state for one row.
fn expected_message(row: &HeldCase) -> String {
    format!(
        "Recipe {} version {} are used by {} {}.",
        row.recipe,
        row.requested,
        row.holder.kind(),
        row.holder.name()
    )
}

/// `BatchDeleteRecipeVersion` reports the held version as a per-item failure of a 200
/// response, and that item's `ErrorMessage` is the sentence, with the recipe name, the
/// identifier as sent and the holder's name all interpolated.
#[tokio::test]
async fn the_batch_item_states_the_recipe_version_and_its_holder() {
    for row in cases() {
        let emulator = seed(&row).await;
        let case = row.case;
        let expected = expected_message(&row);

        let replied = batch_delete(&emulator, row.recipe, &[row.requested]).await;
        assert_eq!(
            replied.status, 200,
            "case {case:?}: a partial failure is still a 200, got {} {:?}",
            replied.status, replied.body
        );
        let reported = replied
            .body
            .get("Errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: the held version must be reported as one item, got {:?}",
            replied.body
        );
        assert_eq!(
            reported[0].get("ErrorMessage").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {case:?}: expected {expected:?}, got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0].get("ErrorCode").and_then(Value::as_str),
            Some("409"),
            "case {case:?}: a held version carries the status of the conflict, got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0].get("RecipeVersion").and_then(Value::as_str),
            Some(row.requested),
            "case {case:?}: the item echoes the identifier the request named, got {:?}",
            reported[0]
        );
        assert_eq!(
            describe_version(&emulator, row.recipe, row.requested)
                .await
                .status,
            200,
            "case {case:?}: the held version must still read back"
        );
    }
}

/// `DeleteRecipeVersion` refuses the same held version outright, and its
/// `ConflictException` states the same sentence: the refusal class owns the wording, not
/// one operation.
#[tokio::test]
async fn the_single_delete_refusal_states_the_same_sentence() {
    for row in cases() {
        let emulator = seed(&row).await;
        let case = row.case;
        let expected = expected_message(&row);

        let refused = delete_version(&emulator, row.recipe, row.requested).await;
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
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
        assert_eq!(
            describe_version(&emulator, row.recipe, row.requested)
                .await
                .status,
            200,
            "case {case:?}: the held version must still read back"
        );
    }
}
