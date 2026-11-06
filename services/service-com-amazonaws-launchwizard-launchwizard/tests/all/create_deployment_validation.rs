// SPDX-License-Identifier: CC0-1.0

//! What `CreateDeployment` judges the `specifications` and `tags` maps it is given
//! against, driven over HTTP through the same router the production emulator serves
//! with.
//!
//! Two rules, each asserted over a TABLE spanning the class rather than over one
//! recorded call:
//!
//! 1. a deployment's settings are judged against the specification schema the
//!    deployment pattern PUBLISHES (the one `GetWorkloadDeploymentPattern` reports,
//!    with its `required` and `allowedValues`): a map that omits a field marked
//!    required, names a field the pattern does not publish, or carries a value outside
//!    a field's `allowedValues` is refused `ValidationException` (400) and nothing is
//!    created — whether or not `dryRun` is set, because a dry run is a pre-flight of
//!    the very same request;
//! 2. a map's KEYS are judged by the shape the model gives them —
//!    `DeploymentSpecifications` keys are `KeyString` (len 3..256, pattern
//!    `^[a-zA-Z0-9-:]+$`) and `Tags` keys are `TagKey` (len 1..128, pattern
//!    `^[a-zA-Z0-9 _.:/=+@-]+$`) — so `CreateDeployment` cannot write a tag set its own
//!    tagging API would declare invalid.
//!
//! The rows differ along what each rule generalises over: every workload and deployment
//! pattern the catalogue holds, every kind of clause the published schema can state,
//! each published key bound and one past it, the character set of a key, and whether
//! the request is a dry run. No field name, allowed value or bound is written by hand
//! where the service or the model reports it: the schema rows are read out of what
//! `GetWorkloadDeploymentPattern` publishes, and the restated key bounds are pinned to
//! `smithy-model/` by [`the_specification_key_rules_the_handlers_enforce_are_the_models`].

use std::collections::BTreeSet;

use serde_json::{json, Map, Value};

use crate::deployment_lifecycle::get;
use crate::deployment_update::{
    conforming_specifications, constrained_field, error_message, every_pattern, first_allowed_value,
    one_pattern_per_workload, pattern_data, schema_fields,
};
use crate::harness::{deployment_name, walk_pages, Harness};
use crate::resource_tagging::{deployment_arn, encode_component, model, shape_traits};
use service_com_amazonaws_launchwizard_launchwizard::state::{
    DRY_RUN_OPERATION_CODE, DRY_RUN_OPERATION_STATUS,
};

/// The wire error code and HTTP status every refusal below carries
/// (`smithy_query(query="shape",shape="ValidationException")`: `errorKind: client`,
/// `httpError: 400`).
const CODE: &str = "ValidationException";
/// See [`CODE`].
const STATUS: u16 = 400;

/// Modelled bounds of a `DeploymentSpecifications` KEY, from
/// `smithy_query(query="shape", shape="DeploymentSpecifications,KeyString")`: the key
/// targets `KeyString`, `@length(min: 3, max: 256)` with `@pattern("^[a-zA-Z0-9-:]+$")`.
/// Both are read back out of `smithy-model/` by
/// [`the_specification_key_rules_the_handlers_enforce_are_the_models`], so a row below
/// cannot be asserting a bound the model no longer declares.
const MIN_SPEC_KEY_LEN: usize = 3;
/// See [`MIN_SPEC_KEY_LEN`].
const MAX_SPEC_KEY_LEN: usize = 256;
/// Modelled bounds of a `Tags` KEY (`TagKey`: `@length(min: 1, max: 128)`), pinned to
/// the model by `resource_tagging::the_tag_rules_the_handlers_enforce_are_the_models`.
const MIN_TAG_KEY_LEN: usize = 1;
/// See [`MIN_TAG_KEY_LEN`].
const MAX_TAG_KEY_LEN: usize = 128;

/// The marker the framework's constraint renderer puts in every message about a value
/// that broke its MODELLED shape (`state::validation_report`). A refusal carrying it is
/// the key-shape rule; the schema rule states its own clauses instead.
const SHAPE_MARKER: &str = "failed to satisfy constraint";

