// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for DataBrew recipes: `CreateRecipe` -> `DescribeRecipe`.
//!
//! Every case drives the real wire path (`MultiServiceRouter::dispatch` with a
//! restJson1 request), so what is asserted is the HTTP status, the error code and the
//! JSON body a client sees.
//!
//! The tables below span the classes the implementation generalises over rather than
//! one recorded request: every shape a `RecipeStepList` can take (no steps, one step,
//! several steps in order, actions with and without parameters, steps with and without
//! condition expressions), every version identifier the model documents (both aliases,
//! a numeric `X.Y` version, each at its published digit bound and one past it) and
//! every form a recipe name can take.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply, ARN_PREFIX, CALLER_PRINCIPAL};

/// The version a recipe carries the moment it is created: the first minor of major 0,
/// the working version no publish has frozen (`INITIAL_RECIPE_VERSION` in
/// `src/state.rs`).
const INITIAL_VERSION: &str = "0.1";

/// `CreateRecipe`: `POST /recipes` (model: `smithy_query(query="operation",
/// operation="CreateRecipe")`).
async fn create_recipe(emulator: &Emulator, body: Value) -> Reply {
    emulator.call("POST", "/recipes", Some(body)).await
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

/// A recipe reads back exactly the definition it was created with, for every shape a
/// `RecipeStepList` takes and whether or not the create carried a description or tags.
///
/// The rows differ along step count, whether an action carries `Parameters`, and
/// whether a step carries `ConditionExpressions` — the three places the modelled
/// `RecipeStep` leaves optional (model: `smithy_query(query="shape",
/// shape="RecipeStep,RecipeAction,ConditionExpression")`). The `Operation` and
/// `Condition` values below satisfy the modelled `^[A-Z\_]+$` pattern.
#[tokio::test]
async fn a_recipe_round_trips_every_shape_of_steps() {
    let cases: [(&str, Option<&str>, Value, Option<Value>); 6] = [
        (
            "one step with parameters",
            Some("upper-cases one column"),
            json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}}}]),
            Some(json!({"team": "data"})),
        ),
        (
            "one step whose action carries no parameters",
            None,
            json!([{"Action": {"Operation": "DELETE"}}]),
            None,
        ),
        (
            "several steps, in the order given",
            Some("rename, retype, then filter"),
            json!([
                {"Action": {"Operation": "RENAME", "Parameters": {"sourceColumn": "id", "targetColumn": "orderId"}}},
                {"Action": {"Operation": "CHANGE_DATA_TYPE", "Parameters": {"sourceColumn": "amount", "columnDataType": "double"}}},
                {"Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "status"}}}
            ]),
            Some(json!({"team": "data", "env": "test"})),
        ),
        (
            "a step with several condition expressions",
            None,
            json!([{
                "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "status"}},
                "ConditionExpressions": [
                    {"Condition": "IS", "Value": "cancelled", "TargetColumn": "status"},
                    {"Condition": "IS_NOT", "TargetColumn": "status"}
                ]
            }]),
            None,
        ),
        (
            "no steps at all",
            Some("a recipe that does nothing yet"),
            json!([]),
            None,
        ),
        (
            "an empty description",
            Some(""),
            json!([{"Action": {"Operation": "REPLACE_TEXT", "Parameters": {"sourceColumn": "note", "pattern": "a", "value": "b"}}}]),
            Some(json!({"only": "one"})),
        ),
    ];

    let emulator = Emulator::new();
    for (index, (case, description, steps, tags)) in cases.into_iter().enumerate() {
        let name = format!("recipe-{index}");
        let mut body = json!({"Name": name, "Steps": steps});
        if let Some(description) = description {
            body["Description"] = json!(description);
        }
        if let Some(tags) = &tags {
            body["Tags"] = tags.clone();
        }

        let created = create_recipe(&emulator, body.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected CreateRecipe to answer 200, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            created.text("Name"),
            name,
            "case {case:?}: CreateRecipe must echo the name it stored, got {:?}",
            created.body
        );

        let described = describe_recipe(&emulator, &name, None).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected DescribeRecipe to answer 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.member("Steps"),
            body["Steps"],
            "case {case:?}: expected the stored Steps back, got {:?}",
            described.body["Steps"]
        );
        match description {
            Some(description) => assert_eq!(
                described.text("Description"),
                description,
                "case {case:?}: expected the stored Description back, got {:?}",
                described.body["Description"]
            ),
            None => assert!(
                described.is_absent("Description"),
                "case {case:?}: a recipe created without a Description must report none, got {:?}",
                described.body["Description"]
            ),
        }
        match &tags {
            Some(tags) => assert_eq!(
                described.member("Tags"),
                *tags,
                "case {case:?}: expected the stored Tags back, got {:?}",
                described.body["Tags"]
            ),
            // `TagMap` is `len 1..200`, so an untagged recipe carries no map at all.
            None => assert!(
                described.is_absent("Tags"),
                "case {case:?}: an untagged recipe must report no Tags, got {:?}",
                described.body["Tags"]
            ),
        }
        assert_eq!(
            described.text("Name"),
            name,
            "case {case:?}: expected the recipe's own name back, got {:?}",
            described.body["Name"]
        );
        assert_eq!(
            described.text("RecipeVersion"),
            INITIAL_VERSION,
            "case {case:?}: a created recipe is at its working version, got {:?}",
            described.body["RecipeVersion"]
        );
        assert_eq!(
            described.text("ResourceArn"),
            format!("{ARN_PREFIX}recipe/{name}"),
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
        // A recipe that has only been created has not been modified, published or
        // attached to a project, and the model makes all of those members optional.
        for absent in [
            "LastModifiedDate",
            "LastModifiedBy",
            "PublishedBy",
            "PublishedDate",
            "ProjectName",
        ] {
            assert!(
                described.is_absent(absent),
                "case {case:?}: a freshly created recipe must report no {absent}, got {:?}",
                described.body[absent]
            );
        }
    }
}

