// SPDX-License-Identifier: CC0-1.0

//! What `UpdateRecipe` rewrites, and what it leaves alone.
//!
//! The rule pinned here: `UpdateRecipe` is a PUT carrying the whole definition of a
//! stored recipe's `LATEST_WORKING` version (model: `smithy_query(query="operation",
//! operation="UpdateRecipe")` — `PUT /recipes/{Name}`, with `Description` and `Steps`
//! both optional), so it REPLACES that draft's description and steps with what the
//! request carries — an omitted member leaving the draft with none — stamps the draft's
//! last-modified pair, keeps the draft's own version number and create pair, and touches
//! neither the recipe's own members nor any version a publish froze. A name this account
//! holds no recipe under is refused 404 `ResourceNotFoundException` and writes nothing.
//!
//! The tables span what the rule generalises over: every shape a `RecipeStepList` takes,
//! whether the update carries a description, none at all or an empty one, how many
//! updates the draft has had, and whether the recipe had been published before the
//! update.
//!
//! The draft's own version NUMBER follows the publish history rather than the update: it
//! is the first minor revision on top of the latest published major version (`0.1` while
//! nothing is published, `X.1` once `X.0` is), so the cases below read it back after a
//! publish as well as before one.
//!
//! Every case drives the real wire path, so what is asserted is the HTTP status, the
//! error code and the JSON body a client sees, read back through `DescribeRecipe`.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply, ARN_PREFIX, CALLER_PRINCIPAL};

/// The version a recipe carries the moment it is created: the working draft, which no
/// publish has frozen (`INITIAL_RECIPE_VERSION` in `src/state.rs`).
const WORKING_VERSION: &str = "0.1";

/// The alias naming the version development continues on — the one `UpdateRecipe`
/// modifies (model: `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// `CreateRecipe`: `POST /recipes`.
async fn create_recipe(
    emulator: &Emulator,
    name: &str,
    description: Option<&str>,
    steps: &Value,
    tags: Option<Value>,
) -> Reply {
    let mut body = json!({"Name": name, "Steps": steps.clone()});
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    if let Some(tags) = tags {
        body["Tags"] = tags;
    }
    emulator.call("POST", "/recipes", Some(body)).await
}

/// `UpdateRecipe`: `PUT /recipes/{Name}`, carrying only the definition members the case
/// asks for, so an omitted member is genuinely absent from the request.
async fn update_recipe(
    emulator: &Emulator,
    name: &str,
    description: Option<&str>,
    steps: Option<&Value>,
) -> Reply {
    let mut body = json!({});
    if let Some(description) = description {
        body["Description"] = json!(description);
    }
    if let Some(steps) = steps {
        body["Steps"] = steps.clone();
    }
    emulator
        .call("PUT", &format!("/recipes/{}", label(name)), Some(body))
        .await
}

/// `PublishRecipe`: `POST /recipes/{Name}/publishRecipe`.
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

/// Every shape a `RecipeStepList` takes: one step with and without parameters, several
/// steps, a step carrying condition expressions, and no steps at all (model:
/// `smithy_query(query="shape", shape="RecipeStep,RecipeAction,ConditionExpression")`).
/// The `Operation` and `Condition` values satisfy the modelled `^[A-Z\_]+$` pattern.
fn step_shapes() -> [(&'static str, Value); 5] {
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
                {"Action": {"Operation": "LOWER_CASE", "Parameters": {"sourceColumn": "city"}}}
            ]),
        ),
        (
            "a step carrying condition expressions",
            json!([{
                "Action": {"Operation": "REMOVE_VALUES", "Parameters": {"sourceColumn": "status"}},
                "ConditionExpressions": [{"Condition": "IS", "Value": "cancelled", "TargetColumn": "status"}]
            }]),
        ),
        ("no steps at all", json!([])),
    ]
}

/// One row of the update table: the case name, the description the update carries and
/// the steps it carries (`None` meaning the request omits that member entirely).
type UpdateRow = (&'static str, Option<&'static str>, Option<Value>);

