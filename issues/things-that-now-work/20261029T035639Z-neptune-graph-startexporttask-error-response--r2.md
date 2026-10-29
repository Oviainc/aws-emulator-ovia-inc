Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 08a55bcc278a7e8a745e318c09a635676933476e

# StartExportTask reports a full task store as a 400 CONSTRAINT_VIOLATION while every sibling path in the crate reports the identical condition as a 409 ConflictException

_Filed: 2026-10-29T03:56:39Z_
_Operation: StartExportTask | Class: error-response_

> _Note: this issue was identified against the code base at commit `08a55bcc278a7e8a745e318c09a635676933476e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Reaching an emulator store ceiling is one condition, and this crate states the rule for how to report it in three places: an operation that models no `ServiceQuotaExceededException` reports it with the `ConflictException` it does model, because the request conflicts with what the store already holds. `StartImportTask` does that for `MAX_IMPORT_TASKS`, and `crate::state::mint_final_snapshot` does that for `MAX_SNAPSHOTS` on behalf of `DeleteGraph` and `ResetGraph`.

`StartExportTask` is the outlier: reaching `MAX_EXPORT_TASKS` is answered with a 400 `ValidationException` carrying `reason: CONSTRAINT_VIOLATION`, even though `StartExportTask` models EXACTLY the same error set as `StartImportTask` (ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400), so the conflict channel the sibling uses is equally available to it.

The consequence for a caller is not cosmetic: a request whose `graphIdentifier`, `roleArn`, `format`, `destination`, `kmsKeyIdentifier` and `tags` are all valid and all accepted by the handler's own earlier checks is told its INPUT violated a constraint, and classified with a modelled `reason` value (`CONSTRAINT_VIOLATION`) that names something wrong with the request. A client that retries on 409 and treats 400/CONSTRAINT_VIOLATION as a permanent bad-input bug diagnoses a full store as a malformed request. The message is also a bare sentence rather than the `crate::state::validation_envelope` + `constraint_clause` wording every other `ValidationException` this crate renders uses, because there is no member path or rejected value to name.

Note this is not a claim about the ceiling VALUE (`MAX_EXPORT_TASKS`) — only about which modelled error the crate uses to report reaching it, where the crate states one rule three times and breaks it once.

## Evidence

The outlier — `services/service-com-amazonaws-neptunegraph-amazonneptunegraph/src/ops/start_export_task.rs:285-301`:
```
        // The store is shared and grows by one task per call, and a finished task stays
        // readable, so it has a ceiling (`crate::state::MAX_EXPORT_TASKS`). This
        // operation models no quota error, and throttling would promise that a retry can
        // succeed when nothing frees an entry, so the bound is reported as the
        // validation failure that names it.
        if store.export_tasks.len() >= crate::state::MAX_EXPORT_TASKS {
            return Err(StartExportTaskErrorModeled::ValidationException(
                ValidationException {
                    message: format!(
                        "this emulator holds at most {} export tasks",
                        crate::state::MAX_EXPORT_TASKS
                    ),
                    reason: Some("CONSTRAINT_VIOLATION".to_string()),
                },
            )
            .into());
        }
```

The sibling, same condition, same modelled error set — `src/ops/start_import_task.rs:284-300`:
```
        // The store is shared and grows by one task per call, and a task is never
        // reclaimed, so it has a ceiling (`crate::state::MAX_IMPORT_TASKS`). This
        // operation models no quota error, so reaching the ceiling is reported with the
        // `ConflictException` it does model: the request conflicts with what the store
        // already holds.
        if store.import_tasks.len() >= crate::state::MAX_IMPORT_TASKS {
            return Err(StartImportTaskErrorModeled::ConflictException(
                ConflictException {
                    message: format!(
                        "this emulator holds at most {} import tasks",
                        crate::state::MAX_IMPORT_TASKS
                    ),
                    reason: None,
                },
            )
            .into());
        }
```

The same rule a third time, in the shared state module — `src/state.rs:1520-1532` (`mint_final_snapshot`, reached by `DeleteGraph` and `ResetGraph`):
```
        // The snapshot store grows by one entry per final snapshot, so it has the same
        // ceiling `CreateGraphSnapshot` enforces. Neither operation that takes a final
        // snapshot models a quota error, so a store with no room for the snapshot the
        // caller asked for is reported as the conflict it is: the request cannot be
        // carried out against what the store already holds.
        if self.snapshots.len() >= MAX_SNAPSHOTS {
```
and the two handlers lift that `Err` into `ConflictException`: `src/ops/delete_graph.rs:226-235` and `src/ops/reset_graph.rs:219-228`.

The modelled error sets are identical, so the channel is available: `smithy_query(query="operation", operation="StartExportTask,StartImportTask")` gives both `[ConflictException 409, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400]`.

Test coverage: `tests/all/export_task_lifecycle.rs`, `tests/all/export_task_listing.rs` and `tests/all/task_namespace_refusals.rs` exercise StartExportTask, and `tests/all/precondition_refusal_grammar.rs` pins the refusal grammar — none of them fills the store to `MAX_EXPORT_TASKS`, so nothing pins the 400 this handler answers.

## Steps to Reproduce

1. Create a graph (POST /graphs with graphName `export-src`, provisionedMemory 16) and wait out PROVISIONING_SPAN so GetGraph reports AVAILABLE.
2. Call StartExportTask (POST /exporttasks) repeatedly against that graph with a same-account roleArn, format CSV, destination `s3://bucket/prefix` and a valid kmsKeyIdentifier, until `crate::state::MAX_EXPORT_TASKS` tasks are stored.
3. Call StartExportTask once more with the identical, fully valid body — observe HTTP 400 `ValidationException` with `reason: "CONSTRAINT_VIOLATION"` and a bare message, although no input member is invalid.
4. Repeat the same experiment with StartImportTask (POST /graphs/{graphIdentifier}/importtasks) past `MAX_IMPORT_TASKS` — observe HTTP 409 `ConflictException` for the identical class of condition, from an operation that models exactly the same errors.
