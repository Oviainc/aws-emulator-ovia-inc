// SPDX-License-Identifier: CC0-1.0

//! The refusal every alias operation answers a function version the function has not got
//! with.
//!
//! The rule under test: a version member of an alias operation names one of the FUNCTION's
//! own versions — its unpublished `$LATEST` or a version a publish minted — so a version
//! number the function does not hold is a resource that does not exist, refused with the
//! modelled `ResourceNotFoundException` at HTTP 404 whose message is this service's fixed
//! `Function not found: ` prefix followed by the function's own ARN qualified by exactly
//! the version the request asked for, and the refused call changes nothing. That holds for
//! every member of the alias surface that resolves a version: `CreateAlias` and
//! `UpdateAlias` `FunctionVersion`, the `RoutingConfig.AdditionalVersionWeights` keys of
//! both (the second version traffic is split to), and the `FunctionVersion` filter of
//! `ListAliases`.
//!
//! The rows differ along what the rule generalises over: the version number asked for
//! (just past the newest published one, far past it, zero, the largest 64-bit number and a
//! number wider than 64 bits), the member and the operation that resolves it, the
//! documented `FunctionName` form the request addresses the function by, and how many
//! versions the function has published (none, so every number is absent, and three). The
//! versions the function DOES hold are driven through the same members as a control, so
//! the refusal is shown to be about the version's absence rather than about the member
//! being refused wholesale.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ComAmazonawsLambdaAwsgirapiserviceState;

use crate::alias_update::{
    call, create_alias, create_function, get_alias, router_on, update_alias, Answer, ACCOUNT,
    FUNCTIONS_URI, LATEST, REGION,
};

/// The error code the model declares, at HTTP 404, for a resource an alias operation
/// cannot find: `smithy_query(query="operation", operation="UpdateAlias")` lists
/// `ResourceNotFoundException` — "The resource specified in the request does not exist."
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";

/// How many versions the functions under test publish, so there is a newest published
/// version for the rows to reach just past.
const PUBLISHED_VERSIONS: usize = 3;

/// The unqualified ARN a function of `name` carries in this suite's Region and account.
fn function_arn(name: &str) -> String {
    format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}")
}

/// The sentence a version `function` does not hold is refused with: the function's own ARN
/// qualified by the version asked for.
fn expected_message(function: &str, version: &str) -> String {
    format!("Function not found: {}:{version}", function_arn(function))
}

/// `ListAliases` on the function `reference` names, with a raw query string.
async fn list_aliases(
    router: &smithy_server_runtime::MultiServiceRouter,
    reference: &str,
    query: &str,
) -> Answer {
    call(
        router,
        "GET",
        &format!("{FUNCTIONS_URI}/{reference}/aliases{query}"),
        &Value::Null,
    )
    .await
}

/// Version numbers no publish of a function with [`PUBLISHED_VERSIONS`] versions ever
/// minted, spanning the class the version namespace admits. The namespace is the digits
/// (`Version`, pattern `^(\$LATEST|[0-9]+)$`, from
/// `smithy_query(query="shape", shape="com.amazonaws.lambda#Version")`), and a publish
/// mints `1`, `2`, `3`, … — so every row below is well formed and names nothing.
fn absent_versions() -> Vec<(String, String)> {
    vec![
        (
            "the version just past the newest published one".to_string(),
            // Derived from the fixture, so the row stays absent if the fixture publishes
            // more versions.
            (PUBLISHED_VERSIONS + 1).to_string(),
        ),
        (
            "a version far past the newest published one".to_string(),
            "97".to_string(),
        ),
        (
            "zero, which no publish ever mints".to_string(),
            "0".to_string(),
        ),
        (
            "the largest 64-bit version number".to_string(),
            "18446744073709551615".to_string(),
        ),
        (
            "a version number wider than 64 bits".to_string(),
            "999999999999999999999999999999".to_string(),
        ),
    ]
}

/// The documented `FunctionName` forms an alias operation addresses a function by: "Function
/// name", "Function ARN" and "Partial ARN", from
/// `smithy_query(query="doc", shape="com.amazonaws.lambda#UpdateAliasRequest",
/// member="FunctionName")`. The ARN in the refusal is the function's own, whichever form
/// the request used.
fn references(function: &str) -> Vec<String> {
    vec![
        function.to_string(),
        function_arn(function),
        format!("{ACCOUNT}:function:{function}"),
    ]
}

