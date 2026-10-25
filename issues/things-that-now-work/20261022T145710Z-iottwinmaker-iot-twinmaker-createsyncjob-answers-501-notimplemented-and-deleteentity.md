Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iottwinmaker.create-sync-job-unassumable-role-rejected iottwinmaker.delete-entity-with-children-without-recursive-rejected iottwinmaker.workspace-get-persists-role-and-s3-location
Issue-Shape-Id: com.amazonaws.iottwinmaker#AWSIoTTwinMaker
Issue-Key: iottwinmaker.create-sync-job-unassumable-role-rejected|run#10|AssertionError|assert code == "ValidationException", f"unexpected code={code!r}"
Issue-Key: iottwinmaker.delete-entity-with-children-without-recursive-rejected|run#13|AssertionError|assert err["Message"] == expected_message, f"expected message {expected_message!r}, got {err['Message']!r}"
Issue-Key: iottwinmaker.workspace-get-persists-role-and-s3-location|run#12|AssertionError|assert err["Message"] == expected_message, f"expected message {expected_message!r}, got {err['Message']!r}"
Issue-Commit: dd79a542b09a035cf711a2d0c52670e9a3e4e70a

# IoT TwinMaker: CreateSyncJob answers 501 NotImplemented, and DeleteEntity / workspace not-found refusals use non-AWS message constructions (3 causes)

