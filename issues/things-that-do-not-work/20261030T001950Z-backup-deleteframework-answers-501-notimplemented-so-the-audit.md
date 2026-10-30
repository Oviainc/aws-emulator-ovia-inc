Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.delete-framework-in-use-conflict-rejected
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.delete-framework-in-use-conflict-rejected|run#8|AssertionError|assert code == "ConflictException", f"expected ConflictException deleting a framework with an in-progress deployment, got {code!r}"
Issue-Commit: a1f92506050befa9acc0ff29de706b69960d0a20

# backup DeleteFramework answers 501 NotImplemented, so the audit-framework delete path and its ConflictException / ResourceNotFoundException refusals are unreachable

> _Note: this issue was identified against the code base at commit `a1f92506050befa9acc0ff29de706b69960d0a20`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteFramework

## Description
Cause 1 of 1: `DeleteFramework` (DELETE /audit/frameworks/{FrameworkName}) is still a scaffold stub in the AWS Backup crate, so every delete of an audit framework is refused `NotImplemented` and none of the operation's modeled refusal classes can ever be observed. Failing check: backup.delete-framework-in-use-conflict-rejected.

AWS: Smithy model services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json — operation `DeleteFramework`, http { method: DELETE, uri: /audit/frameworks/{FrameworkName}, code: 200 }, input member `FrameworkName` (@httpLabel, required, @length 1..256, @pattern ^[a-zA-Z][_a-zA-Z0-9]*$), empty output (Unit). Modeled errors: `ConflictException` (HTTP 409, documented "Backup can't perform the action that you requested until it finishes performing a previous action. Try again later."), `InvalidParameterValueException` (400), `MissingParameterValueException` (400), `ResourceNotFoundException` (400), `ServiceUnavailableException` (500). AWS Backup deploys a framework's controls to AWS Config asynchronously after CreateFramework returns and serializes framework deployment mutations, so a DeleteFramework issued while `DescribeFramework.DeploymentStatus` is still `CREATE_IN_PROGRESS` is refused 409 `ConflictException` and the framework is left intact; once the deployment reaches a terminal status (`COMPLETED`/`FAILED`) the same DeleteFramework succeeds with an empty 200 body and `DescribeFramework` on that name then answers 400 `ResourceNotFoundException`. Recorded behavior for the two message templates (the model carries only the generic doc text): the conflict message is the fixed text "Operation failed because of a concurrent update for the framework " + the request's `FrameworkName` + " in account " + the caller's account id + the fixed text ". Try again later."; the not-found message is the fixed text "Framework " + the request's `FrameworkName` + " not found in account " + the caller's account id + ".". In both, the only variable parts are the FrameworkName taken from the request and the account id taken from the caller's credentials — the framework name and account digits shown in any single replay are substituted values, not part of the requirement.

Emulator: the handler returns `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteFramework"))` unconditionally, so the call is answered HTTP 501 with error code `NotImplemented` and message "operation DeleteFramework is scaffolded but not implemented in this emulator", for every input and every framework deployment state. The replay asserts the in-progress delete is refused `ConflictException` and gets `'NotImplemented'` instead (AssertionError: expected ConflictException deleting a framework with an in-progress deployment, got 'NotImplemented'). Because the refusal is unconditional, the rest of the lifecycle is unreachable too: the settled delete never succeeds, and `DescribeFramework` therefore never transitions to `ResourceNotFoundException`.

Why: a client that creates audit frameworks can never delete them against this emulator. Error-handling code is driven off the wrong signal — a 501/`NotImplemented` is not retryable and is not in `DeleteFramework`'s modeled error set, whereas the 409 `ConflictException` AWS returns is precisely the retry-after-deployment-settles signal clients loop on; a caller written against AWS will either crash on an unmodeled error or treat a transient conflict as a permanent failure. Frameworks also accumulate forever, so teardown of any test or stack that created one leaks state and subsequent `CreateFramework` calls with the same name hit AlreadyExists.

