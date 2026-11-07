// SPDX-License-Identifier: CC0-1.0

//! How each of the two version-delete operations WORDS its refusal of a recipe's working
//! draft while that recipe still holds another version.
//!
//! Both operations take the draft by either of its identifiers — the `LATEST_WORKING`
//! alias or the number the draft currently carries (model:
//! `smithy_query(query="operation", operation="DeleteRecipeVersion,BatchDeleteRecipeVersion")`
//! — `RecipeVersion`/`RecipeVersions` accept "a numeric versions (X.Y) or LATEST_WORKING")
//! — and each refuses it in its OWN words:
//!
//! * `DeleteRecipeVersion` refuses the whole call 400 `ValidationException` with one
//!   sentence of entirely fixed text: it states that this version is not deletable through
//!   this operation and names nothing of the request or of the recipe's state.
//! * `BatchDeleteRecipeVersion` answers 200 and reports the draft as a per-item failure
//!   whose `ErrorCode` is `409` and whose `ErrorMessage` states the ORDERING rule ("The
//!   LATEST_WORKING version will only be deleted if the recipe has no other versions",
//!   model: `BatchDeleteRecipeVersion`), naming the recipe the request addressed and the
//!   version identifier that item asked for.
//!
//! The table spans what the two sentences generalise over: how many versions stand beside
//! the draft, which of the draft's two identifier forms addresses it, and the recipe name
//! — the one value the batch sentence interpolates and the single-version sentence must
//! not. Every case drives the real wire path and asserts the status, the error code and the
//! message body a client sees.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply};

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// The whole message `DeleteRecipeVersion` refuses the working draft with. It carries NO
/// variable part: the alias in it is the literal identifier this operation will not
/// delete, not a value echoed from the request.
const DELETE_REFUSED_MESSAGE: &str = "Recipe version LATEST_WORKING is not allowed to be deleted";

/// One row: a recipe name, how many publishes stand beside the draft, and which of the
/// draft's identifiers the request addresses it by.
struct Row {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// The recipe the request addresses — the value the batch sentence interpolates.
    recipe: &'static str,
    /// Publishes before the delete, so the recipe holds that many other versions.
    publishes: usize,
    /// Whether the request names the alias (`true`) or the draft's own number.
    by_alias: bool,
}

