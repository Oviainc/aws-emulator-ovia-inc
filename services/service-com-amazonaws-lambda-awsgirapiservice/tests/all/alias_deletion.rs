// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `DeleteAlias`: "Deletes a Lambda Event Function alias."
//!
//! The rule under test: the operation is `@idempotent` with a modelled 204, so its answer
//! states the END STATE rather than whether this call reached it — a name the function
//! holds no alias of succeeds having removed nothing, and a name it does hold is removed
//! together with everything addressed by that alias's own ARN (its function URL, its
//! resource-based policy), leaving every other alias of the function where it was. The
//! FUNCTION still has to exist, so a name that resolves to none is the modelled
//! `ResourceNotFoundException`. An alias a provisioned concurrency configuration is
//! attached to is in use by that allocation and is refused with the modelled
//! `ResourceConflictException` until the configuration is deleted.
//!
//! Every row drives real HTTP requests through the router — over the ONE alias harness this
//! test binary has, [`crate::alias_update`] — and reads the result back through `GetAlias`
//! and `ListAliases`, so a delete that answered 204 without removing the record, or that
//! removed more than it was asked for, fails here. The rows differ along what the rule
//! generalises over: whether the named alias exists, the SHAPE of the name (a word, a
//! hyphenated name, an underscored one, a name with digits, a 128-character one), the
//! documented `FunctionName` form the request addresses the function by, and what else the
//! deleted alias carried.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::{
    ComAmazonawsLambdaAwsgirapiserviceState, ACTIVATION_DELAY,
};

use crate::alias_update::{
    call, create_alias, create_function, delete_alias, get_alias, router_on, Answer, ACCOUNT,
    FUNCTIONS_URI, LATEST, REGION,
};

/// Where the provisioned-concurrency operations are bound, from
/// `smithy_query(query="operation", operation="PutProvisionedConcurrencyConfig")`.
const PROVISIONED_CONCURRENCY_PATH: &str = "provisioned-concurrency";

/// Where the function-URL operations are bound.
const FUNCTION_URL_URI: &str = "/2021-10-31/functions";

/// How long past a due instant this suite waits before calling. The transition is owed by
/// the CLOCK, so nothing here sleeps on the emulator's behalf: it waits past the instant
/// the create made due.
const SLACK: Duration = Duration::from_millis(100);

/// The alias names the store holds for `function`, read out of the engine's own state.
fn stored_aliases(engine: &Arc<engine::InMemoryEngine>, function: &str) -> Vec<String> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|record| record.aliases.keys().cloned().collect())
        .unwrap_or_default()
}

/// The qualifiers `function` holds a function URL configuration on.
fn stored_url_qualifiers(engine: &Arc<engine::InMemoryEngine>, function: &str) -> Vec<String> {
    let state = engine.state::<ComAmazonawsLambdaAwsgirapiserviceState>();
    let store = state.lock();
    store
        .functions
        .get(function)
        .map(|record| record.url_configs.keys().cloned().collect())
        .unwrap_or_default()
}

/// `PutProvisionedConcurrencyConfig` on the qualifier `qualifier` of `function`.
async fn put_provisioned_concurrency(
    router: &smithy_server_runtime::MultiServiceRouter,
    function: &str,
    qualifier: &str,
    executions: i32,
) -> Answer {
    call(
        router,
        "PUT",
        &format!(
            "/2019-09-30/functions/{function}/{PROVISIONED_CONCURRENCY_PATH}?Qualifier={qualifier}"
        ),
        &json!({ "ProvisionedConcurrentExecutions": executions }),
    )
    .await
}

/// `DeleteProvisionedConcurrencyConfig` on the qualifier `qualifier` of `function`.
async fn delete_provisioned_concurrency(
    router: &smithy_server_runtime::MultiServiceRouter,
    function: &str,
    qualifier: &str,
) -> Answer {
    call(
        router,
        "DELETE",
        &format!(
            "/2019-09-30/functions/{function}/{PROVISIONED_CONCURRENCY_PATH}?Qualifier={qualifier}"
        ),
        &Value::Null,
    )
    .await
}