/// Assert one answer is the modelled 404 naming `version` of `function`.
fn assert_version_not_found(
    case: &str,
    function: &str,
    version: impl AsRef<str>,
    answer: &Answer,
) {
    let version = version.as_ref();
    assert_eq!(
        answer.status,
        404,
        "case {case:?}: expected the modelled 404, got {} {}",
        answer.status,
        answer.text()
    );
    assert_eq!(
        answer.error_code(),
        RESOURCE_NOT_FOUND,
        "case {case:?}: expected {RESOURCE_NOT_FOUND}, got {}",
        answer.error_code()
    );
    let expected = expected_message(function, version);
    assert_eq!(
        answer.message(),
        expected,
        "case {case:?}: expected {expected:?}, got {:?}",
        answer.message()
    );
}

/// The `(FunctionVersion, RevisionId)` the store holds for one alias of `function`.
fn stored_alias(
    engine: &Arc<engine::InMemoryEngine>,
    function: &str,
    alias: &str,
) -> Option<(String, String)> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .and_then(|record| record.aliases.get(alias))
        .map(|record| {
            (
                record.function_version.clone(),
                record.revision_id.clone(),
            )
        })
}

#[tokio::test]
async fn update_alias_refuses_a_version_the_function_never_published() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "av-update";
    create_function(&router, function, PUBLISHED_VERSIONS).await;
    let created = create_alias(
        &router,
        function,
        &json!({ "Name": "live", "FunctionVersion": "1", "Description": "first" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the alias under test must be created, got {} {}",
        created.status,
        created.text()
    );
    let before = stored_alias(&engine, function, "live")
        .expect("the created alias must be stored");

    for (label, version) in absent_versions() {
        for reference in references(function) {
            let case = format!("{label} ({version}) addressed by {reference}");
            let answer = update_alias(
                &router,
                &reference,
                "live",
                &json!({ "FunctionVersion": version }),
            )
            .await;
            assert_version_not_found(&case, function, &version, &answer);

            // A refused retarget leaves the alias on the version it already invokes, with
            // the revision it already carried: nothing was written on the way to the 404.
            let after = stored_alias(&engine, function, "live");
            assert_eq!(
                after.as_ref(),
                Some(&before),
                "case {case:?}: a refused update must leave the alias as it was, got \
                 {after:?}"
            );
        }
    }

    // The control: every version the function DOES hold is served through the same
    // member, so the rows above are refused for the version's absence.
    for version in [LATEST, "1", "2", "3"] {
        let case = format!("retargeting to the held version {version}");
        let answer = update_alias(
            &router,
            function,
            "live",
            &json!({ "FunctionVersion": version }),
        )
        .await;
        assert_eq!(
            (answer.status, answer.string("FunctionVersion")),
            (200, Some(version.to_string())),
            "case {case:?}: a version the function holds must be served, got {} {}",
            answer.status,
            answer.text()
        );
        let read = get_alias(&router, function, "live").await;
        assert_eq!(
            (read.status, read.string("FunctionVersion")),
            (200, Some(version.to_string())),
            "case {case:?}: GetAlias must report the version the update stored, got {} {}",
            read.status,
            read.text()
        );
    }
}

#[tokio::test]
async fn create_alias_refuses_a_version_the_function_never_published() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "av-create";
    create_function(&router, function, PUBLISHED_VERSIONS).await;

    for (label, version) in absent_versions() {
        for reference in references(function) {
            let case = format!("{label} ({version}) addressed by {reference}");
            let answer = call(
                &router,
                "POST",
                &format!("{FUNCTIONS_URI}/{reference}/aliases"),
                &json!({ "Name": "ghost", "FunctionVersion": version }),
            )
            .await;
            assert_version_not_found(&case, function, &version, &answer);

            // The refused create stored no alias: the name it asked for is still free.
            assert_eq!(
                stored_alias(&engine, function, "ghost"),
                None,
                "case {case:?}: a refused create must store no alias, got {:?}",
                stored_alias(&engine, function, "ghost")
            );
        }
    }

    // The control: the same name on a version the function holds is created.
    let created = create_alias(
        &router,
        function,
        &json!({ "Name": "ghost", "FunctionVersion": "3" }),
    )
    .await;
    assert_eq!(
        (created.status, created.string("FunctionVersion")),
        (201, Some("3".to_string())),
        "a version the function holds must be served, got {} {}",
        created.status,
        created.text()
    );
}

