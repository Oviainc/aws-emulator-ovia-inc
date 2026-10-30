// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetDimensionValues`'s operation-scoped `Context` rule, and for
//! the generic enum-set validator staying in place everywhere else.
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::ops::get_dimension_values::{
    ADMISSIBLE_CONTEXTS, CONTEXT_REJECTION_MESSAGE,
};

use crate::harness::{error_code, error_message, Emulator};

/// A `GetDimensionValues` body carrying the two required members, plus `Context` when
/// one is given. `Dimension` and `TimePeriod` are valid in every case so the only thing
/// under test is `Context`.
fn request(context: Option<&str>) -> Value {
    let mut body = json!({
        "TimePeriod": {"Start": "2024-01-01", "End": "2024-02-01"},
        // A member of the `Dimension` enum (model shape `Dimension`).
        "Dimension": "SERVICE",
    });
    if let Some(context) = context {
        body["Context"] = json!(context);
    }
    body
}

/// RULE: a member whose operation documents a proper subset of the enum shape it targets
/// admits only that subset, and a value outside it is refused with the service's fixed,
/// operation-scoped sentence — which names the admissible values and neither echoes the
/// rejected value nor enumerates the enum shape.
///
/// Here that member is `GetDimensionValuesRequest$Context`: documented as "This can be
/// RESERVATIONS or COST_AND_USAGE. The default value is COST_AND_USAGE", while the
/// `Context` enum it targets also carries `SAVINGS_PLANS`.
///
/// The table spans the class the rule generalises over: both admissible values and the
/// absent (defaulted) case, the enum value this operation does NOT admit, and values
/// outside the enum altogether — a wrong case, an empty string, a near miss and a
/// surrounding-whitespace form.
#[tokio::test]
async fn context_admits_only_the_values_this_operation_documents() {
    struct Case {
        label: &'static str,
        context: Option<&'static str>,
        refused: bool,
    }

    // Admissible set source: `GetDimensionValuesRequest$Context`'s own documentation
    // (`smithy_query(query="doc", shape="GetDimensionValuesRequest", member="Context")`).
    let cases = [
        Case {
            label: "absent (the documented COST_AND_USAGE default)",
            context: None,
            refused: false,
        },
        Case {
            label: "an admissible value",
            context: Some("COST_AND_USAGE"),
            refused: false,
        },
        Case {
            label: "the other admissible value",
            context: Some("RESERVATIONS"),
            refused: false,
        },
        Case {
            label: "in the Context enum, but not admissible for this operation",
            context: Some("SAVINGS_PLANS"),
            refused: true,
        },
        Case {
            label: "outside the Context enum",
            context: Some("NOT_A_CONTEXT"),
            refused: true,
        },
        Case {
            label: "an admissible value in the wrong case (enum values are exact)",
            context: Some("cost_and_usage"),
            refused: true,
        },
        Case {
            label: "the empty string",
            context: Some(""),
            refused: true,
        },
        Case {
            label: "a near miss on an admissible value",
            context: Some("RESERVATION"),
            refused: true,
        },
        Case {
            label: "an admissible value with surrounding whitespace",
            context: Some(" COST_AND_USAGE "),
            refused: true,
        },
    ];

    let emulator = Emulator::start();
    for case in &cases {
        let (status, body) = emulator
            .call("GetDimensionValues", &request(case.context))
            .await;
        let code = error_code(&body);
        let message = error_message(&body);

        if case.refused {
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {:?} (Context={:?}): expected a 400 ValidationException, got {status}: \
                 {body}",
                case.label,
                case.context
            );
            assert_eq!(
                message, CONTEXT_REJECTION_MESSAGE,
                "case {:?} (Context={:?}): expected the fixed per-operation sentence, got \
                 {message:?}",
                case.label, case.context
            );
            // The refusal is that one fixed sentence for every inadmissible value, so it
            // carries nothing derived from the request: the equality above is what proves
            // the rejected value is not echoed. It must also not advertise the enum value
            // this operation refuses.
            assert!(
                !message.contains("SAVINGS_PLANS"),
                "case {:?}: the refusal must not advertise SAVINGS_PLANS, got {message:?}",
                case.label
            );
        } else {
            assert!(
                !(status == 400 && message == CONTEXT_REJECTION_MESSAGE),
                "case {:?} (Context={:?}): an admissible context must not be refused, got \
                 {status}: {body}",
                case.label,
                case.context
            );
            // Retrieval itself is still scaffolded — the emulator holds no cost-and-usage
            // data for a dimension to be read out of — so an admissible request is
            // answered as unimplemented, never as a successful empty page.
            assert_eq!(
                (status, code.as_str()),
                (501, "NotImplemented"),
                "case {:?} (Context={:?}): expected 501 NotImplemented, got {status}: {body}",
                case.label,
                case.context
            );
        }
    }

    // The set the handler enforces is the documented pair, and nothing wider.
    assert_eq!(
        ADMISSIBLE_CONTEXTS.len(),
        2,
        "the documented admissible set holds exactly two values, got {ADMISSIBLE_CONTEXTS:?}"
    );
}

/// RULE (the other half): a member whose operation admits its enum shape in full keeps
/// the framework's generic constraint report, which names the member, echoes the value
/// and enumerates the shape's whole value set.
///
/// The rows differ along the operation, the member and the violation kind, so the
/// rewording above is shown to be scoped to the one narrowed member rather than applied
/// to every refusal this service answers.
#[tokio::test]
async fn every_other_refusal_keeps_the_generic_constraint_report() {
    struct Case {
        label: &'static str,
        operation: &'static str,
        body: Value,
        /// The dotted member path the generic report must name.
        path: &'static str,
    }

    let cases = [
        Case {
            label: "GetDimensionValues' own Dimension, outside the Dimension enum",
            operation: "GetDimensionValues",
            body: json!({
                "TimePeriod": {"Start": "2024-01-01", "End": "2024-02-01"},
                "Dimension": "NOT_A_DIMENSION",
            }),
            path: "dimension",
        },
        Case {
            label: "GetDimensionValues with a required member absent",
            operation: "GetDimensionValues",
            body: json!({"Dimension": "SERVICE"}),
            path: "timePeriod",
        },
        Case {
            // `TermInYears` targets the `TermInYears` enum (values ONE_YEAR,
            // THREE_YEARS) and this service words no sentence of its own for it, so the
            // generic report stands. `GetCostAndUsage`'s `Granularity` used to sit here;
            // it has its own fixed sentence now (see `granularity_refusal.rs`).
            label: "another operation's enum member, outside its enum",
            operation: "GetReservationPurchaseRecommendation",
            body: json!({
                "Service": "Amazon Elastic Compute Cloud - Compute",
                "TermInYears": "NOT_A_TERM",
            }),
            path: "termInYears",
        },
    ];

    let emulator = Emulator::start();
    for case in &cases {
        let (status, body) = emulator.call(case.operation, &case.body).await;
        let message = error_message(&body);
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "ValidationException"),
            "case {:?}: expected a 400 ValidationException, got {status}: {body}",
            case.label
        );
        assert!(
            message.contains("failed to satisfy constraint")
                && message.contains(&format!("'{}'", case.path)),
            "case {:?}: expected the generic constraint report naming {:?}, got {message:?}",
            case.label,
            case.path
        );
        assert_ne!(
            message, CONTEXT_REJECTION_MESSAGE,
            "case {:?}: the Context sentence must not be reused for another member",
            case.label
        );
    }
}