/// One deletion row: the alias name, whether it was ever created, how the request
/// addresses the function, and what the alias carried.
struct DeleteCase {
    name: &'static str,
    function: &'static str,
    /// The alias name the delete addresses.
    alias: String,
    /// Whether that alias is created before the delete.
    created: bool,
    /// What the create states beside the name, for an alias that exists.
    extras: Value,
    /// How the delete addresses the function, built from the function name.
    reference: fn(&str) -> String,
}

/// The rows. Half name an alias the function holds and half name one it never held; they
/// differ in the shape of the name (the forms the modelled `Alias` pattern
/// `^(?!^[0-9]+$)([a-zA-Z0-9-_]+)$` admits, up to its `@length(max: 128)`), in the
/// documented `FunctionName` form the delete uses, and in what the alias carried.
fn delete_cases() -> Vec<DeleteCase> {
    vec![
        DeleteCase {
            name: "an alias the function holds, addressed by the bare name",
            function: "da-held-bare",
            alias: "live".to_string(),
            created: true,
            extras: json!({ "FunctionVersion": "1" }),
            reference: |name| name.to_string(),
        },
        DeleteCase {
            name: "a hyphenated alias, addressed by the full function arn",
            function: "da-held-arn",
            alias: "blue-green".to_string(),
            created: true,
            extras: json!({ "FunctionVersion": "2", "Description": "canary" }),
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        },
        DeleteCase {
            name: "an underscored alias routing traffic to a second version, by partial arn",
            function: "da-held-partial",
            alias: "prod_eu".to_string(),
            created: true,
            extras: json!({
                "FunctionVersion": "1",
                "RoutingConfig": { "AdditionalVersionWeights": { "2": 0.3 } },
            }),
            reference: |name| format!("{ACCOUNT}:function:{name}"),
        },
        DeleteCase {
            name: "an alias name with digits in it, which the function never held",
            function: "da-absent-digits",
            alias: "v2-canary".to_string(),
            created: false,
            extras: json!({}),
            reference: |name| name.to_string(),
        },
        DeleteCase {
            name: "a never-created alias, addressed by the function: prefix form",
            function: "da-absent-prefix",
            alias: "never-created".to_string(),
            created: false,
            extras: json!({}),
            reference: |name| format!("function:{name}"),
        },
        DeleteCase {
            name: "a never-created alias at the modelled maximum name length",
            function: "da-absent-long",
            // 128 characters, the `Alias` shape's `@length(max: 128)`.
            alias: "a".repeat(128),
            created: false,
            extras: json!({}),
            reference: |name| format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:{name}"),
        },
    ]
}

#[tokio::test]
async fn a_delete_leaves_the_function_holding_no_alias_of_that_name() {
    for case in delete_cases() {
        let label = case.name;
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, case.function, 2).await;

        // A second alias that no delete names: it must survive every row.
        let bystander = create_alias(
            &router,
            case.function,
            &json!({ "Name": "bystander", "FunctionVersion": "1" }),
        )
        .await;
        assert_eq!(
            bystander.status,
            201,
            "case {label:?}: the bystander alias must be created, got {} {}",
            bystander.status,
            bystander.text()
        );

        if case.created {
            let mut body = case.extras.as_object().cloned().unwrap_or_default();
            body.insert("Name".to_string(), json!(case.alias.clone()));
            let created = create_alias(&router, case.function, &Value::Object(body)).await;
            assert_eq!(
                created.status,
                201,
                "case {label:?}: the alias under test must be created, got {} {}",
                created.status,
                created.text()
            );
        }

        let reference = (case.reference)(case.function);
        // Deleted TWICE: an `@idempotent` operation answers the same end state both
        // times, whichever call was the one that removed the alias.
        for attempt in 1..=2 {
            let answer = delete_alias(&router, &reference, &case.alias).await;
            assert_eq!(
                answer.status,
                204,
                "case {label:?}: attempt {attempt} expected the modelled 204 from \
                 DeleteAlias, got {} {}",
                answer.status,
                answer.text()
            );
            assert!(
                answer.body.is_empty(),
                "case {label:?}: attempt {attempt} a 204 carries no body, got {:?}",
                answer.text()
            );
        }

        assert_eq!(
            stored_aliases(&engine, case.function),
            vec!["bystander".to_string()],
            "case {label:?}: the function must hold only the bystander alias afterwards, \
             got {:?}",
            stored_aliases(&engine, case.function)
        );
        let read = get_alias(&router, case.function, &case.alias).await;
        assert_eq!(
            read.status,
            404,
            "case {label:?}: GetAlias of the deleted name must be the modelled 404, got {} {}",
            read.status,
            read.text()
        );
        assert!(
            read.error_code().contains("ResourceNotFoundException"),
            "case {label:?}: expected ResourceNotFoundException from GetAlias, got {}",
            read.error_code()
        );
        let survivor = get_alias(&router, case.function, "bystander").await;
        assert_eq!(
            (survivor.status, survivor.string("RevisionId")),
            (200, bystander.string("RevisionId")),
            "case {label:?}: every other alias must be untouched, got {} {}",
            survivor.status,
            survivor.text()
        );
    }
}