/// A `CreateDeployment` request body over an explicit settings map. `name` is the
/// case's descriptive label, carried into a name the service's own name rule accepts by
/// [`deployment_name`].
fn create_request(
    workload: &str,
    pattern: &str,
    name: &str,
    specifications: &Map<String, Value>,
    dry_run: bool,
) -> Value {
    json!({
        "workloadName": workload,
        "deploymentPatternName": pattern,
        "name": deployment_name(name),
        "specifications": Value::Object(specifications.clone()),
        "dryRun": dry_run,
    })
}

/// Every deployment NAME the service currently stores — what a refused create must not
/// have added to.
async fn stored_names(harness: &Harness, case: &str) -> BTreeSet<String> {
    walk_pages(harness, case, "/listDeployments", &json!({}), "deployments", 100)
        .await
        .iter()
        .map(|deployment| {
            deployment["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a deployment summary carries no name"))
                .to_string()
        })
        .collect()
}

#[test]
fn the_specification_key_rules_the_handlers_enforce_are_the_models() {
    // `CreateDeployment` and `UpdateDeployment` judge a specification key against rules
    // RESTATED in Rust rather than read off a generated schema, because codegen captures
    // a collection's element/value constraints but not a map's KEY constraints. This
    // pins the restatement to the model, so a model refresh that moved a bound fails
    // here instead of leaving the emulator judging by a stale one.
    use service_com_amazonaws_launchwizard_launchwizard::state;

    let model = model();
    let key_target = model["shapes"]["com.amazonaws.launchwizard#DeploymentSpecifications"]["key"]
        ["target"]
        .clone();
    assert_eq!(
        key_target,
        json!("com.amazonaws.launchwizard#KeyString"),
        "a DeploymentSpecifications key must still target KeyString, got {key_target}"
    );
    let key_string = shape_traits(&model, "KeyString");
    assert_eq!(
        key_string["smithy.api#length"]["min"],
        json!(state::SPECIFICATION_KEY_MIN_LEN),
        "KeyString's modelled length floor must be the one the handlers enforce, got {key_string}"
    );
    assert_eq!(
        key_string["smithy.api#length"]["max"],
        json!(state::SPECIFICATION_KEY_MAX_LEN),
        "KeyString's modelled length ceiling must be the one the handlers enforce, got {key_string}"
    );
    assert_eq!(
        key_string["smithy.api#pattern"],
        json!(state::SPECIFICATION_KEY_PATTERN),
        "KeyString's modelled pattern must be the one the handlers enforce, got {key_string}"
    );
    assert_eq!(
        (
            state::SPECIFICATION_KEY_MIN_LEN as usize,
            state::SPECIFICATION_KEY_MAX_LEN as usize
        ),
        (MIN_SPEC_KEY_LEN, MAX_SPEC_KEY_LEN),
        "this suite's KeyString bounds must be the ones the handlers enforce"
    );
    assert_eq!(
        (
            state::TAG_KEY_MIN_LEN as usize,
            state::TAG_KEY_MAX_LEN as usize
        ),
        (MIN_TAG_KEY_LEN, MAX_TAG_KEY_LEN),
        "this suite's TagKey bounds must be the ones the handlers enforce"
    );
}