Fix: implement the operation's handler, `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/delete_framework.rs` — remove the `not_implemented` return and the SCAFFOLDED marker, look the framework up in the crate's framework state (`crate::state::service_state`), refuse 409 `ConflictException` while that framework's deployment status is still in a transient `*_IN_PROGRESS` state, refuse 400 `ResourceNotFoundException` when the name is unknown, and otherwise remove the framework so `DescribeFramework` on the name begins answering `ResourceNotFoundException`. Both refusal messages belong in the same handler's validation path, rendered from the request's FrameworkName and the caller's account id rather than hard-coded.

Traces: backup.delete-framework-in-use-conflict-rejected.

Note: the other framework-mutation failure in this sweep, backup.update-framework-concurrent-conflict-detected ("neither concurrent UpdateFramework call succeeded"), has the same shape but a different operation and is already covered by the auto-filed `UpdateFramework` scaffold-stub issue naming services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/update_framework.rs, so it is not bundled here.

## Evidence
Check backup.delete-framework-in-use-conflict-rejected [mismatch]:

  File "check_backup_delete_framework_in_use_conflict_rejected.py", line 47, in run
    assert code == "ConflictException", f"expected ConflictException deleting a framework with an in-progress deployment, got {code!r}"
  AssertionError: expected ConflictException deleting a framework with an in-progress deployment, got 'NotImplemented'

Expected (AWS), delete raced against the still-in-progress CREATE deployment:
  HTTP 409, Error.Code = "ConflictException"
  Error.Message = "Operation failed because of a concurrent update for the framework <FrameworkName> in account <AccountId>. Try again later."
    fixed text: "Operation failed because of a concurrent update for the framework ", " in account ", ". Try again later."
    variable: <FrameworkName> from the request, <AccountId> from the caller's credentials
  Effect absent: DescribeFramework on that name still returns the framework.
Then, after DeploymentStatus leaves CREATE_IN_PROGRESS:
  DeleteFramework -> HTTP 200, empty body
  DescribeFramework -> HTTP 400, Error.Code = "ResourceNotFoundException",
    Error.Message = "Framework <FrameworkName> not found in account <AccountId>."
    fixed text: "Framework ", " not found in account ", "."
    variable: <FrameworkName> from the request, <AccountId> from the caller's credentials

Actual (emulator), for every input and every deployment state:
  HTTP 501, Error.Code = "NotImplemented"
  Error.Message = "operation DeleteFramework is scaffolded but not implemented in this emulator"

Source: services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/delete_framework.rs line 4 ("//! SCAFFOLDED, NOT IMPLEMENTED") and line 183 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteFramework"))`). Modeled errors declared at lines 110-118 of the same file (ConflictException, InvalidParameterValueException, MissingParameterValueException, ResourceNotFoundException, ServiceUnavailableException) are never constructed.

## Steps to Reproduce
1. Point a botocore `backup` client at the emulator.
2. CreateFramework(FrameworkName=<a name matching ^[a-zA-Z][_a-zA-Z0-9]*$>, FrameworkDescription=..., FrameworkControls=[{"ControlName": "BACKUP_RESOURCES_PROTECTED_BY_BACKUP_PLAN", "ControlScope": {"ComplianceResourceTypes": ["RDS"], "Tags": {...}}}]) — this succeeds.
3. Immediately call DeleteFramework(FrameworkName=<that name>) while DescribeFramework still reports DeploymentStatus=CREATE_IN_PROGRESS.
   Observe HTTP 501 / Error.Code "NotImplemented" instead of HTTP 409 / "ConflictException" with the concurrent-update message.
4. Poll DescribeFramework until DeploymentStatus is no longer CREATE_IN_PROGRESS, then call DeleteFramework again.
   Observe the same HTTP 501 / "NotImplemented": the delete never succeeds, so DescribeFramework never starts answering ResourceNotFoundException.
5. Equivalent one-shot replay: re-run the behavioral check backup.delete-framework-in-use-conflict-rejected.