#[tokio::test]
async fn a_second_version_the_function_never_published_is_refused_the_same_way() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "av-routing";
    create_function(&router, function, PUBLISHED_VERSIONS).await;
    let created = create_alias(
        &router,
        function,
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the alias under test must be created, got {} {}",
        created.status,
        created.text()
    );
    let before = stored_alias(&engine, function, "live")
        .expect("the created alias must be stored");

    for (label, version) in absent_versions() {
        let routing = json!({ "AdditionalVersionWeights": { version.clone(): 0.25 } });

        // The second version is resolved by the update...
        let case = format!("UpdateAlias splitting traffic to {label} ({version})");
        let answer = update_alias(
            &router,
            function,
            "live",
            &json!({ "RoutingConfig": routing.clone() }),
        )
        .await;
        assert_version_not_found(&case, function, &version, &answer);
        let after = stored_alias(&engine, function, "live");
        assert_eq!(
            after.as_ref(),
            Some(&before),
            "case {case:?}: a refused update must leave the alias as it was, got {after:?}"
        );

        // ...and by the create, which is the same member of the same shape.
        let case = format!("CreateAlias splitting traffic to {label} ({version})");
        let answer = create_alias(
            &router,
            function,
            &json!({
                "Name": "ghost",
                "FunctionVersion": "1",
                "RoutingConfig": routing,
            }),
        )
        .await;
        assert_version_not_found(&case, function, &version, &answer);
        assert_eq!(
            stored_alias(&engine, function, "ghost"),
            None,
            "case {case:?}: a refused create must store no alias, got {:?}",
            stored_alias(&engine, function, "ghost")
        );
    }

    // The control: a second version the function holds, and which the alias does not
    // already invoke, is served.
    let answer = update_alias(
        &router,
        function,
        "live",
        &json!({ "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.25 } } }),
    )
    .await;
    assert_eq!(
        answer.status,
        200,
        "a second version the function holds must be served, got {} {}",
        answer.status,
        answer.text()
    );
    let read = get_alias(&router, function, "live").await;
    assert_eq!(
        read.json().get("RoutingConfig").cloned(),
        Some(json!({ "AdditionalVersionWeights": { "2": 0.25 } })),
        "GetAlias must report the routing configuration the update stored, got {}",
        read.text()
    );
}

#[tokio::test]
async fn list_aliases_refuses_a_filter_version_the_function_never_published() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "av-listing";
    create_function(&router, function, PUBLISHED_VERSIONS).await;
    let created = create_alias(
        &router,
        function,
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the alias under test must be created, got {} {}",
        created.status,
        created.text()
    );

    for (label, version) in absent_versions() {
        for reference in references(function) {
            let case = format!("{label} ({version}) addressed by {reference}");
            let answer = list_aliases(&router, &reference, &format!("?FunctionVersion={version}"))
                .await;
            assert_version_not_found(&case, function, &version, &answer);
        }
    }

    // The control: the versions the function holds filter the listing rather than
    // refusing it, and the filter SELECTS the aliases invoking that version — including
    // the versions no alias invokes, which select nothing rather than being refused.
    for (version, expected) in [
        (LATEST, Vec::new()),
        ("1", vec!["live".to_string()]),
        ("2", Vec::new()),
        ("3", Vec::new()),
    ] {
        let case = format!("filtering on the held version {version}");
        let answer = list_aliases(&router, function, &format!("?FunctionVersion={version}")).await;
        assert_eq!(
            answer.status,
            200,
            "case {case:?}: a version the function holds must be served, got {} {}",
            answer.status,
            answer.text()
        );
        let listed: Vec<String> = answer
            .json()
            .get("Aliases")
            .and_then(Value::as_array)
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|entry| entry.get("Name").and_then(Value::as_str))
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default();
        assert_eq!(
            listed, expected,
            "case {case:?}: expected the aliases invoking that version to be {expected:?}, \
             got {listed:?}"
        );
    }
}

#[tokio::test]
async fn a_function_with_no_published_version_holds_no_version_number_at_all() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let function = "av-unpublished";
    create_function(&router, function, 0).await;
    let created = create_alias(
        &router,
        function,
        &json!({ "Name": "live", "FunctionVersion": LATEST }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "the alias under test must be created, got {} {}",
        created.status,
        created.text()
    );

    // Nothing has been published, so the FIRST version number is already absent — the
    // rule is about what the function holds, not about how far past the newest the number
    // reaches.
    for version in ["1", "2", "97"] {
        let case = format!("version {version} of a function that published none");
        let update = update_alias(
            &router,
            function,
            "live",
            &json!({ "FunctionVersion": version }),
        )
        .await;
        assert_version_not_found(&case, function, version, &update);

        let listed = list_aliases(&router, function, &format!("?FunctionVersion={version}")).await;
        assert_version_not_found(&case, function, version, &listed);
    }

    // The unpublished version is one the function always holds, so the alias can be
    // retargeted at it.
    let answer = update_alias(
        &router,
        function,
        "live",
        &json!({ "FunctionVersion": LATEST, "Description": "still latest" }),
    )
    .await;
    assert_eq!(
        (answer.status, answer.string("FunctionVersion")),
        (200, Some(LATEST.to_string())),
        "the unpublished version must be served, got {} {}",
        answer.status,
        answer.text()
    );
}