/// The successive updates one draft is put through, each row of [`step_shapes`] used as
/// the steps an update carries, so the table spans the step shapes AND the description
/// forms AND the number of updates the draft has had.
fn update_rows() -> Vec<UpdateRow> {
    let mut rows: Vec<UpdateRow> = step_shapes()
        .into_iter()
        .map(|(case, steps)| -> UpdateRow {
            (case, Some("the redrafted body"), Some(steps))
        })
        .collect();
    rows.push((
        "an update carrying no description of its own",
        None,
        Some(json!([{"Action": {"Operation": "DELETE"}}])),
    ));
    rows.push((
        "an update carrying an empty description",
        Some(""),
        Some(json!([{"Action": {"Operation": "DELETE"}}])),
    ));
    rows.push((
        "an update carrying no Steps member at all",
        Some("steps dropped"),
        None,
    ));
    rows.push(("an update carrying nothing but the name", None, None));
    rows
}

/// The definition a draft holds after an update row: an omitted `Steps` is the same
/// definition as an empty list, because the request carries the draft's WHOLE
/// definition.
fn expected_steps(row: &UpdateRow) -> Value {
    row.2.clone().unwrap_or_else(|| json!([]))
}

/// Assert the draft of `name` reports the definition `row` asked for, and that the
/// members the update is not allowed to change are unchanged.
async fn assert_draft_matches(emulator: &Emulator, case: &str, name: &str, row: &UpdateRow) {
    let draft = describe_recipe(emulator, name, Some(LATEST_WORKING)).await;
    assert_eq!(
        draft.status, 200,
        "case {case:?}: the updated draft must resolve, got {} {:?}",
        draft.status, draft.body
    );
    assert_eq!(
        draft.member("Steps"),
        expected_steps(row),
        "case {case:?}: expected the draft to hold {:?}, got {:?}",
        expected_steps(row),
        draft.body["Steps"]
    );
    match row.1 {
        Some(description) => assert_eq!(
            draft.text("Description"),
            description,
            "case {case:?}: expected the draft to carry {description:?}, got {:?}",
            draft.body["Description"]
        ),
        None => assert!(
            draft.is_absent("Description"),
            "case {case:?}: an update carrying no description leaves the draft with none, \
             got {:?}",
            draft.body["Description"]
        ),
    }
    // The update modifies the draft's DEFINITION; it mints no version, so the draft
    // keeps the number it was created at.
    assert_eq!(
        draft.text("RecipeVersion"),
        WORKING_VERSION,
        "case {case:?}: the draft keeps its own version number across an update, got {:?}",
        draft.body["RecipeVersion"]
    );
    // An updated version has a modification to report, which is what tells it from a
    // freshly created one.
    assert_eq!(
        draft.text("LastModifiedBy"),
        CALLER_PRINCIPAL,
        "case {case:?}: the draft must name the updating principal, got {:?}",
        draft.body["LastModifiedBy"]
    );
    assert!(
        draft.body["LastModifiedDate"].as_f64().unwrap_or_default() > 0.0,
        "case {case:?}: the draft must carry the update instant, got {:?}",
        draft.body["LastModifiedDate"]
    );
    // The create pair belongs to the create, not to this call.
    assert_eq!(
        draft.text("CreatedBy"),
        CALLER_PRINCIPAL,
        "case {case:?}: the draft must still name who created it, got {:?}",
        draft.body["CreatedBy"]
    );
    assert!(
        draft.body["CreateDate"].as_f64().unwrap_or_default() > 0.0,
        "case {case:?}: the draft must still carry the create instant, got {:?}",
        draft.body["CreateDate"]
    );
    // No publish has frozen the draft, so it still reports no publish of its own.
    for absent in ["PublishedBy", "PublishedDate"] {
        assert!(
            draft.is_absent(absent),
            "case {case:?}: an update publishes nothing, so the draft reports no {absent}, \
             got {:?}",
            draft.body[absent]
        );
    }
}

