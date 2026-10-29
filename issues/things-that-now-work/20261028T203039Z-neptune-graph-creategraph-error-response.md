Issue-Severity: LOW
Issue-Gap-Class: error-response
Issue-Commit: 0f071a90384ece90fe46e5865745f415e3851c94

# Only the FIRST handler-enforced constraint violation is reported, so the refusal count differs from the boundary's and from the sibling handler that checks the same two members

_Filed: 2026-10-28T20:30:39Z_
_Operation: CreateGraph | Class: error-response_

> _Note: this issue was identified against the code base at commit `0f071a90384ece90fe46e5865745f415e3851c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateGraph enforces two constraints the boundary validator cannot check — the `graphName` pattern (its leading negative lookahead does not compile) and the stored tag bound — and returns on the first one it finds, so a request that breaks BOTH is answered "1 validation error detected: ..." naming only `graphName`. The framework's refusal renderer this handler claims to be indistinguishable from reports EVERY violation in one envelope, and the sibling handler that enforces exactly the same two checks (RestoreGraphFromSnapshot) accumulates them into one multi-clause envelope. So the same two broken members are reported as 1 error by CreateGraph and as 2 errors by RestoreGraphFromSnapshot, and a caller can tell which side of the boundary caught the violation — the thing the comment above the check says it cannot. CreateGraphSnapshot, StartImportTask and StartExportTask short-circuit the same way.

## Evidence

The rule the handler states about itself — src/ops/create_graph.rs:203-205: "The refusal is worded by the same builder the boundary refusal renderer uses (`crate::service`), so a caller cannot tell which side of the boundary caught the violation." — followed by src/ops/create_graph.rs:206-216 (`return Err(...)` on the graphName clause alone) and src/ops/create_graph.rs:224-232 (a second, independent `return Err(...)` for `bound_tags`), so the second clause is unreachable whenever the first fires.

The boundary renderer it claims parity with accumulates — src/service.rs:116-125:
```
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
```
and the envelope itself counts the clauses it is given — src/state.rs:3146-3155: `[only] => format!("1 validation error detected: {only}")` / `many => format!("{} validation errors detected: {}", many.len(), many.join("; "))`.

The sibling with the SAME two checks does accumulate — src/ops/restore_graph_from_snapshot.rs:195-233: "Every refusal below is a constraint the BOUNDARY validator cannot check, so this is where it is enforced; each is worded by the same builders in `crate::state` the boundary refusal renderer uses, so a caller cannot tell which side of the boundary caught the violation." with `let mut violations: Vec<String> = Vec::new();`, `violations.push(...)` for the graphName clause (line 208-214) and for `bound_tags` (line 219-223), then one `validation_envelope(&violations)` (line 228). src/ops/create_graph_using_import_task.rs:248-321 does the same for five checks.

Test coverage: tests/all/error_message_rendering.rs:215-269 pins single-violation messages only ("1 validation error detected: Value 'g-reserved' at 'graphName' ..."); no test sends a request that breaks two handler-checked constraints at once, so accumulating them changes no existing assertion.

## Steps to Reproduce

1. Boot the emulator and point a neptune-graph client at it.
2. Call CreateGraph with graphName='g-reserved' (breaks the handler-enforced `^(?!g-)...` pattern) AND tags containing one key of 200 characters (breaks the handler-enforced tag bound).
3. Observe 400 ValidationException whose message is "1 validation error detected: Value 'g-reserved' at 'graphName' failed to satisfy constraint: ..." — the tag violation is not reported at all.
4. Create a snapshot and call RestoreGraphFromSnapshot with the same graphName and the same tags map.
5. Observe 400 ValidationException whose message is "2 validation errors detected: ..." — the same two broken members, reported twice over, from the same envelope builder.
