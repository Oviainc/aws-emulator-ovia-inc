// SPDX-License-Identifier: CC0-1.0

//! `DisassociateSourceViews`, end to end over the awsJson1_0 wire.
//!
//! One rule: the call removes from the billing view `arn` names exactly the
//! source views `sourceViews` names and nothing else — the association is a SET,
//! so what the view keeps is the set difference, and a source it does not draw
//! from removes nothing (the model marks the operation `@idempotent`). Only a
//! `CUSTOM` view has an association a caller owns, so a managed view — the
//! account's `PRIMARY` view — is refused with the modeled `ValidationException`,
//! an ARN no view is stored under with `ResourceNotFoundException`, and one
//! outside the caller's account with `AccessDeniedException`.
//!
//! Every removal is read back through `ListSourceViewsForBillingView` and
//! `GetBillingView`'s derived `sourceViewCount`, so the assertion is about the
//! persisted view and not about the answer this one call composed.
//!
//! The rows span what the rule generalises over: how many source views the view
//! holds (1 through 10, the `BillingViewSourceViewsList` bounds), how many of
//! them the request names, where in the stored order they sit, and whether the
//! request names any the view does not hold.
//!
//! The harness (`call`, `error_code`, …) is the one `billing_views.rs` owns.

use serde_json::{json, Value};
use service_com_amazonaws_billing_awsbilling::ComAmazonawsBillingAwsbillingState;
use smithy_server_runtime::MultiServiceRouter;

use crate::billing_views::{
    call, emulator_with_engine, error_code, ABSENT_ARN, ACCOUNT_ID, PRIMARY_ARN,
};
use crate::rejection_fidelity::message;

/// A pattern-valid `BillingViewArn` for a source view with resource id `id`.
pub(crate) fn source_arn(id: &str) -> String {
    format!("arn:aws:billing::{ACCOUNT_ID}:billingview/{id}")
}

/// One row of the disassociation table.
struct Row {
    case: &'static str,
    /// The source views the view is created holding, in this order.
    held: Vec<String>,
    /// The source views the request asks to remove.
    remove: Vec<String>,
    /// What the view must hold afterwards, in stored order.
    remaining: Vec<String>,
}

fn rows() -> Vec<Row> {
    let ten: Vec<String> = (0..10).map(|i| source_arn(&format!("src-{i}"))).collect();
    vec![
        Row {
            case: "the only source view",
            held: vec![PRIMARY_ARN.to_string()],
            remove: vec![PRIMARY_ARN.to_string()],
            remaining: vec![],
        },
        Row {
            case: "the first of three",
            held: vec![source_arn("c"), source_arn("a"), source_arn("b")],
            remove: vec![source_arn("c")],
            remaining: vec![source_arn("a"), source_arn("b")],
        },
        Row {
            case: "the last of three",
            held: vec![source_arn("c"), source_arn("a"), source_arn("b")],
            remove: vec![source_arn("b")],
            remaining: vec![source_arn("c"), source_arn("a")],
        },
        Row {
            case: "the middle of three",
            held: vec![source_arn("c"), source_arn("a"), source_arn("b")],
            remove: vec![source_arn("a")],
            remaining: vec![source_arn("c"), source_arn("b")],
        },
        Row {
            case: "two of three, named in the other order",
            held: vec![source_arn("c"), source_arn("a"), source_arn("b")],
            remove: vec![source_arn("b"), source_arn("c")],
            remaining: vec![source_arn("a")],
        },
        Row {
            case: "a source the view does not hold removes nothing",
            held: vec![source_arn("a"), source_arn("b")],
            remove: vec![source_arn("never-associated")],
            remaining: vec![source_arn("a"), source_arn("b")],
        },
        Row {
            case: "one held source beside one it does not hold",
            held: vec![source_arn("a"), source_arn("b")],
            remove: vec![source_arn("b"), source_arn("never-associated")],
            remaining: vec![source_arn("a")],
        },
        Row {
            case: "all ten, the modeled maximum of one request",
            held: ten.clone(),
            remove: ten.clone(),
            remaining: vec![],
        },
        Row {
            case: "three of ten",
            held: ten.clone(),
            remove: vec![ten[0].clone(), ten[4].clone(), ten[9].clone()],
            remaining: ten
                .iter()
                .enumerate()
                .filter(|(index, _)| ![0, 4, 9].contains(index))
                .map(|(_, arn)| arn.clone())
                .collect(),
        },
    ]
}

