// SPDX-License-Identifier: CC0-1.0

//! What `PublishRecipe` freezes, and what it leaves alone.
//!
//! The rule pinned here: a publish is ADDITIVE — it copies the definition the recipe's
//! working version holds into a version of its own, numbered with the next whole major
//! number at minor 0 (model: `Recipe$RecipeVersion` — `X.Y`, both halves required and
//! `"0.0"` invalid, so a recipe created at major 0 publishes `1.0` first and each later
//! publish takes the next whole number), stamped with the publishing principal and the
//! publish instant, and reachable as `LATEST_PUBLISHED`; the working version carries on
//! unchanged as the draft, and a version an earlier publish froze is never rewritten.
//!
//! The tables span what the rule generalises over: how many publishes a recipe has had,
//! every shape a `RecipeStepList` takes, and whether the publish carries a description
//! of its own, none at all, or an empty one.
//!
//! Every case drives the real wire path, so what is asserted is the HTTP status, the
//! error code and the JSON body a client sees, read back through `DescribeRecipe`.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply, CALLER_PRINCIPAL};

/// The alias naming the most recent published version (model: `Recipe$RecipeVersion`).
const LATEST_PUBLISHED: &str = "LATEST_PUBLISHED";

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// `CreateRecipe`: `POST /recipes`.
async fn create_recipe(
    emulator: &Emulator,
    name: &str,
    description: Option<&str>,
    steps: &Value,
) -> Reply {
    let mut body = json!({"Name": name, "Steps": steps.clone()});
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    emulator.call("POST", "/recipes", Some(body)).await
}

/// `PublishRecipe`: `POST /recipes/{Name}/publishRecipe`, with the optional
/// `Description` in the body (model: `smithy_query(query="operation",
/// operation="PublishRecipe")`).
async fn publish_recipe(emulator: &Emulator, name: &str, description: Option<&str>) -> Reply {
    let body = match description {
        Some(description) => json!({"Description": description}),
        None => json!({}),
    };
    emulator
        .call(
            "POST",
            &format!("/recipes/{}/publishRecipe", label(name)),
            Some(body),
        )
        .await
}

/// `DescribeRecipe`: `GET /recipes/{Name}`, with the optional `recipeVersion` query
/// parameter the model binds `RecipeVersion` to.
async fn describe_recipe(emulator: &Emulator, name: &str, version: Option<&str>) -> Reply {
    let uri = match version {
        Some(version) => format!("/recipes/{}?recipeVersion={version}", label(name)),
        None => format!("/recipes/{}", label(name)),
    };
    emulator.call("GET", &uri, None).await
}

/// The `Message` of a modelled DataBrew error, whichever casing the protocol put it on
/// the wire with.
fn error_message(reply: &Reply) -> String {
    reply
        .body
        .get("Message")
        .or_else(|| reply.body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// Every shape a `RecipeStepList` takes: no steps, one step with and without
/// parameters, several steps, and a step carrying condition expressions (model:
/// `smithy_query(query="shape", shape="RecipeStep,RecipeAction,ConditionExpression")`).
/// The `Operation` and `Condition` values satisfy the modelled `^[A-Z\_]+$` pattern.
fn step_shapes() -> [(&'static str, Value); 4] {
    [
        (
            "one step with parameters",
            json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]),
        ),
        (
            "one step whose action carries no parameters",
            json!([{"Action": {"Operation": "DELETE"}}]),
        ),
        (
            "several steps, in the order given",
            json!([
                {"Action": {"Operation": "RENAME", "Parameters": {"sourceColumn": "id", "targetColumn": "orderId"}}},
                {
                    "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "status"}},
                    "ConditionExpressions": [{"Condition": "IS", "Value": "cancelled", "TargetColumn": "status"}]
                }
            ]),
        ),
        ("no steps at all", json!([])),
    ]
}

/// One row of the description table: the case name, the description the recipe was
/// created with, and the description each successive publish carries.
type DescriptionRun = (
    &'static str,
    Option<&'static str>,
    &'static [Option<&'static str>],
);

