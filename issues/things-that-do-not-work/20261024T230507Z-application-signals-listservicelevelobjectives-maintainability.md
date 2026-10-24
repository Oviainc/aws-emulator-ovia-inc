Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8839c6cff18de024fc6b68ddfb2d1b408a6dd709

# ListServiceLevelObjectives re-implements the modelled maxResults range refusal the crate's own refusal renderer already words

_Filed: 2026-10-24T23:05:07Z_
_Operation: ListServiceLevelObjectives | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8839c6cff18de024fc6b68ddfb2d1b408a6dd709`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListServiceLevelObjectives` enforces `MaxResults`'s modelled `range 1..50` inside the handler, rebuilding the refusal message out of `validation_envelope`/`constraint_clause` and restating the bound `MAX_SLO_PAGE`. The crate already enforces and words that rule once, for every operation: the centralized walk validates before `handle` runs, and `crate::service::constraint_clause` renders a `MinValue`/`MaxValue` violation through the same two builders with the same sentence. Every sibling list in this crate relies on that and says so.

So the same rule — the bound, the member path `maxResults` and the requirement sentence — exists twice in this crate's code, and the handler copy is unreachable: the two can drift (a model bound change updates the renderer's copy but not `MAX_SLO_PAGE`) while producing byte-identical responses today. Deleting the handler's two range branches (keeping the `None => MAX_SLO_PAGE` default, which is this service's own choice for an absent member) leaves every response and every existing test unchanged.

## Evidence

The handler's copy:
- `src/ops/list_service_level_objectives.rs:189` `Some(max_results) if max_results < 1 => {` … `:191` `crate::state::validation_envelope(&[crate::state::constraint_clause(` / `:192` `    "maxResults",` / `:193` `    "Member must have value greater than or equal to 1",` and `:197`-`:206` the same for the maximum, formatting `crate::state::MAX_SLO_PAGE`.

The crate's single renderer for the same rule, reached before any handler:
- `src/service.rs:150` `ViolationKind::MinValue(min) => {` / `:151` `    format!("Member must have value greater than or equal to {min}")` / `:153`-`:155` the `MaxValue` arm, both finished by `:169` `Some(crate::state::constraint_clause(&violation.path, &requirement))`.
- Walk order, stated in every handler header, e.g. `src/ops/create_service_level_objective.rs:10` "drives it through the centralized request walk:" / `:11` "deserialize → validate → `handle` → serialize. You write ONLY the `handle` body."

The siblings that rely on it (and prove the framework really refuses it):
- `src/ops/list_services.rs:206` "// `MaxResults` carries the model's own `@default` of 50 and its `range 1..100` is" / `:207` "// refused before this handler runs, so the bound is read straight off the input." — and `tests/all/list_services.rs:455`-`:465` asserts HTTP 400 for `MaxResults=0` and `101` although that handler contains no range check.
- `src/ops/list_audit_findings.rs:206` "// member documents. A value outside the modelled `range 1..10` is already refused" / `:207` "// before this handler runs."

Tests that cover the behaviour and must stay green: `tests/all/error_identity.rs:328`-`:348` (the two `maxResults` message cases for `/slos`) and `tests/all/slo_lifecycle.rs:711`-`:722` (`MaxResults` in [0, -1, 51, 1000] → 400).

## Steps to Reproduce

1. Read src/ops/list_service_level_objectives.rs:187-209: the handler refuses maxResults < 1 and > MAX_SLO_PAGE, wording the refusal with validation_envelope/constraint_clause.
2. Read src/service.rs:138-170: the same crate renders a ViolationKind::MinValue/MaxValue refusal with the identical sentence through the identical builders, and src/ops/create_service_level_objective.rs:10-11 states validation runs before handle.
3. Read src/ops/list_services.rs:206-212 plus tests/all/list_services.rs:455-465: a sibling list with no handler-side range check still answers 400 for MaxResults=0 and 101, so the handler branches in step 1 are an unreachable second copy of one rule.
4. Delete the two range branches in step 1 and re-run tests/all/error_identity.rs and tests/all/slo_lifecycle.rs: responses and assertions are unchanged.
