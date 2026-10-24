// SPDX-License-Identifier: CC0-1.0

//! Two rules of the access-scope slice that the round-trip suite
//! (`application_access_scopes.rs`) does not pin, both over the `awsJson1_1` wire:
//!
//! 1. an access scope is held ON an application, and an application lives inside the
//!    instance it was created under, so the scopes of an application STOP EXISTING with
//!    that instance — nothing created afterwards inherits them;
//! 2. a put whose `Scope` or `AuthorizedTargets` breaks a constraint the MODEL declares
//!    is refused `ValidationException` and stores nothing: the scope stays as absent as
//!    one no put ever named, so the read of it is still `ResourceNotFoundException`.
//!
//! Rule 2's rows each sit ONE PAST a published bound of the member they break, taken from
//! `smithy_query(query="shape",shape="Scope,ScopeTargets,ScopeTarget")` rather than from
//! any one request.
//!
//! Every call goes through `MultiServiceRouter::dispatch`, so the modelled constraints,
//! the `awsJson1_1` envelope and the `X-Amz-Target` routing are exercised rather than
//! bypassed. The harness is the one `ssoadmin_permission_sets.rs` owns.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ssoadmin_permission_sets::{
    call, error_code, router_over, seeded_instance_arn, with_store,
};

/// The provider the applications here are created under: the `custom` entry of the
/// catalogue `ListApplicationProviders` publishes (`state::BUILT_IN_APPLICATION_PROVIDERS`).
const CUSTOM_PROVIDER_ARN: &str = "arn:aws:sso::aws:applicationProvider/custom";

/// The longest segment the model's `Scope` pattern admits:
/// `^([A-Za-z0-9_]{1,50})(:[A-Za-z0-9_]{1,50}){0,1}(:[A-Za-z0-9_]{1,50}){0,1}$` — up to
/// three segments of 1..=50 of `[A-Za-z0-9_]`.
const SCOPE_SEGMENT_MAX: usize = 50;

/// The most authorized targets one scope may carry: `ScopeTargets` is `length 1..10`.
const SCOPE_TARGETS_MAX: usize = 10;

/// A `ScopeTarget` in its INSTANCE form, which the member's pattern admits beside the
/// application form:
/// `^arn:aws(-[a-z]{1,5}){0,3}:sso::(\d{12}:application/(sso)?ins-[a-zA-Z0-9-.]{16}/apl-[a-zA-Z0-9]{16}|:instance/(sso)?ins-[a-zA-Z0-9-.]{16})$`.
/// `n` only varies the 16-character id, so distinct `n` give distinct targets.
fn instance_target(n: usize) -> String {
    format!("arn:aws:sso:::instance/ssoins-{n:016}")
}

/// Create one application under `instance_arn` and return the ARN the create minted.
async fn minted_application(router: &MultiServiceRouter, instance_arn: &str, name: &str) -> String {
    let (status, body) = call(
        router,
        "CreateApplication",
        &json!({
            "InstanceArn": instance_arn,
            "ApplicationProviderArn": CUSTOM_PROVIDER_ARN,
            "Name": name,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateApplication {name:?} must answer 200, got {status} {body}"
    );
    body["ApplicationArn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateApplication minted no ApplicationArn: {body}"))
        .to_string()
}

/// `PutApplicationAccessScope`, sending `AuthorizedTargets` only when `targets` is `Some`
/// — the member is optional, so an omitted one has to be omitted on the wire.
async fn put_scope(
    router: &MultiServiceRouter,
    application_arn: &str,
    scope: &str,
    targets: Option<&[String]>,
) -> (u16, Value) {
    let mut body = json!({"ApplicationArn": application_arn, "Scope": scope});
    if let Some(targets) = targets {
        body["AuthorizedTargets"] = json!(targets);
    }
    call(router, "PutApplicationAccessScope", &body).await
}

async fn get_scope(
    router: &MultiServiceRouter,
    application_arn: &str,
    scope: &str,
) -> (u16, Value) {
    call(
        router,
        "GetApplicationAccessScope",
        &json!({"ApplicationArn": application_arn, "Scope": scope}),
    )
    .await
}

/// How many access scopes this account's store holds in all, across every application.
fn stored_scope_count(engine: &Arc<engine::InMemoryEngine>) -> usize {
    with_store(engine, |store| {
        store
            .applications
            .values()
            .map(|application| application.access_scopes.len())
            .sum()
    })
}

/// Rule 1: the scopes of an application are gone once its instance is.
#[tokio::test]
async fn the_access_scopes_of_an_application_stop_existing_with_its_instance() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;

    // Two applications, each carrying scopes of a different shape, so the cascade is not
    // asserted on a single stored row.
    let first = minted_application(&router, &instance_arn, "doomed-app-one").await;
    let second = minted_application(&router, &instance_arn, "doomed-app-two").await;
    let rows: Vec<(&str, &String, &str, Option<Vec<String>>)> = vec![
        (
            "a scope with one authorized target",
            &first,
            "sso:account",
            Some(vec![instance_target(1)]),
        ),
        (
            "a second scope on the same application, with two targets",
            &first,
            "glue:read_write",
            Some(vec![instance_target(2), instance_target(3)]),
        ),
        (
            "a scope with no authorized targets, on another application",
            &second,
            "openid",
            None,
        ),
    ];
    for (case, application_arn, scope, targets) in &rows {
        let (status, body) = put_scope(&router, application_arn, scope, targets.as_deref()).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the put must answer 200, got {status} {body}"
        );
        let (status, body) = get_scope(&router, application_arn, scope).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the scope must read back before the delete, got {status} {body}"
        );
    }
    assert_eq!(
        stored_scope_count(&engine),
        rows.len(),
        "every row must be stored before the instance is deleted"
    );

    let (status, body) = call(
        &router,
        "DeleteInstance",
        &json!({"InstanceArn": instance_arn}),
    )
    .await;
    assert_eq!(
        status, 200,
        "DeleteInstance must answer 200, got {status} {body}"
    );
    // The deletion falls due a span after the call; a test moves the stored due instant
    // rather than sleeping for it.
    with_store(&engine, |store| {
        for instance in store.instances.values_mut() {
            if instance.deleted_due.is_some() {
                instance.deleted_due =
                    Some(std::time::SystemTime::now() - std::time::Duration::from_secs(60));
            }
        }
    });

    for (case, application_arn, scope, _) in &rows {
        let (status, body) = get_scope(&router, application_arn, scope).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the scope of a gone application must be gone, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body}"
        );
    }
    assert_eq!(
        stored_scope_count(&engine),
        0,
        "the access scopes of a deleted instance's applications must be gone too"
    );
}