/// A `RecipeVersion` resolves to a version the named recipe holds, and to nothing
/// else.
///
/// Source for the identifier forms: the model's `Recipe$RecipeVersion` doc — a numeric
/// `X.Y` version with "the maximum length of each is 6 digits", neither half negative
/// and `"0.0"` invalid, or the `LATEST_WORKING` and `LATEST_PUBLISHED` aliases — and
/// `DescribeRecipeRequest$RecipeVersion`, which makes an omitted identifier ask for the
/// latest published version. A recipe nobody has published holds only its working
/// version, so that is what resolves; `LATEST_PUBLISHED` asked for by name resolves
/// nothing.
#[tokio::test]
async fn a_recipe_version_resolves_only_a_version_the_recipe_holds() {
    let emulator = Emulator::new();
    let steps = json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}}}]);
    assert_eq!(
        create_recipe(&emulator, json!({"Name": "versioned", "Steps": steps})).await.status,
        200,
        "the recipe under test must be created"
    );

    let resolved = [
        ("no identifier at all", None),
        ("the working-version alias", Some("LATEST_WORKING")),
        ("the working version by number", Some(INITIAL_VERSION)),
    ];
    for (case, version) in resolved {
        let described = describe_recipe(&emulator, "versioned", version).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected 200, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.text("RecipeVersion"),
            INITIAL_VERSION,
            "case {case:?}: expected the resolved version rather than the alias asked for, got {:?}",
            described.body["RecipeVersion"]
        );
        assert_eq!(
            described.member("Steps"),
            steps,
            "case {case:?}: expected that version's steps, got {:?}",
            described.body["Steps"]
        );
    }

    // A well-formed identifier naming a version this recipe does not hold: nothing is
    // published yet, and no other numeric version exists. `999999.999999` is the
    // documented 6-digit maximum of each half, so it is well formed and still unheld.
    let unheld = [
        ("the published-version alias, nothing published", "LATEST_PUBLISHED"),
        ("a published major version", "1.0"),
        ("another minor of the working major", "0.2"),
        ("both halves at the documented 6-digit maximum", "999999.999999"),
    ];
    for (case, version) in unheld {
        let described = describe_recipe(&emulator, "versioned", Some(version)).await;
        assert_eq!(
            described.status, 404,
            "case {case:?} ({version}): expected 404, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case {case:?} ({version}): expected ResourceNotFoundException, got {:?}",
            described.error_type
        );
    }

    // An identifier of no documented form is a bad request, not a missing resource:
    // `DescribeRecipe` models `ValidationException` (HTTP 400) for exactly that.
    let malformed = [
        ("zero version", "0.0"),
        ("zero version, padded", "00.00"),
        ("a major with no minor", "1"),
        ("a trailing separator", "1."),
        ("a leading separator", ".1"),
        ("three halves", "1.2.3"),
        ("a negative major", "-1.0"),
        ("a 7-digit major, one past the documented maximum", "1000000.0"),
        ("a 7-digit minor, one past the documented maximum", "1.1000000"),
        ("an alias in the wrong case", "latest_working"),
        ("a truncated alias", "LATEST"),
        ("a non-numeric minor", "1.a"),
        ("a prefixed version", "v1.0"),
    ];
    for (case, version) in malformed {
        let refused = describe_recipe(&emulator, "versioned", Some(version)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({version}): expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({version}): expected ValidationException, got {:?}",
            refused.error_type
        );
        assert!(
            error_message(&refused).contains("RecipeVersion"),
            "case {case:?} ({version}): the message must name the member that failed, got {:?}",
            refused.body
        );
    }

    // The version is resolved within the recipe the request names: a version another
    // recipe holds is still not this one's, and a recipe nobody created resolves
    // nothing whichever version is asked for.
    for version in [None, Some("LATEST_WORKING"), Some(INITIAL_VERSION)] {
        let described = describe_recipe(&emulator, "ghost", version).await;
        assert_eq!(
            described.status, 404,
            "case \"a recipe nobody created\" ({version:?}): expected 404, got {} {:?}",
            described.status, described.body
        );
        assert_eq!(
            described.error_type.as_deref(),
            Some("ResourceNotFoundException"),
            "case \"a recipe nobody created\" ({version:?}): expected ResourceNotFoundException, got {:?}",
            described.error_type
        );
    }
}

