Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: aa53e03837fde54d44dc5b021f911ec63548de2f

# Bootstrap the forecastquery (Amazon Forecast Runtime) emulator

_Filed: 2026-10-21T22:23:10Z_
_Operation: service-com-amazonaws-forecastquery-amazonforecastruntime | Class: stub_

> _Note: this issue was identified against the code base at commit `aa53e03837fde54d44dc5b021f911ec63548de2f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in `services/service-com-amazonaws-forecastquery-amazonforecastruntime/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented` while holding zero state. The crate binds only two operations, so the first slice is the whole data-plane read path plus the state it reads from.

## Happy path to implement (in order)

1. **Seed/ingest forecast state** (prerequisite, not an operation of this crate). This runtime service has no create operation: forecasts are produced by the control plane (`service-com-amazonaws-forecast-amazonforecast`: `CreateForecast` -> `ForecastArn`, `CreateWhatIfForecast` -> `WhatIfForecastArn`). The bootstrap must make queryable forecast data reachable from `crate::state::service_state(&*self.engine)` — either by sharing the engine-keyed store the forecast control plane writes (preferred: the ARN minted by `CreateForecast` is exactly the ARN `QueryForecast` is called with), or, until that crate is implemented, by a deterministic materializer that derives and then PERSISTS a time series the first time an unknown-but-well-formed ARN is queried, so repeated queries for the same (ARN, filters, date range) return byte-identical predictions. Nothing may be generated fresh per request.
2. **QueryForecast** — input `ForecastArn` (required, len 0..256, pattern `^arn:([a-z\d-]+):forecast:.*:.*:.+$`), `Filters` (required, map<string,string>, 1..50 entries, e.g. `{"item_id":"client_21"}`), optional `StartDate`/`EndDate` (ISO 8601 `yyyy-MM-dd'T'HH:mm:ss`), optional `NextToken` (len 1..3000). Output `Forecast{Predictions: map<Statistic -> list<DataPoint{Timestamp, Value}>>}`. Must: look the ARN up in state and return `ResourceNotFoundException` (404) when absent; apply `Filters` against the stored series' dimension keys (unknown filter key / no matching series -> `InvalidInputException` 400 or an empty prediction set consistent with the stored data, not a fabricated series); clip the returned `DataPoint`s to `[StartDate, EndDate]` inclusive when supplied; return the forecast types the forecast was created with (default quantile keys `p10`/`p50`/`p90`).
3. **QueryForecast pagination** — when the filtered series exceeds the page size, return a `NextToken` and resume exactly where the previous page stopped, with no duplicated or skipped `DataPoint`. A token that was not minted by this service (or refers to a different ARN/filters) must fail with `InvalidNextTokenException` (400). A token must be a handle into persisted state, not an echo of the request.
4. **QueryWhatIfForecast** — input `WhatIfForecastArn` (required, len 0..300, same ARN pattern), `Filters` (required), `StartDate`/`EndDate`, `NextToken`; same `Forecast` output shape. Must resolve the what-if forecast ARN in state (404 `ResourceNotFoundException` when unknown) and return the what-if series, which must be observably DIFFERENT from the base forecast whenever the stored what-if transformations/replacements say so — not the base forecast echoed back.
5. **Negative path** — query a deleted/unknown ARN and get `ResourceNotFoundException` (404) with the awsJson1_1 error shape (`X-Amzn-Errortype` / `__type` plus `Message`); modeled constraints (ARN pattern, `Filters` 1..50, `NextToken` 1..3000) are refused with 400 before the handler runs.

Every response must be computed from persisted state. No canned `Forecast`, no echoing the request, no random values regenerated per call: two identical `QueryForecast` calls must return identical predictions, and a `StartDate`/`EndDate` narrowing must return a strict subset of the unfiltered call.

## State Model

In `src/state.rs` (reached only via `crate::state::service_state(&*self.engine)`, under its lock):

- `forecasts: HashMap<String /* ForecastArn */, StoredForecast>`
- `what_if_forecasts: HashMap<String /* WhatIfForecastArn */, StoredForecast>` (each carrying `base_forecast_arn: Option<String>`)
- `StoredForecast { arn: String, name: String, forecast_types: Vec<String> /* e.g. ["p10","p50","p90"], 1..5 */, frequency: String, series: Vec<StoredSeries>, created_at: SystemTime, base_forecast_arn: Option<String> }`
- `StoredSeries { dimensions: BTreeMap<String, String> /* item_id + forecast dimensions — what Filters matches on */, points: BTreeMap<String /* statistic */, Vec<StoredPoint>> }`
- `StoredPoint { timestamp: String /* ISO 8601, sorted ascending */, value: f64 }`
- `query_tokens: HashMap<String /* NextToken */, QueryCursor>` with `QueryCursor { arn: String, filters: BTreeMap<String,String>, start_date: Option<String>, end_date: Option<String>, offset: usize, expires_at: SystemTime /* 24h per the model's NextToken doc */ }`

`QueryForecast`/`QueryWhatIfForecast` select the `StoredSeries` whose `dimensions` superset-match `Filters`, slice `points` by date range and cursor offset, and project into `Forecast.Predictions`.

Done = an integration test can seed (or create via the control plane) a forecast, query it by `item_id`, assert the returned `Predictions` keys and `DataPoint` values match what was stored, page through with `NextToken` to completion without gaps, query the what-if ARN and see the transformed series, and get a 404 for an unknown ARN.

Operations beyond this slice: there are none in this crate — the long tail here is the forecast control plane, filed separately.

## Evidence

`services/service-com-amazonaws-forecastquery-amazonforecastruntime/OPERATIONS.md` lists exactly 2 operations (`QueryForecast`, `QueryWhatIfForecast`), protocol `awsJson1_1`, `X-Amz-Target: AmazonForecastRuntime.<Op>`, with the input members, the `Forecast` output and the error set (`InvalidInputException` 400, `InvalidNextTokenException` 400, `LimitExceededException` 409, `ResourceInUseException` 409, `ResourceNotFoundException` 404) quoted above. `src/ops/query_forecast.rs` lines 4-6 carry `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 179 returns `Err(crate::scaffolded::Scaffolded::not_implemented("QueryForecast"))`; `src/ops/query_what_if_forecast.rs` is the same scaffold — the only two op files in the crate, so the service holds no state at all. The `Forecast` shape in the Smithy model is `Predictions: map<Statistic -> TimeSeries>` with `DataPoint{Timestamp, Value:double}` and documents the quantile keys `p10`/`p50`/`p90` (default `["0.1","0.5","0.9"]`, 1..5 types). The `NextToken` member doc states tokens are returned on truncation and expire after 24 hours, grounding the pagination and `InvalidNextTokenException` requirements. ARN provenance is grounded in the sibling crate `services/service-com-amazonaws-forecast-amazonforecast/OPERATIONS.md` (`CreateForecast` -> `ForecastArn`, `CreateWhatIfForecast` -> `WhatIfForecastArn`). No tests exist beyond the generated `tests/state_roundtrip.rs`.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `forecastquery` client at it (endpoint prefix `forecastquery`, sigv4 signing name `forecast`).
2. Seed a forecast into state (via the forecast control plane `CreateForecast`, or the bootstrap's persisted materializer) so a known `ForecastArn` with an `item_id=client_21` series exists.
3. Call QueryForecast with that `ForecastArn` and `Filters={"item_id":"client_21"}` — expect 200 with `Forecast.Predictions` containing `p10`/`p50`/`p90` lists of `{Timestamp, Value}`; today it returns HTTP 501 NotImplemented.
4. Call QueryForecast again with the same arguments and assert the predictions are byte-identical to the first call (state-derived, not regenerated).
5. Call QueryForecast with `StartDate`/`EndDate` narrowing the window and assert the returned DataPoints are a date-clipped subset of step 3's result.
6. If the series is longer than one page, follow the returned `NextToken` until exhausted and assert the concatenated DataPoints equal the full series with no duplicates or gaps; then pass a bogus `NextToken` and expect `InvalidNextTokenException` (400).
7. Call QueryWhatIfForecast with a seeded `WhatIfForecastArn` and the same `Filters` — expect 200 with the what-if series, differing from the base forecast where the stored transformations apply.
8. Call QueryForecast with a well-formed but unknown ARN — expect `ResourceNotFoundException` (HTTP 404) in awsJson1_1 error shape, not 501 and not 200.
