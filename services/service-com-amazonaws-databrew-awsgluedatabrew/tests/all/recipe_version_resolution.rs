// SPDX-License-Identifier: CC0-1.0

//! How `DescribeRecipe` resolves a recipe VERSION, and what a miss of one says.
//!
//! The rule pinned here: `DescribeRecipe` addresses the (recipe, version) PAIR, where
//! the version identifier is the one the request carried or — when it carried none —
//! the latest-published alias, which is what the model documents an omitted identifier
//! asks for (`smithy_query(query="doc", shape="DescribeRecipeRequest",
//! member="RecipeVersion")`: "If this parameter isn't specified, then the latest
//! published version is returned"). So an omitted identifier resolves exactly what
//! `LATEST_PUBLISHED` resolves and never falls back to the working draft, and a pair
//! that resolves nothing — whichever half is missing — is `ResourceNotFoundException`
//! (HTTP 404) naming both halves.
//!
//! The tables span what the rule generalises over: how many versions a recipe has
//! published (none, one, several), which identifier form addresses them (omitted, each
//! documented alias, a numeric version), and which half of the pair is missing.
//!
//! Versions are frozen into the store directly rather than through `PublishRecipe`, so
//! the cases can address version numbers no publish mints (`999999.999999`, say)
//! alongside the ones it does; what `PublishRecipe` itself freezes is covered by
//! `tests/all/recipe_publishing.rs`. The store written here is the one shared engine the
//! handlers serve from, so every assertion below is still made over the wire.

use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_databrew_awsgluedatabrew::ComAmazonawsDatabrewAwsgluedatabrewState;

use crate::databrew_harness::{label, Emulator, Reply, CALLER_PRINCIPAL};

/// The version a recipe carries the moment it is created: the first minor of major 0,
/// the working draft no publish has frozen (`INITIAL_RECIPE_VERSION` in `src/state.rs`).
const WORKING_VERSION: &str = "0.1";

/// The alias an omitted `RecipeVersion` asks for (model: `Recipe$RecipeVersion`).
const LATEST_PUBLISHED: &str = "LATEST_PUBLISHED";

/// The alias naming the version development continues on (model:
/// `Recipe$RecipeVersion`).
const LATEST_WORKING: &str = "LATEST_WORKING";

/// The steps every recipe under test is created with.
fn steps() -> Value {
    json!([{"Action": {"Operation": "UPPER_CASE", "Parameters": {"sourceColumn": "dept"}}}])
}

/// The description the draft of `recipe` carries, so a body served for the draft is
/// distinguishable from a body served for a published version.
fn draft_description(recipe: &str) -> String {
    format!("the unpublished draft of {recipe}")
}

/// The description the version `version` of a recipe was frozen with.
fn published_description(version: &str) -> String {
    format!("frozen at {version}")
}

/// `CreateRecipe` a recipe holding only its working draft.
async fn create_recipe(emulator: &Emulator, name: &str) -> Reply {
    emulator
        .call(
            "POST",
            "/recipes",
            Some(json!({
                "Name": name,
                "Description": draft_description(name),
                "Steps": steps(),
            })),
        )
        .await
}

/// `DescribeRecipe`: `GET /recipes/{Name}`, with the `recipeVersion` query parameter
/// the model binds `RecipeVersion` to left off entirely when `version` is `None`.
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

/// Freeze `versions` onto the stored recipe, oldest first, each carrying its own
/// description and the publish pair a frozen version reports.
fn freeze_versions(emulator: &Emulator, name: &str, versions: &[&str]) {
    let state = emulator
        .engine()
        .state::<ComAmazonawsDatabrewAwsgluedatabrewState>();
    let mut store = state.lock();
    let recipe = store
        .recipes
        .get_mut(name)
        .expect("the recipe under test must be stored");
    for (index, version) in versions.iter().enumerate() {
        let mut frozen = recipe.working.clone();
        frozen.version = (*version).to_string();
        frozen.description = Some(published_description(version));
        frozen.published_by = Some(CALLER_PRINCIPAL.to_string());
        frozen.published_date = Some(SystemTime::now() + Duration::from_secs(index as u64));
        recipe.published.push(frozen);
    }
}