#[tokio::test]
async fn a_create_is_judged_against_the_specification_schema_the_pattern_publishes() {
    let harness = Harness::new();
    // One row group per deployment pattern the catalogue holds — the rule is about the
    // schema a pattern publishes, so every published schema is exercised.
    for (index, (workload, pattern)) in every_pattern(&harness).await.into_iter().enumerate() {
        let case = format!("{workload}/{pattern}");
        let data = pattern_data(&harness, &case, &workload, &pattern).await;

        // A map the schema accepts is stored and read back, at two sizes: only the
        // fields marked required, and every field the pattern publishes.
        for (label, include_optional) in [
            ("the required fields only", false),
            ("every published field", true),
        ] {
            let case = format!("{case} with {label}");
            let specifications = conforming_specifications(&case, &data, include_optional);
            let name = format!("spec-ok-{index}-{}", usize::from(include_optional));
            let created = harness
                .ok(
                    &case,
                    "/createDeployment",
                    create_request(&workload, &pattern, &name, &specifications, false),
                )
                .await;
            let id = created["deploymentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no deploymentId in {created}"))
                .to_string();
            let deployment = get(&harness, &case, &id).await;
            assert_eq!(
                deployment["specifications"],
                Value::Object(specifications.clone()),
                "case {case:?}: settings the schema accepts must come back as stored, got {}",
                deployment["specifications"]
            );

            // The same map as a dry run passes the same checks and stores nothing: it
            // is answered with the documented pre-flight error response rather than
            // the schema refusal below (see `tests/all/dry_run_preflight.rs`).
            let dry = harness
                .refused(
                    &case,
                    "/createDeployment",
                    create_request(
                        &workload,
                        &pattern,
                        &format!("{name}-dry"),
                        &specifications,
                        true,
                    ),
                    DRY_RUN_OPERATION_STATUS,
                    DRY_RUN_OPERATION_CODE,
                )
                .await;
            assert!(
                dry.get("deploymentId").is_none() || dry["deploymentId"].is_null(),
                "case {case:?}: a dry run stores nothing, so it mints no id, got {}",
                dry["deploymentId"]
            );
        }

        // One refusal row per clause the published schema can state, built from the
        // schema the service itself reports: every field it marks required (removed),
        // every field it restricts to `allowedValues` (given one outside them), and a
        // well-shaped key it does not publish at all.
        let conforming = conforming_specifications(&case, &data, true);
        let mut rows: Vec<(String, Map<String, Value>, String)> = Vec::new();
        for field in schema_fields(&case, &data) {
            let name = field["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a schema field carries no name"))
                .to_string();
            if field["required"].as_str() == Some("Yes") {
                let mut specifications = conforming.clone();
                specifications.remove(&name);
                rows.push((
                    format!("without the required {name}"),
                    specifications,
                    name.clone(),
                ));
            }
            if first_allowed_value(&field).is_some() {
                let mut specifications = conforming.clone();
                specifications.insert(name.clone(), json!(OUTSIDE_ALLOWED));
                rows.push((
                    format!("{name} outside its allowedValues"),
                    specifications,
                    name.clone(),
                ));
            }
        }
        let unpublished = "NoSuchSetting";
        let mut with_unpublished = conforming.clone();
        with_unpublished.insert(unpublished.to_string(), json!(UNPUBLISHED_VALUE));
        rows.push((
            "a setting the pattern does not publish".to_string(),
            with_unpublished,
            unpublished.to_string(),
        ));
        assert!(
            rows.len() >= 3,
            "case {case:?}: the schema must publish enough to state each clause, got {}",
            rows.len()
        );

        for (row, (label, specifications, named)) in rows.into_iter().enumerate() {
            // A dry run is a pre-flight of the same request, so each row runs both ways
            // and must be refused both ways.
            for dry_run in [false, true] {
                let case = format!("{case} {label} (dryRun {dry_run})");
                let name = format!("spec-bad-{index}-{row}-{}", usize::from(dry_run));
                let before = stored_names(&harness, &case).await;
                let body = harness
                    .refused(
                        &case,
                        "/createDeployment",
                        create_request(&workload, &pattern, &name, &specifications, dry_run),
                        STATUS,
                        CODE,
                    )
                    .await;
                let message = error_message(&body);
                assert!(
                    message.contains(&named),
                    "case {case:?}: the refusal must name the setting {named:?} so the caller \
                     can fix the request, got {message:?}"
                );
                // `DeploymentSpecifications` is @sensitive: a submitted VALUE is never
                // replayed into the error channel.
                for secret in [OUTSIDE_ALLOWED, UNPUBLISHED_VALUE] {
                    assert!(
                        !message.contains(secret),
                        "case {case:?}: the refusal must not echo a submitted value, got \
                         {message:?}"
                    );
                }
                assert_eq!(
                    stored_names(&harness, &case).await,
                    before,
                    "case {case:?}: a refused create must store nothing"
                );
            }
        }
    }
}

