Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 743fc0a4792edb6d196f368afdefaae152d33ff0

# Forecasts created through the Amazon Forecast control plane never reach the forecastquery store, so QueryForecast/QueryWhatIfForecast can never answer 200 for any client workflow

_Filed: 2026-10-24T02:19:12Z_
_Operation: QueryForecast | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `743fc0a4792edb6d196f368afdefaae152d33ff0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

QueryForecast and QueryWhatIfForecast are implemented and project their answer from persisted state (`ComAmazonawsForecastqueryAmazonforecastruntimeState.forecasts` / `.what_if_forecasts`), but NOTHING reachable over the wire ever writes those maps. The only writers of `put_forecast` / `put_what_if_forecast` are this crate's own integration tests and one router test; the Amazon Forecast CONTROL plane, which is the only thing that mints a `ForecastArn` / `WhatIfForecastArn`, does not write into this store at all — and every forecast-producing operation there is still a scaffold.

Expected (real AWS): CreateForecast -> (status becomes ACTIVE) -> QueryForecast with Filters={"item_id": ...} returns HTTP 200 with `Forecast.Predictions` for the forecast that was just created; CreateWhatIfForecast -> QueryWhatIfForecast likewise; DeleteForecast makes the same ARN answer 404 afterwards.

Actual (emulator): there is no sequence of API calls that can make either query operation return 200. A client that creates a forecast gets 501 from CreateForecast, and if it supplies any ARN to QueryForecast it gets 404 `ResourceNotFoundException` ("No resource found <arn>") forever, because the data-plane store is populated only from Rust test code. The two query handlers are therefore only exercisable in-process, never through the emulated API surface.

Scope of the fix is the cross-service wiring: the Forecast control plane's CreateForecast / CreateWhatIfForecast (and DeleteForecast / DeleteWhatIfForecast) must publish into the shared-engine forecastquery state — the crate already documents this contract in src/state.rs ("written from OUTSIDE a query handler — by the control plane once it is implemented (it reaches this store with a path dependency on this crate and the same shared engine)") — so that a created forecast becomes queryable and a deleted one stops being queryable.

## Evidence

Spec/semantics: `com.amazonaws.forecastquery#QueryForecast` is a pure data-plane read of a resource produced by the Amazon Forecast control plane (`CreateForecast`); the operation doc and `Forecast$Predictions` doc describe returning the predictions of the forecast that was created, so a create->query workflow must be observable. `QueryWhatIfForecast` likewise for `CreateWhatIfForecast`.
Handlers: services/service-com-amazonaws-forecastquery-amazonforecastruntime/src/ops/query_forecast.rs lines 223-245 read `state.forecasts.get(&input.forecast_arn)` and answer 404 when absent; src/ops/query_what_if_forecast.rs lines 228-254 do the same against `state.what_if_forecasts`.
State: src/state.rs lines 7-19 state explicitly that the maps are written from outside a query handler, "by the control plane once it is implemented … or by a test that stands in for it"; `put_forecast` / `put_what_if_forecast` (lines 58-68) are the only writers.
Wiring: `grep -r 'put_forecast|put_what_if_forecast|ComAmazonawsForecastqueryAmazonforecastruntimeState'` matches only this crate's tests/all/*.rs and core/router/tests/all/forecast_signing_name_routing.rs — no production code outside this crate touches the store.
Control plane: every forecast op in services/service-com-amazonaws-forecast-amazonforecast/src/ops/ (create_forecast.rs, create_what_if_forecast.rs, describe_forecast.rs, list_forecasts.rs, delete_forecast.rs, delete_what_if_forecast.rs, …) still carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and answers 501, and none of them reference the forecastquery state type.
Tests: tests/all/forecastquery.rs seeds forecasts directly through `engine.state::<ForecastQueryState>().lock().put_forecast(..)` (lines 46-57), i.e. the suite stands in for the missing propagation; no test asserts that an API-created forecast becomes queryable.

## Steps to Reproduce

1. Start the emulator with both the Amazon Forecast control plane and the forecastquery data plane mounted on the shared engine.
2. Call forecast.CreateForecast(ForecastName='electricity', PredictorArn=...) — the emulator answers HTTP 501 NotImplemented, so no ARN is minted.
3. Call forecastquery.QueryForecast(ForecastArn='arn:aws:forecast:us-east-1:000000000000:forecast/electricity', Filters={'item_id': 'client_21'}) — the emulator answers HTTP 404 ResourceNotFoundException 'No resource found <arn>', and no API call exists that could have made it answer 200.
4. Repeat for CreateWhatIfForecast -> QueryWhatIfForecast: same outcome against the separate what_if_forecasts map.
5. On real AWS the create->query workflow returns 200 with Forecast.Predictions for the created forecast; in the emulator the data-plane store is reachable only from in-process Rust test code.
