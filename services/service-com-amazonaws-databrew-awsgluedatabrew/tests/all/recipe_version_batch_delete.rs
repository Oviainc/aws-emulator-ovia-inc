// SPDX-License-Identifier: CC0-1.0

//! What `BatchDeleteRecipeVersion` deletes, what it keeps, and how it reports a version
//! it could not delete.
//!
//! The rules pinned here, both from the operation's own documentation
//! (`smithy_query(query="operation", operation="BatchDeleteRecipeVersion")`):
//!
//! * The WHOLE request is rejected, with nothing deleted, when the recipe does not
//!   exist, when the list carries an identifier this operation does not accept
//!   (`LATEST_PUBLISHED` is not one, nor is anything outside the documented `X.Y` form),
//!   and when it names a version twice.
//! * Everything else is decided PER VERSION: the request completes 200 and each version
//!   that survives is an item of `Errors` carrying the status of its refusal as
//!   `ErrorCode` (model: `RecipeVersionErrorDetail$ErrorCode` is
//!   `pattern ^[1-5][0-9][0-9]$`), the sentence of that refusal, and the identifier the
//!   request asked for. A version nothing holds is deleted and resolves nothing
//!   afterwards; the working version goes only when the recipe has no other versions and
//!   neither a project nor a job holds it.
//!
//! The tables span what those rules generalise over: how many versions the recipe holds,
//! which identifier form addresses each one (a number, an alias), how many versions one
//! request names and in which order, and every reason a single version survives.
//!
//! Every case drives the real wire path, so what is asserted is the HTTP status, the
//! error code and the JSON body a client sees, read back through `DescribeRecipe`.

use serde_json::{json, Value};

use crate::databrew_harness::{create_project, label, Emulator, Reply, ROLE_ARN};

/// The version a recipe carries the moment it is created, before any publish: its
/// working draft (`INITIAL_RECIPE_VERSION` in `src/state.rs`, from
/// `Recipe$RecipeVersion`). The draft's number is a function of the publish history —
/// it revises the highest published major — so a recipe with `N` publishes carries its
/// draft at `N.1` ([`draft_number`]).
const WORKING_VERSION: &str = "0.1";

/// The number the working draft carries after `publishes` publishes: the first minor
/// revision on top of the highest published major.
fn draft_number(publishes: usize) -> String {
    format!("{publishes}.1")
}

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// The alias naming the most recent published version, which this operation does NOT
/// accept ("LATEST_PUBLISHED is not supported", model:
/// `BatchDeleteRecipeVersionRequest$RecipeVersions`).
const LATEST_PUBLISHED: &str = "LATEST_PUBLISHED";

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

/// `BatchDeleteRecipeVersion`: `POST /recipes/{Name}/batchDeleteRecipeVersion`, with
/// `RecipeVersions` in the body.
async fn batch_delete(emulator: &Emulator, name: &str, versions: &[&str]) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/recipes/{}/batchDeleteRecipeVersion", label(name)),
            Some(json!({"RecipeVersions": versions})),
        )
        .await
}

/// The response's `Errors` list as the wire carried it, empty when the response omits it
/// (every version asked for was deleted).
fn errors(reply: &Reply) -> Vec<Value> {
    reply
        .body
        .get("Errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
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

/// One row of the per-version table: the versions a recipe holds, the versions one
/// request names, and which of them survive with which status.
struct BatchCase {
    /// What this row varies, named in every assertion.
    case: &'static str,
    /// How many publishes the recipe has had before the delete.
    publishes: usize,
    /// The `RecipeVersions` the request carries, in order.
    request: &'static [&'static str],
    /// The identifier of each expected `Errors` item, in order, with its `ErrorCode`.
    expected_failures: &'static [(&'static str, &'static str)],
    /// The versions that must still read back afterwards.
    still_held: &'static [&'static str],
    /// The versions that must resolve nothing afterwards.
    gone: &'static [&'static str],
    /// The number the working draft carries afterwards, or `None` when the delete took
    /// the recipe itself. A delete of a published version changes the publish history
    /// the draft's number is derived from, so the draft revises whatever major is left.
    draft_after: Option<&'static str>,
}