/// The value a row puts where the schema publishes `allowedValues`, and the value of a
/// setting the pattern does not publish. Neither may appear in a refusal.
const OUTSIDE_ALLOWED: &str = "not-an-allowed-value";
/// See [`OUTSIDE_ALLOWED`].
const UNPUBLISHED_VALUE: &str = "unexpected-setting-value";

#[tokio::test]
async fn specification_keys_are_judged_by_the_modelled_key_shape() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern}");
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);

    // Rows span each published bound of `KeyString` and one past it, plus the two ways
    // its character class can be broken. Every row carries the conforming map as well,
    // so the ONLY thing left to refuse is the key itself.
    let outside: Vec<(String, String)> = vec![
        (
            "one character below the length floor".to_string(),
            "k".repeat(MIN_SPEC_KEY_LEN - 1),
        ),
        ("an empty key".to_string(), String::new()),
        (
            "one character past the length ceiling".to_string(),
            "k".repeat(MAX_SPEC_KEY_LEN + 1),
        ),
        ("a space, outside the key pattern".to_string(), "bad key".to_string()),
        (
            "punctuation outside the key pattern".to_string(),
            "bad.key!".to_string(),
        ),
    ];
    for (row, (label, key)) in outside.into_iter().enumerate() {
        let case = format!("{case}: {label}");
        let mut specifications = conforming.clone();
        specifications.insert(key.clone(), json!("x"));
        let before = stored_names(&harness, &case).await;
        let body = harness
            .refused(
                &case,
                "/createDeployment",
                create_request(
                    &workload,
                    &pattern,
                    &format!("key-bad-{row}"),
                    &specifications,
                    false,
                ),
                STATUS,
                CODE,
            )
            .await;
        let message = error_message(&body);
        assert!(
            message.contains(SHAPE_MARKER),
            "case {case:?}: a key outside its modelled shape is refused by that shape, got \
             {message:?}"
        );
        assert_eq!(
            stored_names(&harness, &case).await,
            before,
            "case {case:?}: a refused create must store nothing"
        );
    }

    // A key INSIDE the key shape — at each of its bounds, and over the punctuation its
    // pattern admits — gets past the key rule: it is then refused only as a setting the
    // pattern does not publish, which is the other rule and names the key.
    let inside = vec![
        ("at the length floor", "k".repeat(MIN_SPEC_KEY_LEN)),
        ("at the length ceiling", "k".repeat(MAX_SPEC_KEY_LEN)),
        // The two punctuation characters `^[a-zA-Z0-9-:]+$` admits.
        ("every punctuation character the pattern admits", "a-b:c".to_string()),
    ];
    for (row, (label, key)) in inside.into_iter().enumerate() {
        let case = format!("{case}: a well-shaped key {label}");
        let mut specifications = conforming.clone();
        specifications.insert(key.clone(), json!("x"));
        let body = harness
            .refused(
                &case,
                "/createDeployment",
                create_request(
                    &workload,
                    &pattern,
                    &format!("key-ok-{row}"),
                    &specifications,
                    false,
                ),
                STATUS,
                CODE,
            )
            .await;
        let message = error_message(&body);
        assert!(
            !message.contains(SHAPE_MARKER),
            "case {case:?}: a key inside its modelled shape must not be refused by that \
             shape, got {message:?}"
        );
        assert!(
            message.contains(&key),
            "case {case:?}: the refusal must name the unpublished setting, got {message:?}"
        );
    }
}