#[tokio::test]
async fn deleting_an_alias_takes_the_function_url_addressed_by_its_arn_with_it() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "da-cascade", 1).await;
    for alias in ["live", "staging"] {
        let created = create_alias(
            &router,
            "da-cascade",
            &json!({ "Name": alias, "FunctionVersion": "1" }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "the {alias:?} alias must be created, got {} {}",
            created.status,
            created.text()
        );
        let url = call(
            &router,
            "POST",
            &format!("{FUNCTION_URL_URI}/da-cascade/url?Qualifier={alias}"),
            &json!({ "AuthType": "NONE" }),
        )
        .await;
        assert_eq!(
            url.status,
            201,
            "the function URL of {alias:?} must be created, got {} {}",
            url.status,
            url.text()
        );
    }

    let deleted = delete_alias(&router, "da-cascade", "live").await;
    assert_eq!(
        deleted.status,
        204,
        "the delete must succeed, got {} {}",
        deleted.status,
        deleted.text()
    );
    assert_eq!(
        stored_url_qualifiers(&engine, "da-cascade"),
        vec!["staging".to_string()],
        "the deleted alias's function URL goes with it and no other does, got {:?}",
        stored_url_qualifiers(&engine, "da-cascade")
    );

    // A later alias of the same name holds no function URL: it never had one.
    let recreated = create_alias(
        &router,
        "da-cascade",
        &json!({ "Name": "live", "FunctionVersion": "1" }),
    )
    .await;
    assert_eq!(
        recreated.status,
        201,
        "the name must be free to take a new alias, got {} {}",
        recreated.status,
        recreated.text()
    );
    let read_url = call(
        &router,
        "GET",
        &format!("{FUNCTION_URL_URI}/da-cascade/url?Qualifier=live"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        read_url.status,
        404,
        "a re-created alias must hold no function URL, got {} {}",
        read_url.status,
        read_url.text()
    );
}

#[tokio::test]
async fn an_alias_with_provisioned_concurrency_attached_is_not_deleted() {
    // One row per qualifier shape a provisioned concurrency configuration can be attached
    // to that an alias delete addresses: the alias itself, and (as the control) the
    // published version the alias invokes, whose configuration is a different resource and
    // does not block the alias.
    for (label, function, configured_on, blocks) in [
        ("the alias the delete names", "da-pc-alias", "live", true),
        (
            "the published version the alias invokes",
            "da-pc-version",
            "1",
            false,
        ),
    ] {
        let engine = Arc::new(engine::InMemoryEngine::new());
        let router = router_on(&engine);
        create_function(&router, function, 1).await;
        let created = create_alias(
            &router,
            function,
            &json!({ "Name": "live", "FunctionVersion": "1" }),
        )
        .await;
        assert_eq!(
            created.status,
            201,
            "case {label:?}: the alias under test must be created, got {} {}",
            created.status,
            created.text()
        );
        // "When the code is Creating, you can't invoke or modify the function": a
        // provisioned concurrency configuration is accepted once the function is active,
        // which the clock makes due.
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
        let configured = put_provisioned_concurrency(&router, function, configured_on, 5).await;
        assert_eq!(
            configured.status,
            202,
            "case {label:?}: the provisioned concurrency configuration must be accepted, \
             got {} {}",
            configured.status,
            configured.text()
        );

        let answer = delete_alias(&router, function, "live").await;
        if !blocks {
            assert_eq!(
                answer.status,
                204,
                "case {label:?}: a configuration on another qualifier must not block the \
                 delete, got {} {}",
                answer.status,
                answer.text()
            );
            continue;
        }
        assert_eq!(
            answer.status,
            409,
            "case {label:?}: expected the modelled conflict status, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains("ResourceConflictException"),
            "case {label:?}: expected ResourceConflictException, got {}",
            answer.error_code()
        );
        assert!(
            answer.message().contains(&format!(
                "arn:aws:lambda:{REGION}:{ACCOUNT}:function:{function}:live"
            )),
            "case {label:?}: the refusal names the alias that is in use, got {:?}",
            answer.message()
        );
        assert_eq!(
            stored_aliases(&engine, function),
            vec!["live".to_string()],
            "case {label:?}: a refused delete removes nothing, got {:?}",
            stored_aliases(&engine, function)
        );

        // Once the configuration is gone the alias is free to be deleted.
        let removed = delete_provisioned_concurrency(&router, function, "live").await;
        assert_eq!(
            removed.status,
            204,
            "case {label:?}: the configuration must be deletable, got {} {}",
            removed.status,
            removed.text()
        );
        let retried = delete_alias(&router, function, "live").await;
        assert_eq!(
            retried.status,
            204,
            "case {label:?}: the alias must be deletable once nothing holds it, got {} {}",
            retried.status,
            retried.text()
        );
        assert!(
            stored_aliases(&engine, function).is_empty(),
            "case {label:?}: the alias must be gone, got {:?}",
            stored_aliases(&engine, function)
        );
    }
}

#[tokio::test]
async fn a_delete_that_names_no_function_or_no_alias_namespace_is_refused() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    create_function(&router, "da-refusals", 1).await;
    let created = create_alias(
        &router,
        "da-refusals",
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

    for (label, reference, alias, status, code) in [
        (
            "a function that does not exist",
            "da-absent".to_string(),
            "live",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a function arn that names no function",
            format!("arn:aws:lambda:{REGION}:{ACCOUNT}:function:da-absent"),
            "live",
            404,
            "ResourceNotFoundException",
        ),
        (
            "a function name qualified with an alias",
            "da-refusals:live".to_string(),
            "live",
            400,
            "InvalidParameterValueException",
        ),
        (
            "a function name qualified with a published version",
            "da-refusals:1".to_string(),
            "live",
            400,
            "InvalidParameterValueException",
        ),
    ] {
        let answer = delete_alias(&router, &reference, alias).await;
        assert_eq!(
            answer.status,
            status,
            "case {label:?}: expected {status} from DeleteAlias, got {} {}",
            answer.status,
            answer.text()
        );
        assert!(
            answer.error_code().contains(code),
            "case {label:?}: expected {code}, got {}",
            answer.error_code()
        );
    }

    assert_eq!(
        stored_aliases(&engine, "da-refusals"),
        vec!["live".to_string()],
        "a refused delete must leave the alias where it was, got {:?}",
        stored_aliases(&engine, "da-refusals")
    );
    // The unpublished version is not an alias namespace: nothing above deleted the
    // function's own `$LATEST` state.
    let still_there = call(
        &router,
        "GET",
        &format!("{FUNCTIONS_URI}/da-refusals?Qualifier={LATEST}"),
        &Value::Null,
    )
    .await;
    assert_eq!(
        still_there.status,
        200,
        "the function itself must be untouched, got {} {}",
        still_there.status,
        still_there.text()
    );
}
