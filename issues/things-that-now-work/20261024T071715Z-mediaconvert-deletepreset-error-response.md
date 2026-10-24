Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# DeletePreset on an absent name answers 404 instead of the idempotent empty 202 the sibling deletes answer

_Filed: 2026-10-24T07:17:15Z_
_Operation: DeletePreset | Class: error-response_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

MediaConvert's control-plane deletes are idempotent on the resource name: a DELETE of a name that is no longer stored still answers the operation's empty success (202) even though a READ of the same name is a 404. This emulator applies that rule for DeleteQueue (state::delete_queue -> DeleteQueueOutcome::AlreadyAbsent -> empty 202) and for DeleteJobTemplate (ops/delete_job_template.rs, Entry::Vacant(_) => Ok(DeleteJobTemplateOutput {})), but NOT for DeletePreset.

Expected: `DELETE /2017-08-29/presets/{Name}` for a preset that was already deleted (or never existed) answers HTTP 202 with an empty body, leaving GetPreset's 404 unchanged, so a teardown path that runs twice does not have to special-case the repeat.

Actual: ops/delete_preset.rs maps the vacant map entry to NotFoundException (HTTP 404), so the second delete of a preset — the exact shape of a cleanup hook that also runs on the happy path — fails, while the identical repeat on a queue or a job template succeeds. One of the two behaviours in this crate must be wrong, and the recorded corpus pins the idempotent one.

## Evidence

Recorded AWS behaviour for the sibling operation is already in the backlog: issue 20261022T124827Z-mediaconvert-repeat-create-delete-cancel-rejected-instead-of (cause 4, trace mediaconvert-queue-lifecycle#teardown#14) records DeleteQueue on an already-absent name answering 202 with an empty body immediately after a 404 from GetQueue on that name — 'the delete is idempotent even though the read is not'. That issue's scope covered DeleteQueue only; DeletePreset was never in scope, so this is net-new. Spec: DeletePreset is DELETE /2017-08-29/presets/{Name} with success code 202 and an empty output (OPERATIONS.md / smithy_query operation=DeletePreset). Handler: services/.../src/ops/delete_preset.rs lines ~176-181 — `Entry::Vacant(_) => Err(... NotFoundException ...)`. The sibling handler ops/delete_job_template.rs even documents the rule it follows ('INFERRED, not observed: the recorded corpus shows the idempotent repeat for DeleteQueue ... This operation follows the same rule rather than keeping a one-operation exception to it') while the preset handler keeps exactly that exception. Test coverage: tests/all/preset_deletion.rs line ~293 asserts the 404 for names like "absent" — i.e. the current divergent behaviour is enshrined by a test and must be revised with the fix.

## Steps to Reproduce

1. Boot the emulator and point a botocore mediaconvert client at it.
2. create_preset(Name='p', Settings={'ContainerSettings': {'Container': 'MP4'}})
3. delete_preset(Name='p') -> expect HTTP 202 with an empty body (it is).
4. get_preset(Name='p') -> expect NotFoundException 404 (it is).
5. delete_preset(Name='p') again -> expect HTTP 202 with an empty body, the same answer DeleteQueue/DeleteJobTemplate give for an already-absent name; observe NotFoundException (HTTP 404).