/// `CreateRecipe`: `POST /recipes`.
async fn create_recipe(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "POST",
            "/recipes",
            Some(json!({
                "Name": name,
                "Steps": [{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}}}],
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

/// The `Errors` items of a batch reply, in the order the response carries them.
fn errors(reply: &Reply) -> Vec<Value> {
    reply
        .body
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
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

/// The rows every case below is driven over.
const ROWS: [Row; 6] = [
    Row {
        case: "one published version, draft by alias",
        recipe: "one-published",
        publishes: 1,
        by_alias: true,
    },
    Row {
        case: "one published version, draft by its own number",
        recipe: "one-published",
        publishes: 1,
        by_alias: false,
    },
    Row {
        case: "two published versions, draft by alias",
        recipe: "two published",
        publishes: 2,
        by_alias: true,
    },
    Row {
        case: "two published versions, draft by its own number",
        recipe: "two published",
        publishes: 2,
        by_alias: false,
    },
    Row {
        case: "three published versions, draft by alias",
        recipe: "brew.recipe-3",
        publishes: 3,
        by_alias: true,
    },
    Row {
        case: "three published versions, draft by its own number",
        recipe: "brew.recipe-3",
        publishes: 3,
        by_alias: false,
    },
];

/// The draft's own number after `publishes` publishes: the first minor revision on top of
/// the highest published major.
fn draft_number(publishes: usize) -> String {
    format!("{publishes}.1")
}

/// `DeleteRecipeVersion` refuses the draft with ONE fixed sentence, the same for every
/// recipe, every number of versions standing beside it and either identifier form — and it
/// interpolates nothing: neither the recipe name nor the number the draft carries appears
/// in it.
#[tokio::test]
async fn the_single_version_delete_refuses_the_draft_in_its_own_fixed_words() {
    for row in ROWS {
        let Row {
            case,
            recipe,
            publishes,
            by_alias,
        } = row;
        let emulator = Emulator::new();
        recipe_with_published_versions(&emulator, recipe, publishes).await;
        let draft = draft_number(publishes);
        let addressed = if by_alias { LATEST_WORKING } else { &draft };

        let refused = delete_version(&emulator, recipe, addressed).await;
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
            !message.contains(recipe),
            "case {case:?}: the sentence has no variable part, so it must not name the \
             recipe, got {message:?}"
        );
        assert!(
            !message.contains(draft.as_str()),
            "case {case:?}: the sentence must not carry the number the draft holds, got \
             {message:?}"
        );
        assert_eq!(
            describe_version(&emulator, recipe, addressed).await.status,
            200,
            "case {case:?}: the refused delete must leave the draft where it was"
        );
    }
}

/// The BATCH operation says something different about the very same version: a 200 whose
/// per-item failure carries `409` and states the ordering rule, naming the recipe the
/// request addressed and the identifier that item asked for.
#[tokio::test]
async fn the_batch_delete_reports_the_draft_with_the_ordering_rule() {
    for row in ROWS {
        let Row {
            case,
            recipe,
            publishes,
            by_alias,
        } = row;
        let emulator = Emulator::new();
        recipe_with_published_versions(&emulator, recipe, publishes).await;
        let draft = draft_number(publishes);
        let addressed = if by_alias { LATEST_WORKING } else { &draft };

        let replied = batch_delete(&emulator, recipe, &[addressed]).await;
        assert_eq!(
            replied.status, 200,
            "case {case:?}: a batch delete reports per-item failures on a 200, got {} {:?}",
            replied.status, replied.body
        );
        let reported = errors(&replied);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected one item, got {:?}",
            replied.body
        );
        assert_eq!(
            reported[0].get("ErrorCode").and_then(Value::as_str),
            Some("409"),
            "case {case:?}: the draft is held, so the item carries the conflict status, \
             got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0].get("RecipeVersion").and_then(Value::as_str),
            Some(addressed),
            "case {case:?}: the item echoes the identifier the request named, got {:?}",
            reported[0]
        );
        let expected = format!(
            "Recipe {recipe} version {addressed} can only be deleted if all other \
             versions got deleted."
        );
        let message = reported[0]
            .get("ErrorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
        assert_ne!(
            message, DELETE_REFUSED_MESSAGE,
            "case {case:?}: the two delete operations word this refusal differently"
        );
        assert_eq!(
            describe_version(&emulator, recipe, addressed).await.status,
            200,
            "case {case:?}: the reported failure must leave the draft where it was"
        );
    }
}

/// The two wordings belong to the OPERATION, not to the recipe's state: the same recipe
/// answers the single-version delete its fixed sentence and the batch delete the ordering
/// item, and once the other versions are gone neither refusal is stated at all.
#[tokio::test]
async fn each_operation_keeps_its_own_wording_for_one_recipe() {
    let emulator = Emulator::new();
    let recipe = "both-paths";
    recipe_with_published_versions(&emulator, recipe, 2).await;

    let refused = delete_version(&emulator, recipe, LATEST_WORKING).await;
    assert_eq!(
        refused.error_message(),
        DELETE_REFUSED_MESSAGE,
        "case \"single delete first\": expected {DELETE_REFUSED_MESSAGE:?}, got {:?}",
        refused.error_message()
    );

    let batched = batch_delete(&emulator, recipe, &[LATEST_WORKING]).await;
    let reported = errors(&batched);
    assert_eq!(
        reported.len(),
        1,
        "case \"batch delete second\": expected one item, got {:?}",
        batched.body
    );
    let expected = format!(
        "Recipe {recipe} version {LATEST_WORKING} can only be deleted if all other \
         versions got deleted."
    );
    assert_eq!(
        reported[0].get("ErrorMessage").and_then(Value::as_str),
        Some(expected.as_str()),
        "case \"batch delete second\": expected {expected:?}, got {:?}",
        reported[0]
    );

    // Nothing stands beside the draft any more, so neither operation states a refusal:
    // the draft goes and takes the recipe with it.
    for version in ["1.0", "2.0"] {
        assert_eq!(
            delete_version(&emulator, recipe, version).await.status,
            200,
            "case \"clearing {version}\": a published version nothing holds must go"
        );
    }
    let deleted = delete_version(&emulator, recipe, LATEST_WORKING).await;
    assert_eq!(
        deleted.status, 200,
        "case \"draft last\": the draft must go once nothing stands beside it, got {} {:?}",
        deleted.status, deleted.body
    );
    assert_eq!(
        describe_version(&emulator, recipe, LATEST_WORKING).await.status,
        404,
        "case \"draft last\": the deleted draft must resolve nothing"
    );
}