/// An update replaces the draft's definition with what the request carries, whatever the
/// draft held before and however many updates it has had, and leaves the recipe's own
/// members alone.
#[tokio::test]
async fn an_update_replaces_the_working_definition() {
    let emulator = Emulator::new();
    let rows = update_rows();

    for (created_index, (created_case, created_steps)) in step_shapes().into_iter().enumerate() {
        let name = format!("drafted-{created_index}");
        let tags = json!({"team": "data"});
        assert_eq!(
            create_recipe(
                &emulator,
                &name,
                Some("the first draft"),
                &created_steps,
                Some(tags.clone()),
            )
            .await
            .status,
            200,
            "case {created_case:?}: the recipe under test must be created"
        );
        let expected_arn = format!("{ARN_PREFIX}recipe/{name}");

        // Successive updates on ONE draft: each row is applied on top of the previous
        // one, so the row's definition is what the draft holds whatever it held before.
        for (row_index, row) in rows.iter().enumerate() {
            let case = format!("{created_case} / update {row_index}: {}", row.0);
            let updated = update_recipe(&emulator, &name, row.1, row.2.as_ref()).await;
            assert_eq!(
                updated.status, 200,
                "case {case:?}: expected 200, got {} {:?}",
                updated.status, updated.body
            );
            assert_eq!(
                updated.text("Name"),
                name,
                "case {case:?}: the update must echo the recipe it updated, got {:?}",
                updated.body
            );

            assert_draft_matches(&emulator, &case, &name, row).await;

            // Repeating the same update is idempotent: the definition it writes does not
            // depend on the definition it replaced.
            let repeated = update_recipe(&emulator, &name, row.1, row.2.as_ref()).await;
            assert_eq!(
                repeated.status, 200,
                "case {case:?}: the repeated update must answer 200, got {} {:?}",
                repeated.status, repeated.body
            );
            assert_draft_matches(&emulator, &case, &name, row).await;

            // The recipe's own members carry no member on this request, so the update
            // leaves them exactly as the create wrote them.
            let draft = describe_recipe(&emulator, &name, Some(LATEST_WORKING)).await;
            assert_eq!(
                draft.text("Name"),
                name,
                "case {case:?}: the recipe keeps its name, got {:?}",
                draft.body["Name"]
            );
            assert_eq!(
                draft.text("ResourceArn"),
                expected_arn,
                "case {case:?}: the recipe keeps its ARN, got {:?}",
                draft.body["ResourceArn"]
            );
            assert_eq!(
                draft.member("Tags"),
                tags,
                "case {case:?}: the recipe keeps its tags, got {:?}",
                draft.body["Tags"]
            );
        }
    }
}

/// A version a publish froze keeps the definition it was frozen with after the draft is
/// updated, and a later publish freezes what the update wrote: the published history and
/// the working draft are independent.
#[tokio::test]
async fn an_update_leaves_every_published_version_alone() {
    let emulator = Emulator::new();
    let frozen_steps =
        json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]);

    for (steps_index, (steps_case, updated_steps)) in step_shapes().into_iter().enumerate() {
        let case = format!("a published recipe redrafted with {steps_case}");
        let name = format!("published-then-updated-{steps_index}");
        assert_eq!(
            create_recipe(
                &emulator,
                &name,
                Some("the first draft"),
                &frozen_steps,
                None
            )
            .await
            .status,
            200,
            "case {case:?}: the recipe under test must be created"
        );
        assert_eq!(
            publish_recipe(&emulator, &name, Some("frozen from the first draft"))
                .await
                .status,
            200,
            "case {case:?}: the first publish must answer 200"
        );

        let updated = update_recipe(
            &emulator,
            &name,
            Some("the second draft"),
            Some(&updated_steps),
        )
        .await;
        assert_eq!(
            updated.status, 200,
            "case {case:?}: the update must answer 200, got {} {:?}",
            updated.status, updated.body
        );

        // The draft is renumbered onto the major just frozen — it is the first minor
        // revision on top of the latest published version — and the update modifies its
        // definition without minting a version of its own.
        let draft = describe_recipe(&emulator, &name, Some(LATEST_WORKING)).await;
        assert_eq!(
            draft.text("RecipeVersion"),
            "1.1",
            "case {case:?}: after one publish the draft revises major 1, got {:?}",
            draft.body["RecipeVersion"]
        );
        assert_eq!(
            draft.member("Steps"),
            updated_steps,
            "case {case:?}: the draft must hold what the update wrote, got {:?}",
            draft.body["Steps"]
        );
        // The number the publish history gives the draft is also how the draft is
        // addressed by number, and the number it was created at no longer names a
        // version this recipe holds.
        let by_number = describe_recipe(&emulator, &name, Some("1.1")).await;
        assert_eq!(
            by_number.status, 200,
            "case {case:?}: the draft must resolve by its own number, got {} {:?}",
            by_number.status, by_number.body
        );
        assert_eq!(
            by_number.member("Steps"),
            updated_steps,
            "case {case:?}: the draft addressed by number must hold what the update wrote, \
             got {:?}",
            by_number.body["Steps"]
        );
        // The number the draft was created at revised a major this recipe has moved past,
        // so it names no version the recipe still holds.
        let created_number = describe_recipe(&emulator, &name, Some(WORKING_VERSION)).await;
        assert_eq!(
            created_number.status, 404,
            "case {case:?}: {WORKING_VERSION} must name no version after the renumber, \
             got {} {:?}",
            created_number.status, created_number.body
        );
        assert_eq!(
            created_number.error_message(),
            format!("The recipe {name} for version {WORKING_VERSION} wasn't found."),
            "case {case:?}: expected the (recipe, version) miss to name both halves, got {:?}",
            created_number.error_message()
        );

        // The version the earlier publish froze is untouched by the update.
        let frozen = describe_recipe(&emulator, &name, Some("1.0")).await;
        assert_eq!(
            frozen.member("Steps"),
            frozen_steps,
            "case {case:?}: version 1.0 keeps the definition it froze, got {:?}",
            frozen.body["Steps"]
        );
        assert_eq!(
            frozen.text("Description"),
            "frozen from the first draft",
            "case {case:?}: version 1.0 keeps the body it froze, got {:?}",
            frozen.body["Description"]
        );

        // A publish after the update freezes what the update wrote, at the next whole
        // major number.
        assert_eq!(
            publish_recipe(&emulator, &name, None).await.status,
            200,
            "case {case:?}: the second publish must answer 200"
        );
        let second = describe_recipe(&emulator, &name, Some("2.0")).await;
        assert_eq!(
            second.member("Steps"),
            updated_steps,
            "case {case:?}: version 2.0 must freeze what the update wrote, got {:?}",
            second.body["Steps"]
        );
        assert_eq!(
            second.text("Description"),
            "the second draft",
            "case {case:?}: version 2.0 must freeze the updated body, got {:?}",
            second.body["Description"]
        );
        let redrafted = describe_recipe(&emulator, &name, Some(LATEST_WORKING)).await;
        assert_eq!(
            redrafted.text("RecipeVersion"),
            "2.1",
            "case {case:?}: after the second publish the draft revises major 2, got {:?}",
            redrafted.body["RecipeVersion"]
        );
    }
}

