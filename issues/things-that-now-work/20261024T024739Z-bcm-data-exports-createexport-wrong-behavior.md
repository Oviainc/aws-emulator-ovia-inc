Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: ceffded050eaaeab66f25b25154c54549736906e

# Minted ExportArn omits the per-export unique id suffix the real service appends

_Filed: 2026-10-24T02:47:39Z_
_Operation: CreateExport | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ceffded050eaaeab66f25b25154c54549736906e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateExport` mints `arn:aws:bcm-data-exports:<region>:<account>:export/<Export.Name>` — a pure function of region and name. Real BCM Data Exports ARNs carry a server-generated unique id after the name (`export/<name>/<uuid>`), which is why deleting and recreating an export under the same name yields a NEW ARN on AWS.

Expected: the ARN returned by `CreateExport` (and echoed by `GetExport`/`ListExports`) carries a per-export generated id segment, and a recreate-after-delete of the same name produces a different ARN.
Actual: the ARN is name-derived, so the ARN of a deleted-and-recreated export is byte-identical to the old one, and any client that parses or correlates ARNs sees a shape the service does not emit.

## Evidence

The ARNs this repo's own suites label as RECORDED from AWS all carry a uuid segment after the export name: `tests/all/tagging_arn_validation.rs` line 46-53 ("the recorded cross-account ARN" = `...:export/isolated-test/00000000-0000-0000-0000-000000000000`) and `tests/all/not_found_message.rs` lines 80-86. The emulator mints without it: `src/state.rs` `export_arn` = `emulator_arn(SIGNING_NAME, region, "export/<name>")`, used by `src/ops/create_export.rs` line 179. Test coverage: `tests/all/export_lifecycle.rs` lines 667 and 1110 assert the uuid-less form, and line 1394 asserts a recreated export reuses the freed ARN — i.e. the current shape is pinned by tests rather than matched to the recording.

## Steps to Reproduce

1. CreateExport with Export.Name = "my_export" and note the returned ExportArn.
2. Observe it ends in `:export/my_export` with no generated id segment, unlike recorded AWS ARNs of the form `:export/my_export/<uuid>`.
3. DeleteExport that ARN, then CreateExport again with the same name.
4. Observe the second ARN is identical to the first, where AWS would return a new ARN for the new export.
