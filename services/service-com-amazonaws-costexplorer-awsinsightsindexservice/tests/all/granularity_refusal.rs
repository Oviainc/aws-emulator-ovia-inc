// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how Cost Explorer words a `Granularity` outside the modelled
//! enum, across every operation that takes the member.
//!
//! Every test drives the service over its real wire protocol through the shared
//! [`crate::harness`].

use serde_json::{json, Value};
use service_com_amazonaws_costexplorer_awsinsightsindexservice::service::GRANULARITY_REJECTION_MESSAGE;

use crate::harness::{error_code, error_message, Emulator};

/// Every operation whose input carries a `Granularity` member, from the model
/// (`smithy_query(query="digest")` — the `Granularity` member of
/// `GetCostAndUsageRequest`, `GetCostAndUsageWithResourcesRequest`,
/// `GetApproximateUsageRecordsRequest`, `GetCostForecastRequest`,
/// `GetUsageForecastRequest`, `GetReservationCoverageRequest`,
/// `GetReservationUtilizationRequest`, `GetSavingsPlansCoverageRequest`,
/// `GetSavingsPlansUtilizationRequest`). It is required on the first five and optional on
/// the rest.
const OPERATIONS_WITH_GRANULARITY: &[&str] = &[
    "GetCostAndUsage",
    "GetCostAndUsageWithResources",
    "GetApproximateUsageRecords",
    "GetCostForecast",
    "GetUsageForecast",
    "GetReservationCoverage",
    "GetReservationUtilization",
    "GetSavingsPlansCoverage",
    "GetSavingsPlansUtilization",
];

/// The modelled `Granularity` value set (`smithy_query(query="shape",
/// shape="Granularity")`).
const GRANULARITY_ENUM: &[&str] = &["DAILY", "MONTHLY", "HOURLY"];

/// A valid `TimePeriod` — the one member every operation below either requires or
/// accepts, kept valid so `Granularity` is the only thing under test.
fn time_period() -> Value {
    json!({"Start": "2024-01-01", "End": "2024-02-01"})
}

/// A request body for `operation` whose every member other than `Granularity` is valid,
/// with `Granularity` set when one is given.
fn request(operation: &str, granularity: Option<&str>) -> Value {
    let mut body = match operation {
        // `TimePeriod` and `Metrics` are the other required members.
        "GetCostAndUsage" => json!({
            "TimePeriod": time_period(),
            "Metrics": ["UnblendedCost"],
        }),
        // `Filter` is required here, and is an `Expression`.
        "GetCostAndUsageWithResources" => json!({
            "TimePeriod": time_period(),
            "Filter": {"Dimensions": {"Key": "SERVICE", "Values": ["Amazon Simple Storage Service"]}},
        }),
        // `ApproximationDimension` is required and targets its own enum
        // (SERVICE, RESOURCE); no `TimePeriod` member exists on this input.
        "GetApproximateUsageRecords" => json!({"ApproximationDimension": "SERVICE"}),
        // `Metric` is required and targets the `Metric` enum.
        "GetCostForecast" | "GetUsageForecast" => json!({
            "TimePeriod": time_period(),
            "Metric": "UNBLENDED_COST",
        }),
        // `TimePeriod` is the only other required member.
        "GetReservationCoverage"
        | "GetReservationUtilization"
        | "GetSavingsPlansCoverage"
        | "GetSavingsPlansUtilization" => json!({"TimePeriod": time_period()}),
        other => panic!("no request body is defined for {other}"),
    };
    if let Some(granularity) = granularity {
        body["Granularity"] = json!(granularity);
    }
    body
}

