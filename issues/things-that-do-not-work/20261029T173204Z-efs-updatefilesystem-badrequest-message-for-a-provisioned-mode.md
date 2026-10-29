Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs.update-file-system-throughput-mode-missing-param-rejected
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: efs.update-file-system-throughput-mode-missing-param-rejected|run#9|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Commit: f4e5251e6df88363d8e23ace2ff5bff3be31b423

# EFS UpdateFileSystem: BadRequest message for a provisioned-mode switch missing ProvisionedThroughputInMibps is not AWS's sentence (1 cause)

> _Note: this issue was identified against the code base at commit `f4e5251e6df88363d8e23ace2ff5bff3be31b423`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateFileSystem

## Description
Replay of the behavioral suite simparity-behavioral-aws-v1 against the emulator built from this checkout found ONE surviving, unowned EFS (service token `efs`, Smithy sdkId EFS / sigv4Name elasticfilesystem, api 2015-02-01, restJson1) root cause. THIS ISSUE HAS 1 CAUSE.

Related open issue, named so the work is merged rather than done twice: 20261022T154034Z-efs-control-plane-backup-policy-file-system-policy.md. Its Cause 3 recorded the SAME check failing because UpdateFileSystem answered 501/NotImplemented instead of refusing with BadRequest. That part now works — the emulator reaches the validation and raises BadRequest — but the refusal's Message is not the one AWS emits, which is a separate, still-open requirement of that same handler. This is NOT a report-filed scaffold-stub failure and the replay did not mark it OWNED by any open issue.

Cause 1 of 1 — the conditional-parameter refusal of UpdateFileSystem carries the emulator's own wording instead of EFS's fixed sentence (check efs.update-file-system-throughput-mode-missing-param-rejected).
AWS: Smithy UpdateFileSystem = PUT /2015-02-01/file-systems/{FileSystemId}, success code 202, output FileSystemDescription; input members ThroughputMode (enum ThroughputMode: bursting|provisioned|elastic) and ProvisionedThroughputInMibps (double, @range min 1.0); modeled errors BadRequest (errorKind client, httpError 400 — doc: "Returned if the request is malformed or contains an error such as an invalid parameter value or a missing required parameter"), FileSystemNotFound (404), IncorrectFileSystemLifeCycleState (409), ThroughputLimitExceeded (400), TooManyRequests (429), InsufficientThroughputCapacity (503), InternalServerError (500). The model marks ProvisionedThroughputInMibps optional, so the "required when ThroughputMode resolves to provisioned" rule lives in the handler, and the message that announces it is recorded behavior only: AWS refuses with 400 BadRequest and the Message is the FIXED sentence 'Provisioned throughput must be set for file systems that use provisioned throughput mode.' — the whole string is fixed text, there are NO variable parts in it (no file-system id, no member name, no limit interpolated from the request or from state), and it is terminated with a period. The recording also shows the refusal is effect-free: a following DescribeFileSystems still reports ThroughputMode=bursting and LifeCycleState=available, and the same call WITH ProvisionedThroughputInMibps=1.0 is accepted and settles at ThroughputMode=provisioned.
Emulator: the HTTP status and the error code are now right (400 / BadRequest), but the Message is 'ProvisionedThroughputInMibps is required when ThroughputMode is provisioned' — a self-authored sentence naming the Smithy member and the enum value, with no terminating period, instead of AWS's fixed sentence. The check asserts the code first (passes) and then the message (fails with: expected message 'Provisioned throughput must be set for file systems that use provisioned throughput mode.', got 'ProvisionedThroughputInMibps is required when ThroughputMode is provisioned').
Why: BadRequest is a single catch-all class that UpdateFileSystem also uses for an out-of-range ProvisionedThroughputInMibps, an unknown ThroughputMode and other malformed input, so the Message is the only thing that tells a caller WHICH input rule it broke. A client, retry wrapper or operator runbook that keys on EFS's documented sentence (or that surfaces it to a user) does not recognise the emulator's wording, so the "supply the provisioned throughput and retry" branch never runs and the refusal reads as an unclassified bad request.
Fix: the UpdateFileSystem handler's request validation in the EFS service crate (services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/update_file_system.rs) — keep the existing BadRequest/400 classification but emit EFS's fixed sentence verbatim for the missing-provisioned-throughput rule, and keep the refusal path effect-free (the stored ThroughputMode and LifeCycleState must be untouched, which the check verifies right after the refusal).
Traces: efs.update-file-system-throughput-mode-missing-param-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 16/29 passed, bootstrap_signal=false (the emulator was serving — CreateFileSystem, DescribeFileSystems, UpdateFileSystem and DeleteFileSystem all answered). Per-service: ecs__efs__logs 0/1, efs 16/25 (9 failed), efs__lambda 0/3.