/// The `sourceViews` an answer carries; an omitted list and `[]` mean the same.
fn source_views(body: &Value) -> Vec<String> {
    body.get("sourceViews")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .map(|entry| entry.as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Put the stored view's `healthStatus.statusCode` at `status`, the one way a
/// view's health moves in this emulator: no modelled operation sets it.
pub(crate) fn set_health_status(
    engine: &std::sync::Arc<engine::InMemoryEngine>,
    arn: &str,
    status: &str,
) {
    let state = engine.state::<ComAmazonawsBillingAwsbillingState>();
    let mut state = state.lock();
    let view = state
        .billing_views
        .get_mut(arn)
        .expect("the harness created this view");
    view.health_status_code = status.to_string();
}

/// Create a `CUSTOM` billing view holding `held` and return its minted ARN.
pub(crate) async fn create_view(
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

/// The source views the STORE holds for `arn`, read back through the list API,
/// and the `sourceViewCount` `GetBillingView` derives for the same view.
async fn read_back(router: &MultiServiceRouter, case: &str, arn: &str) -> (Vec<String>, u64) {
    let (status, listed) =
        call(router, "ListSourceViewsForBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the list to answer, got {status} {listed:?}"
    );
    let (status, got) = call(router, "GetBillingView", &json!({ "arn": arn })).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the get to answer, got {status} {got:?}"
    );
    (
        source_views(&listed),
        got["billingView"]["sourceViewCount"].as_u64().unwrap_or(0),
    )
}

#[tokio::test]
async fn a_disassociation_leaves_the_set_difference_and_nothing_else() {
    let (router, _engine) = emulator_with_engine();

    for (index, row) in rows().iter().enumerate() {
        let case = row.case;
        let arn = create_view(&router, case, &format!("dsv-{index}"), &row.held).await;

        let (status, answer) = call(
            &router,
            "DisassociateSourceViews",
            &json!({ "arn": arn, "sourceViews": row.remove }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {answer:?}"
        );
        assert_eq!(
            answer["arn"].as_str(),
            Some(arn.as_str()),
            "case {case:?}: the answer names the view it changed, got {answer:?}"
        );

        let (listed, count) = read_back(&router, case, &arn).await;
        assert_eq!(
            listed, row.remaining,
            "case {case:?}: expected the remaining source views, got {listed:?}"
        );
        assert_eq!(
            count,
            row.remaining.len() as u64,
            "case {case:?}: sourceViewCount is derived from the stored set, got {count}"
        );

        // `@idempotent`: the same request again answers the same way and leaves
        // the same set behind.
        let (status, again) = call(
            &router,
            "DisassociateSourceViews",
            &json!({ "arn": arn, "sourceViews": row.remove }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: a replayed disassociation answers 200, got {status} {again:?}"
        );
        let (listed_again, count_again) = read_back(&router, case, &arn).await;
        assert_eq!(
            (listed_again, count_again),
            (row.remaining.clone(), row.remaining.len() as u64),
            "case {case:?}: a replayed disassociation changes nothing"
        );
    }
}

#[tokio::test]
async fn a_source_view_can_be_removed_and_associated_again() {
    // The round trip in both directions over one view: what a disassociation
    // removed, an association puts back, and each step is read back from the
    // store rather than from the answer.
    let (router, _engine) = emulator_with_engine();
    let held = vec![source_arn("first"), source_arn("second")];
    let arn = create_view(&router, "round trip", "dsv-round-trip", &held).await;

    let (status, _) = call(
        &router,
        "DisassociateSourceViews",
        &json!({ "arn": arn, "sourceViews": [held[0]] }),
    )
    .await;
    assert_eq!(status, 200, "case \"remove\": expected 200, got {status}");
    let (listed, _) = read_back(&router, "remove", &arn).await;
    assert_eq!(
        listed,
        vec![held[1].clone()],
        "case \"remove\": expected only the second source view, got {listed:?}"
    );

    let (status, _) = call(
        &router,
        "AssociateSourceViews",
        &json!({ "arn": arn, "sourceViews": [PRIMARY_ARN] }),
    )
    .await;
    assert_eq!(status, 200, "case \"re-associate\": expected 200, got {status}");
    let (listed, count) = read_back(&router, "re-associate", &arn).await;
    assert_eq!(
        listed,
        vec![held[1].clone(), PRIMARY_ARN.to_string()],
        "case \"re-associate\": expected the appended source view, got {listed:?}"
    );
    assert_eq!(count, 2, "case \"re-associate\": expected two, got {count}");
}

#[tokio::test]
async fn a_view_that_is_not_healthy_cannot_be_disassociated() {
    // "a billing view's health status prevents an operation from being performed
    // … if the billing view is in a state other than HEALTHY": the rows are
    // every other `BillingViewStatus` value the model declares
    // (`BILLING_VIEW_STATUS_VALUES` = CREATING, HEALTHY, UNHEALTHY, UPDATING).
    let (router, engine) = emulator_with_engine();
    let held = vec![source_arn("a"), source_arn("b")];
    let arn = create_view(&router, "health", "dsv-health", &held).await;

    for status in ["CREATING", "UNHEALTHY", "UPDATING"] {
        set_health_status(&engine, &arn, status);
        let (code, body) = call(
            &router,
            "DisassociateSourceViews",
            &json!({ "arn": arn, "sourceViews": [held[0]] }),
        )
        .await;
        assert_eq!(
            (code, error_code(&body).as_str()),
            (400, "BillingViewHealthStatusException"),
            "case {status:?}: expected 400 BillingViewHealthStatusException, got {code} {body:?}"
        );
        set_health_status(&engine, &arn, "HEALTHY");
        let (listed, count) = read_back(&router, status, &arn).await;
        assert_eq!(
            (listed, count),
            (held.clone(), 2),
            "case {status:?}: a refused disassociation leaves the stored set alone"
        );
    }

    // The control: the same request on the same view once it is HEALTHY again.
    let (code, body) = call(
        &router,
        "DisassociateSourceViews",
        &json!({ "arn": arn, "sourceViews": [held[0]] }),
    )
    .await;
    assert_eq!(
        code, 200,
        "case \"HEALTHY\": expected the disassociation to go through, got {code} {body:?}"
    );
    let (listed, _) = read_back(&router, "HEALTHY", &arn).await;
    assert_eq!(
        listed,
        vec![held[1].clone()],
        "case \"HEALTHY\": expected the remaining source view, got {listed:?}"
    );
}

#[tokio::test]
async fn a_view_whose_association_is_not_the_callers_is_refused() {
    let (router, _engine) = emulator_with_engine();
    let custom = create_view(
        &router,
        "control",
        "dsv-refusals",
        &[source_arn("a"), source_arn("b")],
    )
    .await;

    // (case, the arn, expected status, expected code)
    let rows: Vec<(&str, String, u16, &str)> = vec![
        (
            // A managed view's definition is the service's: the PRIMARY view
            // every account owns has no association a caller may change.
            "the account's PRIMARY view",
            PRIMARY_ARN.to_string(),
            400,
            "ValidationException",
        ),
        (
            "an ARN no view is stored under",
            ABSENT_ARN.to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an ARN in another account",
            "arn:aws:billing::111122223333:billingview/primary".to_string(),
            403,
            "AccessDeniedException",
        ),
        (
            "an ARN in another partition and account",
            "arn:aws-us-gov:billing::999999999999:billingview/custom-1".to_string(),
            403,
            "AccessDeniedException",
        ),
    ];

    for (case, arn, expected_status, expected_code) in rows {
        let (status, body) = call(
            &router,
            "DisassociateSourceViews",
            &json!({ "arn": arn, "sourceViews": [source_arn("a")] }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {body:?}"
        );
        if expected_code == "ResourceNotFoundException" {
            assert_eq!(
                message(&body),
                format!("[{arn}] is an invalid billing view arn"),
                "case {case:?}: expected this service's ARN-validation sentence, got {:?}",
                message(&body)
            );
        }
    }

    // The PRIMARY view and the control view came through every refusal with the
    // source views they held.
    let (listed, count) = read_back(&router, "control after the refusals", &custom).await;
    assert_eq!(
        (listed, count),
        (vec![source_arn("a"), source_arn("b")], 2),
        "case \"control after the refusals\": a refused call changes nothing"
    );
    let (status, primary) = call(&router, "GetBillingView", &json!({ "arn": PRIMARY_ARN })).await;
    assert_eq!(
        status, 200,
        "case \"PRIMARY after the refusals\": got {status} {primary:?}"
    );
    // The PRIMARY view is the one the service manages, so `GetBillingView`
    // reports no source count for it at all; that it is untouched is read off
    // the source views it holds.
    assert!(
        primary["billingView"].get("sourceViewCount").is_none(),
        "case \"PRIMARY after the refusals\": a service-managed view reports no \
         sourceViewCount, got {primary:?}"
    );
    let (listed, _) = read_back(&router, "PRIMARY after the refusals", PRIMARY_ARN).await;
    assert!(
        listed.is_empty(),
        "case \"PRIMARY after the refusals\": it is untouched, got {listed:?}"
    );
}
