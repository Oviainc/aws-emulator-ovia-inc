// SPDX-License-Identifier: CC0-1.0

//! What `accountIds` can name, over the wire, for EVERY reader of this service that takes
//! it.
//!
//! `accountIds` is the MANAGEMENT account's mechanism for naming a member account of its
//! organization — "If your account is the management account of an organization, use this
//! parameter to specify the member account for which you want to return <resource>
//! recommendations. Only one account ID can be specified per request"
//! (`GetAutoScalingGroupRecommendations$accountIds` and the identically worded member of
//! every other reader) — and an account id is "a 12-digit Amazon Web Services account ID"
//! (`Scope$value`).
//!
//! This emulator presents exactly ONE standalone account (`engine::emulator_account_id`)
//! and records no organization, so the only account any reader can report on is the
//! caller's own: a well-formed id naming another account is a request the service cannot
//! answer and is refused with `InvalidParameterValueException` (HTTP 400), never used as a
//! filter key and answered with an empty list a client would read as "that account has no
//! recommendations".
//!
//! The rows below are the CLASS of `accountIds` values (absent, empty, the caller's own,
//! another account, a malformed id, more than one id) crossed with every reader that takes
//! the member, so no reader can accept what another refuses.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_computeoptimizer_computeoptimizerservice::state::ANALYSIS_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call, emulator_with_analysed_services, error_code, opt_in};

/// Every operation of this service that takes `accountIds`, the member of its response
/// that carries what it reports, and whether the store this emulator starts with holds an
/// analysis for that resource type (an Amazon ECS service recommendation is computed over
/// the ECS services the ECS emulator holds, and an Amazon Aurora or RDS database
/// recommendation over the DB instances the Amazon RDS emulator holds — this suite creates
/// neither).
const READERS: [(&str, &str, bool); 9] = [
    (
        "GetAutoScalingGroupRecommendations",
        "autoScalingGroupRecommendations",
        true,
    ),
    ("GetEBSVolumeRecommendations", "volumeRecommendations", true),
    (
        "GetEC2InstanceRecommendations",
        "instanceRecommendations",
        true,
    ),
    (
        "GetLambdaFunctionRecommendations",
        "lambdaFunctionRecommendations",
        true,
    ),
    (
        "GetRDSDatabaseRecommendations",
        "rdsDBRecommendations",
        false,
    ),
    ("GetRecommendationSummaries", "recommendationSummaries", true),
    ("GetLicenseRecommendations", "licenseRecommendations", true),
    (
        "GetECSServiceRecommendations",
        "ecsServiceRecommendations",
        false,
    ),
    // The idle resource analysis is part of the store's initial value, the way the volume,
    // group, function and license observations are: Compute Optimizer's own analysis of an
    // observed workload is what finds a resource idle, and no operation of this API creates
    // one.
    ("GetIdleRecommendations", "idleRecommendations", true),
];

/// The refusal every reader answers a request about another account with: the constant this
/// service's findings readers already recorded for this condition, which names neither the
/// member nor the account — it states the permission that is missing and how an
/// organization account gets it.
const CROSS_ACCOUNT_MESSAGE: &str = "Invalid permission to access another account. If this \
     is an organization account, request the management account to enable service access.";

/// Opt the account in and let the analysis its enrollment starts come due — the gate every
/// recommendation reader of this service applies. The span is the service's own
/// ([`ANALYSIS_DELAY`]); the extra margin covers scheduler jitter.
async fn analysed(router: &MultiServiceRouter) {
    opt_in(router).await;
    tokio::time::sleep(ANALYSIS_DELAY + Duration::from_millis(100)).await;
}

/// What `operation` reports in `member`, as a list.
fn reported(body: &Value, member: &str) -> Vec<Value> {
    body[member].as_array().cloned().unwrap_or_default()
}

/// Assert `request` is refused with the operation's modelled 400, and return the message it
/// carried.
async fn refused(
    router: &MultiServiceRouter,
    operation: &str,
    case: &str,
    request: Value,
) -> String {
    let (status, body) = call(router, operation, request).await;
    assert_eq!(
        status, 400,
        "case {case:?}/{operation}: expected 400, got {status}: {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValueException",
        "case {case:?}/{operation}: expected InvalidParameterValueException, got {body}"
    );
    body["message"].as_str().unwrap_or_default().to_string()
}

