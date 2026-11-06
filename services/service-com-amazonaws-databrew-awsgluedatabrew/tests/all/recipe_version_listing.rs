// SPDX-License-Identifier: CC0-1.0

//! What `ListRecipeVersions` reports for a recipe, and what it leaves out.
//!
//! The rule pinned here: a listing of a versioned resource reports exactly the versions
//! the store holds for the resource the request names, each carrying that version's own
//! definition and audit beside the members belonging to the resource as a whole, in
//! version order, one bounded page at a time — and for this operation the working draft
//! is left out entirely ("Lists the versions of a particular DataBrew recipe, except for
//! LATEST_WORKING", model: `ListRecipeVersions`).
//!
//! The tables span what the rule generalises over: how many versions a recipe holds, the
//! shape of the definition each version froze, whether the recipe carries tags, the page
//! size asked for (including each modelled bound and one past it), and the form of the
//! name the request addressed.
//!
//! Every case drives the real wire path, so what is asserted is the HTTP status, the
//! error code and the JSON body a client sees.

use serde_json::{json, Value};

use crate::databrew_harness::{label, Emulator, Reply, ARN_PREFIX, CALLER_PRINCIPAL};

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`), which this listing never reports.
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

/// `UpdateRecipe`: `PUT /recipes/{Name}`, which moves the working draft on.
async fn update_recipe(emulator: &Emulator, name: &str, steps: &Value) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/recipes/{}", label(name)),
            Some(json!({"Steps": steps.clone()})),
        )
        .await
}

/// `DescribeRecipe`: `GET /recipes/{Name}?recipeVersion=<version>`.
async fn describe_recipe(emulator: &Emulator, name: &str, version: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/recipes/{}?recipeVersion={version}", label(name)),
            None,
        )
        .await
}

/// `ListRecipeVersions`: `GET /recipeVersions`, with `Name` and the pagination members
/// on the query string (model: `ListRecipeVersionsRequest` binds all three to
/// `@httpQuery`).
async fn list_recipe_versions(
    emulator: &Emulator,
    name: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Reply {
    let mut uri = format!("/recipeVersions?name={}", label(name));
    if let Some(max_results) = max_results {
        uri.push_str(&format!("&maxResults={max_results}"));
    }
    if let Some(next_token) = next_token {
        uri.push_str(&format!("&nextToken={next_token}"));
    }
    emulator.call("GET", &uri, None).await
}

/// The `Recipes` list of a reply, as the wire carries it.
fn listed(reply: &Reply) -> Vec<Value> {
    reply
        .member("Recipes")
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The `RecipeVersion` of each listed item, in the order the reply carries them.
fn listed_versions(reply: &Reply) -> Vec<String> {
    listed(reply)
        .iter()
        .map(|item| {
            item.get("RecipeVersion")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Every shape a `RecipeStepList` takes: one step with and without parameters, several
/// steps with a condition expression, and no steps at all (model:
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

/// Publish `recipe` once per entry of `descriptions`, asserting each publish is served,
/// and answer the version numbers that were frozen, oldest first.
async fn publish_each(
    emulator: &Emulator,
    case: &str,
    recipe: &str,
    descriptions: &[Option<&str>],
) -> Vec<String> {
    let mut frozen = Vec::new();
    for (index, description) in descriptions.iter().copied().enumerate() {
        let published = publish_recipe(emulator, recipe, description).await;
        assert_eq!(
            published.status, 200,
            "case {case:?}: publish {index} must answer 200, got {} {:?}",
            published.status, published.body
        );
        // The next whole major number at minor 0 (model: `Recipe$RecipeVersion`).
        frozen.push(format!("{}.0", index + 1));
    }
    frozen
}

/// A listing reports every version a publish froze — with that version's definition,
/// description, publish stamp and audit, and the recipe's own name, ARN and tags — and
/// never the working draft, however many versions the recipe holds.
#[tokio::test]
async fn the_listing_reports_every_published_version_and_never_the_draft() {
    // How many publishes the recipe has had, and the description each one carried: the
    // axis the rule generalises over, from a recipe nobody published to one with a long
    // history.
    let histories: [(&str, &[Option<&str>]); 4] = [
        ("a recipe nobody published", &[]),
        ("a single publish carrying its own description", &[Some("frozen for release")]),
        (
            "three publishes, one of them carrying no description",
            &[Some("first cut"), None, Some("")],
        ),
        (
            "seven publishes",
            &[None, None, None, None, None, None, None],
        ),
    ];
    // Whether the recipe carries tags, which belong to the recipe rather than to one of
    // its versions, so every item of the listing reports the same map (model:
    // `Recipe$Tags`).
    let tag_cases: [(&str, Option<Value>); 2] = [
        ("untagged", None),
        ("tagged", Some(json!({"team": "analytics", "tier": "gold"}))),
    ];

    let emulator = Emulator::new();
    for (steps_index, (steps_case, steps)) in step_shapes().into_iter().enumerate() {
        for (history_index, (history_case, publishes)) in histories.iter().copied().enumerate() {
            for (tag_index, (tag_case, tags)) in tag_cases.iter().cloned().enumerate() {
                let case = format!("{steps_case} / {history_case} / {tag_case}");
                let name = format!("recipe-{steps_index}-{history_index}-{tag_index}");
                let created_with = Some("the draft body");
                assert_eq!(
                    create_recipe(&emulator, &name, created_with, &steps, tags.clone())
                        .await
                        .status,
                    200,
                    "case {case:?}: the recipe under test must be created"
                );
                let frozen = publish_each(&emulator, &case, &name, publishes).await;

                let listing = list_recipe_versions(&emulator, &name, None, None).await;
                assert_eq!(
                    listing.status, 200,
                    "case {case:?}: the listing must answer 200, got {} {:?}",
                    listing.status, listing.body
                );
                assert_eq!(
                    listed_versions(&listing),
                    frozen,
                    "case {case:?}: expected the published versions {frozen:?} oldest first, \
                     got {:?}",
                    listing.body["Recipes"]
                );
                assert!(
                    listing.is_absent("NextToken"),
                    "case {case:?}: one page holds every version, so no token resumes it, \
                     got {:?}",
                    listing.body["NextToken"]
                );

                // The draft is never an item: neither under the alias that addresses it
                // nor under the number it carries (the first minor on top of the major
                // last frozen).
                let draft_version = format!("{}.1", publishes.len());
                for excluded in [LATEST_WORKING, draft_version.as_str()] {
                    assert!(
                        !listed_versions(&listing).iter().any(|held| held == excluded),
                        "case {case:?}: the working draft {excluded} must not be listed, got {:?}",
                        listing.body["Recipes"]
                    );
                }

                for (index, item) in listed(&listing).iter().enumerate() {
                    let version = &frozen[index];
                    assert_eq!(
                        item.get("Name").and_then(Value::as_str),
                        Some(name.as_str()),
                        "case {case:?}: item {version} must name its recipe, got {item:?}"
                    );
                    assert_eq!(
                        item.get("ResourceArn").and_then(Value::as_str),
                        Some(format!("{ARN_PREFIX}recipe/{name}").as_str()),
                        "case {case:?}: item {version} must carry the recipe's ARN, got {item:?}"
                    );
                    assert_eq!(
                        item.get("Steps"),
                        Some(&steps),
                        "case {case:?}: item {version} must hold the definition it froze, \
                         got {item:?}"
                    );
                    match publishes[index].or(created_with) {
                        Some(description) => assert_eq!(
                            item.get("Description").and_then(Value::as_str),
                            Some(description),
                            "case {case:?}: item {version} must carry {description:?}, got {item:?}"
                        ),
                        None => assert!(
                            item.get("Description").is_none(),
                            "case {case:?}: item {version} has no description to report, \
                             got {item:?}"
                        ),
                    }
                    assert_eq!(
                        item.get("PublishedBy").and_then(Value::as_str),
                        Some(CALLER_PRINCIPAL),
                        "case {case:?}: item {version} must name the publishing principal, \
                         got {item:?}"
                    );
                    assert!(
                        item.get("PublishedDate")
                            .and_then(Value::as_f64)
                            .unwrap_or_default()
                            > 0.0,
                        "case {case:?}: item {version} must carry the publish instant, \
                         got {item:?}"
                    );
                    assert_eq!(
                        item.get("CreatedBy").and_then(Value::as_str),
                        Some(CALLER_PRINCIPAL),
                        "case {case:?}: item {version} must name who created it, got {item:?}"
                    );
                    match &tags {
                        Some(expected) => assert_eq!(
                            item.get("Tags"),
                            Some(expected),
                            "case {case:?}: item {version} must carry the recipe's tags, \
                             got {item:?}"
                        ),
                        None => assert!(
                            item.get("Tags").is_none(),
                            "case {case:?}: an untagged recipe carries no tag map, got {item:?}"
                        ),
                    }

                    // The same version read through `DescribeRecipe` reports the same
                    // definition, so the listing serves the stored record rather than a
                    // story of its own.
                    let described = describe_recipe(&emulator, &name, version).await;
                    assert_eq!(
                        described.status, 200,
                        "case {case:?}: version {version} must resolve, got {} {:?}",
                        described.status, described.body
                    );
                    for member in ["Steps", "Description", "PublishedBy", "RecipeVersion"] {
                        assert_eq!(
                            item.get(member),
                            described.body.get(member),
                            "case {case:?}: item {version} must report the same {member} as a \
                             describe of it, got {:?} and {:?}",
                            item.get(member),
                            described.body.get(member)
                        );
                    }
                }

                // Listing is a read: a second call answers exactly what the first did.
                let again = list_recipe_versions(&emulator, &name, None, None).await;
                assert_eq!(
                    again.member("Recipes"),
                    listing.member("Recipes"),
                    "case {case:?}: a second listing must answer what the first did, got {:?}",
                    again.body["Recipes"]
                );
            }
        }
    }
}

/// An edit of the draft between publishes does not change a version already frozen, and
/// the listing reports each version's own definition.
#[tokio::test]
async fn each_listed_version_holds_the_definition_it_froze() {
    let emulator = Emulator::new();
    let name = "redrafted";
    let first = json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}]);
    let second = json!([]);
    let third = json!([
        {"Action": {"Operation": "RENAME", "Parameters": {"sourceColumn": "id", "targetColumn": "orderId"}}}
    ]);
    let case = "a recipe redrafted between publishes";

    assert_eq!(
        create_recipe(&emulator, name, None, &first, None).await.status,
        200,
        "case {case:?}: the recipe under test must be created"
    );
    for (index, steps) in [&first, &second, &third].into_iter().enumerate() {
        if index > 0 {
            let redrafted = update_recipe(&emulator, name, steps).await;
            assert_eq!(
                redrafted.status, 200,
                "case {case:?}: redraft {index} must answer 200, got {} {:?}",
                redrafted.status, redrafted.body
            );
        }
        assert_eq!(
            publish_recipe(&emulator, name, None).await.status,
            200,
            "case {case:?}: publish {index} must answer 200"
        );
    }

    let listing = list_recipe_versions(&emulator, name, None, None).await;
    let expected: [(&str, &Value); 3] = [("1.0", &first), ("2.0", &second), ("3.0", &third)];
    assert_eq!(
        listed_versions(&listing),
        vec!["1.0", "2.0", "3.0"],
        "case {case:?}: expected the three frozen versions oldest first, got {:?}",
        listing.body["Recipes"]
    );
    for (index, (version, steps)) in expected.into_iter().enumerate() {
        let item = &listed(&listing)[index];
        assert_eq!(
            item.get("Steps"),
            Some(steps),
            "case {case:?}: version {version} must hold the definition it froze, got {item:?}"
        );
    }
}

/// A version a delete removed leaves the listing, and the versions left keep their own
/// numbers: the listing is computed from what the store still holds.
#[tokio::test]
async fn a_deleted_version_leaves_the_listing() {
    let emulator = Emulator::new();
    let steps = json!([{"Action": {"Operation": "DELETE"}}]);
    let name = "pruned";
    let case = "a recipe with a version deleted from the middle of its history";

    assert_eq!(
        create_recipe(&emulator, name, None, &steps, None).await.status,
        200,
        "case {case:?}: the recipe under test must be created"
    );
    publish_each(&emulator, case, name, &[None, None, None, None]).await;

    // `BatchDeleteRecipeVersion`: `POST /recipes/{Name}/batchDeleteRecipeVersion`.
    let deleted = emulator
        .call(
            "POST",
            &format!("/recipes/{}/batchDeleteRecipeVersion", label(name)),
            Some(json!({"RecipeVersions": ["2.0", "4.0"]})),
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case {case:?}: the delete of two versions must answer 200, got {} {:?}",
        deleted.status, deleted.body
    );

    let listing = list_recipe_versions(&emulator, name, None, None).await;
    assert_eq!(
        listed_versions(&listing),
        vec!["1.0", "3.0"],
        "case {case:?}: expected the versions still held, got {:?}",
        listing.body["Recipes"]
    );

    // A page that ends on the version before a deleted one still resumes at the next
    // version held.
    let first_page = list_recipe_versions(&emulator, name, Some(1), None).await;
    assert_eq!(
        listed_versions(&first_page),
        vec!["1.0"],
        "case {case:?}: expected a page of one, got {:?}",
        first_page.body["Recipes"]
    );
    let resumed = list_recipe_versions(
        &emulator,
        name,
        Some(1),
        Some(&first_page.text("NextToken")),
    )
    .await;
    assert_eq!(
        listed_versions(&resumed),
        vec!["3.0"],
        "case {case:?}: expected the next version held, got {:?}",
        resumed.body["Recipes"]
    );
}

/// Walking the pages of a listing visits every published version exactly once, in
/// version order, whatever page size the request asked for.
#[tokio::test]
async fn paging_visits_every_version_exactly_once() {
    let emulator = Emulator::new();
    let steps = json!([{"Action": {"Operation": "DELETE"}}]);
    // The page sizes span the modelled `range 1..100` (model:
    // `ListRecipeVersionsRequest$MaxResults`): the lower bound, sizes that split the
    // history unevenly, one that holds it whole, and the upper bound.
    let page_sizes = [1, 2, 3, 5, 100];
    // How many versions the recipe holds, so a page size lands exactly on the end of
    // the history, inside it, and past it.
    let publish_counts = [0_usize, 1, 5, 6];

    for (count_index, count) in publish_counts.into_iter().enumerate() {
        let name = format!("paged-{count_index}");
        assert_eq!(
            create_recipe(&emulator, &name, None, &steps, None)
                .await
                .status,
            200,
            "case \"{count} publishes\": the recipe under test must be created"
        );
        let frozen = publish_each(
            &emulator,
            &format!("{count} publishes"),
            &name,
            &vec![None; count],
        )
        .await;

        for page_size in page_sizes {
            let case = format!("{count} publishes / pages of {page_size}");
            let mut seen: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            let mut pages = 0;
            loop {
                let page = list_recipe_versions(&emulator, &name, Some(page_size), token.as_deref())
                    .await;
                assert_eq!(
                    page.status, 200,
                    "case {case:?}: page {pages} must answer 200, got {} {:?}",
                    page.status, page.body
                );
                let versions = listed_versions(&page);
                assert!(
                    versions.len() <= page_size as usize,
                    "case {case:?}: page {pages} must hold at most {page_size} items, got {:?}",
                    page.body["Recipes"]
                );
                seen.extend(versions);
                pages += 1;
                match page.body.get("NextToken").and_then(Value::as_str) {
                    Some(next) => token = Some(next.to_string()),
                    None => break,
                }
                assert!(
                    pages <= count + 1,
                    "case {case:?}: paging must terminate, got {pages} pages and {seen:?}"
                );
            }
            assert_eq!(
                seen, frozen,
                "case {case:?}: expected every version once in version order, got {seen:?}"
            );
        }
    }
}

/// A page size outside the modelled bounds is refused, and a token this service never
/// issued is refused: both are 400 `ValidationException`.
#[tokio::test]
async fn a_request_outside_the_modelled_bounds_is_refused() {
    let emulator = Emulator::new();
    let steps = json!([{"Action": {"Operation": "DELETE"}}]);
    let name = "bounded";
    assert_eq!(
        create_recipe(&emulator, name, None, &steps, None).await.status,
        200,
        "the recipe under test must be created"
    );
    publish_each(&emulator, "bounded", name, &[None, None]).await;

    // One past each published bound of `MaxResults100` (model: `range 1..100`).
    for (case, max_results) in [("below the lower bound", 0), ("above the upper bound", 101)] {
        let refused = list_recipe_versions(&emulator, name, Some(max_results), None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({max_results}): expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({max_results}): expected ValidationException, got {:?}",
            refused.error_type
        );
    }

    // Each bound itself is served.
    for (case, max_results) in [("the lower bound", 1), ("the upper bound", 100)] {
        let served = list_recipe_versions(&emulator, name, Some(max_results), None).await;
        assert_eq!(
            served.status, 200,
            "case {case:?} ({max_results}): expected 200, got {} {:?}",
            served.status, served.body
        );
    }

    for (case, token) in [
        ("a token that is not hex", "not-a-token"),
        ("a token that decodes to no version", "ffff"),
    ] {
        let refused = list_recipe_versions(&emulator, name, None, Some(token)).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({token:?}): expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({token:?}): expected ValidationException, got {:?}",
            refused.error_type
        );
    }
}

/// Listing the versions of a recipe this account holds none of is refused with the
/// sentence this service words a missing recipe with, whatever form the name takes.
#[tokio::test]
async fn listing_the_versions_of_a_recipe_nobody_created_is_refused() {
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
        let refused = list_recipe_versions(&emulator, name, None, None).await;
        assert_eq!(
            refused.status, 400,
            "case {case:?} ({name:?}): expected 400, got {} {:?}",
            refused.status, refused.body
        );
        assert_eq!(
            refused.error_type.as_deref(),
            Some("ValidationException"),
            "case {case:?} ({name:?}): expected ValidationException, got {:?}",
            refused.error_type
        );
        let expected = format!("Recipe {name} wasn't found.");
        assert_eq!(
            refused.error_message(),
            expected,
            "case {case:?} ({name:?}): expected {expected:?}, got {:?}",
            refused.error_message()
        );
    }

    // A recipe that exists but was never published holds no published version, which is
    // an empty listing rather than a refusal.
    let steps = json!([{"Action": {"Operation": "DELETE"}}]);
    assert_eq!(
        create_recipe(&emulator, "unpublished", None, &steps, None)
            .await
            .status,
        200,
        "case \"an unpublished recipe\": the recipe under test must be created"
    );
    let listing = list_recipe_versions(&emulator, "unpublished", None, None).await;
    assert_eq!(
        listing.status, 200,
        "case \"an unpublished recipe\": expected 200, got {} {:?}",
        listing.status, listing.body
    );
    assert_eq!(
        listing.member("Recipes"),
        json!([]),
        "case \"an unpublished recipe\": expected no versions, got {:?}",
        listing.body["Recipes"]
    );

    // One recipe's versions are never another's: a listing reports only the recipe the
    // request named.
    assert_eq!(
        create_recipe(&emulator, "neighbour", None, &steps, None)
            .await
            .status,
        200,
        "case \"a neighbouring recipe\": the recipe under test must be created"
    );
    publish_each(&emulator, "a neighbouring recipe", "neighbour", &[None, None]).await;
    let neighbour = list_recipe_versions(&emulator, "neighbour", None, None).await;
    assert_eq!(
        listed_versions(&neighbour),
        vec!["1.0", "2.0"],
        "case \"a neighbouring recipe\": expected its own versions, got {:?}",
        neighbour.body["Recipes"]
    );
    let unpublished = list_recipe_versions(&emulator, "unpublished", None, None).await;
    assert_eq!(
        unpublished.member("Recipes"),
        json!([]),
        "case \"an unpublished recipe\": another recipe's publishes are not its versions, \
         got {:?}",
        unpublished.body["Recipes"]
    );
}
