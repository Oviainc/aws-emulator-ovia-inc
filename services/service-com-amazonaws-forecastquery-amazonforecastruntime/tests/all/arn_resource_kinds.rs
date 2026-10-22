// SPDX-License-Identifier: CC0-1.0

//! Which ARN each query operation can RESOLVE, and what it answers for one it cannot.
//!
//! The modelled pattern on both ARN members is `^arn:([a-z\d-]+):forecast:.*:.*:.+$`
//! (`smithy_query(query="operation", operation="QueryForecast,QueryWhatIfForecast")`), so
//! it admits EVERY Amazon Forecast ARN equally and the framework's boundary validator
//! cannot tell a forecast from a predictor, a dataset or a what-if forecast. The rule this
//! suite pins is the one the handlers apply on top of it:
//!
//! > An operation that resolves a resource by ARN first requires the ARN to NAME a
//! > resource of the kind that operation queries — this service's ARN namespace
//! > (`forecast`), that resource's type, and the number of name segments that type's ARNs
//! > carry — and answers the modelled `InvalidInputException` (400) when it does not.
//! > `ResourceNotFoundException` (404) is reserved for an ARN that DOES name a resource
//! > of that kind and that the store holds nothing under.
//!
//! The two kinds and their depth come from the models: a forecast is
//! `forecast/<forecastName>` (`ForecastArn` targets `Arn`, max 256), while a what-if
//! forecast is `what-if-forecast/<whatIfAnalysisName>/<whatIfForecastName>` — its name is
//! "unique within each what-if analysis" (the control plane's
//! `CreateWhatIfForecast.WhatIfForecastName` doc), which is why `WhatIfForecastArn` is the
//! single Forecast ARN member targeting the longer `LongArn` (max 300) while even the
//! what-if ANALYSIS's own ARN is an `Arn`.
//!
//! Nothing here judges a partition, a region, an account or how a name is spelled: those
//! are values the control plane chose, so an ARN carrying ones this emulator never minted
//! is a resource it does not hold (404), not a malformed request.

use std::sync::Arc;

use engine::InMemoryEngine;
use serde_json::{json, Value};
use service_com_amazonaws_forecastquery_amazonforecastruntime as forecastquery;
use smithy_server_runtime::MultiServiceRouter;

use forecastquery::state::{StoredForecast, StoredPoint, StoredSeries};
use forecastquery::ComAmazonawsForecastqueryAmazonforecastruntimeState as ForecastQueryState;

/// A router carrying only this service, plus the engine its store lives in.
fn harness() -> (Arc<InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signatures are verified by the emulator, never by a service; this suite is about
    // which ARN resolves, so it turns verification off rather than signing each request.
    router.set_request_authentication(false);
    router.register_service(forecastquery::EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// An ARN of the shape the control plane mints.
fn arn(resource: &str) -> String {
    engine::emulator_arn("forecast", "us-east-1", resource)
}

/// A forecast with one series (`item_id=client_21`) and one statistic.
fn stored_forecast(forecast_arn: &str) -> StoredForecast {
    let series = StoredSeries::new([("item_id", "client_21")]).with_statistic(
        "p50",
        [StoredPoint::new("2015-01-01T00:00:00", 42.0)],
    );
    StoredForecast::new(forecast_arn, "electricity_demand", "D", ["0.5"]).with_series(series)
}

/// One awsJson1_1 call: `(http status, parsed body)`.
async fn call(router: &MultiServiceRouter, operation: &str, body: &Value) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("AmazonForecastRuntime.{operation}"))
        // The routing key: the SigV4 credential scope's signing name (`forecast`).
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAEXAMPLEEXAMPLE/20150101/us-east-1/forecast/aws4_request, \
             SignedHeaders=content-type;host;x-amz-target, Signature=0",
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("serialize request body"),
        ))
        .expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// The wire error code of a failure response.