/// Rule 2: a put the model refuses stores nothing.
#[tokio::test]
async fn a_put_the_model_refuses_stores_no_access_scope() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = router_over(engine.clone());
    let instance_arn = seeded_instance_arn(&router).await;
    let application_arn = minted_application(&router, &instance_arn, "constrained-app").await;

    // One scope IS stored, so each refusal is about the request and not about an empty
    // application.
    let kept_scope = "sso:account";
    let kept_targets = vec![instance_target(9)];
    let (status, body) = put_scope(&router, &application_arn, kept_scope, Some(&kept_targets)).await;
    assert_eq!(status, 200, "the seed put must answer 200, got {status} {body}");

    // Each row breaks exactly ONE modelled constraint, one past its published bound: the
    // `Scope` pattern (segment length, segment count, character class) and `ScopeTargets`
    // (its minimum of 1, its maximum of 10, and its member's pattern).
    let over_long_segment = "s".repeat(SCOPE_SEGMENT_MAX + 1);
    let too_many_targets: Vec<String> = (0..SCOPE_TARGETS_MAX + 1).map(instance_target).collect();
    let refused: Vec<(&str, String, Option<Vec<String>>, bool)> = vec![
        (
            "a scope segment one character past the 50-character bound",
            over_long_segment,
            Some(vec![instance_target(31)]),
            false,
        ),
        (
            "a scope of four segments, one past the three the pattern admits",
            "a:b:c:d".to_string(),
            Some(vec![instance_target(32)]),
            false,
        ),
        (
            "a scope carrying a character the pattern excludes",
            "sso account".to_string(),
            Some(vec![instance_target(33)]),
            false,
        ),
        (
            "an empty AuthorizedTargets list, one short of the minimum of 1",
            "empty_targets".to_string(),
            Some(Vec::new()),
            true,
        ),
        (
            "one authorized target past the maximum of 10",
            "many_targets".to_string(),
            Some(too_many_targets),
            true,
        ),
        (
            "an authorized target that is not a ScopeTarget ARN",
            "bad_target".to_string(),
            Some(vec!["arn:aws:sso:::not-a-scope-target".to_string()]),
            true,
        ),
    ];

    for (case, scope, targets, scope_is_readable) in &refused {
        let (status, body) = put_scope(&router, &application_arn, scope, targets.as_deref()).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a put the model refuses must answer 400, got {status} {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body}"
        );

        // Where the `Scope` itself is one the model admits, the read of it shows the
        // refused put stored nothing; where the scope name is what broke the pattern, the
        // read is refused on that same constraint instead.
        let (status, body) = get_scope(&router, &application_arn, scope).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the refused put must have stored nothing, got {status} {body}"
        );
        let expected = if *scope_is_readable {
            "ResourceNotFoundException"
        } else {
            "ValidationException"
        };
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }

    // Not one refusal touched the scope that WAS stored, and none of them added a scope.
    assert_eq!(
        stored_scope_count(&engine),
        1,
        "a refused put must store no access scope"
    );
    let (status, body) = get_scope(&router, &application_arn, kept_scope).await;
    assert_eq!(
        status, 200,
        "the stored scope must survive the refusals, got {status} {body}"
    );
    assert_eq!(
        body,
        json!({"Scope": kept_scope, "AuthorizedTargets": kept_targets}),
        "the stored scope must still report exactly its own targets, got {body}"
    );
}
