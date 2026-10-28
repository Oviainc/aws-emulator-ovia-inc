Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 75491f6dad17b9da11cd051dade27675220440ff

# A filter comparison against a non-numeric value silently matches nothing: INCLUDE returns an empty itemList and EXCLUDE removes nothing

_Filed: 2026-10-27T06:58:32Z_
_Operation: GetRecommendations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `75491f6dad17b9da11cd051dade27675220440ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The filter parser accepts a comparison clause whose right-hand operand is a string — its own documented grammar admits a quoted literal there — but the evaluator can only compare numbers, and answers `false` for every catalog entry when the operand does not parse as `f64`. Nothing refuses the expression at registration and nothing refuses the request, so the filter silently does the opposite of what it says:

* `INCLUDE ItemID WHERE Items.GENRE = "Comedy"` registers successfully (`register_filter` returns `Ok(true)`) and then makes every candidate fail the clause, so `GetRecommendations` answers HTTP 200 with an EMPTY `itemList` — the filter removed the whole catalog.
* `EXCLUDE ItemID WHERE Items.GENRE = "Comedy"` registers successfully and removes NOTHING: `matches()` is false for every entry, so `retains()` is true for all of them — exactly the "quietly filtering nothing" outcome `register_filter`'s own documentation says can never happen.
* The same hole is reachable from the REQUEST with a perfectly ordinary stored filter: `INCLUDE ItemID WHERE Items.YEAR >= $YEAR` with `filterValues: {"YEAR": "\"recent\""}` binds the parameter to `recent`, which does not parse, so the request gets a 200 with an empty list instead of a refusal — even though `filter::bind` is documented as the one place where a value a request fails to supply usably is reported.

The crate already made the opposite choice for the analogous case it did think about: a clause over the `Interactions` dataset is REFUSED at `parse` time precisely because "silently ignoring it would make the filter claim to filter when it does not". A non-numeric comparison is the same class of unapplicable clause, and it is neither refused at registration nor at inference — it is silently applied as "never true".

The defect is distinct from the already-fixed GetRecommendations score-normalisation issue (`20261024T002543Z-personalize-runtime-getrecommendations-wrong-behavior.md`), which is about what an item SCORES; this one is about WHICH items come back. It reaches all three operations through the shared `bind_filter`/`retains` path (`Items.*` for GetRecommendations and GetPersonalizedRanking, `Actions.*` for GetActionRecommendations, and a promotion's own filter).

## Evidence

The crate's stated rule — `src/state.rs:46-48`:
```
//! * A filter's expression is validated when the filter is REGISTERED, exactly as
//!   `CreateFilter` validates it, so a stored filter is always one the runtime can
//!   apply.
```
and `src/state.rs:765-767` (`register_filter`):
```
/// `Err` carries the parse error, so an expression the runtime could not apply is
/// refused where the filter is created — never at inference time, and never by
/// quietly filtering nothing.
```
and `src/state/filter.rs:439-441` (`parse`):
```
/// `Err` carries a message naming what was wrong, for the caller that registered the
/// filter: the real service validates the expression when the filter is CREATED, so a
/// stored filter is always one this evaluator can apply.
```
The same principle applied to the case the crate DID refuse — `src/state/filter.rs:29-31`:
```
//! [`parse`] accepts only those two datasets: an expression over `Interactions` would
//! have to be evaluated against interaction history this emulator does not store, and
//! silently ignoring it would make the filter claim to filter when it does not.
```
The grammar the module documents, which admits a quoted string on the right of a comparison — `src/state/filter.rs:16-18`:
```
//! predicate  := ["NOT"] "IN" "(" value { "," value } ")"
//!             | ("=" | "<>" | "<" | "<=" | ">" | ">=") value
//! value      := "$" parameter | '"' literal '"' | bare-literal
```
while the comparison is numeric-only by construction — `src/state/filter.rs:89` (`/// A numeric comparison a clause can carry.`) and `src/state/filter.rs:108-117` (`fn holds(self, left: f64, right: f64)`).