fn error_code(body: &Value) -> String {
    body.get("__type")
        .or_else(|| body.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One request, and the answer the ARN it carries earns.
struct Case {
    /// How the row reads in an assertion message.
    name: &'static str,
    /// `QueryForecast` or `QueryWhatIfForecast`.
    operation: &'static str,
    /// The ARN the request's ARN member carries.
    queried: String,
    /// The HTTP status expected.
    status: u16,
    /// The modelled error code expected, or `None` for a successful query.
    code: Option<&'static str>,
}

#[tokio::test]
async fn an_arn_resolves_only_when_it_names_a_resource_of_the_queried_kind() {
    let (engine, router) = harness();
    let forecast_arn = arn("forecast/electricity_demand");
    let what_if_arn = arn("what-if-forecast/electricity_analysis/price_cut");
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_forecast(stored_forecast(&forecast_arn));
    engine
        .state::<ForecastQueryState>()
        .lock()
        .put_what_if_forecast(stored_forecast(&what_if_arn).derived_from(&forecast_arn));

    // The resource types below other than the two queried ones are Amazon Forecast's own,
    // taken from the control plane's model (`smithy_query(query="digest",
    // model_path=".../com-amazonaws-forecast-amazonforecast.json")`: Create/Describe
    // operations for dataset, dataset-group, dataset-import-job, predictor,
    // forecast-export-job, what-if-analysis, …). None of them is a forecast or a what-if
    // forecast, so none of them resolves here.
    let cases: Vec<Case> = vec![
        Case {
            name: "the forecast ARN the store holds",
            operation: "QueryForecast",
            queried: forecast_arn.clone(),
            status: 200,
            code: None,
        },
        Case {
            name: "the what-if forecast ARN the store holds",
            operation: "QueryWhatIfForecast",
            queried: what_if_arn.clone(),
            status: 200,
            code: None,
        },
        Case {
            name: "a forecast ARN nothing is stored under",
            operation: "QueryForecast",
            queried: arn("forecast/never_created"),
            status: 404,
            code: Some("ResourceNotFoundException"),
        },
        Case {
            name: "a forecast name spelled with hyphens, nothing stored under it",
            operation: "QueryForecast",
            queried: arn("forecast/sim2real-any-thing"),
            status: 404,
            code: Some("ResourceNotFoundException"),
        },
        Case {
            name: "a what-if forecast ARN nothing is stored under",
            operation: "QueryWhatIfForecast",
            queried: arn("what-if-forecast/other_analysis/other_variant"),
            status: 404,
            code: Some("ResourceNotFoundException"),
        },
        Case {
            name: "a forecast ARN in another partition, region and account",
            operation: "QueryForecast",
            queried: "arn:aws-cn:forecast:cn-north-1:000000000000:forecast/electricity_demand"
                .to_string(),
            status: 404,
            code: Some("ResourceNotFoundException"),
        },
        Case {
            name: "a what-if forecast ARN missing its analysis name",
            operation: "QueryWhatIfForecast",
            queried: arn("what-if-forecast/price_cut"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a what-if forecast ARN with a name segment too many",
            operation: "QueryWhatIfForecast",
            queried: arn("what-if-forecast/electricity_analysis/price_cut/extra"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a forecast ARN through QueryWhatIfForecast",
            operation: "QueryWhatIfForecast",
            queried: forecast_arn.clone(),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a what-if forecast ARN through QueryForecast",
            operation: "QueryForecast",
            queried: what_if_arn.clone(),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a what-if analysis ARN through QueryWhatIfForecast",
            operation: "QueryWhatIfForecast",
            queried: arn("what-if-analysis/electricity_analysis"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a predictor ARN through QueryForecast",
            operation: "QueryForecast",
            queried: arn("predictor/electricity_predictor"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a dataset ARN through QueryWhatIfForecast",
            operation: "QueryWhatIfForecast",
            queried: arn("dataset/electricity_usage"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a forecast-export-job ARN through QueryForecast: the type matches \
                   exactly, never by prefix",
            operation: "QueryForecast",
            queried: arn("forecast-export-job/electricity_demand"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a forecast ARN with an extra name segment",
            operation: "QueryForecast",
            queried: arn("forecast/electricity_demand/extra"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a forecast ARN with no name at all",
            operation: "QueryForecast",
            queried: arn("forecast/"),
            status: 400,
            code: Some("InvalidInputException"),
        },
        Case {
            name: "a resource field with no type separator",
            operation: "QueryForecast",
            queried: arn("electricity_demand"),
            status: 400,
            code: Some("InvalidInputException"),
        },
    ];

    for case in cases {
        let Case {
            name,
            operation,
            queried,
            status: expected_status,
            code: expected_code,
        } = case;
        let member = if operation == "QueryForecast" {
            "ForecastArn"
        } else {
            "WhatIfForecastArn"
        };
        let (status, body) = call(
            &router,
            operation,
            &json!({member: queried, "Filters": {"item_id": "client_21"}}),
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {name:?}: expected HTTP {expected_status}, got {status} with {body}"
        );
        match expected_code {
            Some(expected_code) => {
                assert_eq!(
                    error_code(&body),
                    expected_code,
                    "case {name:?}: expected {expected_code}, got {body}"
                );
                assert!(
                    body.get("Message")
                        .or_else(|| body.get("message"))
                        .and_then(Value::as_str)
                        .is_some_and(|message| message.contains(&queried)),
                    "case {name:?}: the rejection must name the ARN it refused, got {body}"
                );
            }
            None => assert!(
                body.pointer("/Forecast/Predictions/p50")
                    .and_then(Value::as_array)
                    .is_some_and(|points| points.len() == 1),
                "case {name:?}: expected the stored series, got {body}"
            ),
        }
    }
}
