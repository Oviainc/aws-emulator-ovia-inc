// SPDX-License-Identifier: CC0-1.0

//! What `DeleteRecipeVersion` deletes, what it refuses, and what the recipe holds
//! afterwards.
//!
//! The rules pinned here (model:
//! `smithy_query(query="operation", operation="DeleteRecipeVersion")` —
//! `DELETE /recipes/{Name}/recipeVersion/{RecipeVersion}`, errors `ConflictException`
//! 409, `ResourceNotFoundException` 404, `ValidationException` 400):
//!
//! * The call addresses ONE version of one recipe, and a version the pair resolves is
//!   deleted: it reads back no more, every other version the recipe holds does, and the
//!   working draft revises whatever published major is left.
//! * The identifiers it accepts are a numeric `X.Y` version or `LATEST_WORKING`
//!   ("LATEST_PUBLISHED is not supported", model:
//!   `DeleteRecipeVersionRequest$RecipeVersion`); anything else is a 400 and deletes
//!   nothing.
//! * A pair that resolves nothing — no such recipe, or a version this recipe does not
//!   hold — is a 404 naming both halves.
//! * The working version goes only when the recipe holds no other version ("The
//!   LATEST_WORKING version will only be deleted if the recipe has no other versions",
//!   model: `BatchDeleteRecipeVersion` — the rule belongs to the version, so it holds
//!   however the version is addressed and whichever operation addresses it), and the
//!   delete that takes it takes the recipe with it. THIS operation words that refusal as
//!   its own fixed sentence, which `recipe_version_working_refusal_wording.rs` pins
//!   against the batch operation's per-item ordering sentence.
//! * A version something still holds is a 409 naming the holder: a project that develops
//!   the recipe's working version, and a job that runs a version.
//!
//! The tables span what those rules generalise over: how many versions the recipe holds,
//! which identifier form addresses one (a number, an alias), which version of the history
//! is deleted (oldest, newest, middle, only), and every reason a version cannot go.
//!
//! Every case drives the real wire path, so what is asserted is the HTTP status, the
//! error code and the JSON body a client sees, read back through `DescribeRecipe`.

use serde_json::json;

use crate::databrew_harness::{create_project, label, Emulator, Reply, ROLE_ARN};

/// The version a recipe carries the moment it is created, before any publish: its
/// working draft (`INITIAL_RECIPE_VERSION` in `src/state.rs`, from
/// `Recipe$RecipeVersion`).
const WORKING_VERSION: &str = "0.1";

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// The alias naming the most recent published version, which this operation does NOT
/// accept (model: `DeleteRecipeVersionRequest$RecipeVersion`).
const LATEST_PUBLISHED: &str = "LATEST_PUBLISHED";

/// The sentence `DeleteRecipeVersion` refuses the working draft with — fixed text with no
/// variable part, carrying the literal identifier this operation will not delete.
const WORKING_DELETE_REFUSED_MESSAGE: &str =
    "Recipe version LATEST_WORKING is not allowed to be deleted";

/// The number the working draft carries after `publishes` publishes: the first minor
/// revision on top of the highest published major.
fn draft_number(publishes: usize) -> String {
    format!("{publishes}.1")
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
            &format!(
                "/recipes/{}/recipeVersion/{}",
                label(name),
                label(version)
            ),
            None,
        )
        .await
}

/// A recipe holding `publishes` published versions beside its working draft, so a row can
/// address `1.0`, `2.0`, ... by number.
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

/// One row of the delete table: the versions a recipe holds, the one version the request
/// addresses, and what the recipe holds afterwards.
struct DeleteCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// How many publishes the recipe has had before the delete.
    publishes: usize,
    /// The `RecipeVersion` the request addresses.
    request: &'static str,
    /// The versions that must still read back afterwards.
    still_held: &'static [&'static str],
    /// The versions that must resolve nothing afterwards.
    gone: &'static [&'static str],
    /// The number the working draft carries afterwards, or `None` when the delete took
    /// the recipe itself: a delete of a published version changes the publish history the
    /// draft's number is derived from, so the draft revises whatever major is left.
    draft_after: Option<&'static str>,
}

