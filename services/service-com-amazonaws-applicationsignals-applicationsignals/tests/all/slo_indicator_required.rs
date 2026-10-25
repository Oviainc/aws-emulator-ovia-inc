// SPDX-License-Identifier: CC0-1.0

//! What a `CreateServiceLevelObjective` request must DEFINE for an SLO to exist.
//!
//! Two rules, each asserted over a SET of requests rather than on one:
//!
//! 1. An SLO is defined by EXACTLY ONE service level indicator. The operation doc states
//!    that SLOs "use service level indicators (SLIs) to calculate whether the application is
//!    performing at the level that you want" and that creating one is where "you specify
//!    whether it is a period-based SLO or a request-based SLO" — the classification
//!    `EvaluationType` reports and the only source of `MetricSourceType`. So a create that
//!    names NEITHER `SliConfig` nor `RequestBasedSliConfig`, like one that names BOTH, is
//!    refused with the modelled `ValidationException` (HTTP 400) and stores nothing, while
//!    one that names exactly one is stored with the indicator, the kind and the source that
//!    config describes.
//! 2. A request that attaches more tags to a resource than that resource's API documents a
//!    maximum of is refused the same way. The SLO's `Tags` member documents "You can
//!    associate as many as 50 tags with an SLO", while the modelled `TagList` admits
//!    `length 0..200`, so the boundary validator passes lists the service refuses.
//!
//! The request helpers (`router`, `call`, …) are the ones `slo_lifecycle` already drives this
//! service's `restJson1` surface with, so every suite speaks to one harness.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::resource_tags::{list_tags_raw, reported_tags, tag, tag_list};
use crate::slo_lifecycle::{
    arn_path_segment, call, create, name_path_segment, router, sli_config, ACCOUNT, REGION,
};

/// The documented maximum number of tags an SLO may carry (`CreateServiceLevelObjective`'s
/// `Tags` doc: "You can associate as many as 50 tags with an SLO").
const MAX_TAGS: usize = 50;

/// The largest `Tags` list the MODEL admits (`smithy_query(query="operation",
/// operation="CreateServiceLevelObjective")`: `TagList` has `length 0..200`), which is wider
/// than the documented per-resource maximum.
const MODELLED_MAX_TAGS: usize = 200;

/// The refusal a create answers when it carries more tags than [`MAX_TAGS`]: the member path
/// and the requirement, in the same envelope every other constraint violation uses.
fn tag_count_message() -> String {
    format!(
        "1 validation error detected: Value at 'tags' failed to satisfy constraint: \
         Member must have length less than or equal to {MAX_TAGS}"
    )
}

/// A request-based SLI config naming an Application Signals service entity.
fn request_based_sli_config(service_name: &str) -> Value {
    json!({
        "RequestBasedSliMetricConfig": {
            "KeyAttributes": { "Type": "Service", "Name": service_name, "Environment": "prod" },
            "MetricType": "AVAILABILITY"
        }
    })
}

/// A request-based SLI config that spells its own CloudWatch metric queries instead of naming
/// an Application Signals entity: a different FORM of the same kind of config.
fn request_based_sli_config_with_queries() -> Value {
    json!({
        "RequestBasedSliMetricConfig": {
            "TotalRequestCountMetric": [{
                "Id": "total",
                "MetricStat": {
                    "Metric": { "Namespace": "AWS/ApiGateway", "MetricName": "Count" },
                    "Period": 60,
                    "Stat": "Sum"
                }
            }]
        }
    })
}

/// A period-based SLI config that spells its own metric queries instead of naming an entity.
fn sli_config_with_queries() -> Value {
    json!({
        "SliMetricConfig": {
            "MetricDataQueries": [{
                "Id": "errors",
                "MetricStat": {
                    "Metric": { "Namespace": "AWS/ApiGateway", "MetricName": "5XXError" },
                    "Period": 60,
                    "Stat": "Sum"
                }
            }]
        },
        "MetricThreshold": 1.0,
        "ComparisonOperator": "LessThan"
    })
}

/// `count` distinct tags, keyed so no two rows of a table collide.
fn generated_tags(count: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|index| tag(&format!("key-{index:03}"), &format!("value-{index:03}")))
        .collect()
}

/// Read a stored SLO back by name: the status and the body `GetServiceLevelObjective`
/// answers.
async fn get_slo(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/slo/{}", name_path_segment(name)),
        None,
    )
    .await
}

/// Every SLO name `ListServiceLevelObjectives` reports.
async fn listed_names(router: &MultiServiceRouter) -> Vec<String> {
    let (status, page) = call(router, "POST", "/slos", Some(json!({}))).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from ListServiceLevelObjectives, got {status} with {page}"
    );
    page["SloSummaries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Assert `name` is stored NOWHERE: neither the reader nor the list knows it.
async fn assert_not_stored(router: &MultiServiceRouter, label: &str, name: &str) {
    let (status, fetched) = get_slo(router, name).await;
    assert_eq!(
        status, 404,
        "case {label:?}: expected a refused create to store nothing, \
         so a Get answers 404, got {status} with {fetched}"
    );
    let listed = listed_names(router).await;
    assert!(
        !listed.iter().any(|listed| listed == name),
        "case {label:?}: expected {name:?} to be absent from the list, got {listed:?}"
    );
}

/// How many indicator configs a create names, which is what rule 1 generalises over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Indicator {
    /// Neither config: the request defines no indicator at all.
    None,
    /// One config, of either kind.
    One,
    /// Both mutually exclusive configs in one call.
    Both,
}