#[tokio::test]
async fn createdeployment_and_tagresource_judge_a_tag_key_the_same_way() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern}");
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);

    // A deployment to tag through `TagResource`, so the two paths are compared on the
    // same key.
    let subject = harness
        .ok(
            "a deployment to tag",
            "/createDeployment",
            create_request(&workload, &pattern, "tag-key-parity", &conforming, false),
        )
        .await["deploymentId"]
        .as_str()
        .expect("a created deployment reports its id")
        .to_string();
    let arn = deployment_arn(&subject);

    // Rows span each published bound of `TagKey` and one past it, and both sides of its
    // character class. `accepted` is what BOTH paths must answer.
    let rows: Vec<(&str, String, bool)> = vec![
        ("at the length floor", "k".repeat(MIN_TAG_KEY_LEN), true),
        ("at the length ceiling", "k".repeat(MAX_TAG_KEY_LEN), true),
        (
            "every punctuation character the pattern admits",
            "A0 _.:/=+@-z".to_string(),
            true,
        ),
        ("an empty key", String::new(), false),
        (
            "one character past the length ceiling",
            "k".repeat(MAX_TAG_KEY_LEN + 1),
            false,
        ),
        ("a key outside the key pattern", "no\u{a3}good".to_string(), false),
    ];

    for (row, (label, key, accepted)) in rows.into_iter().enumerate() {
        let case = format!("{case}: a tag key {label}");
        let tags = json!({key.clone(): "v"});
        let mut request = create_request(
            &workload,
            &pattern,
            &format!("tag-key-{row}"),
            &conforming,
            false,
        );
        request["tags"] = tags.clone();

        if accepted {
            let created = harness.ok(&case, "/createDeployment", request).await;
            let id = created["deploymentId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: no deploymentId in {created}"))
                .to_string();
            let deployment = get(&harness, &case, &id).await;
            assert_eq!(
                deployment["tags"], tags,
                "case {case:?}: an accepted tag set must come back as stored, got {}",
                deployment["tags"]
            );
            // And the same key is accepted by the tagging API.
            harness
                .expect_ok(
                    &case,
                    "POST",
                    &format!("/tags/{}", encode_component(&arn)),
                    Some(&json!({"tags": tags})),
                )
                .await;
            continue;
        }

        let before = stored_names(&harness, &case).await;
        harness
            .refused(&case, "/createDeployment", request, STATUS, CODE)
            .await;
        assert_eq!(
            stored_names(&harness, &case).await,
            before,
            "case {case:?}: a refused create must store nothing"
        );
        // The parity the issue is about: a key `CreateDeployment` refuses is a key
        // `TagResource` refuses, so no deployment can carry a tag set this service's own
        // tagging API declares invalid.
        harness
            .expect_refused(
                &case,
                "POST",
                &format!("/tags/{}", encode_component(&arn)),
                Some(&json!({"tags": tags})),
                STATUS,
                CODE,
            )
            .await;
    }
}

#[tokio::test]
async fn an_update_judges_its_specification_keys_by_the_same_key_shape() {
    let harness = Harness::new();
    let (workload, pattern) = one_pattern_per_workload(&harness)
        .await
        .into_iter()
        .next()
        .expect("the catalogue holds a pattern");
    let case = format!("{workload}/{pattern}");
    let data = pattern_data(&harness, &case, &workload, &pattern).await;
    let conforming = conforming_specifications(&case, &data, true);
    let id = harness
        .ok(
            &case,
            "/createDeployment",
            create_request(&workload, &pattern, "update-key-shape", &conforming, false),
        )
        .await["deploymentId"]
        .as_str()
        .expect("a created deployment reports its id")
        .to_string();
    let stored = get(&harness, &case, &id).await;

    // The key rule belongs to the model, not to the operation: an update is refused by
    // it too, and `force` — which overrides the schema's WARNINGS — does not override a
    // key the model refuses.
    let restricted = constrained_field(&case, &data);
    for (label, force) in [("without force", false), ("with force", true)] {
        let case = format!("{case}: an undersized key {label}");
        let mut specifications = conforming.clone();
        specifications.insert("k".repeat(MIN_SPEC_KEY_LEN - 1), json!("x"));
        // One schema warning as well, so `force` would have applied the update if the
        // key rule were a warning.
        specifications.insert(restricted.clone(), json!(OUTSIDE_ALLOWED));
        let body = harness
            .refused(
                &case,
                "/updateDeployment",
                json!({
                    "deploymentId": id,
                    "specifications": Value::Object(specifications),
                    "force": force,
                }),
                STATUS,
                CODE,
            )
            .await;
        let message = error_message(&body);
        assert!(
            message.contains(SHAPE_MARKER),
            "case {case:?}: a key outside its modelled shape is refused by that shape, got \
             {message:?}"
        );
        let read = get(&harness, &case, &id).await;
        assert_eq!(
            read["specifications"], stored["specifications"],
            "case {case:?}: a refused update must persist nothing, got {}",
            read["specifications"]
        );
    }
}
