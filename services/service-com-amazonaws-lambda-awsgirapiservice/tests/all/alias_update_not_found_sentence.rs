// SPDX-License-Identifier: CC0-1.0

//! The SENTENCE `UpdateAlias` refuses a name the function holds no alias of with.
//!
//! The rule under test: Lambda words a missing alias per OPERATION, not once for the
//! resource kind — the update of an alias answers the modelled
//! `ResourceNotFoundException` (HTTP 404) whose message is the fixed lead-in
//! [`UPDATE_NOT_FOUND_PREFIX`] followed by the fully-qualified alias ARN the request asked
//! about (the function's own unqualified ARN with the requested alias name as its
//! qualifier), while the READ of that same vanished alias keeps its own lead-in
//! ([`READ_NOT_FOUND_PREFIX`]), so the two refusals are tellable apart by the phrase
//! alone. Both ARNs are wholly request-derived: `FunctionName` and `Name` are the
//! operation's two URI labels.
//!
//! The rows differ along what the rule generalises over: the alias name asked for (its
//! modelled length bounds, either separator the pattern admits, mixed case with digits),
//! the documented `FunctionName` form the request addressed the function by (name,
//! function ARN, partial ARN), which members the refused update stated (none, the
//! description, the version, the routing configuration), and whether the alias never
//! existed or was deleted after having been created. A missing FUNCTION is checked too,
//! because it is a different resource and keeps its own sentence; the alias the function
//! does hold is updated as a control, so the refusal is shown to be about that name's
//! absence rather than about the operation refusing wholesale.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;

use crate::alias_update::{
    create_alias, create_function, delete_alias, get_alias, router_on, update_alias, Answer,
    ACCOUNT, REGION,
};

/// The error code the model declares, at HTTP 404, for a resource `UpdateAlias` cannot
/// find: `smithy_query(query="operation", operation="UpdateAlias")` lists
/// `ResourceNotFoundException` — "The resource specified in the request does not exist."
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// The lead-in of the sentence the UPDATE of a missing alias is refused with, which the
/// alias ARN follows.
const UPDATE_NOT_FOUND_PREFIX: &str = "Alias not found: ";

/// The lead-in of the sentence the READ of the same missing alias is refused with — a
/// different phrase for the same absence, which `alias_not_found_sentence.rs` pins on
/// `GetAlias` itself.
const READ_NOT_FOUND_PREFIX: &str = "Cannot find alias arn: ";

/// The lead-in of the sentence a FUNCTION that does not exist is refused with.
const FUNCTION_NOT_FOUND_PREFIX: &str = "Function not found: ";

/// The `@length` maximum the model puts on `Alias` (`@length(min: 1, max: 128)`), from
/// `smithy_query(query="shape", shape="com.amazonaws.lambda#Alias")`.
const ALIAS_MAX_LEN: usize = 128;

/// How many versions the function under test publishes, so an update has versions to
/// retarget to.
const PUBLISHED_VERSIONS: usize = 2;

/// The unqualified ARN a function of `name` carries in this suite's Region and account.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The alias ARN a refusal names: the function's own ARN qualified by the alias name.
fn alias_arn(function: &str, alias: &str) -> String {
    format!("{}:{alias}", function_arn(function))
}

/// Alias names the modelled `Alias` constraints admit (pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$`, length 1..128), spanning the class the sentence
/// generalises over: the length minimum, both separators, mixed case with digits, and the
/// length maximum.
fn absent_alias_names() -> Vec<(String, String)> {
    vec![
        ("one character, the length minimum".to_string(), "g".to_string()),
        ("a hyphenated name".to_string(), "ghost-alias".to_string()),
        ("an underscored name".to_string(), "ghost_alias".to_string()),
        ("mixed case with digits".to_string(), "Ghost2024v2".to_string()),
        (
            "at the length maximum".to_string(),
            format!("g{}", "h".repeat(ALIAS_MAX_LEN - 1)),
        ),
    ]
}

/// The documented `FunctionName` forms the operation addresses a function by ("Function
/// name", "Function ARN", "Partial ARN", from
/// `smithy_query(query="doc", shape="com.amazonaws.lambda#UpdateAliasRequest",
/// member="FunctionName")`). The ARN in the refusal is the function's own, whichever form
/// the request used.
fn references(function: &str) -> Vec<String> {
    vec![
        function.to_string(),
        function_arn(function),
        format!("{ACCOUNT}:function:{function}"),
    ]
}

/// The member subsets an update can state, so the refusal is shown not to depend on which
/// of them the request carried.
fn update_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("stating no updatable member", json!({})),
        ("stating the description", json!({ "Description": "post-delete" })),
        ("stating the version", json!({ "FunctionVersion": "1" })),
        (
            "stating the routing configuration",
            json!({ "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.25 } } }),
        ),
    ]
}

