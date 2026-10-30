// SPDX-License-Identifier: CC0-1.0

//! The MESSAGE TEXT of two billing refusals, end to end over the awsJson1_0
//! wire — the status and the error code of both are asserted elsewhere; what is
//! asserted here is the sentence each one serializes.
//!
//! Two rules, each asserted over the whole class of inputs it holds for, not
//! over one request:
//!
//! 1. The per-feature billing-access `AccessDeniedException` reports, in its
//!    bracketed `[Operation = …]` segment, the token of the BACKEND operation
//!    that made the decision — the public preference operation's name with the
//!    preference backend's suffix appended — never the public API name alone.
//!    The rows span both preference operations (the read and the write) and every
//!    `BillingFeature` the model declares, so the token tracks the operation and
//!    the feature segment tracks the request.
//!
//! 2. `DisassociateSourceViews` refusing a view that is not `CUSTOM` answers its
//!    OWN single sentence, which is fixed text for the error class: it
//!    interpolates no ARN, name, account or source view. The rows span every
//!    non-`CUSTOM` `BillingViewType` the model declares and requests that differ
//!    in how many source views they name and whether the view holds them, and
//!    the view-lifecycle writers' differently-worded refusal on the same views is
//!    the control.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::ops::types::BILLING_VIEW_TYPE_VALUES;
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;
use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;

// The per-feature harness — the model-derived feature catalogue, the two request
// bodies and the access switch — is the one `billing_feature_access.rs` owns, so
// the feature list exists once for both suites.
use crate::billing_feature_access::{get_body, set_access, update_body, ALL_FEATURES};
use crate::billing_views::{
    call, emulator_with_engine, error_code, standalone_emulator_with_engine, ACCOUNT_ID,
    PRIMARY_ARN,
};
use crate::rejection_fidelity::message;

/// The two public operations that gate a request on per-feature billing access.
const PREFERENCE_OPERATIONS: &[&str] = &["GetBillingPreferences", "UpdateBillingPreferences"];

/// The suffix the preference surface's backend operation token carries on top of
/// the public operation name — spelled out here so the expectation is written
/// independently of the service code that builds the message.
const BACKEND_SUFFIX: &str = "Paperwork";

/// The request body `operation` sends for `feature`.
fn preference_body(operation: &str, feature: &str) -> Value {
    if operation == "GetBillingPreferences" {
        get_body(feature)
    } else {
        update_body(feature)
    }
}

/// The whole message the per-feature refusal must serialize: fixed brackets, the
/// caller's account, the backend token of the operation that refused, and the
/// feature the request named.
fn denied_message(operation: &str, feature: &str) -> String {
    format!(
        "[Exception=AccessDeniedException][AccountId = {ACCOUNT_ID}]\
         [Operation = {operation}{BACKEND_SUFFIX}][Message=No access to feature {feature}]"
    )
}

/// Put the stored view's `billingViewType` at `view_type`: no modelled operation
/// mints a view of any type but `CUSTOM`, so this is how a managed view other
/// than the account's `PRIMARY` one comes to exist in a test.
fn set_view_type(engine: &Arc<engine::InMemoryEngine>, arn: &str, view_type: &str) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    state
        .billing_views
        .get_mut(arn)
        .expect("the harness created this view")
        .billing_view_type = view_type.to_string();
}

/// The source views the STORE holds for `arn`.
fn stored_source_views(engine: &Arc<engine::InMemoryEngine>, arn: &str) -> Vec<String> {
    engine
        .state::<ComAmazonawsBillingAwsbillingState>()
        .lock()
        .billing_views
        .get(arn)
        .map(|view| view.source_views.clone())
        .unwrap_or_default()
}

/// Create a `CUSTOM` billing view holding `held` and return its minted ARN.
async fn create_view(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    held: &[String],
) -> String {
    let (status, created) = call(
        router,
        "CreateBillingView",
        &json!({ "name": name, "sourceViews": held }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the harness create must answer 200, got {status} {created:?}"
    );
    created["arn"].as_str().unwrap_or_default().to_string()
}

/// A pattern-valid `BillingViewArn` for a source view with resource id `id`.
fn source_arn(id: &str) -> String {
    format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}")
}