/// A version the pair resolves and nothing holds is deleted: the response names the
/// recipe and the version it deleted, that version resolves nothing afterwards, and every
/// other version the recipe holds still reads back.
#[tokio::test]
async fn the_addressed_version_is_deleted_and_the_rest_are_kept() {
    let cases = [
        DeleteCase {
            case: "the oldest published version of three",
            publishes: 3,
            request: "1.0",
            still_held: &["2.0", "3.0", LATEST_WORKING],
            gone: &["1.0"],
            draft_after: Some("3.1"),
        },
        DeleteCase {
            case: "the newest published version of three",
            publishes: 3,
            request: "3.0",
            still_held: &["1.0", "2.0", LATEST_WORKING],
            // The newest published major is gone, so the draft no longer revises it.
            gone: &["3.0", "3.1"],
            draft_after: Some("2.1"),
        },
        DeleteCase {
            case: "the middle published version of three",
            publishes: 3,
            request: "2.0",
            still_held: &["1.0", "3.0", LATEST_WORKING],
            gone: &["2.0"],
            draft_after: Some("3.1"),
        },
        DeleteCase {
            case: "the only published version",
            publishes: 1,
            request: "1.0",
            still_held: &[LATEST_WORKING],
            // Nothing is published any more, so the draft is back at major 0.
            gone: &["1.0", "1.1"],
            draft_after: Some(WORKING_VERSION),
        },
        DeleteCase {
            case: "the working version of a recipe that holds nothing else, by its alias",
            publishes: 0,
            request: LATEST_WORKING,
            still_held: &[],
            gone: &[LATEST_WORKING, WORKING_VERSION, LATEST_PUBLISHED],
            draft_after: None,
        },
        DeleteCase {
            case: "the working version of a recipe that holds nothing else, by its number",
            publishes: 0,
            request: WORKING_VERSION,
            still_held: &[],
            gone: &[LATEST_WORKING, WORKING_VERSION, LATEST_PUBLISHED],
            draft_after: None,
        },
    ];

    for row in cases {
        let DeleteCase {
            case,
            publishes,
            request,
            still_held,
            gone,
            draft_after,
        } = row;
        let emulator = Emulator::new();
        let recipe = "one-at-a-time";
        recipe_with_published_versions(&emulator, recipe, publishes).await;
        assert_eq!(
            describe_version(&emulator, recipe, &draft_number(publishes))
                .await
                .status,
            200,
            "case {case:?}: the draft of a recipe with {publishes} publishes is {}, \
             which must read back before the delete",
            draft_number(publishes)
        );

        let replied = delete_version(&emulator, recipe, request).await;
        assert_eq!(
            replied.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            replied.status, replied.body
        );
        assert_eq!(
            replied.text("Name"),
            recipe,
            "case {case:?}: the response must name the recipe, got {:?}",
            replied.body
        );
        assert_eq!(
            replied.text("RecipeVersion"),
            request,
            "case {case:?}: the response must name the version that was deleted, got {:?}",
            replied.body
        );

        for version in still_held {
            let described = describe_version(&emulator, recipe, version).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: version {version} must still read back, got {} {:?}",
                described.status, described.body
            );
        }
        for version in gone {
            let described = describe_version(&emulator, recipe, version).await;
            assert_eq!(
                described.status, 404,
                "case {case:?}: version {version} must resolve nothing, got {} {:?}",
                described.status, described.body
            );
        }
        match draft_after {
            Some(draft) => {
                let described = describe_version(&emulator, recipe, draft).await;
                assert_eq!(
                    described.status, 200,
                    "case {case:?}: the draft must revise the major that is left ({draft}), \
                     got {} {:?}",
                    described.status, described.body
                );
                assert_eq!(
                    describe_version(&emulator, recipe, LATEST_WORKING)
                        .await
                        .text("RecipeVersion"),
                    draft,
                    "case {case:?}: the draft must report {draft} as its own number"
                );
            }
            None => {
                // The record was what held the recipe's versions, so the recipe itself is
                // gone and a second delete of it is a miss rather than a second success.
                let again = delete_version(&emulator, recipe, request).await;
                assert_eq!(
                    again.status, 404,
                    "case {case:?}: the recipe is gone, so a second delete must miss, \
                     got {} {:?}",
                    again.status, again.body
                );
            }
        }
    }
}

