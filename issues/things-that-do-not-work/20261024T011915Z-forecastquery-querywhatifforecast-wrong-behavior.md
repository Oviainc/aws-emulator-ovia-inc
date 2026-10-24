Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# QueryWhatIfForecast refuses StartDate/EndDate spellings its own AWS validation pattern admits (zone designator, offset, fractional seconds, minute precision)

_Filed: 2026-10-24T01:19:15Z_
_Operation: QueryWhatIfForecast | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Identical divergence to QueryForecast, because both operations share `crate::state::shape_validation_message` and `StoredForecast::query`.

The shape layer accepts any `StartDate`/`EndDate` matching the AWS-recorded pattern `^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}[a-zA-Z0-9\:\+\-\[\]\.\/]*$` (so a zone designator, a UTC offset, fractional seconds, lowercase `t`, or minute precision all pass), and the projection layer then rejects anything that is not the exact 19-byte `yyyy-MM-dd'T'HH:mm:ss` layout with `InvalidInputException` (400).

Expected: `2015-01-02T00:00:00Z` / `2015-01-02T00:00` is parsed as an ISO 8601 instant and used to clip the what-if series — HTTP 200 with `Forecast.Predictions` restricted to the window.
Actual: HTTP 400 `InvalidInputException` "StartDate '<value>' is not a valid date; specify the date using this format: yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format)", so a windowed what-if query from a client that formats timestamps with a zone or without seconds can never succeed.

## Evidence

Spec: `QueryWhatIfForecast$StartDate`/`$EndDate` target `com.amazonaws.forecastquery#DateTime` (plain string, no modelled constraint) and are documented as "ISO 8601 format"; the enforced pattern comes from a real AWS recording (fixed issue 20261022T200802Z..., cause 1).
Handler/state: src/ops/query_what_if_forecast.rs lines 220-226 (shape layer) and line 279 `forecast.query(.., start_date, end_date)`; src/state.rs `matches_date_time_pattern` (admits the extended spellings) vs `parse_date_time` lines 499-524 (`bytes.len() != 19` -> None) and `DateWindow::parse` -> `QueryRejection::MalformedDate` -> `InvalidInputException`.
Tests: tests/all/forecastquery.rs lines 567-576 assert the refusal for the QueryForecast twin; no test asserts that a zone-suffixed or minute-precision bound is honoured for either operation.

## Steps to Reproduce

1. Seed a what-if forecast under arn:aws:forecast:us-east-1:<acct>:what-if-forecast/<analysis>/<name> with daily points (as tests/all/forecastquery.rs does).
2. Call QueryWhatIfForecast with {WhatIfForecastArn: <that ARN>, Filters: {"item_id": "client_21"}, EndDate: "2015-01-03T00:00:00Z"} — emulator answers 400 InvalidInputException instead of 200 with points up to 2015-01-03.
3. Repeat with EndDate "2015-01-03T00:00" and "2015-01-03T00:00:00.000" — same 400, although both satisfy the pattern the shape layer enforces.
