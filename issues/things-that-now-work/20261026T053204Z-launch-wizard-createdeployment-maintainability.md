Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 465e7599899ea277984bee5e564f3564a11f5f81

# The "N validation errors detected" report sentence is rendered twice in state.rs, so the two copies can drift

_Filed: 2026-10-26T05:32:04Z_
_Operation: CreateDeployment | Class: maintainability_

> _Note: this issue was identified against the code base at commit `465e7599899ea277984bee5e564f3564a11f5f81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate words every refusal message with the same AWS report shape — how many clauses failed, then the clauses joined with "; " — and it builds that sentence in TWO places in `src/state.rs`, each with its own count-and-pluralise logic:

* `validation_report` (the renderer behind every constraint refusal: `map_key_refusal` -> `specification_key_refusal` / `tag_key_refusal`, `constraint_refusal`, `filter_pair_refusal`) formats `"{count} validation {noun} detected: {clauses}"` with its own `if count == 1 { "error" } else { "errors" }`.
* `counted_clauses` (the renderer behind `specification_create_refusal` and `specification_warning_refusal`) formats the identical `"{} validation {plural} detected: {}"` with its own "append an s" pluralisation.

`counted_clauses`'s own doc admits the duplication — it says it is "the shape [`validation_report`] states a refusal in, because a caller reads both the same way" — yet it restates that shape instead of calling it. Because the sentence and its pluralisation are written out twice, a change to one copy (the noun/plural rule, the separator, the "N validation X detected: " prefix) silently stops matching the other, and this service then answers two different report shapes for two refusals a caller reads the same way.

The two copies are textually interchangeable for the noun `validation_report` uses: `counted_clauses("error", &clauses)` produces exactly `"{count} validation error(s) detected: {clauses joined with \"; \"}"`, i.e. the same bytes `validation_report` produces today. Expressing the sentence once therefore leaves every response and every existing test unchanged; only the second implementation of the rule goes away.

## Evidence

Copy 1 — `services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:2347-2349` (inside `pub fn validation_report`):
```
    let count = clauses.len();
    let noun = if count == 1 { "error" } else { "errors" };
    format!("{count} validation {noun} detected: {}", clauses.join("; "))
```

Copy 2 — `services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs:2180-2194`:
```
/// How many clauses, then the clauses — the shape [`validation_report`] states a
/// refusal in, because a caller reads both the same way. `noun` is pluralised by
/// appending an `s`, which is what both callers' nouns take.
fn counted_clauses(noun: &str, clauses: &[String]) -> String {
    let plural = if clauses.len() == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    };
    format!(
        "{} validation {plural} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}
```

Both copies are reached from this service's handlers: `src/ops/create_deployment.rs:179-193` calls `crate::state::specification_key_refusal(..)` / `tag_key_refusal(..)` (which word their message through `validation_report`, state.rs:2003-2015), and `src/ops/create_deployment.rs:266-274` calls `crate::state::specification_create_refusal(&warnings)` (which words its message through `counted_clauses`, state.rs:2236-2242). `src/ops/update_deployment.rs:291-298` reaches the same pair through `specification_warning_refusal`.

Test coverage that must stay green and unchanged (it pins the exact rendered sentence of both copies):
* `tests/all/validation_messages.rs:181` — `expected: "1 validation error detected: Value null at 'deploymentId' failed to …"` (copy 1), plus :212 and :234;
* `tests/all/list_deployments_filter_pairs.rs:91-98` rebuilds the same sentence (`"{} validation {noun} detected: {}"`) and asserts it, and :245 asserts `format!("{MAX_FILTER_PAIR_CLAUSES} validation errors detected: ")` (copy 1);
* `tests/all/tag_key_rule_single_source.rs:96` splits a refusal on `" detected: "` (copy 1);
* `tests/all/deployment_update.rs` and `tests/all/create_deployment_validation.rs` exercise the specification warning/error refusals (copy 2).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-launchwizard-launchwizard/src/state.rs` and read `counted_clauses` (lines 2183-2194) and the tail of `validation_report` (lines 2347-2349): both format the same `"<count> validation <noun(s)> detected: <clauses joined with '; '>"` sentence, each with its own singular/plural branch.
2. Note that `counted_clauses`'s doc comment (state.rs:2180-2182) states it is reproducing `validation_report`'s shape rather than calling it.
3. Change the sentence in one copy only (e.g. the separator or the prefix wording) and run the suite: `tests/all/validation_messages.rs` / `tests/all/list_deployments_filter_pairs.rs` still pass against the other copy, so this service now answers two different report shapes — the drift the duplication allows.
4. Confirm the dedup is output-identical: `counted_clauses("error", &clauses)` renders exactly what `validation_report`'s own `format!` renders for the same clause list, so expressing the rule once leaves every response and every existing test byte-for-byte unchanged.
