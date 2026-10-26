Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 058acc6c8aebc1cbfed6b02aa08fd3a994f3f9ff

# Version-scoped DeleteWorkflow re-derives the latest version from `created_at` equality and can flag two versions as latest

_Filed: 2026-10-26T11:02:00Z_
_Operation: DeleteWorkflow | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `058acc6c8aebc1cbfed6b02aa08fd3a994f3f9ff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

When `DeleteWorkflow` removes ONE version of several and that version was the latest, the handler promotes a replacement by taking the maximum `created_at` over the remaining records and then setting `is_latest` on EVERY record whose `created_at` equals that maximum. Two consequences:

1. If two surviving version records share a `created_at` instant, BOTH are flagged latest. The store then holds two "latest" versions: `ListWorkflowVersions` reports `IsLatestVersion: true` twice, and `Workflow::version(None)` (used by GetWorkflow, ListWorkflows and StartWorkflowRun's default version) resolves to whichever one comes first in the vector, which need not be the one `current_version`/`revision_id` were just set from. That contradicts the rule the crate implements everywhere else — `UpdateWorkflow` clears `is_latest` on all records and sets exactly one.
2. The promotion keys on the wall clock at all, even though this crate deliberately keeps an append-position field (`WorkflowVersionRecord::sequence`) precisely because the clock cannot be trusted to separate two records, and the version listing walks that append order. The promoted version should be the newest APPENDED one (`versions.last()`), which is what the crate's own test asserts the latest flag means.

## Evidence

The promotion, `services/.../src/ops/delete_workflow.rs:287-303`:
```
                    if !workflow.versions.iter().any(|rec| rec.is_latest) {
                        let newest = workflow
                            .versions
                            .iter()
                            .map(|rec| rec.created_at)
                            .max()
                            .unwrap_or(workflow.created_at);
                        for record in &mut workflow.versions {
                            record.is_latest = record.created_at == newest;
                        }
```
The sibling path that keeps exactly one latest, `src/ops/update_workflow.rs:304-318`: "// \"Creates a new version with the updated configuration and disables scheduling on\n// all previous versions to ensure only one version is actively scheduled at a\n// time\"" followed by `for record in &mut workflow.versions { record.is_latest = false; }` then `workflow.versions.push(... is_latest: true ...)`.

The order the crate actually maintains for versions, `src/state.rs:221-229`: "/// Where this version falls in the order its workflow's versions were\n/// appended: the value of the monotonic mint counter ... It only ever grows", and `src/ops/list_workflow_versions.rs:254-255`: "// The versions are held in append order, which is also increasing `sequence`\n// order".

That the clock is NOT trusted to separate two records in this crate, `src/state.rs:259-261`: "/// It breaks a tie between two runs the clock cannot separate, so [`WorkflowRun::listing_key`] is a TOTAL order".

The invariant the crate tests, `tests/all/workflow_version_listing.rs:327-339`: `assert_eq!(flagged.len(), 1, "case {case:?}: exactly one version is the latest, got {flagged:?}")` and "the newest appended version is the latest one" compared against `stored.last()`. The version-scoped delete test (`tests/all/workflow_deletion_teardown.rs:579-653`) checks only the surviving COUNT and status, so it does not cover the promoted flag.

## Steps to Reproduce

1. CreateWorkflow "v" then UpdateWorkflow twice so the workflow holds three version records appended in order V1, V2, V3 (V3 latest).
2. Arrange (or run on a platform whose SystemTime granularity is coarse, e.g. ~15ms) for two of the surviving records to share a `created_at` instant — the crate already assumes the clock can tie two records (state.rs:259-261).
3. Call DeleteWorkflow with WorkflowVersion = V3 (the latest) — one version of three, so the workflow survives.
4. Call ListWorkflowVersions on the workflow.
5. Expected (tests/all/workflow_version_listing.rs:327-331): exactly one summary with IsLatestVersion=true, and it is the newest APPENDED survivor.
6. Actual: every survivor whose `created_at` equals the maximum is flagged IsLatestVersion=true, so two summaries can claim to be latest, and GetWorkflow (which reads `version(None)`) may answer a different version than `current_version`/`revision_id` were set from.
