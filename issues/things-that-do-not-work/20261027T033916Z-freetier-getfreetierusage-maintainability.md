Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: ce187dd52ad3609a091254aaff4a955715a54b39

# Deduplicate the numeric-dimension decode between check_dimension and matches_dimension

_Filed: 2026-10-27T03:39:16Z_
_Operation: GetFreeTierUsage | Class: maintainability_

> _Note: this issue was identified against the code base at commit `ce187dd52ad3609a091254aaff4a955715a54b39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetFreeTierUsage` decodes one `Dimensions` comparison twice in this crate's own code: `check_dimension` decides the arity of `MatchOptions` and what counts as a numeric filter value, and then `matches_dimension` re-derives both from the same `DimensionValues` when it evaluates a record. The two copies encode ONE rule each — "MatchOptions holds exactly one option" and "a numeric dimension's filter value is `trim().parse::<f64>()`" — but they answer differently if they ever disagree: the checker refuses the request with a `ValidationException`, the evaluator silently reports `false` / skips the value, which would turn a refusal into a quietly empty page. The handler's own comments state the second copy is redundant ("[`check_dimension`] has already refused every comparison this cannot make"), so the duplication exists only so the evaluator can re-read what the checker already decided, and the copies can drift. The two-phase split itself (check before any record is read) is deliberate and documented and should stay; only the re-decoding needs to move, e.g. by having the checked phase hand the single option and the parsed magnitudes to the evaluating phase. Every response and every existing assertion stays byte-identical, because the checker guarantees the evaluator's fallbacks are unreachable today.

## Evidence

Copy 1 — the checker, `services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_free_tier_usage.rs:359`:
```rust
    let [option] = dimensions.match_options.as_slice() else {
        return Err(validation(format!(
            "filter.Dimensions.MatchOptions: exactly one match option is allowed, got {}",
            dimensions.match_options.len()
        )));
    };
```
and `:396`:
```rust
            for wanted in &dimensions.values {
                if wanted.trim().parse::<f64>().is_err() {
                    return Err(validation(format!(
                        "filter.Dimensions.Values: {wanted} is not a number, and {key} compares \
                         numbers"
                    )));
```

Copy 2 — the evaluator, same file `:442`:
```rust
fn matches_dimension(record: &crate::state::UsageRecord, dimensions: &DimensionValues) -> bool {
    let [option] = dimensions.match_options.as_slice() else {
        return false;
    };
```
and `:454`:
```rust
            dimensions.values.iter().any(|wanted| {
                wanted.trim().parse::<f64>().is_ok_and(|wanted| {
```

The crate itself records that the second copy re-derives what the first already settled — `get_free_tier_usage.rs:439`: "[`check_dimension`] has already refused every comparison this cannot make." and `:416`: "Total rather than fallible: every expression the service cannot evaluate was refused before any record was read."

Existing tests that cover both copies and must stay unchanged:
* `services/service-com-amazonaws-freetier-awsfreetierservice/tests/all/free_tier.rs:1024` `a_dimensions_filter_selects_exactly_the_matching_records` — rows `("USAGE_PERCENTAGE", "GREATER_THAN_OR_EQUAL", &["0"], true)` and `("USAGE_PERCENTAGE", "GREATER_THAN_OR_EQUAL", &["75"], true)` (lines 1054-1055) drive the evaluating copy.
* `.../tests/all/free_tier.rs:1140` `an_unevaluable_filter_is_refused` — rows "a non-numeric value on a numeric dimension", "two match options" and "no match option" (lines 1155-1165) drive the checking copy.

## Steps to Reproduce

1. Open services/service-com-amazonaws-freetier-awsfreetierservice/src/ops/get_free_tier_usage.rs and read `check_dimension` (lines 344-408) and `matches_dimension` (lines 441-465) side by side.
2. Observe that `dimensions.match_options.as_slice()` is destructured to a single `option` in both functions (lines 359 and 442), and that `wanted.trim().parse::<f64>()` appears in both (lines 397 and 455).
3. Note the divergent outcomes of the two copies for the same malformed input: the checker returns a `ValidationException`, the evaluator returns `false` (no error), so a change to one copy alone converts a refusal into a silent empty page.
4. Confirm no behavioural change is required: `an_unevaluable_filter_is_refused` (tests/all/free_tier.rs:1140) shows the checker already rejects every input that would reach the evaluator's fallbacks, so hoisting the decode leaves all responses and assertions identical.