/// Every version a request names is decided on its own: the ones nothing holds are
/// deleted and resolve nothing afterwards, and the rest come back as `Errors` items in
/// the order the request named them, while the response itself is a 200.
#[tokio::test]
async fn each_requested_version_is_decided_on_its_own() {
    let cases = [
        BatchCase {
            case: "one published version of three",
            publishes: 3,
            request: &["1.0"],
            expected_failures: &[],
            still_held: &["2.0", "3.0", LATEST_WORKING],
            gone: &["1.0"],
            draft_after: Some("3.1"),
        },
        BatchCase {
            case: "two published versions at once",
            publishes: 3,
            request: &["3.0", "1.0"],
            expected_failures: &[],
            still_held: &["2.0", LATEST_WORKING],
            // The newest published major is gone, so the draft no longer revises it.
            gone: &["1.0", "3.0", "3.1"],
            draft_after: Some("2.1"),
        },
        BatchCase {
            case: "a version the recipe never held",
            publishes: 1,
            request: &["9.0"],
            expected_failures: &[("9.0", "404")],
            still_held: &["1.0", LATEST_WORKING],
            gone: &[],
            draft_after: Some("1.1"),
        },
        BatchCase {
            case: "a held version and a missing one, in request order",
            publishes: 2,
            request: &["2.0", "7.0", "1.0"],
            expected_failures: &[("7.0", "404")],
            still_held: &[LATEST_WORKING],
            gone: &["1.0", "2.0"],
            // Nothing is published any more, so the draft is back at major 0.
            draft_after: Some(WORKING_VERSION),
        },
        BatchCase {
            case: "the working version while published ones remain",
            publishes: 2,
            request: &[LATEST_WORKING],
            expected_failures: &[(LATEST_WORKING, "409")],
            still_held: &["1.0", "2.0", LATEST_WORKING],
            gone: &[],
            draft_after: Some("2.1"),
        },
        BatchCase {
            case: "the working version addressed by its own number",
            publishes: 1,
            request: &["1.1"],
            expected_failures: &[("1.1", "409")],
            still_held: &["1.0", LATEST_WORKING],
            gone: &[],
            draft_after: Some("1.1"),
        },
        BatchCase {
            case: "every version the recipe holds, working version last",
            publishes: 2,
            request: &["1.0", "2.0", LATEST_WORKING],
            expected_failures: &[],
            still_held: &[],
            gone: &["1.0", "2.0", LATEST_WORKING, WORKING_VERSION],
            draft_after: None,
        },
        BatchCase {
            case: "every version the recipe holds, working version first",
            publishes: 1,
            request: &[LATEST_WORKING, "1.0"],
            expected_failures: &[],
            still_held: &[],
            gone: &["1.0", LATEST_WORKING],
            draft_after: None,
        },
    ];

    for row in cases {
        let BatchCase {
            case,
            publishes,
            request,
            expected_failures,
            still_held,
            gone,
            draft_after,
        } = row;
        let emulator = Emulator::new();
        let recipe = "batched";
        recipe_with_published_versions(&emulator, recipe, publishes).await;
        assert_eq!(
            describe_version(&emulator, recipe, &draft_number(publishes))
                .await
                .status,
            200,
            "case {case:?}: the draft of a recipe with {publishes} publishes is \
             {}, which must read back before the delete",
            draft_number(publishes)
        );

        let replied = batch_delete(&emulator, recipe, request).await;
        assert_eq!(
            replied.status, 200,
            "case {case:?}: a partial failure is still a 200, got {} {:?}",
            replied.status, replied.body
        );
        assert_eq!(
            replied.text("Name"),
            recipe,
            "case {case:?}: the response must name the recipe it modified, got {:?}",
            replied.body
        );

        let reported = errors(&replied);
        let reported_pairs: Vec<(String, String)> = reported
            .iter()
            .map(|item| {
                (
                    item.get("RecipeVersion")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    item.get("ErrorCode")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                )
            })
            .collect();
        let expected_pairs: Vec<(String, String)> = expected_failures
            .iter()
            .map(|(version, code)| (version.to_string(), code.to_string()))
            .collect();
        assert_eq!(
            reported_pairs, expected_pairs,
            "case {case:?}: expected {expected_pairs:?}, got {:?}",
            replied.body
        );
        for item in &reported {
            assert!(
                item.get("ErrorMessage")
                    .and_then(Value::as_str)
                    .is_some_and(|message| !message.is_empty()),
                "case {case:?}: every item must state why its version survived, got {item:?}"
            );
        }

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
                        .member("RecipeVersion"),
                    json!(draft),
                    "case {case:?}: the draft must report {draft} as its own number"
                );
            }
            None => {
                let described = describe_version(&emulator, recipe, LATEST_WORKING).await;
                assert_eq!(
                    described.status, 404,
                    "case {case:?}: the recipe itself is gone, so no draft remains, got {} {:?}",
                    described.status, described.body
                );
            }
        }
    }
}