/// An omitted `RecipeVersion` resolves exactly what `LATEST_PUBLISHED` resolves: the
/// most recent published version when there is one, and NOTHING — not the working
/// draft — when no version has been published.
#[tokio::test]
async fn a_versionless_describe_resolves_exactly_the_latest_published_alias() {
    // Rows differ along how many versions the recipe has published, which is what the
    // rule generalises over: the alias resolves the LAST of them, and resolves nothing
    // while there are none.
    let cases: [(&str, &[&str]); 4] = [
        ("nothing published", &[]),
        ("one published version", &["1.0"]),
        ("several published versions", &["1.0", "2.0", "3.0"]),
        (
            "published versions at the documented digit bound",
            &["1.0", "999999.999999"],
        ),
    ];

    let emulator = Emulator::new();
    for (case, published) in cases {
        let name = format!("recipe-with-{}-published", published.len());
        assert_eq!(
            create_recipe(&emulator, &name).await.status,
            200,
            "case {case:?}: the recipe under test must be created"
        );
        freeze_versions(&emulator, &name, published);

        let versionless = describe_recipe(&emulator, &name, None).await;
        let by_alias = describe_recipe(&emulator, &name, Some(LATEST_PUBLISHED)).await;

        // The two requests ask the same question, so they answer the same thing —
        // status, error code and body.
        assert_eq!(
            (versionless.status, &versionless.error_type, &versionless.body),
            (by_alias.status, &by_alias.error_type, &by_alias.body),
            "case {case:?}: a versionless describe must answer exactly what {LATEST_PUBLISHED} \
             does, got {} {:?} against {} {:?}",
            versionless.status,
            versionless.body,
            by_alias.status,
            by_alias.body
        );

        match published.last() {
            None => {
                assert_eq!(
                    versionless.status, 404,
                    "case {case:?}: with nothing published a versionless describe must be \
                     refused, got {} {:?}",
                    versionless.status, versionless.body
                );
                assert_eq!(
                    versionless.error_type.as_deref(),
                    Some("ResourceNotFoundException"),
                    "case {case:?}: expected ResourceNotFoundException, got {:?}",
                    versionless.error_type
                );
                let expected =
                    format!("The recipe {name} for version {LATEST_PUBLISHED} wasn't found.");
                assert_eq!(
                    error_message(&versionless),
                    expected,
                    "case {case:?}: expected {expected:?}, got {:?}",
                    error_message(&versionless)
                );
            }
            Some(latest) => {
                assert_eq!(
                    versionless.status, 200,
                    "case {case:?}: with a published version a versionless describe must serve \
                     it, got {} {:?}",
                    versionless.status, versionless.body
                );
                assert_eq!(
                    versionless.text("RecipeVersion"),
                    *latest,
                    "case {case:?}: expected the most recent published version, got {:?}",
                    versionless.body["RecipeVersion"]
                );
                assert_eq!(
                    versionless.text("Description"),
                    published_description(latest),
                    "case {case:?}: expected the published body rather than the draft's, got {:?}",
                    versionless.body["Description"]
                );
                assert_eq!(
                    versionless.text("PublishedBy"),
                    CALLER_PRINCIPAL,
                    "case {case:?}: a published version reports who published it, got {:?}",
                    versionless.body["PublishedBy"]
                );
            }
        }

        // Whatever the publish state, the draft is reachable by the alias that names it
        // and by its own number — the versionless request is simply not how it is asked
        // for.
        for identifier in [LATEST_WORKING, WORKING_VERSION] {
            let draft = describe_recipe(&emulator, &name, Some(identifier)).await;
            assert_eq!(
                draft.status, 200,
                "case {case:?} / {identifier}: the working draft must still resolve, got {} {:?}",
                draft.status, draft.body
            );
            assert_eq!(
                draft.text("RecipeVersion"),
                WORKING_VERSION,
                "case {case:?} / {identifier}: expected the working version, got {:?}",
                draft.body["RecipeVersion"]
            );
            assert_eq!(
                draft.text("Description"),
                draft_description(&name),
                "case {case:?} / {identifier}: expected the draft's own body, got {:?}",
                draft.body["Description"]
            );
            assert!(
                draft.is_absent("PublishedBy") && draft.is_absent("PublishedDate"),
                "case {case:?} / {identifier}: no publish has frozen the draft, got {:?} / {:?}",
                draft.body["PublishedBy"],
                draft.body["PublishedDate"]
            );
        }
    }
}

/// Every miss of a (recipe, version) pair is 404 `ResourceNotFoundException` naming the
/// recipe the request asked for and the version identifier that was looked up — the
/// alias an omitted identifier defaults to included.
#[tokio::test]
async fn a_version_miss_names_the_recipe_and_the_identifier_looked_up() {
    let emulator = Emulator::new();

    // An existing recipe with one published version: so a miss can be a version this
    // recipe does not hold while the recipe itself is there.
    let held = "published-once";
    assert_eq!(
        create_recipe(&emulator, held).await.status,
        200,
        "the recipe under test must be created"
    );
    freeze_versions(&emulator, held, &["1.0"]);

    // Every identifier here is well formed (model: `Recipe$RecipeVersion` — `X.Y` with
    // at most 6 digits each, or one of the two aliases) and names no version this
    // recipe holds.
    let unheld = [
        ("another major", "2.0"),
        ("another minor of the published major", "1.1"),
        ("another minor of the working major", "0.2"),
        ("both halves at the documented 6-digit maximum", "999999.999999"),
    ];
    for (case, version) in unheld {
        let refused = describe_recipe(&emulator, held, Some(version)).await;
        assert_miss(&refused, &format!("{case} ({version})"), held, version);
    }

    // The other half: a recipe nobody created holds no version at all, whichever
    // identifier is asked for — and an omitted one is reported as the alias it defaults
    // to, never as an empty string.
    for name in ["ghost", "a ghost with a space", "g"] {
        for identifier in [None, Some(LATEST_PUBLISHED), Some(LATEST_WORKING), Some("1.0")] {
            let refused = describe_recipe(&emulator, name, identifier).await;
            assert_miss(
                &refused,
                &format!("absent recipe {name:?} / {identifier:?}"),
                name,
                identifier.unwrap_or(LATEST_PUBLISHED),
            );
        }
    }
}

/// One miss: 404, the modelled error code, and the recipe-version sentence naming both
/// halves of the pair that resolved nothing.
fn assert_miss(refused: &Reply, case: &str, name: &str, version: &str) {
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
    let expected = format!("The recipe {name} for version {version} wasn't found.");
    assert_eq!(
        error_message(refused),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        error_message(refused)
    );
}
