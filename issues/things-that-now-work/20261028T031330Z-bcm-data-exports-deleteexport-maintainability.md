Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 8e0ac146ffee686dd3a46765dec581c593b08a30

# remove_executions_of re-implements the (ExportArn, ExecutionId) range walk that executions_of already is

_Filed: 2026-10-28T03:13:30Z_
_Operation: DeleteExport | Class: maintainability_

> _Note: this issue was identified against the code base at commit `8e0ac146ffee686dd3a46765dec581c593b08a30`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The store keys executions by `(export ARN, execution id)` and has one helper for "the executions of one export": `executions_of`, which does the `range(..)` + `take_while(arn == export_arn)` walk. `trim_executions_of` reaches the same set through that helper. `remove_executions_of` does not: it hand-rolls the identical `range((export_arn.to_string(), String::new())..)` + `take_while(|((arn, _), _)| arn == export_arn)` walk a second time. The two copies encode one rule — which keys belong to one export — so a change to the key layout or to the prefix walk has to be made twice, and the delete path can silently stop matching what the listing path matches. `remove_executions_of` can collect its doomed keys from `self.executions_of(export_arn)` (the borrow ends at the `collect`, exactly as in `trim_executions_of`), leaving every response and every existing test unchanged.

## Evidence

The helper — services/.../src/state.rs:321-329:
```
    pub fn executions_of<'a>(
        &'a self,
        export_arn: &'a str,
    ) -> impl Iterator<Item = &'a StoredExecution> + 'a {
        self.executions
            .range((export_arn.to_string(), String::new())..)
            .take_while(move |((arn, _), _)| arn == export_arn)
            .map(|(_, execution)| execution)
    }
```
One path uses it — services/.../src/state.rs:403-409 (`trim_executions_of`):
```
        if self.executions_of(export_arn).count() <= MAX_EXECUTIONS_PER_EXPORT {
            return;
        }
        let mut held: Vec<(SystemTime, String)> = self
            .executions_of(export_arn)
```
The other path duplicates it — services/.../src/state.rs:425-431 (`remove_executions_of`):
```
    pub fn remove_executions_of(&mut self, export_arn: &str) {
        let doomed: Vec<(String, String)> = self
            .executions
            .range((export_arn.to_string(), String::new())..)
            .take_while(|((arn, _), _)| arn == export_arn)
            .map(|(key, _)| key.clone())
            .collect();
```
Called from the delete handler — services/.../src/ops/delete_export.rs:175 `store.remove_executions_of(&input.export_arn);`.

Existing tests that pin the behaviour both copies must keep: services/.../tests/all/execution_listing.rs:332-362 ("Deleting an export takes its executions' listing with it": `DeleteExport` then `assert_eq!(body["Executions"], json!([]))`), and the listing/lookup paths that read `executions_of` in tests/all/execution_listing.rs and tests/all/execution_lookup.rs.

## Steps to Reproduce

1. Read services/.../src/state.rs:321-329 (`executions_of`) and note the `range` + `take_while` prefix walk over the `(export ARN, execution id)` key space.
2. Read services/.../src/state.rs:400-418 (`trim_executions_of`): it reaches that same set through `self.executions_of(export_arn)`.
3. Read services/.../src/state.rs:425-435 (`remove_executions_of`): the same walk is written out a second time instead of calling the helper.
4. Confirm the behaviour is identical today by running the suite that covers it: tests/all/execution_listing.rs (delete-takes-executions) and tests/all/execution_lookup.rs — a refactor to the helper must leave both unchanged.