/// One row of the indicator table.
struct Case {
    label: &'static str,
    /// Every member of the create request but `Name`, which the row's label supplies.
    body: Value,
    indicator: Indicator,
    /// For an accepted row, the `EvaluationType` and `MetricSourceType` the stored record
    /// must carry, and the indicator member it must fill. Empty for a refused row.
    expected: Option<(&'static str, &'static str, &'static str)>,
}

/// The rows rule 1 is asserted over.
///
/// They differ along what the rule generalises over: how many of the two configs the request
/// names (none, one, both), WHICH one it names (hence the evaluation kind), the FORM of the
/// config it names (an Application Signals entity, an operation of one, or the caller's own
/// CloudWatch metric queries — hence the metric source type), and, for the rows that name
/// none, which of the operation's OTHER members the request fills: a create carrying a
/// description, a goal, burn rates, tags or `CreateRecommendedSlo` still defines no
/// indicator. `CreateRecommendedSlo` is a row of its own because its doc narrows exactly what
/// it excuses — "you don't need to specify the MetricThreshold or ComparisonOperator in the
/// SliConfig or RequestBasedSliConfig" — which is members INSIDE a config, not the config.
fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "no-indicator-name-only",
            body: json!({}),
            indicator: Indicator::None,
            expected: None,
        },
        Case {
            label: "no-indicator-with-description-and-goal",
            body: json!({
                "Description": "an objective with no indicator",
                "Goal": { "AttainmentGoal": 99.9, "WarningThreshold": 30.0 }
            }),
            indicator: Indicator::None,
            expected: None,
        },
        Case {
            label: "no-indicator-with-burn-rates",
            body: json!({
                "BurnRateConfigurations": [{ "LookBackWindowMinutes": 60 }],
                "AutoInvestigationEnabled": true
            }),
            indicator: Indicator::None,
            expected: None,
        },
        Case {
            label: "no-indicator-with-tags",
            body: json!({ "Tags": tag_list(&[tag("team", "checkout")]) }),
            indicator: Indicator::None,
            expected: None,
        },
        Case {
            label: "no-indicator-recommended-slo",
            body: json!({ "CreateRecommendedSlo": true }),
            indicator: Indicator::None,
            expected: None,
        },
        Case {
            label: "both-indicators",
            body: json!({
                "SliConfig": sli_config("checkout", "GET /cart", 100.0),
                "RequestBasedSliConfig": request_based_sli_config("checkout")
            }),
            indicator: Indicator::Both,
            expected: None,
        },
        Case {
            label: "period-based-on-an-operation",
            body: json!({ "SliConfig": sli_config("checkout", "POST /checkout", 250.0) }),
            indicator: Indicator::One,
            expected: Some(("PeriodBased", "ServiceOperation", "Sli")),
        },
        Case {
            label: "period-based-on-own-queries",
            body: json!({ "SliConfig": sli_config_with_queries() }),
            indicator: Indicator::One,
            expected: Some(("PeriodBased", "CloudWatchMetric", "Sli")),
        },
        Case {
            label: "request-based-on-a-service",
            body: json!({ "RequestBasedSliConfig": request_based_sli_config("orders") }),
            indicator: Indicator::One,
            expected: Some(("RequestBased", "Service", "RequestBasedSli")),
        },
        Case {
            label: "request-based-on-own-queries",
            body: json!({ "RequestBasedSliConfig": request_based_sli_config_with_queries() }),
            indicator: Indicator::One,
            expected: Some(("RequestBased", "CloudWatchMetric", "RequestBasedSli")),
        },
    ]
}

