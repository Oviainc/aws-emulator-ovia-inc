Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: faf4f30e21f0f3c9d0f7cbf10e9cf177e369ddf1

# An INCLUDE filter parameter bound to an empty filterValues value is silently dropped instead of refused

_Filed: 2026-10-25T18:13:48Z_
_Operation: GetRecommendations | Class: validation_

> _Note: this issue was identified against the code base at commit `faf4f30e21f0f3c9d0f7cbf10e9cf177e369ddf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`state::filter::bind` is the one place all three runtime operations bind a stored filter expression to a request's `filterValues`, and its own doc states the documented asymmetry it implements: an unbound parameter is an ERROR for an `INCLUDE` expression, and only an `EXCLUDE` clause is dropped. The `Predicate::In` branch implements exactly that. The `Predicate::Compare` branch does not: when `resolve()` returns a value list with no entries it executes `continue` for BOTH kinds, dropping the clause from an `INCLUDE` expression too.

`resolve()` returns an EMPTY list (not `None`) whenever the request supplies the parameter with a value that `split_values()` reads as naming no value at all — the empty string, a whitespace-only string, or a string of bare separators (`","`). `FilterAttributeValue` is modelled `length 0..1000`, so an empty value passes the framework and reaches the handler.

Three different, mutually inconsistent outcomes follow from that one input class, none of them the documented refusal:

1. `INCLUDE ItemID WHERE Items.YEAR >= $YEAR` with `filterValues {"YEAR": ""}` — the only clause is dropped, so `BoundFilter.clauses` is empty, `matches()` returns `false` for every entry and `retains()` (kind `Include`) removes the whole catalog. The operation answers HTTP 200 with an empty `itemList` and a fresh `recommendationId` instead of the documented `InvalidInputException`. The `matches()` doc asserts this state is "reachable only for an EXCLUDE expression", which is false.
2. `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE) AND Items.YEAR >= $YEAR` with `filterValues {"GENRE": "Comedy", "YEAR": ""}` — the YEAR clause disappears and the response carries Comedy items of ANY year, i.e. the INCLUDE criteria the caller asked for are silently weakened and items the filter excludes are returned as recommendations.
3. The `In` branch treats the same empty value as an empty value SET rather than as an unbound parameter, so `EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)` with `{"GENRE": ""}` makes `matched != negated` hold for every entry and removes the entire catalog — the opposite of the "doesn't use that portion of the expression" behaviour the same function's doc quotes for EXCLUDE.

The defect is one rule ("a parameter the request did not usefully bind") taking three different paths out of `bind`, two of which contradict the function's own stated contract and turn a refusal into a wrong 200.

## Evidence

Shared helper, stated contract — `services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state/filter.rs:516-520`:
```
/// Follows the documented asymmetry between the two filter kinds: for an `INCLUDE`
/// expression "you must provide values for all parameters that are defined in the
/// expression", so an unbound parameter is an error; for an `EXCLUDE` expression "you
/// can omit the filter-values. In this case, Amazon Personalize doesn't use that
/// portion of the expression to filter recommendations", so the clause is dropped.
```
The `In` branch honours it — filter.rs:554-562:
```
                match unbound {
                    Some(name) if expression.kind == FilterKind::Include => {
                        return Err(format!(
                            "filterValues provides no value for the filter expression parameter \
                             ${name}, which the INCLUDE expression requires"
                        ))
                    }
                    // EXCLUDE: that portion of the expression is not used.
                    Some(_) => continue,
```
The `Compare` branch does not — filter.rs:569-573:
```
            Predicate::Compare { op, operand } => match resolve(operand, filter_values) {
                Some(resolved) => {
                    let Some(value) = resolved.into_iter().next() else {
                        continue;
                    };
```
Empty value list is reachable from a model-allowed request — filter.rs:609-616 (`resolve` returns `Some(split_values(raw))`) with filter.rs:636-681 (`split_values`: a value that is empty after trimming is never pushed, so `""`, `" "` and `","` all return an empty `Vec`); `smithy_query(query="shape", shape="FilterAttributeValue")` gives `"length": {"min": 0, "max": 1000}`, so the empty string passes framework validation.

The claim the dropped INCLUDE clause breaks — filter.rs:713-721:
```
    /// No clauses left to evaluate means nothing matches — reachable only for an
    /// `EXCLUDE` expression whose every clause the request left unbound, which the
    /// documentation says removes nothing.
    fn matches(&self, attributes: &BTreeMap<String, String>) -> bool {
        let mut clauses = self.clauses.iter();
        let Some((_, first)) = clauses.next() else {
            return false;
        };
```
Model sentence backing the refusal, `GetRecommendationsRequest$filterValues` doc: "For filter expressions that use an INCLUDE element to include items, you must provide values for all parameters that are defined in the expression."

Call sites in this crate's handlers: `src/ops/get_recommendations.rs:246-252` (request filter) and `src/ops/get_recommendations.rs:461-470` (promotion filter), both through `state.rs:473-486` `bind_filter`.

Test coverage: `tests/all/inference.rs:710-738` (`an_include_expression_with_an_unbound_placeholder_is_refused`) only covers `filterValues` ABSENT and `filterValues` binding a DIFFERENT parameter; `tests/all/filter_value_quoting.rs` and `tests/all/action_filter_value_forms.rs` only send non-empty values. No test sends an empty, whitespace-only or separator-only `filterValues` value, so every outcome above is unpinned.

## Steps to Reproduce

1. Register a USER_PERSONALIZATION campaign whose Items catalog carries a YEAR column (state seam `register_campaign`), and a filter `INCLUDE ItemID WHERE Items.YEAR >= $YEAR` (`register_filter`).
2. POST /recommendations with {"campaignArn": <campaign>, "userId": "user-1", "numResults": 10, "filterArn": <filter>, "filterValues": {"YEAR": ""}}.
3. Observed: HTTP 200 with an empty `itemList` and a fresh `recommendationId`. Expected per filter.rs:516-520 and the `filterValues` doc: 400 InvalidInputException naming the parameter the INCLUDE expression requires.
4. Register a filter `INCLUDE ItemID WHERE Items.GENRE IN ($GENRE) AND Items.YEAR >= $YEAR` and POST the same route with {"filterValues": {"GENRE": "Comedy", "YEAR": ""}}. Observed: Comedy items of every year are returned (the YEAR clause was dropped). Expected: the same refusal.
5. Register `EXCLUDE ItemID WHERE Items.GENRE NOT IN ($GENRE)` and POST with {"filterValues": {"GENRE": ""}}. Observed: HTTP 200 with an empty `itemList` (every entry removed), while the EXCLUDE rule the same function quotes says an unbound portion of the expression removes nothing.