/// Assert one answer is the modelled 404 whose message is the update's own sentence around
/// the ARN of `alias` on `function`.
fn assert_update_refusal(case: &str, function: &str, alias: &str, answer: &Answer) {
    let expected = format!("{UPDATE_NOT_FOUND_PREFIX}{}", alias_arn(function, alias));
    assert_eq!(
        (answer.status, answer.error_code()),
        (404, RESOURCE_NOT_FOUND.to_string()),
        "case {case:?}: expected 404 {RESOURCE_NOT_FOUND}, got {} {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        answer.message()
    );
}

#[tokio::test]
async fn an_update_of_an_alias_the_function_does_not_hold_names_it_by_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "ua-ghosted", PUBLISHED_VERSIONS).await;
    // One alias that DOES exist, so each refusal below is about the name asked for rather
    // than about a function with no aliases at all.
    let created = create_alias(
        &router,
        "ua-ghosted",
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the alias that proves the function is writable must be created, got {} {}",
        created.status,
        created.text()
    );

    for (form, alias) in absent_alias_names() {
        for reference in references("ua-ghosted") {
            for (members, body) in update_bodies() {
                let case = format!("{form} addressed by {reference}, {members}");
                let answer = update_alias(&router, &reference, &alias, &body).await;
                assert_update_refusal(&case, "ua-ghosted", &alias, &answer);

                // The refused update created nothing: the name is still absent, and the
                // read of it keeps its OWN sentence for the same absence.
                let read = get_alias(&router, &reference, &alias).await;
                let read_expected =
                    format!("{READ_NOT_FOUND_PREFIX}{}", alias_arn("ua-ghosted", &alias));
                assert_eq!(
                    (read.status, read.message()),
                    (404, read_expected.clone()),
                    "case {case:?}: GetAlias must still answer {read_expected:?}, got {} {}",
                    read.status,
                    read.text()
                );
            }
        }
    }

    // The control: the alias the function DOES hold is updated through the same labels.
    let updated = update_alias(
        &router,
        "ua-ghosted",
        "live",
        &json!({ "Description": "still writable" }),
    )
    .await;
    assert_eq!(
        (updated.status, updated.string("Description")),
        (200, Some("still writable".to_string())),
        "the alias the function holds must still be updatable, got {} {}",
        updated.status,
        updated.text()
    );

    // Nothing the refusals asked for was stored: the function holds exactly the one alias
    // that was created.
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    let record = store
        .functions
        .get("ua-ghosted")
        .expect("the function must be stored");
    assert_eq!(
        record.aliases.keys().cloned().collect::<Vec<_>>(),
        vec!["live".to_string()],
        "a refused UpdateAlias must store no alias, got {:?}",
        record.aliases.keys().collect::<Vec<_>>()
    );
}

/// RULE: the sentence is about the name's absence, however it came to be absent — an alias
/// that was created and then deleted is refused exactly as one that never existed.
#[tokio::test]
async fn an_update_after_the_alias_was_deleted_names_it_by_its_arn() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "ua-deleted", PUBLISHED_VERSIONS).await;

    for (form, alias) in absent_alias_names() {
        let created = create_alias(
            &router,
            "ua-deleted",
            &json!({ "Name": alias, "FunctionVersion": "1" }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {form:?}: the alias to delete must be created, got {} {}",
            created.status,
            created.text()
        );
        let deleted = delete_alias(&router, "ua-deleted", &alias).await;
        assert_eq!(
            deleted.status,
            204,
            "case {form:?}: the alias must be deleted, got {} {}",
            deleted.status,
            deleted.text()
        );

        for (members, body) in update_bodies() {
            let case = format!("{form}, deleted, {members}");
            let answer = update_alias(&router, "ua-deleted", &alias, &body).await;
            assert_update_refusal(&case, "ua-deleted", &alias, &answer);
        }
    }
}

/// RULE: the missing resource is the one the sentence names — a FUNCTION that does not
/// exist keeps the function-shaped refusal, so a caller can tell the two apart.
#[tokio::test]
async fn an_update_on_a_function_that_does_not_exist_keeps_its_own_refusal() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for (form, alias) in absent_alias_names() {
        let answer = update_alias(
            &router,
            "ua-never-created",
            &alias,
            &json!({ "Description": "x" }),
        )
        .await;
        let expected = format!(
            "{FUNCTION_NOT_FOUND_PREFIX}{}",
            function_arn("ua-never-created")
        );
        assert_eq!(
            (answer.status, answer.error_code()),
            (404, RESOURCE_NOT_FOUND.to_string()),
            "case {form:?}: expected 404 {RESOURCE_NOT_FOUND}, got {} {}",
            answer.status,
            answer.text()
        );
        assert_eq!(
            answer.message(),
            expected,
            "case {form:?}: a missing function must be named as the function, got {:?}",
            answer.message()
        );
    }
}
