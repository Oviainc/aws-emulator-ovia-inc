Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.conflict-exception-concurrent-state
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.conflict-exception-concurrent-state|run#66|AssertionError|assert code == "InvalidParameterException", (
Issue-Commit: ff813631b44d590ec7aaf15ff09fde7e2cd62f49

# logs CreateImportTask: a nonexistent import source is refused ResourceNotFoundException instead of InvalidParameterException "Invalid Event Data Store ARN: <importSourceArn>"

> _Note: this issue was identified against the code base at commit `ff813631b44d590ec7aaf15ff09fde7e2cd62f49`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateImportTask

## Description
REOPENS 20261023T113357Z-logs-createimporttask-stub.md: its fix left the call refused. CreateImportTask is no longer a 501 scaffold stub, but the operation's own not-found path now answers the wrong refusal class and a wholly emulator-authored sentence, so the call is still refused in a way no AWS client recognises.

Cause 1 of 1: CreateImportTask with an importSourceArn that names no CloudTrail event data store is refused with the wrong error class and message.

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328, logs-2014-03-28) models CreateImportTask with input members importSourceArn (target Arn, required) and importRoleArn (target RoleArn, required, @length min 1), and declares the errors AccessDeniedException, ConflictException, InvalidOperationException, InvalidParameterException, ResourceNotFoundException, ThrottlingException and ValidationException — all client errors, httpError 400. For an importSourceArn that resolves to no event data store, the recorded behavior is HTTP 400 with code InvalidParameterException and the message `Invalid Event Data Store ARN: <importSourceArn>`. In that message the fixed text is `Invalid Event Data Store ARN: ` and the only variable part is the caller-supplied importSourceArn value echoed verbatim from the request; the recorded run used an arn:aws:cloudtrail:<region>:<account>:eventdatastore/<name> value, which is one instance filling that template, not part of it. CloudWatch Logs classifies the unresolvable source ARN as a parameter defect on the request, not as a missing-resource lookup — ResourceNotFoundException is reserved in this API for the named log group / log stream / policy targets.

Emulator: the same request is refused with code ResourceNotFoundException and the message `The specified import source does not exist.` — a different modeled error shape and a sentence that is not AWS's (no mention of the ARN, and no `Invalid Event Data Store ARN:` prefix).

Why: a client that branches on the refusal class gets the wrong branch. InvalidParameterException is a permanent request defect the caller must fix by correcting the ARN; ResourceNotFoundException is routinely treated as a resource that may appear later (retry/wait, or create-then-retry) and, in botocore/CLI error handling and in CloudFormation custom resources, is frequently swallowed as "already gone". Validation suites and onboarding flows that assert the modeled InvalidParameterException for a bad importSourceArn fail against the emulator, and the message carries none of the offending ARN, so an operator cannot see which ARN was rejected.

Fix: the CreateImportTask handler's input validation for the logs service (the importSourceArn resolution step in services/service-com-amazonaws-cloudwatchlogs-logs-20140328's create_import_task operation) must classify an importSourceArn that names no existing event data store as InvalidParameterException and render AWS's sentence with the request's importSourceArn interpolated, instead of raising the generic ResourceNotFoundException from its state lookup.

Traces: logs.conflict-exception-concurrent-state (the check's final step, after the delivery-source/destination and scheduled-query phases have already passed).

## Evidence
Failing check logs.conflict-exception-concurrent-state:

```
File "check_logs_conflict_exception_concurrent_state.py", line 281, in run
assert code == "InvalidParameterException", (
AssertionError: expected CreateImportTask against a nonexistent source to be rejected with InvalidParameterException, got 'ResourceNotFoundException' 'The specified import source does not exist.'
```

The check's request and the two assertions it makes:

```python
fake_source_arn = f"arn:aws:cloudtrail:{ctx.region}:{account}:eventdatastore/{ctx.unique_name('')}"
fake_role_arn = f"arn:aws:iam::{account}:role/{ctx.unique_name('sim2real-behav-importrole-')}"
code, msg = _error(lambda: svc.create_import_task(importSourceArn=fake_source_arn, importRoleArn=fake_role_arn))
assert code == "InvalidParameterException"
assert msg == f"Invalid Event Data Store ARN: {fake_source_arn}"
```

Expected: HTTP 400, Code InvalidParameterException, Message `Invalid Event Data Store ARN: <the importSourceArn sent in the request>`.
Actual: Code ResourceNotFoundException, Message `The specified import source does not exist.`

Smithy (CreateImportTask) confirms both classes are modeled on this operation, so the emulator is choosing the wrong one rather than lacking the shape: errors = AccessDeniedException, ConflictException, InvalidOperationException, InvalidParameterException, ResourceNotFoundException, ThrottlingException, ValidationException.

## Steps to Reproduce
1. Build and boot the emulator from the checkout and replay the behavioral suite.
2. Re-replay logs.conflict-exception-concurrent-state.
3. Manual repro: call logs CreateImportTask with importSourceArn set to an event-data-store ARN that exists nowhere (e.g. arn:aws:cloudtrail:<region>:<account>:eventdatastore/<random>) and any importRoleArn. Observed: 400 ResourceNotFoundException `The specified import source does not exist.` Expected: 400 InvalidParameterException `Invalid Event Data Store ARN: <that same importSourceArn>`.