/// A version the recipe does not hold is reported as the miss it is, naming the recipe
/// and the identifier the item asked for — over every form a numeric identifier takes.
#[tokio::test]
async fn a_version_the_recipe_does_not_hold_is_reported_as_a_miss() {
    let emulator = Emulator::new();
    let recipe = "sparse";
    recipe_with_published_versions(&emulator, recipe, 1).await;

    // Every identifier here is well formed (model: `Recipe$RecipeVersion` — `X.Y` with up
    // to 6 digits in each half, never `0.0`) and names no version this recipe holds.
    for version in ["2.0", "0.2", "12.34", "999999.999999"] {
        let replied = batch_delete(&emulator, recipe, &[version]).await;
        assert_eq!(
            replied.status, 200,
            "case {version:?}: expected 200, got {} {:?}",
            replied.status, replied.body
        );
        let reported = errors(&replied);
        assert_eq!(
            reported.len(),
            1,
            "case {version:?}: expected one item, got {:?}",
            replied.body
        );
        let expected = format!("The recipe {recipe} version {version} wasn't found.");
        assert_eq!(
            reported[0].get("ErrorMessage").and_then(Value::as_str),
            Some(expected.as_str()),
            "case {version:?}: expected {expected:?}, got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0].get("ErrorCode").and_then(Value::as_str),
            Some("404"),
            "case {version:?}: a miss carries the status of the not-found refusal, got {:?}",
            reported[0]
        );
    }
}

/// The working version is held by whatever develops or runs it: a project that develops
/// the recipe and a job that references the version each keep it, and the item names the
/// holder so a client knows what to clear first.
#[tokio::test]
async fn a_version_something_still_holds_survives_the_delete() {
    // A project develops the recipe's working version.
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

    for version in [LATEST_WORKING, WORKING_VERSION] {
        let replied = batch_delete(&emulator, recipe, &[version]).await;
        assert_eq!(
            replied.status, 200,
            "case {version:?}: expected 200, got {} {:?}",
            replied.status, replied.body
        );
        let reported = errors(&replied);
        assert_eq!(
            reported.len(),
            1,
            "case {version:?}: expected one item, got {:?}",
            replied.body
        );
        let message = reported[0]
            .get("ErrorMessage")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains("holder") && message.contains(version),
            "case {version:?}: the item must name the project holding the version, got {:?}",
            reported[0]
        );
        assert_eq!(
            reported[0].get("ErrorCode").and_then(Value::as_str),
            Some("409"),
            "case {version:?}: a held version carries the status of the conflict, got {:?}",
            reported[0]
        );
        assert_eq!(
            describe_version(&emulator, recipe, version).await.status,
            200,
            "case {version:?}: the held version must still read back"
        );
    }

    // A job that references one published version holds exactly that version: the other
    // published version of the same recipe is deleted in the same request.
    let emulator = Emulator::new();
    let recipe = "run-by-a-job";
    recipe_with_published_versions(&emulator, recipe, 2).await;
    assert_eq!(
        emulator
            .call(
                "POST",
                "/recipeJobs",
                Some(json!({
                    "Name": "job-on-1-0",
                    "RoleArn": ROLE_ARN,
                    "RecipeReference": {"Name": recipe, "RecipeVersion": "1.0"},
                })),
            )
            .await
            .status,
        200,
        "the job that runs version 1.0 must be created"
    );

    let replied = batch_delete(&emulator, recipe, &["1.0", "2.0"]).await;
    assert_eq!(
        replied.status, 200,
        "case \"a version a job runs\": expected 200, got {} {:?}",
        replied.status, replied.body
    );
    let reported = errors(&replied);
    assert_eq!(
        reported.len(),
        1,
        "case \"a version a job runs\": only the referenced version survives, got {:?}",
        replied.body
    );
    let message = reported[0]
        .get("ErrorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        message.contains("job-on-1-0") && message.contains("1.0"),
        "case \"a version a job runs\": the item must name the job, got {:?}",
        reported[0]
    );
    assert_eq!(
        describe_version(&emulator, recipe, "1.0").await.status,
        200,
        "case \"a version a job runs\": the referenced version must still read back"
    );
    assert_eq!(
        describe_version(&emulator, recipe, "2.0").await.status,
        404,
        "case \"a version a job runs\": the unreferenced version must be deleted"
    );
}

