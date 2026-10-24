Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# Forecast lifecycle status is not modelled, so the documented ResourceInUseException (409) is unreachable for QueryForecast/QueryWhatIfForecast

_Filed: 2026-10-24T01:19:15Z_
_Operation: QueryForecast | Class: error-response_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Amazon Forecast only serves predictions for a forecast whose lifecycle `Status` is `ACTIVE`. Querying a forecast that exists but is still being produced (`CREATE_PENDING` / `CREATE_IN_PROGRESS`, or `CREATE_STOPPED` / `DELETE_PENDING`) is refused with the modelled `ResourceInUseException` ("The specified resource is in use.", HTTP 409) — that is the only reason this conflict error is modelled on a pure read operation of the data plane.

The emulator's state has no notion of a forecast's status: `crate::state::StoredForecast` carries `arn`, `name`, `forecast_types`, `frequency`, `series`, `created_at` and `base_forecast_arn` only. Consequently a forecast is either absent (404 `ResourceNotFoundException`) or fully queryable (200), and neither `QueryForecast` nor `QueryWhatIfForecast` can ever answer `ResourceInUseException`: `grep` shows the variant is declared in both operations' error enums and constructed nowhere.

Expected: a forecast/what-if forecast that exists but is not yet `ACTIVE` answers HTTP 409 `ResourceInUseException`; only an `ACTIVE` one answers 200 with `Forecast.Predictions`.
Actual: any entry present in the store answers 200 regardless of how it was produced, and the 409 branch is dead code — a client that waits on `ResourceInUseException` (or distinguishes "not ready" from "ready") sees behaviour that cannot occur.

Fix scope is this crate's state model (add the lifecycle status to `StoredForecast` and gate both query handlers on it), so that the control plane's `CreateForecast` / `CreateWhatIfForecast` can record a non-ACTIVE forecast.

## Evidence

Spec: `com.amazonaws.forecastquery#ResourceInUseException` (errorKind client, httpError 409, doc "The specified resource is in use.") is declared on BOTH `QueryForecast` and `QueryWhatIfForecast` (smithy_query query="digest"/"operation"); a read operation on the data plane has no other documented conflict condition, and Amazon Forecast documents that a forecast must reach status ACTIVE before it can be queried.
Handler: src/ops/query_forecast.rs declares `ResourceInUseException` in `QueryForecastErrorModeled` (line 134) and never constructs it; the only outcomes are InvalidInputException, ValidationException, ResourceNotFoundException, InvalidNextTokenException and 200. Same in src/ops/query_what_if_forecast.rs (line 136).
State: src/state.rs `StoredForecast` (lines ~286-311) has no status field, and `StoredForecast::query` (lines ~387-441) projects any stored forecast unconditionally.
Tests: no test in tests/all/ mentions ResourceInUseException or a non-ACTIVE forecast (tests/all/forecastquery.rs only exercises 200/400/404 paths).

## Steps to Reproduce

1. Seed a forecast into the forecastquery store — there is no way to mark it as still being created, because `StoredForecast` has no status member.
2. Call QueryForecast with a valid Filters for that forecast: the emulator answers 200 with predictions, the only possible outcome for a stored forecast.
3. On real AWS, calling QueryForecast against a forecast whose DescribeForecast Status is CREATE_IN_PROGRESS answers HTTP 409 ResourceInUseException ("The specified resource is in use."); the emulator can never produce that response for QueryForecast or QueryWhatIfForecast.