Excluded from this issue: the failures the report marked ALREADY FILED (blocked by the unimplemented ec2 CreateInternetGateway, services/service-com-amazonaws-ec2-amazonec2/src/ops/create_internet_gateway.rs) and the ones it marked OWNED (efs.backup-policy-status-transitions and efs.put-file-system-policy-lockout-rejected, owned by 20261022T154034Z-efs-control-plane-backup-policy-file-system-policy.md; efs__lambda.deep-lambda-2-efs-mount-shared-between-two-functions-and-clientmount-denied, owned by 20261026T232755Z-efs-lambda-putfilesystempolicy-answers-501-notimplemented-aborting-the.md).

The one surviving, unowned failure:

[mismatch] efs.update-file-system-throughput-mode-missing-param-rejected
  check_efs_update_file_system_throughput_mode_missing_param_rejected.py, line 31, in run
  assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
  AssertionError: expected message 'Provisioned throughput must be set for file systems that use provisioned throughput mode.', got 'ProvisionedThroughputInMibps is required when ThroughputMode is provisioned'

Expected (AWS):  HTTP 400, Error.Code = BadRequest, Error.Message = 'Provisioned throughput must be set for file systems that use provisioned throughput mode.' (entirely fixed text; nothing interpolated from the request or from state).
Actual (emulator): HTTP 400, Error.Code = BadRequest (both correct), Error.Message = 'ProvisionedThroughputInMibps is required when ThroughputMode is provisioned'.

The preceding assertion in the same check — assert code == "BadRequest" — now PASSES, which is the part Cause 3 of 20261022T154034Z-efs-control-plane-backup-policy-file-system-policy.md described (it previously got 'NotImplemented'). Only the message assertion diverges now.

Model consulted: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json — UpdateFileSystem: http PUT /2015-02-01/file-systems/{FileSystemId}, success code 202, output shape FileSystemDescription; input ThroughputMode (enum), ProvisionedThroughputInMibps (double, @range min 1.0, NOT required); errors BadRequest (client, 400), FileSystemNotFound (client, 404), IncorrectFileSystemLifeCycleState (client, 409), ThroughputLimitExceeded (client, 400), TooManyRequests (client, 429), InsufficientThroughputCapacity (server, 503), InternalServerError (server, 500). Generated file-system ids and creation tokens in the check are environment-specific; the status, the error code and the fixed message sentence are the requirements.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 (run_behavioral_suite) and look at the efs service group; or run check efs.update-file-system-throughput-mode-missing-param-rejected on its own.
3. CreateFileSystem with a fresh CreationToken and ThroughputMode='bursting'; poll DescribeFileSystems until LifeCycleState='available'.
4. Call UpdateFileSystem(FileSystemId=<that id>, ThroughputMode='provisioned') with ProvisionedThroughputInMibps OMITTED.
5. Observe: HTTP 400 with Error.Code='BadRequest' (correct) but Error.Message='ProvisionedThroughputInMibps is required when ThroughputMode is provisioned'. AWS answers the same status and code with Error.Message='Provisioned throughput must be set for file systems that use provisioned throughput mode.'
6. Confirm the refusal is effect-free: DescribeFileSystems(FileSystemId=<id>) must still report ThroughputMode='bursting' and LifeCycleState='available'.
7. Positive control: UpdateFileSystem(FileSystemId=<id>, ThroughputMode='provisioned', ProvisionedThroughputInMibps=1.0) must be accepted and the file system must settle at LifeCycleState='available' with ThroughputMode='provisioned'.
