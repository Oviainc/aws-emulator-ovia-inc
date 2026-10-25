// SPDX-License-Identifier: CC0-1.0

//! @op QueryForecast
//!
//! Retrieves the predictions of one time series of a forecast the Amazon Forecast control
//! plane produced. The whole answer is a projection of persisted state
//! ([`crate::state::StoredForecast::query`]): resolve the `ForecastArn`, select the series
//! `Filters` names, clip its points to `[StartDate, EndDate]`, and report one statistic
//! per forecast type the forecast was created with. The handler writes nothing, so two
//! identical calls answer identically.
//!
//! Everything it decides once that ARN has been looked up — whether the resource is
//! there, whether it is readable yet, whether a supplied `NextToken` can be one this
//! service minted, and how a projection it cannot answer is classified — is the decision
//! `QueryWhatIfForecast` must answer identically, so it is stated once in
//! [`crate::state::ResourceKind::resolve_query`] and this handler only renders the
//! [`crate::state::QueryOutcome`] it returns into this operation's own modelled errors.
//!
//! ## Pagination
//!
//! `QueryForecastResponse` models exactly one member, `Forecast`, and `Forecast` models
//! exactly one member, `Predictions` — there is NO response member a continuation token
//! could be returned in, which matches the operation's own doc: "By default, QueryForecast
//! returns the complete date range for the filtered forecast." So this operation issues no
//! token, and therefore no `NextToken` a caller supplies can be one it issued: a request
//! carrying one is refused with the modelled `InvalidNextTokenException`.
//!
//! That refusal comes AFTER the `ForecastArn` is resolved. A token is a position inside
//! ONE resource's results, so there is nothing to judge it against until the resource is
//! known: a request naming a forecast this service holds nothing for is
//! `ResourceNotFoundException` even when it also carries a token this service never
//! minted.
//!
//! ## What is judged before the resource
//!
//! Two things, in this order. The ARN the request names first: its own modelled SYNTAX
//! (an `Arn` outside its pattern or length never reaches this handler — see
//! `crate::service::override_refusal`) and then the resource kind it names, both refused
//! with `InvalidInputException`. Then the SHAPE of the rest of the request — the values
//! of `StartDate`, `EndDate` and the keys of `Filters` against the constraints of the
//! shapes they target ([`crate::state::shape_validation_message`]) — answered with
//! `ValidationException` (400) because it is the request's shape that is wrong, not this
//! forecast, and answered whether or not the store holds the forecast the ARN names.
//!
//! ## The ARN must NAME a forecast
//!
//! That syntax check is the modelled pattern, which admits every Amazon Forecast ARN
//! equally — so it cannot tell a forecast from a predictor, a dataset or a what-if
//! forecast. This handler therefore also requires the ARN to NAME a forecast:
//! `forecast/<forecastName>` in the `forecast` ARN namespace (see
//! [`crate::state::FORECAST`]). An ARN that does not identifies no forecast however the
//! store is filled, so it is answered with the same modelled `InvalidInputException`
//! (400) as an unreadable one, BEFORE resource resolution; `ResourceNotFoundException`
//! (404) stays reserved for an ARN that does name a forecast and that the store holds
//! nothing under.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `QueryForecast`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json")
//! ```
//!
//! Retrieves a forecast for a single item, filtered by the supplied criteria. The criteria
//! is a key-value pair. The key is either item_id (or the equivalent non-timestamp,
//! non-target field) from the TARGET_TIME_SERIES dataset, or one of the forecast dimensions
//! specified as part of the FeaturizationConfig object. By default, QueryForecast returns
//! the complete date range for the filtered forecast. You can request a specific date
//! range. To get the full forecast, use the CreateForecastExportJob operation. The
//! forecasts generated by Amazon Forecast are in the same timezone as the dataset that was
//! used to create the predictor.
//!
//! ## Input
//!
//! * `EndDate` (DateTime) — The end date for the forecast. Specify the date using this
//!   format: yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format). For example, 2015-01-01T20:00:00.
//! * `Filters` (Filters, len 1..50) **required** — The filtering criteria to apply when
//!   retrieving the forecast. For example, to get the forecast for client_21 in the
//!   electricity usage dataset, specify the following: {"item_id" : "client_21"} To get the
//!   full forecast, use the CreateForecastExportJob operation.
//! * `ForecastArn` (Arn, len 0..256, pattern ^arn:([a-z\d-]+):forecast:.*:.*:.+$)
//!   **required** — The Amazon Resource Name (ARN) of the forecast to query.
//! * `NextToken` (NextToken, len 1..3000) — If the result of the previous request was
//!   truncated, the response includes a NextToken. To retrieve the next set of results, use
//!   the token in the next request. Tokens expire after 24 hours.
//! * `StartDate` (DateTime) — The start date for the forecast. Specify the date using this
//!   format: yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format). For example, 2015-01-01T08:00:00.
//!
//! ## Output
//!
//! * `Forecast` (Forecast) — The forecast.
//!
//! ## Errors
//!
//! * `InvalidInputException` (HTTP 400) — The value is invalid or is too long.
//! * `InvalidNextTokenException` (HTTP 400) — The token is not valid. Tokens expire after 24
//!   hours.
//! * `LimitExceededException` (HTTP 409) — The limit on the number of requests per second has
//!   been exceeded.
//! * `ResourceInUseException` (HTTP 409) — The specified resource is in use.
//! * `ResourceNotFoundException` (HTTP 404) — We can't find that resource. Check the
//!   information that you've provided and try again.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `QueryForecastError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `QueryForecastError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.forecastquery#QueryForecast")]
#[derive(Debug)]
pub enum QueryForecastErrorModeled {
    InvalidInputException(InvalidInputException),
    InvalidNextTokenException(InvalidNextTokenException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::query_forecast::QueryForecastError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type QueryForecastError = crate::scaffolded::Scaffolded<QueryForecastErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<QueryForecastErrorModeled> for QueryForecastError {
    fn from(modeled: QueryForecastErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `QueryForecast` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct QueryForecast<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> QueryForecast<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for QueryForecast<Eng>
{
    type Input = QueryForecastInput;
    type Output = QueryForecastOutput;
    type Error = QueryForecastError;

    const NAME: &'static str = "QueryForecast";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::QUERY_FORECAST_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::QUERY_FORECAST_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        // `ForecastArn` must NAME a forecast: the modelled pattern admits every Amazon
        // Forecast ARN equally, so the resource type and depth are checked here. An ARN
        // that names some other Forecast resource identifies no forecast at all, whatever
        // the store holds, so it is invalid input rather than an absent resource. It is
        // read FIRST, as the ARN's modelled syntax is (by the validation layer in front of
        // this handler): the resource the request names is what the rest of it is about.
        if !crate::state::FORECAST.names(&input.forecast_arn) {
            return Err(QueryForecastErrorModeled::InvalidInputException(
                InvalidInputException {
                    message: Some(
                        crate::state::FORECAST.invalid_arn_message(&input.forecast_arn),
                    ),
                },
            )
            .into());
        }

        // Then the SHAPE of the rest of the request, BEFORE the forecast is resolved: a
        // `DateTime` outside its pattern, or a `Filters` key outside the key shape's
        // pattern or length, is a request this service cannot read — so it is refused
        // whether or not the store holds the forecast the ARN names, and with
        // `ValidationException` (400) rather than one of this operation's own modelled
        // errors. `Filters` KEYS are judged here because the framework's boundary walk
        // captures a map key as a path segment and never judges it.
        if let Some(message) = crate::state::shape_validation_message(
            &input.filters,
            input.start_date.as_deref(),
            input.end_date.as_deref(),
        ) {
            return Err(crate::scaffolded::Scaffolded::validation(message));
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // The store is keyed on the shared `InMemoryEngine`; mounted on any other
            // engine this service has no forecasts to read and cannot serve the
            // operation at all. Not a client error, so not a 4xx.
            return Err(crate::scaffolded::Scaffolded::not_implemented("QueryForecast"));
        };

        // One lock, one read: everything below is computed from the forecast as stored.
        // WHICH condition is judged, in which order, and with which message is the
        // decision both query operations must answer identically, so it is stated once in
        // [`crate::state::ResourceKind::resolve_query`] and this handler only renders the
        // outcome it hands back into this operation's own modelled errors. Reading it
        // from the store writes nothing, so two identical calls answer identically.
        let outcome = {
            let state = state.lock();
            crate::state::FORECAST.resolve_query(
                state.forecasts.get(&input.forecast_arn),
                &crate::state::QueryRequest {
                    arn: &input.forecast_arn,
                    filters: &input.filters,
                    start_date: input.start_date.as_deref(),
                    end_date: input.end_date.as_deref(),
                    next_token: input.next_token.as_deref(),
                },
                ::std::time::SystemTime::now(),
            )
        };

        let predictions = match outcome {
            crate::state::QueryOutcome::Predictions(predictions) => predictions,
            // The forecast, or the series within it, is not there (404).
            crate::state::QueryOutcome::NotFound(message) => {
                return Err(QueryForecastErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(message),
                    },
                )
                .into())
            }
            // It exists and its creation run is not due yet (409).
            crate::state::QueryOutcome::NotActive(message) => {
                return Err(QueryForecastErrorModeled::ResourceInUseException(
                    ResourceInUseException {
                        message: Some(message),
                    },
                )
                .into())
            }
            // It carried a continuation token this service never issued (400).
            crate::state::QueryOutcome::UnmintedNextToken(message) => {
                return Err(QueryForecastErrorModeled::InvalidNextTokenException(
                    InvalidNextTokenException {
                        message: Some(message),
                    },
                )
                .into())
            }
            // The request is not one this forecast can be queried with (400).
            crate::state::QueryOutcome::Unqueryable(message) => {
                return Err(QueryForecastErrorModeled::InvalidInputException(
                    InvalidInputException {
                        message: Some(message),
                    },
                )
                .into())
            }
        };

        Ok(QueryForecastOutput {
            forecast: Some(Forecast {
                predictions: Some(
                    predictions
                        .into_iter()
                        .map(|(statistic, points)| {
                            (statistic, crate::state::data_points(&points))
                        })
                        .collect(),
                ),
            }),
        })
    }
}