/// Naming the caller's own account — or naming no account at all — reads the caller's own
/// analysis; naming ANY other account is refused rather than answered with an empty list.
#[tokio::test]
async fn account_ids_reads_only_the_caller_s_own_account() {
    let router = emulator_with_analysed_services();
    analysed(&router).await;
    let own = ::engine::emulator_account_id().to_string();

    // Well-formed 12-digit ids that are not the caller's: the id the report cites, the
    // documentation's own example account, an id one digit from the caller's own, and the
    // extremes of the 12-digit range.
    let other_accounts = [
        "210987654321",
        "111122223333",
        "123456789012",
        "000000000001",
        "999999999999",
    ];

    for (operation, member, has_analysis) in READERS {
        // What the reader reports when the request names no account: the caller's own
        // analysis, which every row below is compared against.
        let (status, body) = call(&router, operation, json!({})).await;
        assert_eq!(
            status, 200,
            "case \"absent\"/{operation}: expected 200, got {status}: {body}"
        );
        let own_analysis = reported(&body, member);
        assert_eq!(
            !own_analysis.is_empty(),
            has_analysis,
            "case \"absent\"/{operation}: expected rows={has_analysis} for the caller's own \
             account, got {body}"
        );

        // The caller's own account, named explicitly or by an empty list, reads exactly
        // that.
        for (case, request) in [
            ("own account", json!({"accountIds": [own.clone()]})),
            ("empty list", json!({"accountIds": []})),
        ] {
            let (status, body) = call(&router, operation, request).await;
            assert_eq!(
                status, 200,
                "case {case:?}/{operation}: expected 200, got {status}: {body}"
            );
            assert_eq!(
                reported(&body, member),
                own_analysis,
                "case {case:?}/{operation}: expected the caller's own analysis, got {body}"
            );
        }

        // Any other account is a member account of an organization this service records
        // none of: refused, with the same message whichever account is named.
        for account_id in other_accounts {
            let message = refused(
                &router,
                operation,
                account_id,
                json!({"accountIds": [account_id]}),
            )
            .await;
            assert_eq!(
                message, CROSS_ACCOUNT_MESSAGE,
                "case {account_id:?}/{operation}: expected the cross-account refusal, got \
                 {message:?}"
            );
        }
    }
}

/// An `accountIds` value that cannot name one account — a value that is not a 12-digit
/// account id, or more than one id — is refused by every reader, and the refusal says which
/// member and which constraint the value failed.
#[tokio::test]
async fn account_ids_refuses_a_malformed_id_and_more_than_one_id() {
    let router = emulator_with_analysed_services();
    analysed(&router).await;
    let own = ::engine::emulator_account_id().to_string();

    // Rows: empty, too short, too long, 12 characters that are not all digits, and a
    // 12-digit id with surrounding space — each one past the "12-digit" constraint in a
    // different direction.
    let malformed = ["", "12345", "0000000000000", "00000000000a", " 00000000000"];
    // Rows: two different ids, and the SAME id twice — both ask for more than one account.
    let too_many = [
        vec![own.clone(), "210987654321".to_string()],
        vec![own.clone(), own.clone()],
        vec!["210987654321".to_string(), "999999999999".to_string()],
    ];

    for (operation, _, _) in READERS {
        for account_id in malformed {
            let message = refused(
                &router,
                operation,
                account_id,
                json!({"accountIds": [account_id]}),
            )
            .await;
            assert!(
                message.contains("accountIds") && message.contains("12-digit"),
                "case {account_id:?}/{operation}: the refusal must name the member and its \
                 constraint, got {message:?}"
            );
        }

        for account_ids in &too_many {
            let case = format!("{account_ids:?}");
            let message = refused(
                &router,
                operation,
                &case,
                json!({"accountIds": account_ids}),
            )
            .await;
            assert!(
                message.contains("accountIds") && message.contains("one account ID"),
                "case {case:?}/{operation}: the refusal must name the member and the one-account \
                 rule, got {message:?}"
            );
        }
    }
}

/// A request the service cannot answer is refused BEFORE the enrollment gate: an account
/// that never opted in reads nothing (403), but a request naming another account is
/// answered with the 400 whether or not the caller is enrolled — the refusal is about the
/// request, not about what the store holds.
#[tokio::test]
async fn the_cross_account_refusal_precedes_the_opt_in_gate() {
    let router = emulator_with_analysed_services();

    for (operation, _, _) in READERS {
        let (status, body) = call(&router, operation, json!({})).await;
        assert_eq!(
            status, 403,
            "case \"not opted in\"/{operation}: expected 403, got {status}: {body}"
        );
        assert!(
            error_code(&body).contains("OptInRequiredException"),
            "case \"not opted in\"/{operation}: expected OptInRequiredException, got {body}"
        );

        let message = refused(
            &router,
            operation,
            "not opted in, another account",
            json!({"accountIds": ["210987654321"]}),
        )
        .await;
        assert_eq!(
            message, CROSS_ACCOUNT_MESSAGE,
            "case \"not opted in, another account\"/{operation}: expected the cross-account \
             refusal, got {message:?}"
        );
    }
}
