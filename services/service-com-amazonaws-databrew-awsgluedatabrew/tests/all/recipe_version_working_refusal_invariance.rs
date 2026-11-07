// SPDX-License-Identifier: CC0-1.0

//! What the single-version delete's refusal of the working draft does NOT depend on, and
//! what that refusal leaves behind.
//!
//! `recipe_version_working_refusal_wording.rs` pins the two sentences themselves — the
//! fixed one `DeleteRecipeVersion` refuses the draft with, and the per-item ordering one
//! `BatchDeleteRecipeVersion` reports for the same version. This suite takes the other
//! half of that rule:
//!
//! * the single-version sentence is INVARIANT: the same bytes answer every recipe name
//!   `RecipeName` allows (model: `smithy_query(query="shape", shape="RecipeName")` —
//!   `length 1..255`, no pattern) and every publish history, and the text carries neither
//!   the recipe name, nor any digit (so no version number of the recipe reached it), nor
//!   a terminating period;
//! * the refusal CHANGES NOTHING: the draft, with the number it carried, and every
//!   published version still read back through `DescribeRecipe` afterwards;
//! * the condition the batch operation words as the ordering rule really is a condition
//!   that clears: a batch naming every version the recipe holds reports no failure and
//!   takes the draft with the recipe.
//!
//! Every case drives the real wire path, so what is asserted is the status, the error code
//! and the JSON body a client sees.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply};

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// The whole message `DeleteRecipeVersion` refuses the working draft with, as
/// `recipe_version_working_refusal_wording.rs` states it: fixed text end to end.
const DELETE_REFUSED_MESSAGE: &str = "Recipe version LATEST_WORKING is not allowed to be deleted";

/// One row: the recipe name the request addresses and how many publishes stand beside its
/// draft.
struct NameCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The recipe the request addresses.
    recipe: String,
    /// Publishes before the delete, so the recipe holds that many other versions.
    publishes: usize,
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

/// `DeleteRecipeVersion`: `DELETE /recipes/{Name}/recipeVersion/{RecipeVersion}`.
async fn delete_version(emulator: &Emulator, name: &str, version: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!("/recipes/{}/recipeVersion/{}", label(name), label(version)),
            None,
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

/// `DescribeRecipe`: `GET /recipes/{Name}?recipeVersion=<version>`.
async fn describe_version(emulator: &Emulator, name: &str, version: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/recipes/{}?recipeVersion={version}", label(name)),
            None,
        )
        .await
}

/// A recipe holding `publishes` published versions beside its working draft.
async fn recipe_with_published_versions(emulator: &Emulator, name: &str, publishes: usize) {
    assert_eq!(
        create_recipe(emulator, name).await.status,
        200,
        "the recipe {name} must be created"
    );
    for round in 0..publishes {
        let published = publish_recipe(emulator, name).await;
        assert_eq!(
            published.status, 200,
            "publish {} of recipe {name} must succeed, got {} {:?}",
            round + 1,
            published.status,
            published.body
        );
    }
}

/// The recipe-name forms the sentence must be invariant over. Each is a name `RecipeName`
/// allows (`length 1..255`, no pattern), and none of them occurs in the fixed sentence, so
/// an interpolated name would show.
fn name_cases() -> Vec<NameCase> {
    vec![
        NameCase {
            case: "a one-character name, the shortest RecipeName allows",
            recipe: "q".to_string(),
            publishes: 1,
        },
        NameCase {
            case: "a name carrying a space",
            recipe: "quarterly rollup".to_string(),
            publishes: 2,
        },
        NameCase {
            // Valid characters are "alphanumeric (A-Z, a-z, 0-9), hyphen (-), period (.),
            // and space" (model: `CreateRecipeRequest$Name` documentation).
            case: "a name carrying a hyphen and a period",
            recipe: "brew.rollup-v2.final".to_string(),
            publishes: 3,
        },
        NameCase {
            case: "a 255-character name, the longest RecipeName allows",
            recipe: "z".repeat(255),
            publishes: 1,
        },
    ]
}