The parser accepting it anyway — `src/state/filter.rs:369-388`:
```
            Some(Token::Operator(operator)) => {
                self.at += 1;
                let op = match operator.as_str() { "=" | "==" => Comparison::Equal, ... };
                Ok(Predicate::Compare { op, operand: self.operand()? })
            }
```
with `src/state/filter.rs:327-328`: `Some(Token::Quoted(literal)) => Ok(Operand::Literal(literal)),` — so `Items.GENRE = "Comedy"` parses and `register_filter` stores it.

The evaluator turning it into "never true" — `src/state/filter.rs:771-778`:
```
        BoundPredicate::Compare { op, value } => {
            let Ok(right) = value.parse::<f64>() else {
                return false;
            };
```
which `retains` then inverts for an EXCLUDE filter — `src/state/filter.rs:723-729`:
```
    pub fn retains(&self, attributes: &BTreeMap<String, String>) -> bool {
        let matched = self.matches(attributes);
        match self.kind {
            FilterKind::Include => matched,
            FilterKind::Exclude => !matched,
```
and which the handler applies verbatim — `src/ops/get_recommendations.rs:246-252` (`guard.bind_filter(registry::present(input.filter_arn.as_deref()), Entity::Items, input.filter_values.as_ref())`) and `src/ops/get_recommendations.rs:297-301`:
```
        candidates.retain(|(index, _)| {
            request_filter
                .as_ref()
                .is_none_or(|filter| filter.retains(&resolved.items[*index].attributes))
        });
```

Test coverage leaves the case open: every comparison clause any suite registers is numeric — `tests/all/inference.rs:137` (`INCLUDE ItemID WHERE Items.YEAR >= 2010`), `tests/all/unbound_filter_values.rs:144` (`INCLUDE ItemID WHERE Items.YEAR >= $YEAR`), `tests/all/action_filter_value_forms.rs:136` (`INCLUDE ActionID WHERE Actions.VALUE >= $VALUE`) — and the comparison rows bind only numbers (`tests/all/action_filter_value_forms.rs:237-253`: `"a bare number in a comparison"`, `"a quoted number in a comparison"`, `"a bare number past every stored value"`). No test registers `=`/`<>` at all, and the registration-refusal table `tests/all/inference.rs:1657-1672` lists no non-numeric comparison among the expressions it expects to be refused.

## Steps to Reproduce

1. Register a USER_PERSONALIZATION campaign whose Items catalog carries a GENRE column via `registry::register_campaign` (as `tests/all/inference.rs:91-102` does).
2. Call `registry::register_filter(engine, arn_a, "INCLUDE ItemID WHERE Items.GENRE = \"Comedy\"")`. Observed: `Ok(true)` — the expression is stored, although `Comparison` is documented as numeric and the evaluator can never make this clause hold.
3. POST /recommendations with {"campaignArn": <campaign>, "userId": "user-1", "numResults": 10, "filterArn": arn_a}. Observed: HTTP 200 with an empty `itemList`. Expected per `src/state.rs:765-767` and `src/state/filter.rs:439-441`: the unapplicable expression is refused where the filter is created, so this request can never be quietly answered with an empty list.
4. Call `registry::register_filter(engine, arn_b, "EXCLUDE ItemID WHERE Items.GENRE = \"Comedy\"")` and POST the same request with filterArn=arn_b. Observed: the full ranking comes back, the Comedy items included — the filter removed nothing, which `register_filter`'s doc says never happens ("never by quietly filtering nothing").
5. Register `INCLUDE ItemID WHERE Items.YEAR >= $YEAR` and POST /recommendations with filterValues {"YEAR": "\"recent\""}. Observed: HTTP 200 with an empty `itemList` and no refusal, although `filter::bind` (src/state/filter.rs:505-507) is documented as the one place a parameter the request failed to bind usably is reported.