> _Note: this issue was identified against the code base at commit `dd79a542b09a035cf711a2d0c52670e9a3e4e70a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSyncJob

## Description
Three independent root causes in the emulated IoT TwinMaker control plane, all surfaced by the behavioral suite against a bootstrapped `iottwinmaker` service. All three are net-new (no open issue mentions iottwinmaker; `list_existing_issues(query='iottwinmaker')` returns nothing). The checks blocked by the already-filed CreateComponentType / ExecuteQuery / TagResource stubs are NOT part of this issue.

=== Cause 1 of 3: CreateSyncJob is an unimplemented scaffold stub, so the whole SyncJob surface is unreachable ===
AWS: Smithy (com-amazonaws-iottwinmaker-awsiottwinmaker) models `CreateSyncJob` as POST /workspaces/{workspaceId}/sync-jobs/{syncSource}, success 200, output `{arn (required), creationDateTime (required), state (required, SyncJobState one of ACTIVE|CREATING|DELETING|ERROR|INITIALIZING)}`, with modeled errors AccessDeniedException (403), ConflictException (409), InternalServerException (500), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400). The recorded behavior being asserted: a `syncRole` whose trust policy does not permit `iottwinmaker.amazonaws.com` to assume it is refused SYNCHRONOUSLY with HTTP 400 `ValidationException` (not deferred to an async ERROR job state) and no SyncJob is persisted, so a following `GetSyncJob` on the same workspace/syncSource is `ResourceNotFoundException`; the same syncSource with a properly trusted role is accepted and `GetSyncJob` then reports a state in {CREATING, INITIALIZING, ACTIVE, ERROR}.
Emulator: every CreateSyncJob call answers HTTP 501 with error code `NotImplemented` and the message "operation CreateSyncJob is scaffolded but not implemented in this emulator". The check fails at `assert code == "ValidationException"` with `unexpected code='NotImplemented'`. `NotImplemented` is not a modeled error for this operation and 501 is not a modeled status.
Why: a client cannot create, read back, or negatively test a SyncJob at all. Worse, the refusal is indistinguishable from a real rejection only by code, so SDK error handling written for ValidationException/ConflictException never fires, retry logic sees a non-retryable unmodeled code, and any caller that treats 501 as a transport fault will loop or abort.
Fix: the operation's handler — implement `CreateSyncJob::handle` (replace the `Scaffolded::not_implemented` return), including synchronous syncRole trust-policy validation returning the modeled ValidationException, SyncJob persistence in the service state so GetSyncJob/DeleteSyncJob see it, and the SCAFFOLDED marker removal.
Traces: iottwinmaker.create-sync-job-unassumable-role-rejected. Handler: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/create_sync_job.rs:173 (`Err(crate::scaffolded::Scaffolded::not_implemented("CreateSyncJob"))`), SCAFFOLDED marker at line 4.

=== Cause 2 of 3: DeleteEntity's has-children refusal message is not AWS's construction ===
AWS: Smithy models `DeleteEntity` as DELETE /workspaces/{workspaceId}/entities/{entityId} with `isRecursive` as an httpQuery boolean, and `ValidationException` (HTTP 400) among its errors. Recorded behavior: deleting an entity that still has children without `isRecursive=true` is refused 400 ValidationException with a message built from the request, naming BOTH the entityId and the containing workspaceId: "Cannot delete entity <entityId> in <workspaceId> as it has child entities." The rule is known because every variable part of the message is a request member, so the construction — not one recording's ids — is the requirement.
Emulator: the status (400) and code (ValidationException) are correct, but the message is a different construction that omits the workspaceId entirely and instead reports a child count: e.g. "entity a793021f-... has 1 child entities; set isRecursive to delete them too" where AWS produces "Cannot delete entity a793021f-... in sim2real-tm-ws-0fc284070bd2 as it has child entities." (recorded values shown as examples).
Why: callers and operators that match or parse this message — log scrapers, idempotent-teardown scripts that distinguish "blocked by children" from other ValidationExceptions, and multi-workspace tooling that reads the workspace id out of the refusal — misclassify the refusal, and the workspace that blocked the delete is not identifiable from the error at all.
Fix: the DeleteEntity handler's validation branch — the message format string for the non-recursive-with-children refusal.
Traces: iottwinmaker.delete-entity-with-children-without-recursive-rejected. Emitted at services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/ops/delete_entity.rs:196-207.

=== Cause 3 of 3: workspace not-found message is not AWS's construction (GetWorkspace, and UpdateWorkspace behind it) ===
AWS: Smithy models `GetWorkspace` (GET /workspaces/{workspaceId}) and `UpdateWorkspace` (PUT /workspaces/{workspaceId}) both with `ResourceNotFoundException` (HTTP 404). Recorded behavior: after DeleteWorkspace, GetWorkspace on the same id is 404 ResourceNotFoundException with the message "Unable to find workspace for id: <workspaceId>", and UpdateWorkspace on the same id is 404 ResourceNotFoundException with the DIFFERENT message "Workspace with id <accountId>|<workspaceId> does not exist." — i.e. AWS uses two distinct, per-operation constructions, each built from the request (and the caller's account for the update path). Both constructions are the requirement; the ids in them are examples.
Emulator: the status (404) and code (ResourceNotFoundException) are correct, but both paths share ONE generic construction, "Workspace <workspaceId> not found." (e.g. "Workspace sim2real-tm-ws-57f9b050e27c not found." where AWS gives "Unable to find workspace for id: sim2real-tm-ws-57f9b050e27c"). The check fails on the GetWorkspace assertion; the UpdateWorkspace assertion sits immediately behind it and will fail next for the same reason, since the emulator derives both from the same shared helper. Note this check also verifies GetWorkspace persists `role` and `s3Location` exactly as CreateWorkspace received them — those assertions PASSED, so the only divergence here is the not-found message.
Why: the two messages are how a caller tells a read-path miss from a mutate-path miss on the same deleted workspace (the update form additionally carries the account scope), and cleanup/reconciliation tooling that keys on either string sees neither.
Fix: the shared not-found message helper plus the two handlers that use it — `workspace_not_found_message` in the service state must become per-operation constructions (read path vs. update path), not one generic string.
Traces: iottwinmaker.workspace-get-persists-role-and-s3-location. Helper: services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/src/state.rs (`workspace_not_found_message`), used e.g. at src/ops/delete_entity.rs:175.

## Evidence
Suite: simparity-behavioral-aws-v1 — iottwinmaker 5/22 passed, 17 failed. 14 of the failures are already filed against the UNIMPLEMENTED CreateComponentType (9), TagResource (3) and ExecuteQuery (2) stubs and are excluded here. The 3 remaining failures are the 3 causes above.

Cause 1 — iottwinmaker.create-sync-job-unassumable-role-rejected [mismatch]:
  expected: HTTP 400, Error.Code == "ValidationException" (synchronous syncRole trust-policy rejection)
  actual:   HTTP 501, Error.Code == "NotImplemented", Message "operation CreateSyncJob is scaffolded but not implemented in this emulator"
  assertion: `assert code == "ValidationException", f"unexpected code={code!r}"` -> AssertionError: unexpected code='NotImplemented'
  (the check polls for up to 45s for a stable non-transient code, so this is not an IAM-propagation artifact)

Cause 2 — iottwinmaker.delete-entity-with-children-without-recursive-rejected [mismatch]:
  Error.Code == "ValidationException" (400) matched; message diverged.
  expected (AWS construction): 'Cannot delete entity <entityId> in <workspaceId> as it has child entities.'
    recorded example: 'Cannot delete entity a793021f-afb7-4515-adc7-853db7e4048f in sim2real-tm-ws-0fc284070bd2 as it has child entities.'
  actual (emulator construction): 'entity <entityId> has <n> child entities; set isRecursive to delete them too'
    observed example: 'entity a793021f-afb7-4515-adc7-853db7e4048f has 1 child entities; set isRecursive to delete them too'
  source: src/ops/delete_entity.rs:199-203 format!("entity {} has {} child entities; set isRecursive to delete them too", ...)

Cause 3 — iottwinmaker.workspace-get-persists-role-and-s3-location [mismatch]:
  GetWorkspace role/s3Location round-trip assertions passed; post-delete Error.Code == "ResourceNotFoundException" (404) matched; message diverged.
  expected (AWS read-path construction): 'Unable to find workspace for id: <workspaceId>'
    recorded example: 'Unable to find workspace for id: sim2real-tm-ws-57f9b050e27c'
  actual (emulator): 'Workspace sim2real-tm-ws-57f9b050e27c not found.'
  next assertion in the same check (not yet reached, same shared helper): UpdateWorkspace on the deleted id must give 404 ResourceNotFoundException with 'Workspace with id <accountId>|<workspaceId> does not exist.'

Smithy consulted (model_path services/service-com-amazonaws-iottwinmaker-awsiottwinmaker/smithy-model/com-amazonaws-iottwinmaker-awsiottwinmaker.json): CreateSyncJob errors {AccessDenied 403, Conflict 409, InternalServer 500, ServiceQuotaExceeded 402, Throttling 429, Validation 400}, success 200 — NotImplemented/501 is not among them. DeleteEntity errors include ValidationException 400 and ResourceNotFoundException 404. GetWorkspace and UpdateWorkspace both model ResourceNotFoundException 404.

## Steps to Reproduce
Boot the emulator and point an iottwinmaker client at it (all steps below only need iottwinmaker + iam + s3).

Cause 1 (CreateSyncJob 501):
1. Create an IAM role whose trust policy allows only ec2.amazonaws.com to sts:AssumeRole.
2. CreateWorkspace with a role trusting iottwinmaker.amazonaws.com and an s3Location arn.
3. Call CreateSyncJob(workspaceId=<ws>, syncSource="SITEWISE", syncRole=<untrusted role arn>).
   Expected: HTTP 400 ValidationException; then GetSyncJob(syncSource="SITEWISE", workspaceId=<ws>) -> 404 ResourceNotFoundException.
   Actual: HTTP 501 NotImplemented.
4. Repeat with a role that DOES trust iottwinmaker.amazonaws.com. Expected: 200 with state in {CREATING, INITIALIZING, ACTIVE, ERROR} and GetSyncJob reporting that state. Actual: HTTP 501 NotImplemented.
Replay: iottwinmaker.create-sync-job-unassumable-role-rejected

Cause 2 (DeleteEntity message):
1. CreateWorkspace <ws>.
2. CreateEntity(workspaceId=<ws>, entityName="parent") -> <parentId>; wait ACTIVE.
3. CreateEntity(workspaceId=<ws>, entityName="child", parentEntityId=<parentId>); wait ACTIVE.
4. DeleteEntity(workspaceId=<ws>, entityId=<parentId>) with no isRecursive.
   Expected: 400 ValidationException, Message 'Cannot delete entity <parentId> in <ws> as it has child entities.'
   Actual: 400 ValidationException with 'entity <parentId> has 1 child entities; set isRecursive to delete them too'.
Replay: iottwinmaker.delete-entity-with-children-without-recursive-rejected

Cause 3 (workspace not-found message):
1. CreateWorkspace <ws> with a role arn and s3Location arn:aws:s3:::<bucket>.
2. GetWorkspace(<ws>) — confirm role and s3Location round-trip (these pass today).
3. DeleteWorkspace(<ws>).
4. GetWorkspace(<ws>).
   Expected: 404 ResourceNotFoundException, Message 'Unable to find workspace for id: <ws>'.
   Actual: 404 ResourceNotFoundException, Message 'Workspace <ws> not found.'
5. UpdateWorkspace(workspaceId=<ws>, description="post-delete-update").
   Expected: 404 ResourceNotFoundException, Message 'Workspace with id <accountId>|<ws> does not exist.'
Replay: iottwinmaker.workspace-get-persists-role-and-s3-location