/// An update of a recipe this account holds none of is 404 `ResourceNotFoundException`
/// naming the recipe that was looked up, and writes nothing.
#[tokio::test]
async fn updating_a_recipe_nobody_created_is_refused() {
    let emulator = Emulator::new();

    // Rows differ along the form of the name the request addressed: every one of them
    // names no stored recipe, so every one of them is the same miss.
    let absent = [
        ("a plain name", "ghost"),
        ("a name with a space", "a ghost with a space"),
        ("a single-character name", "g"),
        ("a name outside the documented character set", "under_score"),
    ];
    let steps = json!([{"Action": {"Operation": "DELETE"}}]);
    for (case, name) in absent {
        let refused = update_recipe(&emulator, name, Some("whatever"), Some(&steps)).await;
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
            refused.error_message(),
            expected,
            "case {case:?} ({name:?}): expected {expected:?}, got {:?}",
            refused.error_message()
        );
        let described = describe_recipe(&emulator, name, Some(LATEST_WORKING)).await;
        assert_eq!(
            described.status, 404,
            "case {case:?} ({name:?}): the refused update must write nothing, got {} {:?}",
            described.status, described.body
        );
    }

    // An update addresses ONE recipe: a sibling this account does hold is untouched by
    // the refused calls above and by an update of the other sibling.
    let names = ["left", "right"];
    for name in names {
        assert_eq!(
            create_recipe(&emulator, name, Some("as created"), &steps, None)
                .await
                .status,
            200,
            "case \"two siblings\": recipe {name} must be created"
        );
    }
    let replacement = json!([{"Action": {"Operation": "LOWER_CASE", "Parameters": {"sourceColumn": "city"}}}]);
    assert_eq!(
        update_recipe(&emulator, "left", Some("redrafted"), Some(&replacement))
            .await
            .status,
        200,
        "case \"two siblings\": the update of recipe left must answer 200"
    );
    let sibling = describe_recipe(&emulator, "right", Some(LATEST_WORKING)).await;
    assert_eq!(
        sibling.member("Steps"),
        steps,
        "case \"two siblings\": recipe right keeps its definition, got {:?}",
        sibling.body["Steps"]
    );
    assert_eq!(
        sibling.text("Description"),
        "as created",
        "case \"two siblings\": recipe right keeps its body, got {:?}",
        sibling.body["Description"]
    );
    assert!(
        sibling.is_absent("LastModifiedBy"),
        "case \"two siblings\": recipe right was never updated, so it reports no \
         LastModifiedBy, got {:?}",
        sibling.body["LastModifiedBy"]
    );
}