/// A publish freezes the working definition at the next whole major version, stamps it
/// with who published it and when, and makes it the latest published version — however
/// many publishes the recipe has had, whatever its steps, and whether or not the publish
/// carried a description of its own.
#[tokio::test]
async fn a_publish_freezes_the_working_definition_at_the_next_whole_version() {
    // The descriptions successive publishes carry, which is the other axis the rule
    // generalises over: one of its own, none at all (the frozen copy keeps the
    // definition's), and an empty one.
    let description_runs: [DescriptionRun; 5] = [
        (
            "one publish carrying its own description",
            Some("the draft body"),
            &[Some("frozen for release")],
        ),
        (
            "three publishes, each with its own description",
            Some("the draft body"),
            &[Some("first cut"), Some("second cut"), Some("third cut")],
        ),
        (
            "a publish carrying no description of its own",
            Some("the draft body"),
            &[None],
        ),
        (
            "a recipe created without a description, published twice",
            None,
            &[None, Some("named at the second publish")],
        ),
        (
            "a publish carrying an empty description",
            Some("the draft body"),
            &[Some("")],
        ),
    ];

    let emulator = Emulator::new();
    for (steps_index, (steps_case, steps)) in step_shapes().into_iter().enumerate() {
        for (run_index, (run_case, created_with, publishes)) in
            description_runs.iter().copied().enumerate()
        {
            let case = format!("{steps_case} / {run_case}");
            let name = format!("recipe-{steps_index}-{run_index}");
            assert_eq!(
                create_recipe(&emulator, &name, created_with, &steps)
                    .await
                    .status,
                200,
                "case {case:?}: the recipe under test must be created"
            );

            for (publish_index, carried) in publishes.iter().copied().enumerate() {
                let published = publish_recipe(&emulator, &name, carried).await;
                assert_eq!(
                    published.status, 200,
                    "case {case:?}: publish {publish_index} must answer 200, got {} {:?}",
                    published.status, published.body
                );
                assert_eq!(
                    published.text("Name"),
                    name,
                    "case {case:?}: publish {publish_index} must echo the recipe it published, \
                     got {:?}",
                    published.body
                );

                // The next whole major number at minor 0: the first publish of a recipe
                // created at major 0 is `1.0`.
                let expected_version = format!("{}.0", publish_index + 1);
                // The description the frozen version carries: the publish's own when it
                // carried one, and otherwise the definition's.
                let expected_description = carried.or(created_with);

                for (addressed, identifier) in [
                    ("by its own number", expected_version.as_str()),
                    ("by the published alias", LATEST_PUBLISHED),
                ] {
                    let frozen = describe_recipe(&emulator, &name, Some(identifier)).await;
                    assert_eq!(
                        frozen.status, 200,
                        "case {case:?}: version {expected_version} {addressed} must resolve, \
                         got {} {:?}",
                        frozen.status, frozen.body
                    );
                    assert_eq!(
                        frozen.text("RecipeVersion"),
                        expected_version,
                        "case {case:?}: expected version {expected_version} {addressed}, got {:?}",
                        frozen.body["RecipeVersion"]
                    );
                    assert_eq!(
                        frozen.member("Steps"),
                        steps,
                        "case {case:?}: version {expected_version} must hold the definition it \
                         froze, got {:?}",
                        frozen.body["Steps"]
                    );
                    match expected_description {
                        Some(description) => assert_eq!(
                            frozen.text("Description"),
                            description,
                            "case {case:?}: expected version {expected_version} to carry \
                             {description:?}, got {:?}",
                            frozen.body["Description"]
                        ),
                        None => assert!(
                            frozen.is_absent("Description"),
                            "case {case:?}: version {expected_version} has no description to \
                             report, got {:?}",
                            frozen.body["Description"]
                        ),
                    }
                    assert_eq!(
                        frozen.text("PublishedBy"),
                        CALLER_PRINCIPAL,
                        "case {case:?}: version {expected_version} must name the publishing \
                         principal, got {:?}",
                        frozen.body["PublishedBy"]
                    );
                    assert!(
                        frozen.body["PublishedDate"].as_f64().unwrap_or_default() > 0.0,
                        "case {case:?}: version {expected_version} must carry the publish \
                         instant, got {:?}",
                        frozen.body["PublishedDate"]
                    );
                    assert_eq!(
                        frozen.text("CreatedBy"),
                        CALLER_PRINCIPAL,
                        "case {case:?}: version {expected_version} must name who created it, \
                         got {:?}",
                        frozen.body["CreatedBy"]
                    );
                }

                // A versionless describe asks for the latest published version (model:
                // `DescribeRecipeRequest$RecipeVersion`), so it answers the version this
                // publish just froze.
                let versionless = describe_recipe(&emulator, &name, None).await;
                assert_eq!(
                    versionless.text("RecipeVersion"),
                    expected_version,
                    "case {case:?}: a versionless describe must answer the newest published \
                     version, got {} {:?}",
                    versionless.status,
                    versionless.body
                );

                // The draft keeps its definition and its body across the publish, and
                // still has no publish of its own to report; its NUMBER follows the
                // publish history, as the draft is the first minor revision on top of
                // the major just frozen.
                let draft = describe_recipe(&emulator, &name, Some(LATEST_WORKING)).await;
                assert_eq!(
                    draft.text("RecipeVersion"),
                    format!("{}.1", publish_index + 1),
                    "case {case:?}: after publish {publish_index} the draft revises the major \
                     just frozen, got {:?}",
                    draft.body["RecipeVersion"]
                );
                assert_eq!(
                    draft.member("Steps"),
                    steps,
                    "case {case:?}: the draft keeps its definition across publish \
                     {publish_index}, got {:?}",
                    draft.body["Steps"]
                );
                match created_with {
                    Some(description) => assert_eq!(
                        draft.text("Description"),
                        description,
                        "case {case:?}: the draft keeps its own body across publish \
                         {publish_index}, got {:?}",
                        draft.body["Description"]
                    ),
                    None => assert!(
                        draft.is_absent("Description"),
                        "case {case:?}: the draft has no description to report, got {:?}",
                        draft.body["Description"]
                    ),
                }
                for absent in ["PublishedBy", "PublishedDate"] {
                    assert!(
                        draft.is_absent(absent),
                        "case {case:?}: no publish has frozen the draft, so it reports no \
                         {absent}, got {:?}",
                        draft.body[absent]
                    );
                }
            }

            // Every version the run published is still addressable by its own number
            // afterwards, so a publish adds beside the earlier ones rather than
            // replacing them.
            for publish_index in 0..publishes.len() {
                let version = format!("{}.0", publish_index + 1);
                let held = describe_recipe(&emulator, &name, Some(&version)).await;
                assert_eq!(
                    held.status, 200,
                    "case {case:?}: version {version} must still resolve after {} publishes, \
                     got {} {:?}",
                    publishes.len(),
                    held.status,
                    held.body
                );
            }
        }
    }
}