/// Deleting the working version of a recipe nothing else holds leaves no recipe at all:
/// the record was what held its versions, so every lookup of it resolves nothing
/// afterwards.
#[tokio::test]
async fn the_last_version_takes_the_recipe_with_it() {
    let emulator = Emulator::new();

    for (case, requested) in [
        ("addressed by its alias", LATEST_WORKING),
        ("addressed by its own number", WORKING_VERSION),
    ] {
        let recipe = "only-a-draft";
        recipe_with_published_versions(&emulator, recipe, 0).await;

        let replied = batch_delete(&emulator, recipe, &[requested]).await;
        assert_eq!(
            replied.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            replied.status, replied.body
        );
        assert!(
            errors(&replied).is_empty(),
            "case {case:?}: nothing held the version, so nothing must be reported, got {:?}",
            replied.body
        );
        for version in [LATEST_WORKING, WORKING_VERSION, LATEST_PUBLISHED] {
            let described = describe_version(&emulator, recipe, version).await;
            assert_eq!(
                described.status, 404,
                "case {case:?}: {version} of a deleted recipe must resolve nothing, got {} {:?}",
                described.status, described.body
            );
        }
        // A second delete of the same recipe is a miss rather than a second success.
        let again = batch_delete(&emulator, recipe, &[requested]).await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: the recipe is gone, so the second delete must miss, got {} {:?}",
            again.status, again.body
        );
    }
}

/// One row of the wholesale-rejection table: a request the operation refuses outright,
/// with the status and error code it refuses with.
struct RejectedCase {
    case: &'static str,
    recipe: &'static str,
    versions: &'static [&'static str],
    status: u16,
    code: &'static str,
}

/// The documented wholesale rejections refuse the request and delete NOTHING: the recipe
/// still holds every version it held before.
#[tokio::test]
async fn a_rejected_request_deletes_nothing() {
    let emulator = Emulator::new();
    let recipe = "intact";
    recipe_with_published_versions(&emulator, recipe, 2).await;

    let cases = [
        RejectedCase {
            case: "a recipe nobody created",
            recipe: "no-such-recipe",
            versions: &["1.0"],
            status: 404,
            code: "ResourceNotFoundException",
        },
        RejectedCase {
            case: "the latest-published alias, which this operation does not accept",
            recipe: "intact",
            versions: &[LATEST_PUBLISHED],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "the latest-published alias beside a version that exists",
            recipe: "intact",
            versions: &["1.0", LATEST_PUBLISHED],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "a version that is not X.Y",
            recipe: "intact",
            versions: &["one"],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "a version with three halves",
            recipe: "intact",
            versions: &["1.2.3"],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "a version with an empty half",
            recipe: "intact",
            versions: &["1."],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "the version the model calls invalid",
            recipe: "intact",
            versions: &["0.0"],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "a half longer than six digits",
            recipe: "intact",
            versions: &["1234567.0"],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "the same version named twice",
            recipe: "intact",
            versions: &["1.0", "1.0"],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "the same alias named twice",
            recipe: "intact",
            versions: &[LATEST_WORKING, LATEST_WORKING],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            // The recipe has had two publishes, so its draft carries the number `2.1`:
            // the alias and that number name ONE version, and naming both names it twice.
            case: "the working version named by both of its identifiers",
            recipe: "intact",
            versions: &[LATEST_WORKING, "2.1"],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "the working version named by its number and then its alias",
            recipe: "intact",
            versions: &["2.1", LATEST_WORKING],
            status: 400,
            code: "ValidationException",
        },
        RejectedCase {
            case: "an empty version list, the modelled minimum",
            recipe: "intact",
            versions: &[],
            status: 400,
            code: "ValidationException",
        },
    ];

    for row in cases {
        let RejectedCase {
            case,
            recipe: addressed,
            versions,
            status,
            code,
        } = row;
        let refused = batch_delete(&emulator, addressed, versions).await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected {status}, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some(code),
            "case {case:?}: expected {code}, got {:?}",
            refused.error_type
        );
        if status == 404 {
            let expected = format!("Recipe {addressed} wasn't found.");
            assert_eq!(
                refused.error_message(),
                expected,
                "case {case:?}: expected {expected:?}, got {:?}",
                refused.error_message()
            );
        }
        for version in ["1.0", "2.0", LATEST_WORKING, "2.1"] {
            let described = describe_version(&emulator, recipe, version).await;
            assert_eq!(
                described.status, 200,
                "case {case:?}: {version} must survive a rejected request, got {} {:?}",
                described.status, described.body
            );
        }
    }
}
