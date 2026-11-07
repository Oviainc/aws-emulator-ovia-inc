Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: omics.start-run-missing-workflow-not-found-rejected
Issue-Shape-Id: com.amazonaws.omics#Omics
Issue-Key: omics|StartRun|ValidationException
Issue-Commit: c74872d8d60dbe577edcf3a10f097886de6715a9

# omics: StartRun refuses a run that names no storageCapacity with an invented 400 ValidationException, so no run can be started

> _Note: this issue was identified against the code base at commit `c74872d8d60dbe577edcf3a10f097886de6715a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRun

## Description
One net-new root cause, grounded in a failing behavioral check of this suite.

Related open issue: 20261029T184954Z-omics-4-net-new-root-causes-startrun-refuses.md describes the same StartRun storage-capacity refusal as its Cause 1, but it carries a DIFFERENT replay key for this check (`omics.start-run-missing-workflow-not-found-rejected|_retry_role_propagation#6|Timeout|time.sleep(interval_s)` — the check then consumed its whole 60 s budget and produced no verdict). On this replay the same path surfaces a raised ValidationException instead of a timeout, so the suite did not report this failure as owned by that issue. The requirement below is the same one and closing it closes both keys; it is recorded here so this replay key is not left unclaimed.

Cause 1 of 1: StartRun invents a storageCapacity requirement and refuses a run real AWS accepts (check: omics.start-run-missing-workflow-not-found-rejected)

AWS: StartRun requires only roleArn, outputUri and requestId besides a workflow reference (Smithy: StartRunRequest.roleArn @required length 1..128 pattern ^arn:.+$, outputUri @required, requestId @required @idempotencyToken; storageCapacity is OPTIONAL, Integer @range 0..100000; storageType is OPTIONAL, StorageType length 1..64). The model documents the default explicitly on StartRunRequest.storageCapacity: "The STATIC storage capacity (in gibibytes, GiB) for this run. The default run storage capacity is 1200 GiB. ... This field is not required if the storage type is DYNAMIC (the system ignores any value that you enter)." So AWS resolves a capacity itself when none is named, for either storage type. Recorded behavior: a StartRun naming a real, ACTIVE private workflow with roleArn + outputUri + requestId + name and NEITHER storageType NOR storageCapacity is ACCEPTED and returns the modeled success, HTTP 201 with a run id (StartRun http {POST /run, code 201}).

Emulator: the request resolves to the STATIC storage type and, because no storageCapacity was supplied, the handler refuses with HTTP 400 ValidationException whose message is the emulator's own text "storageCapacity is required for a run with the STATIC storage type" — a sentence with no variable parts and no counterpart in AWS, invented for a case AWS does not refuse at all. Replay: `botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartRun operation: storageCapacity is required for a run with the STATIC storage type`. The check's role-propagation wrapper retries while StartRun answers ValidationException or AccessDeniedException, so the refusal is repeated for the whole retry window and then raised; the check's positive control never obtains a run id.

Why: a client that starts a run the way AWS documents — omitting storageType and storageCapacity and relying on the service's own 1200 GiB default — cannot start any run against this emulator. Everything downstream of a run (GetRun, ListRuns, CancelRun, DeleteRun, status progression, run output in S3) is unreachable, and the caller is told its request is invalid when AWS accepts it. Clients that DO pass a capacity are unaffected, which makes the defect look like an auth/propagation problem rather than a missing default.

Fix: StartRun's validation and defaulting in the operation's handler — the storage-type/storage-capacity resolution branch and the ValidationException it raises — together with the defaults it reads from the service state. A run that resolves to STATIC with no capacity named anywhere must be accepted and given the service default capacity; a DYNAMIC run must ignore the member entirely. Only a value outside the modeled range 0..100000 is refusable, and such a refusal must carry AWS's wording for the constraint-violation class, not emulator prose.

Traces: omics.start-run-missing-workflow-not-found-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 — omics 7/22 passed (32%), 15 failed. 14 of those failures are already covered (scaffold-stub issues for CreateMultipartReadSetUpload x8, ListRunGroups, CreateShare, UpdateRunGroup, DeleteSequenceStore, TagResource; and the CloudFormation AWS::Omics::RunGroup check, reported OWNED by 20261029T184954Z-omics-4-net-new-root-causes-startrun-refuses.md) and are skipped. The one remaining failure is the evidence below.

[error] omics.start-run-missing-workflow-not-found-rejected:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartRun operation: storageCapacity is required for a run with the STATIC storage type

Expected (AWS): the positive-control StartRun — a real ACTIVE private workflow created by CreateWorkflow(engine=WDL, definitionZip=<main.wdl>), roleArn of a role trusting omics.amazonaws.com, outputUri s3://<bucket>/control-out/, requestId, name, and NO storageType and NO storageCapacity — returns HTTP 201 with a run id (the check asserts `control["id"]` is non-empty). Model backing: storageCapacity is OPTIONAL with @range 0..100000 and documents "The default run storage capacity is 1200 GiB"; storageType is OPTIONAL.

Actual (emulator): HTTP 400, code ValidationException, message "storageCapacity is required for a run with the STATIC storage type". Message anatomy: the entire sentence is emulator-authored fixed text — it interpolates nothing from the request or state, and AWS produces no error of this class for this request at all. The retry wrapper in the check retries ValidationException/AccessDeniedException for 60 s and then re-raises, so the check fails before reaching its later assertions (the unknown-workflow ResourceNotFoundException message and the ListRuns reads), which remain masked by this refusal.

## Steps to Reproduce
Replay the behavioral check omics.start-run-missing-workflow-not-found-rejected against the emulator, or by hand:

1. Create an S3 bucket and an IAM role whose trust policy allows omics.amazonaws.com to sts:AssumeRole (the check also attaches s3/logs/ecr permissions).
2. CreateWorkflow(name=<name>, engine="WDL", definitionZip=<zip containing main.wdl>, requestId=<token>) and poll GetWorkflow until status == ACTIVE. The workflow declares neither storageType nor storageCapacity.
3. Call StartRun(workflowId=<that id>, roleArn=<role arn>, outputUri="s3://<bucket>/control-out/", requestId=<token>, name=<name>) passing NO storageType and NO storageCapacity.

Observed: HTTP 400 ValidationException "storageCapacity is required for a run with the STATIC storage type".
Expected: HTTP 201 with a run id, the run given the service default STATIC capacity (1200 GiB per the model's documentation), and the run subsequently visible in ListRuns(name=<name>).

Regression guard after the fix: repeat step 3 with storageCapacity=100001 and expect a 400 ValidationException for the @range 0..100000 violation (AWS's constraint wording), and with storageType="DYNAMIC" and no capacity and expect 201.
