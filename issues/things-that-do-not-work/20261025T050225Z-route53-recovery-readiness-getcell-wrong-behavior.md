Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 67bdaa95f371a43447a945b4dd8bda66772a6b0f

# ParentReadinessScopes can be reported with more than one element, which the model says it cannot have

_Filed: 2026-10-25T05:02:25Z_
_Operation: GetCell | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `67bdaa95f371a43447a945b4dd8bda66772a6b0f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`parent_readiness_scopes` concatenates EVERY stored cell that nests the target cell with EVERY stored recovery group that lists it, with no bound on the result, and the four handlers that report a cell (`GetCell`, `ListCells`, `CreateCell`, `UpdateCell`) put that vector straight into the `parentReadinessScopes` member. The model states that this member "is a list but currently can have only one element". Nothing in the crate refuses the containment configurations that produce two or more: a cell named as a nested cell by two different cells, or a cell that is both nested in a cell and listed by a recovery group. The emulator then reports a `parentReadinessScopes` array of length 2+ on a 200 response, a shape of answer the model says the API does not produce, so a client that reads the member as "the one parent scope" (e.g. takes element 0 as THE scope) silently loses half the answer. The handler's own helper doc quotes the first half of that very model sentence (the two kinds of scope allowed) while the code below it violates the second half (the cardinality).

## Evidence

Smithy model, verbatim: `smithy_query(query="doc", shape="CellOutput", member="ParentReadinessScopes")` = "<p>The readiness scope for the cell, which can be a cell Amazon Resource Name (ARN) or a recovery group ARN. This is a list but currently can have only one element.</p>" (the identical sentence is on `CreateCellResponse`/`GetCellResponse`/`UpdateCellResponse`, see OPERATIONS.md:107 and src/ops/create_cell.rs:67-69: "`ParentReadinessScopes` (__listOf__string) — The readiness scope for the cell, which can be a cell Amazon Resource Name (ARN) or a recovery group ARN. This is a list but currently can have only one element.").

Code, src/state.rs:490-507:
```
pub fn parent_readiness_scopes(&self, cell_arn: &str) -> Vec<String> {
    let nesting_cells = self.cells.values().filter(|candidate| { ... }).map(|parent| parent.arn.clone());
    let owning_groups = self.recovery_groups.values().filter(|group| group.cell_arns.iter().any(|cell| cell == cell_arn)).map(|group| group.arn.clone());
    nesting_cells.chain(owning_groups).collect()
}
```
No element count is enforced anywhere — `chain(...).collect()` returns as many parents as the store holds.

The same helper's doc comment quotes only the first half of the sentence it then exceeds, src/state.rs:482-486: "the two kinds of scope the member's own documentation allows (\"a cell Amazon Resource Name (ARN) or a recovery group ARN\")".

Reported verbatim by: src/ops/get_cell.rs:169 `parent_readiness_scopes: Some(state.parent_readiness_scopes(&record.arn))`; src/ops/list_cells.rs:168 `record.to_cell_output(state.parent_readiness_scopes(&record.arn))`; src/ops/create_cell.rs:234-239; src/ops/update_cell.rs:183-188.

No comment, doc or test in the crate justifies a 2+ element answer: every existing assertion pins exactly zero or one element — tests/all/cell_lifecycle.rs:336-341 (`Vec::<String>::new()` for an uncontained cell) and :398-411 (`vec![parent_arn.clone()]`), tests/all/account_gate_and_not_found.rs:424-453 (`vec![arn("cell", "parent_cell")]`, `vec![arn("recovery-group", "group_1")]`, `vec![]`). The two-parent configuration is untested and unguarded.

## Steps to Reproduce

1. Start the emulator with ROUTE53_RECOVERY_READINESS_ACCOUNT_ELIGIBLE=1
2. POST /cells {"cellName":"child_cell"} -> 200, note cellArn A = arn:aws:route53-recovery-readiness::<acct>:cell/child_cell
3. POST /cells {"cellName":"parent_one","cells":["A"]} -> 200
4. POST /cells {"cellName":"parent_two","cells":["A"]} -> 200 (alternatively POST /recoverygroups {"recoveryGroupName":"group_one","cells":["A"]})
5. GET /cells/child_cell -> 200 with "parentReadinessScopes":["...cell/parent_one","...cell/parent_two"] — two elements, where the model states the list "currently can have only one element"
6. GET /cells (and PUT /cells/child_cell) report the same two-element list