/// A recipe name is unique and comes from the documented character set, and a refused
/// create stores nothing.
#[tokio::test]
async fn a_recipe_name_is_unique_and_from_the_documented_character_set() {
    let emulator = Emulator::new();
    let steps = json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "name"}}}]);

    // The documented character set: "Valid characters are alphanumeric (A-Z, a-z,
    // 0-9), hyphen (-), period (.), and space"
    // (`smithy_query(query="doc", shape="CreateRecipeRequest", member="Name")`).
    for name in ["plain", "with-hyphen", "with.period", "with space", "MiXeD123"] {
        let created = create_recipe(
            &emulator,
            json!({"Name": name, "Steps": steps, "Description": "first"}),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {name:?}: a name in the documented set must be accepted, got {} {:?}",
            created.status, created.body
        );
        assert_eq!(
            describe_recipe(&emulator, name, None).await.text("Name"),
            name,
            "case {name:?}: the stored recipe must be addressable by that name"
        );
    }

    for name in ["under_score", "with+plus", "with$dollar", "   "] {
        let refused = create_recipe(&emulator, json!({"Name": name, "Steps": steps})).await;
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
    // The refused create stored nothing, read back through the one refused name a URI
    // path can carry unencoded.
    let ghost = describe_recipe(&emulator, "under_score", None).await;
    assert_eq!(
        ghost.status, 404,
        "case \"under_score\": the refused create must store nothing, got {} {:?}",
        ghost.status, ghost.body
    );

    // A second create of the same name conflicts, and leaves the stored recipe as the
    // first create wrote it.
    let duplicate = create_recipe(
        &emulator,
        json!({"Name": "plain", "Steps": json!([]), "Description": "second"}),
    )
    .await;
    assert_eq!(
        duplicate.status, 409,
        "case \"duplicate recipe name\": expected 409, got {} {:?}",
        duplicate.status, duplicate.body
    );
    assert_eq!(
        duplicate.error_type.as_deref(),
        Some("ConflictException"),
        "case \"duplicate recipe name\": expected ConflictException, got {:?}",
        duplicate.error_type
    );
    let described = describe_recipe(&emulator, "plain", None).await;
    assert_eq!(
        described.text("Description"),
        "first",
        "case \"duplicate recipe name\": the refused create must not overwrite the stored one, got {:?}",
        described.body["Description"]
    );
    assert_eq!(
        described.member("Steps"),
        steps,
        "case \"duplicate recipe name\": the stored steps must be the first create's, got {:?}",
        described.body["Steps"]
    );
}