/// Rule 1: a create defines exactly one indicator, or it is refused and stores nothing.
#[tokio::test]
async fn a_create_defines_exactly_one_indicator_or_is_refused() {
    let router = router();
    for case in cases() {
        let mut body = case.body.clone();
        body["Name"] = json!(case.label);

        let (status, answer) = create(&router, body).await;

        let Some((evaluation_type, metric_source_type, indicator_member)) = case.expected else {
            assert_eq!(
                status, 400,
                "case {:?}: expected HTTP 400 for a create naming {:?} indicator configs, got {status} with {answer}",
                case.label, case.indicator
            );
            assert_eq!(
                answer["__type"].as_str(),
                Some("ValidationException"),
                "case {:?}: expected a ValidationException, got {answer}",
                case.label
            );
            assert_not_stored(&router, case.label, case.label).await;
            continue;
        };

        assert_eq!(
            status, 200,
            "case {:?}: expected HTTP 200 for a create naming one indicator config, got {status} with {answer}",
            case.label
        );
        let stored = answer["Slo"].clone();
        assert_eq!(
            stored["EvaluationType"].as_str(),
            Some(evaluation_type),
            "case {:?}: expected EvaluationType {evaluation_type:?}, got {:?}",
            case.label,
            stored["EvaluationType"]
        );
        assert_eq!(
            stored["MetricSourceType"].as_str(),
            Some(metric_source_type),
            "case {:?}: expected MetricSourceType {metric_source_type:?}, got {:?}",
            case.label,
            stored["MetricSourceType"]
        );
        assert!(
            !stored[indicator_member].is_null(),
            "case {:?}: expected the stored record to carry {indicator_member}, got {stored}",
            case.label
        );

        // Round trip: the reader answers the record the create wrote, indicator included.
        let (status, fetched) = get_slo(&router, case.label).await;
        assert_eq!(
            status, 200,
            "case {:?}: expected the created SLO to be readable, got {status} with {fetched}",
            case.label
        );
        assert_eq!(
            fetched["Slo"], stored,
            "case {:?}: a Get must answer the stored record, got {:?}",
            case.label, fetched["Slo"]
        );
    }
}

/// Rule 2: a create carrying at most the documented number of tags stores them all; one
/// carrying more is refused and stores nothing.
///
/// The counts span the class: no `Tags` member at all, the empty list the model admits, one
/// tag, the documented maximum, one past it, and the modelled `TagList` maximum.
#[tokio::test]
async fn a_create_carries_at_most_the_documented_number_of_tags() {
    // `None` is "no Tags member"; `Some(n)` is a list of n tags.
    let counts: [Option<usize>; 6] = [
        None,
        Some(0),
        Some(1),
        Some(MAX_TAGS),
        Some(MAX_TAGS + 1),
        Some(MODELLED_MAX_TAGS),
    ];

    let router = router();
    for count in counts {
        let label = count.map_or_else(|| "no-tags-member".to_string(), |count| format!("tags-{count:03}"));
        let tags = count.map(generated_tags);
        let mut body = json!({
            "Name": label,
            "SliConfig": sli_config("checkout", "GET /cart", 100.0)
        });
        if let Some(tags) = tags.as_ref() {
            body["Tags"] = tag_list(tags);
        }

        let (status, answer) = create(&router, body).await;

        if count.is_some_and(|count| count > MAX_TAGS) {
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400 for more than {MAX_TAGS} tags, got {status} with {answer}"
            );
            assert_eq!(
                answer["__type"].as_str(),
                Some("ValidationException"),
                "case {label:?}: expected a ValidationException, got {answer}"
            );
            assert_eq!(
                answer["message"].as_str(),
                Some(tag_count_message().as_str()),
                "case {label:?}: expected the message {:?}, got {:?}",
                tag_count_message(),
                answer["message"]
            );
            assert_not_stored(&router, &label, &label).await;
            continue;
        }

        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 for at most {MAX_TAGS} tags, got {status} with {answer}"
        );

        // Round trip: every tag the accepted create carried is attached to the stored
        // resource, addressed by the ARN the create answered.
        let (status, listed) = list_tags_raw(&router, &arn_path_segment(&label)).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from ListTagsForResource, got {status} with {listed}"
        );
        let expected: std::collections::BTreeMap<String, String> =
            tags.unwrap_or_default().into_iter().collect();
        assert_eq!(
            reported_tags(&listed),
            expected,
            "case {label:?}: expected the stored tags to be the ones the create carried, got {listed}"
        );
    }
}

/// The ARN a create mints is the one the tag reads above address: `slo/<name>` in the
/// caller's region and the emulator account.
#[tokio::test]
async fn the_minted_arn_is_the_one_the_tag_reads_address() {
    let router = router();
    let rows = [
        (
            "arn-period-based",
            json!({ "SliConfig": sli_config("checkout", "GET /cart", 100.0) }),
        ),
        (
            "arn-request-based",
            json!({ "RequestBasedSliConfig": request_based_sli_config("checkout") }),
        ),
    ];

    for (name, config) in rows {
        let mut body = config.clone();
        body["Name"] = json!(name);
        body["Tags"] = tag_list(&[tag("team", "checkout")]);
        let (status, created) = create(&router, body).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the setup create to succeed, got {status} with {created}"
        );
        assert_eq!(
            created["Slo"]["Arn"].as_str(),
            Some(format!("arn:aws:application-signals:{REGION}:{ACCOUNT}:slo/{name}").as_str()),
            "case {name:?}: expected the minted ARN to carry the caller's region and the \
             emulator account, got {:?}",
            created["Slo"]["Arn"]
        );

        let (status, listed) = list_tags_raw(&router, &arn_path_segment(name)).await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected HTTP 200 from ListTagsForResource, got {status} with {listed}"
        );
        assert_eq!(
            reported_tags(&listed),
            [tag("team", "checkout")].into_iter().collect(),
            "case {name:?}: expected the create's tag to be attached to the minted ARN, got {listed}"
        );
    }
}