/// The working version goes LAST: while the recipe holds a published version the delete is
/// refused and nothing goes, and once those versions are gone the same call deletes the
/// draft and the recipe with it. Rows differ along how many versions stand beside the
/// draft and along which of its two identifiers addresses it.
#[tokio::test]
async fn the_working_version_goes_only_when_the_recipe_holds_nothing_else() {
    for publishes in [1usize, 2, 3] {
        let emulator = Emulator::new();
        let recipe = "still-published";
        recipe_with_published_versions(&emulator, recipe, publishes).await;
        let draft = draft_number(publishes);

        for addressed in [LATEST_WORKING, draft.as_str()] {
            let case = format!("{publishes} publishes, draft addressed as {addressed}");
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
            assert_eq!(
                refused.error_message(),
                WORKING_DELETE_REFUSED_MESSAGE,
                "case {case:?}: expected {WORKING_DELETE_REFUSED_MESSAGE:?}, got {:?}",
                refused.error_message()
            );
            assert_eq!(
                describe_version(&emulator, recipe, addressed).await.status,
                200,
                "case {case:?}: the refused delete must leave the draft where it was"
            );
        }

        // Every published version goes on its own, and then the draft is the only version
        // the recipe holds: the same call now succeeds and takes the recipe with it.
        for round in 1..=publishes {
            let version = format!("{round}.0");
            assert_eq!(
                delete_version(&emulator, recipe, &version).await.status,
                200,
                "case {publishes} publishes: published version {version} must be deletable"
            );
        }
        let deleted = delete_version(&emulator, recipe, LATEST_WORKING).await;
        assert_eq!(
            deleted.status, 200,
            "case {publishes} publishes: the draft must go once nothing stands beside it, \
             got {} {:?}",
            deleted.status, deleted.body
        );
        for version in [LATEST_WORKING, LATEST_PUBLISHED, WORKING_VERSION] {
            let described = describe_version(&emulator, recipe, version).await;
            assert_eq!(
                described.status, 404,
                "case {publishes} publishes: {version} of a deleted recipe must resolve \
                 nothing, got {} {:?}",
                described.status, described.body
            );
        }
    }
}

/// A pair that resolves nothing is refused 404, naming the recipe the request carried and
/// the version identifier it looked up — over every form a numeric identifier takes and
/// over a recipe nobody created.
#[tokio::test]
async fn a_pair_that_resolves_nothing_is_refused_as_a_miss() {
    let emulator = Emulator::new();
    let recipe = "sparse";
    recipe_with_published_versions(&emulator, recipe, 1).await;

    // Every identifier here is well formed (model: `Recipe$RecipeVersion` — `X.Y` with up
    // to 6 digits in each half, never `0.0`) and names no version this recipe holds.
    let cases = [
        ("a version just past the published one", recipe, "2.0"),
        ("a minor revision the recipe never held", recipe, "0.2"),
        ("a two-digit pair", recipe, "12.34"),
        ("the largest version the model allows", recipe, "999999.999999"),
        (
            "the draft's number before the publish renumbered it",
            recipe,
            WORKING_VERSION,
        ),
        ("a recipe nobody created", "no-such-recipe", "1.0"),
        (
            "a recipe nobody created, addressed by alias",
            "no-such-recipe",
            LATEST_WORKING,
        ),
    ];

    for (case, addressed, version) in cases {
        let refused = delete_version(&emulator, addressed, version).await;
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
        let expected = format!("The recipe {addressed} for version {version} wasn't found.");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }

    // Nothing was deleted by any of those refusals.
    for version in ["1.0", LATEST_WORKING, "1.1"] {
        let described = describe_version(&emulator, recipe, version).await;
        assert_eq!(
            described.status, 200,
            "version {version} must survive a refused delete, got {} {:?}",
            described.status, described.body
        );
    }
}

/// An identifier outside the documented set is refused 400 and deletes nothing.
#[tokio::test]
async fn an_identifier_this_operation_does_not_accept_is_refused() {
    let emulator = Emulator::new();
    let recipe = "intact";
    recipe_with_published_versions(&emulator, recipe, 2).await;

    let cases = [
        (
            "the latest-published alias, which this operation does not accept",
            LATEST_PUBLISHED,
        ),
        ("a version that is not X.Y", "one"),
        ("a version with three halves", "1.2.3"),
        ("a version with an empty half", "1."),
        ("a version with no minor half", "1"),
        ("the version the model calls invalid", "0.0"),
        ("a half longer than six digits", "1234567.0"),
        ("a negative half", "-1.0"),
    ];

    for (case, version) in cases {
        let refused = delete_version(&emulator, recipe, version).await;
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
    }

    for version in ["1.0", "2.0", LATEST_WORKING, "2.1"] {
        let described = describe_version(&emulator, recipe, version).await;
        assert_eq!(
            described.status, 200,
            "{version} must survive every rejected request, got {} {:?}",
            described.status, described.body
        );
    }
}

