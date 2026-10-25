Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# Hoist the duplicated post-resolution decision branches (ACTIVE gate, unminted-NextToken refusal, rejection classification) into crate::state

_Filed: 2026-10-24T23:34:31Z_
_Operation: QueryForecast | Class: maintainability_

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state` is declared to be the single place where the rules both query operations must answer identically live, precisely so the two handlers cannot drift. Three post-resolution branches were nevertheless written out twice, once per handler, with no shared helper:

1. the ACTIVE gate — `if !forecast.is_active_at(SystemTime::now())` plus the `not_active_message` refusal;
2. the unminted-`NextToken` refusal — `if input.next_token.is_some()` plus the "returns the complete date range … so it returns no NextToken to resume from" refusal;
3. the `QueryRejection` -> modelled-error classification in the per-handler `fn reject`, whose 404-vs-400 branch is re-written in each file even though `QueryRejection::is_not_found()` already states the classification once in `state.rs`.

Each copy is a rule (which condition, in which order, mapped to which modelled error), not just a message string, so editing one handler — e.g. moving the token check ahead of the ACTIVE gate, or re-classifying a rejection — silently leaves the other operation on the old rule. That is exactly the drift `state.rs`'s own module doc says this module exists to prevent, and the drift would be invisible because no test compares the two handlers' decision order against each other.

Fix: express the post-resolution decision once in `crate::state` (e.g. a neutral outcome value computed from the `ResourceKind`, the stored forecast and the input, which each handler renders into its own `*ErrorModeled` variants), and let both handlers render it. No response changes, no test changes: both handlers already emit the same statuses, codes and message rules, and all of them are pinned by the existing suites.

## Evidence

The stated rule, services/.../src/state.rs:31-36: "Beside the stored shape, this module holds what the two query operations must answer IDENTICALLY, so that they cannot drift apart: which resource an ARN names ([`ResourceKind`]), how an ARN they cannot read and a resource they cannot find are worded …, and the shape-level validation that runs before either resolves anything" — and state.rs:843-845: "The handlers map each case onto the operation's own modelled error, so the mapping is stated once here and rendered per operation."

Copy 1a, src/ops/query_forecast.rs:254-263:
```
if !forecast.is_active_at(::std::time::SystemTime::now()) {
    return Err(QueryForecastErrorModeled::ResourceInUseException(
        ResourceInUseException {
            message: Some(
                crate::state::FORECAST.not_active_message(&input.forecast_arn),
```
Copy 1b, src/ops/query_what_if_forecast.rs:262-272:
```
if !forecast.is_active_at(::std::time::SystemTime::now()) {
    return Err(QueryWhatIfForecastErrorModeled::ResourceInUseException(
        ResourceInUseException {
            message: Some(
                crate::state::WHAT_IF_FORECAST
                    .not_active_message(&input.what_if_forecast_arn),
```
Copy 2a, src/ops/query_forecast.rs:273-275: `if input.next_token.is_some() { return Err(QueryForecastErrorModeled::InvalidNextTokenException(` … Copy 2b, src/ops/query_what_if_forecast.rs:282-284: `if input.next_token.is_some() { return Err(QueryWhatIfForecastErrorModeled::InvalidNextTokenException(`.

Copy 3a, src/ops/query_forecast.rs:317-324:
```
fn reject(rejection: &crate::state::QueryRejection) -> QueryForecastError {
    let message = Some(rejection.message());
    if rejection.is_not_found() {
        QueryForecastErrorModeled::ResourceNotFoundException(ResourceNotFoundException { message })
```
Copy 3b, src/ops/query_what_if_forecast.rs:325-332:
```
fn reject(rejection: &crate::state::QueryRejection) -> QueryWhatIfForecastError {
    let message = Some(rejection.message());
    if rejection.is_not_found() {
        QueryWhatIfForecastErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
```

Tests that pin the behaviour the fix must leave untouched (all run both operations through the real router): tests/all/forecastquery.rs:905 `a_next_token_this_service_never_minted_is_invalid`, tests/all/forecastquery.rs:1270 `resolving_the_resource_precedes_judging_a_next_token`, tests/all/forecastquery.rs:620 `filters_select_the_stored_series_by_its_dimensions` (the 400/404 classification of a `QueryRejection`), tests/all/arn_resource_kinds.rs:121 `an_arn_resolves_only_when_it_names_a_resource_of_the_queried_kind`, tests/all/resource_not_found_message.rs:235 `an_absent_resource_is_reported_as_no_resource_found_plus_the_submitted_arn`.

## Steps to Reproduce

1. Open services/service-com-amazonaws-forecastquery-amazonforecastruntime/src/ops/query_forecast.rs and src/ops/query_what_if_forecast.rs side by side.
2. Compare lines 254-285 of the first with lines 262-295 of the second: the ACTIVE gate and the NextToken refusal are the same two branches in the same order, written twice.
3. Compare the two `fn reject` definitions (query_forecast.rs:317-325 vs query_what_if_forecast.rs:325-336): the same `is_not_found()` -> 404 / otherwise 400 branch, written twice.
4. Read src/state.rs:31-36 and :843-845, which state that rules both operations must answer identically live in state.rs and that the rejection mapping is 'stated once here'.
5. Change one copy only (e.g. judge NextToken before the ACTIVE gate in query_forecast.rs) and run `cargo test -p service-com-amazonaws-forecastquery-amazonforecastruntime`: no test compares the two handlers' decision order, so the operations can diverge unnoticed.
