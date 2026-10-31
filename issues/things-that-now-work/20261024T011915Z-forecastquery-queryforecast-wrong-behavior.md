Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# QueryForecast refuses StartDate/EndDate spellings its own AWS validation pattern admits (zone designator, offset, fractional seconds, minute precision)

_Filed: 2026-10-24T01:19:15Z_
_Operation: QueryForecast | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

QueryForecast judges `StartDate`/`EndDate` in two layers that contradict each other.

Layer 1 (shape validation, `crate::state::shape_validation_message` -> `matches_date_time_pattern`) enforces the pattern the real service is recorded as quoting: `^\d{4}-\d{2}-\d{2}[Tt]\d{2}:\d{2}[a-zA-Z0-9\:\+\-\[\]\.\/]*$`. That pattern deliberately makes everything after `HH:mm` free-form, so it admits ISO 8601 spellings that extend the documented example: `2015-01-01T08:00` (minute precision), `2015-01-01T08:00:00Z`, `2015-01-01T08:00:00+00:00`, `2015-01-01T08:00:00.000`, `2015-01-01t08:00:00`. The module doc of `DATE_TIME_PATTERN` says so in as many words ("plus the ISO 8601 spellings that extend it (a seconds field, a zone designator, an offset, fractional seconds)").

Layer 2 (`crate::state::parse_date_time`, used by `DateWindow::parse` inside `StoredForecast::query`) then requires EXACTLY the 19-byte `yyyy-MM-dd'T'HH:mm:ss` layout with an uppercase `T` and no zone/offset/fraction. Every value that passed layer 1 but is not exactly 19 bytes is rejected with `InvalidInputException` (400) and the message "StartDate '<value>' is not a valid date; specify the date using this format: yyyy-MM-dd'T'HH:mm:ss (ISO 8601 format)".

Expected: a bound that satisfies the validation pattern the service enforces is parsed as an ISO 8601 instant and used to clip the series — HTTP 200 with the predictions inside the window (the member is documented as "ISO 8601 format", and a service does not publish a validation pattern that admits a value its next layer refuses).
Actual: `2015-01-01T08:00:00Z`, `2015-01-01T08:00`, `2015-01-01T08:00:00.000`, `2015-01-01T08:00:00+00:00` and the lowercase-`t` spelling all answer 400 `InvalidInputException`, so a client that sends UTC-suffixed or minute-precision bounds (what most SDK/ISO formatters emit) can never query a forecast with a window at all.

## Evidence

Spec/recording: `com.amazonaws.forecastquery#DateTime` carries no constraint in the model, and the pattern the emulator enforces was taken from a real AWS recording (see fixed issue 20261022T200802Z-forecastquery-queryforecast-shape-level-input-validation-skipped-startdate-filters, cause 1, which quotes AWS's message with that exact regex). The member doc says "ISO 8601 format".
Handler/state: services/service-com-amazonaws-forecastquery-amazonforecastruntime/src/ops/query_forecast.rs lines 215-221 (shape layer) then line 269 `forecast.query(.., start_date, end_date)`; src/state.rs `DATE_TIME_PATTERN` (line ~557) and `matches_date_time_pattern` (line ~590) accept the extended spellings, while `parse_date_time` (lines 499-524) hard-requires `bytes.len() == 19` and an uppercase `T`, and `DateWindow::parse` (line ~757) turns that into `QueryRejection::MalformedDate` -> `InvalidInputException`.
Tests: tests/all/forecastquery.rs lines 567-576 PIN the divergent behavior — `("2015-01-01T08:00", "InvalidInputException")` and `("2015-01-01T08:00:00Z", "InvalidInputException")` — so the current test asserts the refusal rather than the accepted window.

## Steps to Reproduce

1. Seed a forecast in the forecastquery store (as tests/all/forecastquery.rs does) with series item_id=client_21 and points at 2015-01-01T00:00:00 .. 2015-01-06T00:00:00.
2. Call QueryForecast with {ForecastArn: <that ARN>, Filters: {"item_id": "client_21"}, StartDate: "2015-01-02T00:00:00Z"} — emulator answers 400 InvalidInputException ("is not a valid date") instead of 200 with the points from 2015-01-02 onward.
3. Repeat with StartDate "2015-01-02T00:00" (minute precision), "2015-01-02T00:00:00.000", "2015-01-02T00:00:00+00:00" and "2015-01-02t00:00:00" — all 400, although all satisfy the validation pattern the service enforces at its shape layer.
