Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the MailManager emulator

_Filed: 2026-10-21T21:08:52Z_
_Operation: service-com-amazonaws-mailmanager-mailmanagersvc | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 60 operations in `services/service-com-amazonaws-mailmanager-mailmanagersvc/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and answers HTTP 501 `NotImplemented` while holding zero state. Nothing in this service works end-to-end.

This issue defines the FIRST, SMALL slice to implement: the email-archive lifecycle, which is MailManager's primary resource. Implement exactly these five operations, statefully, in this order (wire protocol is `awsJson1_0`, `POST /` with `X-Amz-Target: MailManagerSvc.<OperationName>`):

1. **CreateArchive** — input `ArchiveName` (required, len 1..64, pattern `^[a-zA-Z0-9][a-zA-Z0-9_-]*[a-zA-Z0-9]$`), optional `ClientToken` (@idempotencyToken), `Retention` (ArchiveRetention union with `RetentionPeriod`), `KmsKeyArn`, `Tags`. Output: a freshly generated `ArchiveId` (len 1..66) that is STORED, not discarded. Record `CreatedTimestamp`/`LastUpdatedTimestamp`, `ArchiveState = ACTIVE`, and an `ArchiveArn` of the form `arn:aws:ses:<region>:<account>:mail-manager-archive/<ArchiveId>`. Creating a second archive with the same `ArchiveName` must fail with `ConflictException` (HTTP 409); a repeat with the same `ClientToken` must return the SAME `ArchiveId` rather than creating a duplicate.
2. **GetArchive** — input `ArchiveId` (required). Output must be read back from the stored record: `ArchiveId`, `ArchiveName`, `ArchiveArn`, `ArchiveState` (ACTIVE | PENDING_DELETION), `Retention` (required — default to the value supplied at create, or the service default when none was given), `CreatedTimestamp`, `LastUpdatedTimestamp`, `KmsKeyArn`. Unknown `ArchiveId` → `ResourceNotFoundException` (HTTP 404), never 200.
3. **ListArchives** — `@paginated(inputToken=NextToken, outputToken=NextToken, items=Archives)`. Input `NextToken` (len 1..2048), `PageSize` (1..50). Output `Archives` (required list of `Archive{ArchiveId, ArchiveName, ArchiveState, LastUpdatedTimestamp}`) plus `NextToken` when more remain. Must list exactly the archives created so far — empty list (not an error) when none exist — and must honour `PageSize` and round-trip `NextToken` so paging a set larger than one page returns each archive exactly once; `NextToken` must be absent on the final page.
4. **UpdateArchive** — input `ArchiveId` (required), optional `ArchiveName`, `Retention`. Empty output body. Mutates the stored record and bumps `LastUpdatedTimestamp`; a following GetArchive/ListArchives must observe the new values. Unknown `ArchiveId` → `ResourceNotFoundException`. Updating an archive already in `PENDING_DELETION` → `ConflictException` (the model documents that no further modifications are allowed in that state).
5. **DeleteArchive** — input `ArchiveId` (required). Empty output body. Per the modeled `ArchiveState` docs, delete MARKS the archive `PENDING_DELETION` (permanent removal after 30 days) rather than erasing it immediately: a following GetArchive must succeed and report `ArchiveState = PENDING_DELETION`, and the archive must still appear in ListArchives with that state. Deleting an archive already pending deletion → `ConflictException`.

Every one of these must compute its response from persisted state. No canned bodies, no echoing the input back, and no returning a random `ArchiveId` that was never stored — a create followed by a get/list must return what was created. Delete the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker line from each of the five handler files as it is implemented.

## State Model

In `src/state.rs` (`ComAmazonawsMailmanagerMailmanagersvcState`, reached via `crate::state::service_state(&*self.engine)` — one shared engine, never a fresh `InMemoryEngine` inside a handler):

- `archives: BTreeMap<String /* ArchiveId */, Archive>` — BTreeMap so ListArchives has a stable, deterministic order for pagination.
- `Archive { archive_id: String, archive_name: String, archive_arn: String, archive_state: ArchiveState /* ACTIVE | PENDING_DELETION */, retention: ArchiveRetention /* union: RetentionPeriod */, kms_key_arn: Option<String>, created_timestamp: DateTime, last_updated_timestamp: DateTime, tags: Vec<Tag>, pending_deletion_at: Option<DateTime> }`
- `archive_names: BTreeMap<String /* ArchiveName */, String /* ArchiveId */>` — enforces the name-uniqueness ConflictException.
- `idempotency: BTreeMap<String /* ClientToken */, String /* ArchiveId */>` — makes retried CreateArchive calls return the original id.

Everything else (ingress points, rule sets, traffic policies, relays, address lists and their members/import jobs, archive search/export, addons, tagging) stays 501 for now and will be filed as individual gap issues later. Keep this first slice small.

## Evidence

Spec: `services/service-com-amazonaws-mailmanager-mailmanagersvc/OPERATIONS.md` (generated from `smithy-model/com-amazonaws-mailmanager-mailmanagersvc.json`) documents the archive lifecycle used above — CreateArchive (lines 267-272: required `ArchiveName`, `ClientToken` @idempotencyToken, output required `ArchiveId`, errors incl. ConflictException 409), GetArchive (lines 274-279: @readonly, required output `ArchiveId`/`ArchiveName`/`ArchiveArn`/`ArchiveState`/`Retention`, `ResourceNotFoundException` 404), UpdateArchive (281-286), DeleteArchive (288-293), ListArchives (295-300: `@paginated(inputToken=NextToken, outputToken=NextToken, items=Archives)`, `PageSize` range 1..50). `smithy_query shape=Archive` confirms the list item members `ArchiveId/ArchiveName/ArchiveState/LastUpdatedTimestamp` and documents `PENDING_DELETION` = "marked for deletion and will be permanently deleted in 30 days. No further modifications can be made in this state" — the grounding for delete-marks-not-erases and for the ConflictException on modifying a pending-deletion archive. `smithy_query shape=ArchiveRetention` confirms it is a union with member `RetentionPeriod`.

Implementation: `src/ops/create_archive.rs` line 4 carries `//! SCAFFOLDED, NOT IMPLEMENTED:` and line 174 returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateArchive"))`; all 60 files under `src/ops/` are the same generated stub. No test exercises any archive operation — only the generated `tests/state_roundtrip.rs` example exists.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK v3 MailManager client at it (awsJson1_0, POST / with X-Amz-Target: MailManagerSvc.<Operation>).
2. CreateArchive{ArchiveName: "demo-archive", Retention: {RetentionPeriod: "SIX_MONTHS"}, ClientToken: "tok-1"} → expect 200 with a non-empty ArchiveId; today returns HTTP 501 NotImplemented.
3. Repeat the identical CreateArchive with ClientToken "tok-1" → expect the SAME ArchiveId back (idempotent), not a second archive.
4. CreateArchive{ArchiveName: "demo-archive"} with a fresh token → expect ConflictException (HTTP 409) for the duplicate name.
5. GetArchive{ArchiveId: <id>} → expect ArchiveName "demo-archive", ArchiveState "ACTIVE", Retention echoing the stored SIX_MONTHS, a mail-manager-archive ArchiveArn containing <id>, and CreatedTimestamp/LastUpdatedTimestamp.
6. ListArchives{} → expect Archives to contain exactly the created archive with its id, name and ACTIVE state; with 3 archives created and PageSize=2, expect 2 items + NextToken, then the 3rd item and no NextToken on the follow-up call.
7. UpdateArchive{ArchiveId: <id>, ArchiveName: "demo-archive-renamed"} → expect 200 empty body, then GetArchive shows the new name and a bumped LastUpdatedTimestamp.
8. DeleteArchive{ArchiveId: <id>} → expect 200 empty body, then GetArchive returns ArchiveState "PENDING_DELETION" and UpdateArchive on it returns ConflictException (409).
9. GetArchive{ArchiveId: "does-not-exist"} → expect ResourceNotFoundException (HTTP 404), not 501 and not a hollow 200.
