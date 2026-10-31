Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 564feb857476f3247a0693cb65aa2d44e3322d3a

# The pattern-anchor stripping corrupts ClientToken's alternation pattern, so the ValidationException advertises a regex the model does not spell

_Filed: 2026-10-24T22:01:03Z_
_Operation: CreateRescoreExecutionPlan | Class: error-response_

> _Note: this issue was identified against the code base at commit `564feb857476f3247a0693cb65aa2d44e3322d3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate words every constraint refusal itself (`service.rs`'s `override_refusal` -> `state::refusal_report`), under a rule the crate states explicitly: a `@pattern` requirement is quoted as the model's expression WITHOUT the `^` / `$` anchors, and that rule is declared to apply to "EVERY constraint class and every operation".

`state::unanchored` implements that by unconditionally stripping one leading `^` and one trailing `$` (with a single carve-out for a backslash-escaped `$`). That is only correct when the whole expression is one anchored alternative. `CreateRescoreExecutionPlan`'s `ClientToken` (`ClientTokenName`) is modelled with the ALTERNATION `^$|[\x00-\x7F]+$`: the `^` belongs to the first alternative (`^$`, the empty string) and the final `$` to the second (`[\x00-\x7F]+$`). Stripping both yields `$|[\x00-\x7F]+`, which is neither the model's expression nor that expression minus a surrounding anchor pair — it leaves a stray end-anchor as the first alternative and silently drops the end-anchor of the second, so the refusal advertises a requirement a caller cannot apply to fix the request.

The member is reachable: a `ClientToken` of 1..64 characters that ends in a non-ASCII character (e.g. `"tokenø"`) satisfies the modelled `@length` but violates the `@pattern`, so the boundary validator produces a `ViolationKind::Pattern("^$|[\\x00-\\x7F]+$")` that this code renders. No test in the crate covers `ClientToken`: `tests/all/control_plane_fidelity.rs`'s pattern-invariant table covers only `Name`, `Description` and `NextToken`, and its two assertions (`!quoted.starts_with('^')`, `!quoted.ends_with('$')`) are both satisfied by the corrupted `$|[\x00-\x7F]+`, so the suite cannot catch it.

## Evidence

Rule stated by the crate — services/.../src/state.rs:67-77:
```
// Two rules, both this service's own rendering of the standard AWS report, and
// both applied to EVERY constraint class and every operation (the boundary
// validator reaches them through `service.rs`'s `override_refusal`):
// ...
// * a `@pattern` requirement quotes the expression WITHOUT the `^` / `$` the model
//   spells it with, which is the expression the service advertises.
```
and services/.../src/state.rs:92-94:
```
/// A `@pattern` expression without the anchors a model may spell it with. A `$` escaped
/// by a backslash is a literal dollar sign inside the expression, not the end anchor, so
/// it stays.
```

Code that breaks it — services/.../src/state.rs:95-108:
```
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => { ... if backslashes % 2 == 0 { trimmed } else { body } }
        None => body,
    }
}
```

The model's expression — smithy_query query=shape shape=ClientTokenName:
`{"shape_id": "com.amazonaws.kendraranking#ClientTokenName", "type": "string", "length": {"min": 1, "max": 64}, "pattern": "^$|[\\x00-\\x7F]+$"}`
(and `CreateRescoreExecutionPlanRequest$ClientToken` targets it with `"idempotencyToken": true`).

Expected (the crate's own rule): the clause quotes the model's expression with only the anchors the model spells it with removed.
Actual: `unanchored("^$|[\\x00-\\x7F]+$")` returns `$|[\x00-\x7F]+`, so the body reads `1 validation error detected: Value at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: $|[\x00-\x7F]+` — a stray `$` alternative and a lost end anchor.

Test coverage of the rule, which does not reach this member — services/.../tests/all/control_plane_fidelity.rs:199-215 lists only `("Name", ...)`, `("Description", ...)`, `("NextToken", ...)`; the invariant it asserts (control_plane_fidelity.rs:231-239) passes on the corrupted string.

## Steps to Reproduce

1. Call CreateRescoreExecutionPlan with `{"Name": "fine", "ClientToken": "tokenø"}` (within ClientTokenName's modelled 1..64 length, outside its `^$|[\x00-\x7F]+$` pattern).
2. Observe HTTP 400 `ValidationException` whose message ends `Member must satisfy regular expression pattern: $|[\x00-\x7F]+`.
3. Compare with the model: `ClientTokenName`'s pattern is `^$|[\x00-\x7F]+$`; the `^` and the final `$` are not a surrounding anchor pair but belong to the two alternatives, so the advertised expression is not the model's expression minus its anchors.