/// A version something still holds is refused 409 naming the holder: a project develops
/// its recipe's working version, and a job runs the version its recipe reference
/// addresses — by number, or, when the reference carries none, the latest published one.
#[tokio::test]
async fn a_version_something_holds_cannot_be_deleted() {
    // A project develops the recipe's working version, which is the recipe's only version
    // here, so nothing but the project keeps it.
    let emulator = Emulator::new();
    let dataset = "held-ds";
    assert_eq!(
        emulator
            .call(
                "POST",
                "/datasets",
                Some(json!({
                    "Name": dataset,
                    "Format": "CSV",
                    "Input": {"S3InputDefinition": {"Bucket": "brew-bucket", "Key": "in.csv"}},
                })),
            )
            .await
            .status,
        200,
        "the dataset the project acts upon must be created"
    );
    let recipe = "developed";
    recipe_with_published_versions(&emulator, recipe, 0).await;
    assert_eq!(
        create_project(
            &emulator,
            json!({
                "Name": "holder",
                "DatasetName": dataset,
                "RecipeName": recipe,
                "RoleArn": ROLE_ARN,
            }),
        )
        .await
        .status,
        200,
        "the project that develops the recipe must be created"
    );

    for addressed in [LATEST_WORKING, WORKING_VERSION] {
        let refused = delete_version(&emulator, recipe, addressed).await;
        assert_eq!(
            refused.status, 409,
            "case {addressed:?}: expected 409, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ConflictException"),
            "case {addressed:?}: expected ConflictException, got {:?}",
            refused.error_type
        );
        let message = refused.error_message();
        assert!(
            message.contains("holder") && message.contains(addressed),
            "case {addressed:?}: the refusal must name the project holding the version, \
             got {message:?}"
        );
        assert_eq!(
            describe_version(&emulator, recipe, addressed).await.status,
            200,
            "case {addressed:?}: the held version must still read back"
        );
    }

    // A job referencing one version by number holds exactly that version; a job
    // referencing none holds the latest published one.
    for (case, job, requested_version, held, deletable) in [
        (
            "a job that names a version by number",
            "run-1-0",
            Some("1.0"),
            "1.0",
            "2.0",
        ),
        (
            "a job that names no version, so it runs the latest published one",
            "run-latest",
            None,
            "2.0",
            "1.0",
        ),
    ] {
        let emulator = Emulator::new();
        let recipe = "run-by-a-job";
        recipe_with_published_versions(&emulator, recipe, 2).await;
        let mut reference = json!({"Name": recipe});
        if let Some(version) = requested_version {
            reference["RecipeVersion"] = json!(version);
        }
        assert_eq!(
            emulator
                .call(
                    "POST",
                    "/recipeJobs",
                    Some(json!({
                        "Name": job,
                        "RoleArn": ROLE_ARN,
                        "RecipeReference": reference,
                    })),
                )
                .await
                .status,
            200,
            "case {case:?}: the job that runs the version must be created"
        );

        let refused = delete_version(&emulator, recipe, held).await;
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
        let message = refused.error_message();
        assert!(
            message.contains(job) && message.contains(held),
            "case {case:?}: the refusal must name the job running {held}, got {message:?}"
        );
        assert_eq!(
            describe_version(&emulator, recipe, held).await.status,
            200,
            "case {case:?}: the referenced version must still read back"
        );
        // The version no job references goes in the same recipe.
        assert_eq!(
            delete_version(&emulator, recipe, deletable).await.status,
            200,
            "case {case:?}: the unreferenced version {deletable} must still be deletable"
        );
    }

    // A job that runs the working version of a recipe holding nothing else keeps that
    // version too, so the draft's own rule is not the only one it must pass.
    let emulator = Emulator::new();
    let recipe = "draft-run-by-a-job";
    recipe_with_published_versions(&emulator, recipe, 0).await;
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": "run-draft",
                    "RoleArn": ROLE_ARN,
                    "RecipeReference": {"Name": recipe, "RecipeVersion": LATEST_WORKING},
                })),
            )
            .await
            .status,
        200,
        "the job that runs the draft must be created"
    );
    let refused = delete_version(&emulator, recipe, LATEST_WORKING).await;
    assert_eq!(
        refused.status, 409,
        "case \"a job that runs the draft\": expected 409, got {} {:?}",
        refused.status, refused.body
    );
    let message = refused.error_message();
    assert!(
        message.contains("run-draft") && message.contains(LATEST_WORKING),
        "case \"a job that runs the draft\": the refusal must name the job, got {message:?}"
    );
    assert_eq!(
        describe_version(&emulator, recipe, LATEST_WORKING).await.status,
        200,
        "case \"a job that runs the draft\": the held draft must still read back"
    );
}