/// The sentence does not vary with the recipe or with the publish history: every row gets
/// back the very same bytes, carrying no name, no digit and no terminating period.
#[tokio::test]
async fn the_refusal_sentence_is_invariant_over_recipes_and_histories() {
    for row in name_cases() {
        let NameCase {
            case,
            recipe,
            publishes,
        } = row;
        assert!(
            !recipe.is_empty() && recipe.len() <= 255,
            "case {case:?}: the name must be one RecipeName allows, got {} characters",
            recipe.len()
        );
        let emulator = Emulator::new();
        recipe_with_published_versions(&emulator, &recipe, publishes).await;

        let refused = delete_version(&emulator, &recipe, LATEST_WORKING).await;
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
        let message = refused.error_message();
        assert_eq!(
            message, DELETE_REFUSED_MESSAGE,
            "case {case:?}: expected {DELETE_REFUSED_MESSAGE:?}, got {message:?}"
        );
        assert!(
            !message.contains(&recipe),
            "case {case:?}: the sentence must not name the recipe, got {message:?}"
        );
        assert!(
            !message.contains(|character: char| character.is_ascii_digit()),
            "case {case:?}: no version number can reach the sentence, got {message:?}"
        );
        assert!(
            !message.ends_with('.'),
            "case {case:?}: the sentence carries no terminating period, got {message:?}"
        );
    }
}

/// The refusal is not a partial delete: the draft and every published version of the
/// recipe still read back afterwards, and the draft keeps the number it carried — however
/// the request addressed it.
#[tokio::test]
async fn the_refusal_leaves_every_version_of_the_recipe_in_place() {
    for publishes in [1usize, 2, 3] {
        let emulator = Emulator::new();
        let recipe = "untouched";
        recipe_with_published_versions(&emulator, recipe, publishes).await;
        let draft = format!("{publishes}.1");

        for addressed in [LATEST_WORKING, draft.as_str()] {
            let case = format!("{publishes} publishes, draft addressed as {addressed}");
            let refused = delete_version(&emulator, recipe, addressed).await;
            assert_eq!(
                refused.status, 400,
                "case {case:?}: expected 400, got {} {:?}",
                refused.status, refused.body
            );
            for round in 1..=publishes {
                let version = format!("{round}.0");
                let described = describe_version(&emulator, recipe, &version).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: published version {version} must survive the refusal, \
                     got {} {:?}",
                    described.status, described.body
                );
            }
            let described = describe_version(&emulator, recipe, LATEST_WORKING).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: the draft must survive the refusal, got {} {:?}",
                described.status, described.body
            );
            assert_eq!(
                described.text("RecipeVersion"),
                draft,
                "case {case:?}: the draft must keep the number it carried, got {:?}",
                described.body
            );
        }
    }
}

/// The ordering condition the batch operation words really does clear: a batch naming
/// every version the recipe holds reports no failure at all and takes the draft with the
/// recipe.
#[tokio::test]
async fn the_batch_delete_takes_the_draft_once_nothing_stands_beside_it() {
    for publishes in [1usize, 2, 3] {
        let emulator = Emulator::new();
        let recipe = "all-at-once";
        recipe_with_published_versions(&emulator, recipe, publishes).await;
        let mut versions: Vec<String> = (1..=publishes).map(|round| format!("{round}.0")).collect();
        versions.push(LATEST_WORKING.to_string());
        let named: Vec<&str> = versions.iter().map(String::as_str).collect();

        let replied = batch_delete(&emulator, recipe, &named).await;
        assert_eq!(
            replied.status, 200,
            "case {publishes} publishes: expected 200, got {} {:?}",
            replied.status, replied.body
        );
        let reported = replied
            .body
            .get("Errors")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert!(
            reported.is_empty(),
            "case {publishes} publishes: nothing stands beside the draft once the request \
             names every version, got {reported:?}"
        );
        assert_eq!(
            describe_version(&emulator, recipe, LATEST_WORKING).await.status,
            404,
            "case {publishes} publishes: the draft and the recipe must be gone"
        );
    }
}