/// A version an earlier publish froze keeps the definition it was frozen with after the
/// draft moves on: the frozen copy is independent of the working version.
///
/// The draft is moved on with `UpdateRecipe`, the operation that modifies the working
/// version, so every call below is made over the wire.
#[tokio::test]
async fn an_earlier_version_keeps_the_definition_it_was_frozen_with() {
    let emulator = Emulator::new();
    let first_steps = json!([
        {"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}},
        {"Action": {"Operation": "DELETE", "Parameters": {"sourceColumn": "note"}}}
    ]);
    // The draft after the edit below drops every step, so the two versions hold
    // demonstrably different definitions.
    let second_steps = json!([]);
    let name = "redrafted";

    assert_eq!(
        create_recipe(&emulator, name, Some("the first draft"), &first_steps)
            .await
            .status,
        200,
        "case \"a redrafted recipe\": the recipe under test must be created"
    );
    assert_eq!(
        publish_recipe(&emulator, name, Some("frozen from the first draft"))
            .await
            .status,
        200,
        "case \"a redrafted recipe\": the first publish must answer 200"
    );

    // The draft moves on: `UpdateRecipe` replaces the working version's definition with
    // the one this request carries.
    let redrafted = emulator
        .call(
            "PUT",
            &format!("/recipes/{}", label(name)),
            Some(json!({"Description": "the second draft", "Steps": second_steps.clone()})),
        )
        .await;
    assert_eq!(
        redrafted.status, 200,
        "case \"a redrafted recipe\": the update of the draft must answer 200, got {} {:?}",
        redrafted.status, redrafted.body
    );

    assert_eq!(
        publish_recipe(&emulator, name, None).await.status,
        200,
        "case \"a redrafted recipe\": the second publish must answer 200"
    );

    let expected: [(&str, &str, &Value, &str); 2] = [
        (
            "the version frozen from the first draft",
            "1.0",
            &first_steps,
            "frozen from the first draft",
        ),
        (
            "the version frozen from the second draft",
            "2.0",
            &second_steps,
            "the second draft",
        ),
    ];
    for (case, version, steps, description) in expected {
        let held = describe_recipe(&emulator, name, Some(version)).await;
        assert_eq!(
            held.status, 200,
            "case {case:?}: version {version} must resolve, got {} {:?}",
            held.status, held.body
        );
        assert_eq!(
            held.member("Steps"),
            *steps,
            "case {case:?}: version {version} must hold the definition it froze, got {:?}",
            held.body["Steps"]
        );
        assert_eq!(
            held.text("Description"),
            description,
            "case {case:?}: expected {description:?}, got {:?}",
            held.body["Description"]
        );
    }

    // The newest publish is what the alias and a versionless describe answer.
    for identifier in [None, Some(LATEST_PUBLISHED)] {
        let latest = describe_recipe(&emulator, name, identifier).await;
        assert_eq!(
            latest.text("RecipeVersion"),
            "2.0",
            "case \"the latest published version\" ({identifier:?}): expected the newest publish, \
             got {} {:?}",
            latest.status,
            latest.body
        );
    }
}

/// A publish of a recipe this account holds none of is 404 `ResourceNotFoundException`
/// naming the recipe that was looked up, and freezes nothing.
#[tokio::test]
async fn publishing_a_recipe_nobody_created_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the form of the name the request addressed: every one of them
    // names no stored recipe, so every one of them is the same miss.
    let absent = [
        ("a plain name", "ghost"),
        ("a name with a space", "a ghost with a space"),
        ("a single-character name", "g"),
        ("a name outside the documented character set", "under_score"),
    ];
    for (case, name) in absent {
        let refused = publish_recipe(&emulator, name, Some("whatever")).await;
        assert_eq!(
            refused.status, 404,
            "case {case:?} ({name:?}): expected 404, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?} ({name:?}): expected ResourceNotFoundException, got {:?}",
            refused.error_type
        );
        let expected = format!("Recipe {name} wasn't found.");
        assert_eq!(
            error_message(&refused),
            expected,
            "case {case:?} ({name:?}): expected {expected:?}, got {:?}",
            error_message(&refused)
        );
        let described = describe_recipe(&emulator, name, Some(LATEST_PUBLISHED)).await;
        assert_eq!(
            described.status, 404,
            "case {case:?} ({name:?}): the refused publish must freeze nothing, got {} {:?}",
            described.status, described.body
        );
    }

    // A recipe that exists but has never been published still holds no published
    // version, and the publish of a DIFFERENT name does not give it one.
    let steps = json!([{"Action": {"Operation": "DELETE"}}]);
    assert_eq!(
        create_recipe(&emulator, "untouched", Some("never published"), &steps)
            .await
            .status,
        200,
        "case \"an unpublished recipe\": the recipe under test must be created"
    );
    let unpublished = describe_recipe(&emulator, "untouched", Some(LATEST_PUBLISHED)).await;
    assert_eq!(
        unpublished.status, 404,
        "case \"an unpublished recipe\": a recipe nobody published holds no published version, \
         got {} {:?}",
        unpublished.status, unpublished.body
    );
}