/// RULE: a value outside the modelled `Granularity` enum is refused 400
/// `ValidationException` with the service's own fixed sentence, which names the
/// granularities the service accepts and carries nothing from the request — not the
/// rejected value, not the member path and not the modelled value set — so it is the same
/// sentence for every unsupported granularity and for every operation taking the member.
///
/// The table spans the class twice over: along the operation (every input that carries a
/// `Granularity`, required or optional) and along the rejected value (a plausible period
/// name, a wrong case, a near miss, the empty string, a padded admissible value, a
/// lower-cased one and a non-word).
#[tokio::test]
async fn an_unsupported_granularity_takes_the_services_own_fixed_sentence() {
    let rejected = [
        ("a plausible period the service does not support", "WEEKLY"),
        ("another unsupported period", "ANNUAL"),
        ("an admissible value in the wrong case", "daily"),
        ("a near miss on an admissible value", "MONTHLYS"),
        ("the empty string", ""),
        ("an admissible value with surrounding whitespace", " HOURLY "),
        ("a non-word", "!"),
    ];

    let emulator = Emulator::start();
    for operation in OPERATIONS_WITH_GRANULARITY {
        for (label, granularity) in &rejected {
            let (status, body) = emulator
                .call(operation, &request(operation, Some(granularity)))
                .await;
            let message = error_message(&body);
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "ValidationException"),
                "case {label:?} ({operation} Granularity={granularity:?}): expected a 400 \
                 ValidationException, got {status}: {body}"
            );
            assert_eq!(
                message, GRANULARITY_REJECTION_MESSAGE,
                "case {label:?} ({operation} Granularity={granularity:?}): expected the \
                 service's fixed sentence, got {message:?}"
            );
            // The equality above already proves the sentence is fixed; these state what
            // it must therefore not carry — the generic constraint vocabulary, the
            // member path, the modelled value set or the rejected value.
            assert!(
                !message.contains("failed to satisfy constraint")
                    && !message.contains("enum value set")
                    && !message.contains("granularity"),
                "case {label:?} ({operation}): the refusal must not read as a generic \
                 constraint report, got {message:?}"
            );
            assert!(
                granularity.is_empty() || !message.contains(granularity),
                "case {label:?} ({operation}): the refusal must not echo the rejected \
                 value, got {message:?}"
            );
        }
    }
}

/// RULE (the other half): every value the modelled enum carries is admitted at the
/// boundary, for every operation taking the member — so is an absent `Granularity` on the
/// operations whose model leaves it optional. Whatever those requests are answered with,
/// it is never an out-of-set granularity refusal nor the generic enum report.
#[tokio::test]
async fn every_modelled_granularity_passes_the_boundary() {
    let emulator = Emulator::start();
    for operation in OPERATIONS_WITH_GRANULARITY {
        for granularity in GRANULARITY_ENUM {
            let (status, body) = emulator
                .call(operation, &request(operation, Some(granularity)))
                .await;
            let message = error_message(&body);
            assert_ne!(
                message, GRANULARITY_REJECTION_MESSAGE,
                "case {operation:?} (Granularity={granularity:?}): a modelled granularity \
                 must not be refused as unsupported, got {status}: {body}"
            );
            assert!(
                !message.contains("enum value set"),
                "case {operation:?} (Granularity={granularity:?}): a modelled granularity \
                 must not fail the enum-set validator, got {status}: {body}"
            );
        }

        // `GetCostAndUsage`, `GetCostAndUsageWithResources`,
        // `GetApproximateUsageRecords`, `GetCostForecast` and `GetUsageForecast` require
        // the member, so an absent one is the framework's missing-member refusal there —
        // a different rule, covered by the generic validator. Everywhere else absence is
        // admissible.
        let required = matches!(
            *operation,
            "GetCostAndUsage"
                | "GetCostAndUsageWithResources"
                | "GetApproximateUsageRecords"
                | "GetCostForecast"
                | "GetUsageForecast"
        );
        if !required {
            let (status, body) = emulator.call(operation, &request(operation, None)).await;
            let message = error_message(&body);
            assert_ne!(
                message, GRANULARITY_REJECTION_MESSAGE,
                "case {operation:?} (Granularity absent): an optional granularity must not \
                 be refused, got {status}: {body}"
            );
        }
    }
}

/// The sentence itself: fixed text naming each granularity the service accepts, and
/// nothing a request could influence.
#[test]
fn the_refusal_sentence_names_every_modelled_granularity() {
    for granularity in GRANULARITY_ENUM {
        assert!(
            GRANULARITY_REJECTION_MESSAGE.contains(granularity),
            "case {granularity:?}: the refusal sentence must name it, got \
             {GRANULARITY_REJECTION_MESSAGE:?}"
        );
    }
}
