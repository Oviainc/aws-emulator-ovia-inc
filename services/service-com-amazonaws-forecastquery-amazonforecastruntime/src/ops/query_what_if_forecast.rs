// SPDX-License-Identifier: CC0-1.0

//! @op QueryWhatIfForecast
//!
//! Retrieves the predictions of one time series of a WHAT-IF forecast the Amazon Forecast
//! control plane produced from a base forecast plus the what-if analysis's replacements
//! and transformations. A what-if forecast is its own resource with its own ARN and its
//! own stored series, so this operation resolves `WhatIfForecastArn` in the what-if store
//! only — a base `ForecastArn` is not found here, and the series answered is the what-if's
//! own, never the base forecast's.
//!
//! Apart from which store the ARN resolves in, the projection is identical to
//! `QueryForecast`'s and is shared with it ([`crate::state::StoredForecast::query`]):
//! select the series `Filters` names, clip its points to `[StartDate, EndDate]`, report
//! one statistic per forecast type. The handler writes nothing.
//!
//! ## Pagination
//!
//! `QueryWhatIfForecastResponse` models exactly one member, `Forecast`, and `Forecast`
//! models exactly one member, `Predictions` — there is NO response member a continuation
//! token could be returned in. So this operation issues no token, and a `NextToken` a
//! caller supplies cannot be one it issued: such a request is refused with the modelled
//! `InvalidNextTokenException`.
//!
//! That refusal comes AFTER the `WhatIfForecastArn` is resolved. A token is a position
//! inside ONE resource's results, so there is nothing to judge it against until the
//! resource is known: a request naming a what-if forecast this service holds nothing for
//! is `ResourceNotFoundException` even when it also carries a token this service never
//! minted. The ARN's own SYNTAX is judged first of all — a `LongArn` outside its modelled
//! pattern or length never reaches a lookup and is refused with `InvalidInputException`
//! (see `crate::service::override_refusal`).
//!
//! ## The ARN must NAME a what-if forecast
//!
//! That syntax check is the modelled pattern, which admits every Amazon Forecast ARN
//! equally — so it cannot tell a what-if forecast from a plain forecast or a what-if
//! analysis. This handler therefore also requires the ARN to NAME a what-if forecast:
//! `what-if-forecast/<whatIfAnalysisName>/<whatIfForecastName>` in the `forecast` ARN
//! namespace (see [`crate::state::WHAT_IF_FORECAST`]). An ARN that does not identifies no
//! what-if forecast however the store is filled, so it is answered with the same modelled
//! `InvalidInputException` (400) as an unreadable one, BEFORE resource resolution, and
//! with the same sentence — `Invalid ARN for <resource type>: <the value supplied>`
//! ([`crate::state::ResourceKind::invalid_arn_message`]), since the answer says which
//! resource was expected and not how the value fell short.
//! `ResourceNotFoundException` (404) stays reserved for an ARN that does name a what-if
//! forecast and that the store holds nothing under.
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
//! # Modeled contract for `QueryWhatIfForecast`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-forecastquery-amazonforecastruntime/smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json")
//! ```
//!
//! Retrieves a what-if forecast.
//!
//! ## Input
//!
//! * `EndDate` (DateTime) — The end date for the what-if forecast. Specify the date using
//!   this format: yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format). For example, 2015-01-01T20:00:00.
//! * `Filters` (Filters, len 1..50) **required** — The filtering criteria to apply when
//!   retrieving the forecast. For example, to get the forecast for client_21 in the
//!   electricity usage dataset, specify the following: {"item_id" : "client_21"} To get the
//!   full what-if forecast, use the CreateForecastExportJob operation.
//! * `NextToken` (NextToken, len 1..3000) — If the result of the previous request was
//!   truncated, the response includes a NextToken. To retrieve the next set of results, use
//!   the token in the next request. Tokens expire after 24 hours.
//! * `StartDate` (DateTime) — The start date for the what-if forecast. Specify the date using
//!   this format: yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format). For example, 2015-01-01T08:00:00.
//! * `WhatIfForecastArn` (LongArn, len 0..300, pattern ^arn:([a-z\d-]+):forecast:.*:.*:.+$)
//!   **required** — The Amazon Resource Name (ARN) of the what-if forecast to query.
//!
//! ## Output
//!
//! * `Forecast` (Forecast) — Provides information about a forecast. Returned as part of the
//!   QueryForecast response.
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
// `QueryWhatIfForecastError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-forecastquery-amazonforecastruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `QueryWhatIfForecastError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.forecastquery#QueryWhatIfForecast")]
#[derive(Debug)]
pub enum QueryWhatIfForecastErrorModeled {
    InvalidInputException(InvalidInputException),
    InvalidNextTokenException(InvalidNextTokenException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::query_what_if_forecast::QueryWhatIfForecastError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type QueryWhatIfForecastError = crate::scaffolded::Scaffolded<QueryWhatIfForecastErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<QueryWhatIfForecastErrorModeled> for QueryWhatIfForecastError {
    fn from(modeled: QueryWhatIfForecastErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `QueryWhatIfForecast` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct QueryWhatIfForecast<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> QueryWhatIfForecast<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for QueryWhatIfForecast<Eng>
{
    type Input = QueryWhatIfForecastInput;
    type Output = QueryWhatIfForecastOutput;
    type Error = QueryWhatIfForecastError;

    const NAME: &'static str = "QueryWhatIfForecast";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::QUERY_WHAT_IF_FORECAST_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::QUERY_WHAT_IF_FORECAST_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let _ = ctx;

        // `WhatIfForecastArn` must NAME a what-if forecast — type `what-if-forecast` and
        // the analysis/forecast name pair its ARNs carry (see
        // [`crate::state::WHAT_IF_FORECAST`]). The modelled pattern admits every Amazon
        // Forecast ARN equally, so an ARN that names a plain forecast, or a what-if
        // forecast ARN missing its analysis, identifies no what-if forecast at all,
        // whatever the store holds: invalid input rather than an absent resource.
        if !crate::state::WHAT_IF_FORECAST.names(&input.what_if_forecast_arn) {
            return Err(QueryWhatIfForecastErrorModeled::InvalidInputException(
                InvalidInputException {
                    message: Some(
                        crate::state::WHAT_IF_FORECAST
                            .invalid_arn_message(&input.what_if_forecast_arn),
                    ),
                },
            )
            .into());
        }

        let Some(state) = crate::state::service_state(&*self.engine) else {
            // The store is keyed on the shared `InMemoryEngine`; mounted on any other
            // engine this service has no what-if forecasts to read and cannot serve the
            // operation at all. Not a client error, so not a 4xx.
            return Err(crate::scaffolded::Scaffolded::not_implemented(
                "QueryWhatIfForecast",
            ));
        };

        // One lock, one read: everything below is computed from the what-if forecast as
        // stored.
        let projection = {
            let state = state.lock();
            let Some(forecast) = state.what_if_forecasts.get(&input.what_if_forecast_arn) else {
                return Err(QueryWhatIfForecastErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(format!(
                            "We can't find that resource. No what-if forecast with ARN {} \
                             exists.",
                            input.what_if_forecast_arn
                        )),
                    },
                )
                .into());
            };

            // The what-if forecast the request names RESOLVED, so a continuation token can
            // now be judged against it. The order matters and is not an accident: a token
            // only means anything as a position inside one resource's results, so the
            // resource is resolved first and an ARN that names nothing is answered
            // `ResourceNotFoundException` even when the request also carries a token this
            // service could not have issued. No token this operation could have issued
            // exists (see the module docs), so any `NextToken` that gets this far is one
            // this service did not mint.
            if input.next_token.is_some() {
                return Err(QueryWhatIfForecastErrorModeled::InvalidNextTokenException(
                    InvalidNextTokenException {
                        message: Some(
                            "The token is not valid. Tokens expire after 24 hours. \
                             QueryWhatIfForecast returns the complete date range for the \
                             filtered what-if forecast, so it returns no NextToken to \
                             resume from."
                                .to_string(),
                        ),
                    },
                )
                .into());
            }

            forecast.query(
                &input.filters,
                input.start_date.as_deref(),
                input.end_date.as_deref(),
            )
        };

        let predictions = match projection {
            Ok(predictions) => predictions,
            Err(rejection) => return Err(reject(&rejection)),
        };

        Ok(QueryWhatIfForecastOutput {
            forecast: Some(Forecast {
                predictions: Some(
                    predictions
                        .into_iter()
                        .map(|(statistic, points)| (statistic, crate::state::data_points(&points)))
                        .collect(),
                ),
            }),
        })
    }
}

/// The modelled error a [`crate::state::QueryRejection`] is reported as: a series the
/// what-if forecast does not hold is "we can't find that resource" (404); everything else
/// is a request this forecast cannot be queried with (400).
fn reject(rejection: &crate::state::QueryRejection) -> QueryWhatIfForecastError {
    let message = Some(rejection.message());
    if rejection.is_not_found() {
        QueryWhatIfForecastErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
            message,
        })
        .into()
    } else {
        QueryWhatIfForecastErrorModeled::InvalidInputException(InvalidInputException { message })
            .into()
    }
}

