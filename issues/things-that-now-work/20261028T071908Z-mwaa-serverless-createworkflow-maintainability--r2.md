Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1

# CreateWorkflow re-implements the active-version derivation that Workflow::promote_newest_version single-homes for update and delete

_Filed: 2026-10-28T07:19:08Z_
_Operation: CreateWorkflow | Class: maintainability_

> _Note: this issue was identified against the code base at commit `c18a0019e5dd212e394ad1114dc50f0b0a8a5ba1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate deliberately single-homes the rule that derives what a workflow reports as its ACTIVE version. `Workflow::promote_newest_version` in `state.rs` is that one place: it clears `is_latest` on every record, sets it on the record with the greatest `sequence`, and re-derives `current_version`, `code_snapshotted_at`, `revision_id` (through `mint_revision_id`) and the automated trigger (through `configure_trigger`) from that record. `UpdateWorkflow` and the version-scoped `DeleteWorkflow` both reach it, and `update_workflow.rs` states the reason out loud: "so an update and a version-scoped delete settle the active version the same single way".

`CreateWorkflow` is the third place that appends a version which becomes the workflow's active one, and it is the one place that does NOT call the helper: it open-codes the same five-part derivation inline (`is_latest: true` on the single record it builds, `current_version`, `code_snapshotted_at`, a separately minted `revision_id`, then `configure_trigger`). `promote_newest_version`'s own doc enumerates its callers as "a version appended by an update, or the latest version removed by a delete" — the create's append is missing from that list even though it is the same event.

The copies can drift: any change to what the active version decides (a further field re-derived from the promoted record, or a change to how `revision_id` is minted) has to be made in two places, and nothing in the crate fails if only `promote_newest_version` is changed — a freshly created workflow would then report one thing and the same workflow after its first update another.

The fix is to have `CreateWorkflow` settle the first version through `promote_newest_version(now)` instead of writing the four fields and the trigger by hand. It changes no response byte: the record's `created_at` IS `now`, so `code_snapshotted_at` is unchanged; the single record is the greatest-`sequence` one, so `is_latest`/`current_version` are unchanged; and `revision_id` is `mint_revision_id(&arn, &current_version)` either way.

## Evidence

The single-homed rule, `services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:751-788`:
```
    /// Make the newest APPENDED version of this workflow its ONLY latest one, and
    /// re-derive from that version everything the workflow reports as its current
    /// configuration.
...
    /// Called wherever the set of versions changes the active one — a version
    /// appended by an update, or the latest version removed by a delete — so
    /// [`Workflow::current_version`], [`Workflow::revision_id`],
    /// [`Workflow::code_snapshotted_at`] and the automated trigger
    /// ([`Self::configure_trigger`]) always describe the one version
    /// [`Self::version`] resolves with no version named.
    pub fn promote_newest_version(&mut self, now: SystemTime) {
...
        self.versions[newest].is_latest = true;
        self.current_version = self.versions[newest].version.clone();
        self.code_snapshotted_at = self.versions[newest].created_at;
        self.revision_id = mint_revision_id(&self.arn, &self.current_version);
        self.configure_trigger(now);
    }
```

The stated reason it is single-homed, `src/ops/update_workflow.rs:339-344`:
```
        // every earlier version keeps the configuration it froze and stops being
        // scheduled. The promotion — the flag, what the workflow reports as its current
        // configuration and the trigger in force — is
        // `crate::state::Workflow::promote_newest_version`, so an update and a
        // version-scoped delete settle the active version the same single way.
        workflow.promote_newest_version(now);
```
and the second caller, `src/ops/delete_workflow.rs:293-295`:
```
                    if !workflow.versions.iter().any(|rec| rec.is_latest) {
                        workflow.promote_newest_version(now);
                    }
```

The second copy of the same derivation, `src/ops/create_workflow.rs:364-365`:
```
        let version = crate::state::mint_workflow_version(&arn, &fingerprint, &nonce);
        let revision_id = crate::state::mint_revision_id(&arn, &version);
```
and `src/ops/create_workflow.rs:404-413`:
```
            code_snapshotted_at: now,
            current_version: version.clone(),
            revision_id,
            versions: vec![crate::state::WorkflowVersionRecord {
                version,
                created_at: now,
                is_latest: true,
                snapshot,
                sequence,
            }],
```
and `src/ops/create_workflow.rs:434`:
```
        workflow.configure_trigger(now);
```

Test coverage that must stay green and byte-identical: `tests/all/workflow_version_promotion.rs` ("a workflow has exactly ONE latest version at any instant, and it is the newest version APPENDED to its history", seeding `record.created_at` per row at lines 49-60), `tests/all/workflow_version_listing.rs` (`"case {case:?}: exactly one version is the latest"`), `tests/all/workflow_lifecycle.rs` (the create's reported `WorkflowVersion`/`RevisionId`/`IsLatestVersion`), `tests/all/workflow_update_versions.rs` and `tests/all/scheduled_workflow_runs.rs` (the trigger a created workflow carries).

## Steps to Reproduce

1. Open services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:751-788 and read Workflow::promote_newest_version: it clears is_latest on every record, sets it on the greatest-sequence record, and re-derives current_version, code_snapshotted_at, revision_id and the trigger from it.
2. Note its doc enumerates its callers as "a version appended by an update, or the latest version removed by a delete" — and confirm those two callers at src/ops/update_workflow.rs:344 and src/ops/delete_workflow.rs:294.
3. Open src/ops/create_workflow.rs:364-365, 404-413 and 434: the create appends a version that becomes the workflow's active one and writes is_latest, current_version, code_snapshotted_at, revision_id (via its own mint_revision_id call) and configure_trigger by hand instead of calling the helper.
4. Add a further field to promote_newest_version's derivation (or change how revision_id is minted there) and run the suite: nothing fails, because the create's inline copy still follows the old rule, so a freshly created workflow reports one thing and the same workflow after its first UpdateWorkflow another.