#[tokio::test]
async fn the_per_feature_refusal_names_the_backend_operation_token() {
    // Both preference operations, every feature: the message is built from the
    // account, the backend token of the operation that refused, and the feature
    // the request named — and from nothing else.
    for operation in PREFERENCE_OPERATIONS {
        for feature in ALL_FEATURES {
            let (router, engine) = standalone_emulator_with_engine();
            set_access(&engine, feature, false);
            let case = format!("{operation} on {feature} without access");

            let (status, body) = call(&router, operation, &preference_body(operation, feature)).await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (403, "AccessDeniedException"),
                "case {case:?}: expected 403 AccessDeniedException, got {status} {body:?}"
            );
            let stated = message(&body);
            assert_eq!(
                stated,
                denied_message(operation, feature),
                "case {case:?}: expected the bracketed refusal naming the backend operation \
                 token, got {stated:?}"
            );
            // The public API name alone is never the token: it is the backend
            // operation that decided, so the bare name in that segment is the
            // divergence this rule forbids.
            assert!(
                !stated.contains(&format!("[Operation = {operation}]")),
                "case {case:?}: the operation segment must carry the backend token, \
                 got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn an_accessible_feature_is_not_refused_at_all() {
    // The control for the rule above: the segment only ever appears on a refusal,
    // so a feature the account can reach answers instead of carrying any token.
    let (router, engine) = standalone_emulator_with_engine();
    for feature in ALL_FEATURES {
        set_access(&engine, feature, true);
        let (status, body) = call(
            &router,
            "GetBillingPreferences",
            &preference_body("GetBillingPreferences", feature),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {feature:?}: an accessible feature answers, got {status} {body:?}"
        );
    }
}

/// AWS's sentence for disassociating source views from a view that is not
/// `CUSTOM`: fixed text for the error class.
const NOT_CUSTOM_SENTENCE: &str = "Only custom billing views can have source views disassociated";

#[tokio::test]
async fn disassociating_from_a_non_custom_view_answers_its_own_sentence() {
    let (router, engine) = emulator_with_engine();

    // Every `BillingViewType` the model declares except `CUSTOM` — source:
    // `ops::types::BILLING_VIEW_TYPE_VALUES` — so the refusal is about the KIND
    // of view and not about the one managed type an account starts with.
    let managed_types: Vec<&&str> = BILLING_VIEW_TYPE_VALUES
        .iter()
        .filter(|view_type| **view_type != "CUSTOM")
        .collect();
    assert!(
        managed_types.len() > 1,
        "case \"model types\": expected several managed types, got {managed_types:?}"
    );

    let held: Vec<String> = (0..10).map(|i| source_arn(&format!("rmt-{i}"))).collect();

    for (index, view_type) in managed_types.iter().enumerate() {
        let arn = create_view(&router, view_type, &format!("rmt-view-{index}"), &held).await;
        set_view_type(&engine, &arn, view_type);

        // The requests differ in how many source views they name (one, several,
        // the modeled maximum of ten), where in the stored order they sit, and
        // whether the view holds them at all: none of that reaches the message.
        let requests: Vec<(&str, Vec<String>)> = vec![
            ("the first stored source view", vec![held[0].clone()]),
            ("the last stored source view", vec![held[9].clone()]),
            (
                "three of the stored source views",
                vec![held[1].clone(), held[5].clone(), held[8].clone()],
            ),
            ("all ten stored source views", held.clone()),
            (
                "a source view the view does not hold",
                vec![source_arn("never-associated")],
            ),
        ];

        for (what, source_views) in requests {
            let case = format!("{view_type} view, {what}");
            let (status, body) = call(
                &router,
                "DisassociateSourceViews",
                &json!({ "arn": arn, "sourceViews": source_views }),
            )
            .await;
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {status} {body:?}"
            );
            assert_eq!(
                message(&body),
                NOT_CUSTOM_SENTENCE,
                "case {case:?}: expected this operation's own fixed sentence, got {:?}",
                message(&body)
            );
            assert_eq!(
                stored_source_views(&engine, &arn),
                held,
                "case {case:?}: a refused disassociation leaves the stored set alone"
            );
        }
    }

    // The recorded shape of the refusal: the `PRIMARY` view every account owns,
    // which no test set up.
    let (status, body) = call(
        &router,
        "DisassociateSourceViews",
        &json!({ "arn": PRIMARY_ARN, "sourceViews": [source_arn("never-associated")] }),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "ValidationException"),
        "case \"the account's PRIMARY view\": expected 400 ValidationException, \
         got {status} {body:?}"
    );
    assert_eq!(
        message(&body),
        NOT_CUSTOM_SENTENCE,
        "case \"the account's PRIMARY view\": expected this operation's own fixed sentence, \
         got {:?}",
        message(&body)
    );
}

#[tokio::test]
async fn the_view_lifecycle_writers_keep_their_own_wording() {
    // The sentence is per OPERATION: the writers that refuse the same view for
    // the same reason word it their own way, so the disassociation sentence must
    // not be theirs and theirs must not be the disassociation's.
    let (router, _engine) = emulator_with_engine();

    let (status, body) = call(
        &router,
        "DisassociateSourceViews",
        &json!({ "arn": PRIMARY_ARN, "sourceViews": [source_arn("a")] }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"disassociate PRIMARY\": got {status} {body:?}"
    );
    let disassociate = message(&body);

    for (case, operation, request) in [
        (
            "update PRIMARY",
            "UpdateBillingView",
            json!({ "arn": PRIMARY_ARN, "name": "renamed" }),
        ),
        (
            "delete PRIMARY",
            "DeleteBillingView",
            json!({ "arn": PRIMARY_ARN }),
        ),
    ] {
        let (status, body) = call(&router, operation, &request).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {body:?}"
        );
        assert_ne!(
            message(&body),
            disassociate,
            "case {case:?}: this writer words its own refusal, got {:?}",
            message(&body)
        );
    }
}
