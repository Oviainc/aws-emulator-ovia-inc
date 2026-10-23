Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs.create-access-point-token-reuse-conflict efs.create-access-point-token-reuse-mismatched-params-rejected
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: efs.create-access-point-token-reuse-conflict|run#14|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: efs|CreateAccessPoint|AccessPointAlreadyExists
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# EFS CreateAccessPoint: AccessPointAlreadyExists is serialized with an empty Message member where AWS omits Message entirely

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint

## Description
Replay of the behavioral suite simparity-behavioral-aws-v1 against the emulator found ONE net-new EFS (service token `efs`, Smithy sdkId EFS / sigv4Name elasticfilesystem, api 2015-02-01, restJson1) root cause that is not covered by a scaffold-stub issue this run filed for itself and not owned by an already-open issue. THIS ISSUE HAS 1 CAUSE.

Cause 1 of 1 — the 409 AccessPointAlreadyExists refusal carries an EMPTY `Message` member instead of no `Message` member at all, so a client cannot tell "this error class carries no sentence" from "the sentence is the empty string" (checks efs.create-access-point-token-reuse-conflict, efs.create-access-point-token-reuse-mismatched-params-rejected).

AWS: Smithy error com.amazonaws.efs#AccessPointAlreadyExists, errorKind client, httpError 409, members ErrorCode (required, @length min 1), AccessPointId (required, @length 0..128, pattern ^(arn:aws[-a-z]*:elasticfilesystem:[0-9a-z-:]+:access-point/fsap-[0-9a-f]{8,40}|fsap-[0-9a-f]{8,40})$) and Message (target ErrorMessage, OPTIONAL); doc: "Returned if the access point that you are trying to create already exists, with the creation token you provided in the request." The model declares no sentence for this class and AWS documents none. Recorded behavior only, and it is about member PRESENCE, not message text: both recordings show the refusal arriving with NO Message member, so botocore's parsed error dict has no "Message" key and err.get("Message") is None. (The same recordings show the request-derived messages of the neighbouring classes — AccessPointNotFound arrives as "Access point 'fsap-...' does not exist." — so the absence of Message on AccessPointAlreadyExists is specific to that class, not a general absence of messages.)

Emulator: HTTP status 409 and error code AccessPointAlreadyExists both match, and the handler services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/create_access_point.rs deliberately constructs the refusal with `message: None` (its comment states the recorded AWS refusal sends no Message member). Nevertheless the response that reaches the client carries Message as the empty string: botocore reports err.get("Message") == '' on both checks ("expected message None, got ''"). The optional, unset member is being materialized on the wire as "" rather than omitted, which is a defect of the restJson1 ERROR serialization path (the generated error serializer / shared error channel), not of the handler — the handler already does the right thing.

Why: a client that branches on whether a refusal carries a diagnostic — `if "Message" in err["Error"]`, or logging/propagating the message only when present — takes the wrong branch and emits or forwards an empty diagnostic string instead of falling back to the error class and the required AccessPointId. The same serialization rule applies to every EFS error whose Message the handler leaves absent (e.g. the PolicyNotFound / backup-policy paths, which AWS also sends with no Message), so any consumer that distinguishes absent-vs-empty sees emulator-only empty strings across the service. It also silently inverts the contract direction: absent means "no rule produces a sentence here", empty means "the sentence is empty".

Fix: the EFS error serialization path — the restJson1 error-body serializer behind `crate::scaffolded` / the generated error types in services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/types.rs must OMIT optional error members (here Message) when the handler leaves them `None`, exactly as it omits optional members of success shapes, rather than emitting "". Verify the required members still travel: ErrorCode and AccessPointId must remain present on this refusal, with AccessPointId naming the access point that already holds the ClientToken.

Traces: efs.create-access-point-token-reuse-conflict, efs.create-access-point-token-reuse-mismatched-params-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the emulator built from this checkout: 14/27 passed, bootstrap_signal=false (the emulator was serving — CreateFileSystem, DescribeFileSystems, CreateAccessPoint, DescribeAccessPoints, DeleteAccessPoint and UpdateFileSystem all answered). Per-service: efs 14/25 (11 failed). Failures the report itself marked ALREADY FILED (blocked by the unimplemented ec2 CreateInternetGateway and ModifySubnetAttribute stubs) and the two marked OWNED by 20261022T154034Z-efs-control-plane-backup-policy-file-system-policy.md are excluded. The two failures below are the surviving, unowned ones, and they are the same cause:

1) [mismatch] efs.create-access-point-token-reuse-conflict —
   check_efs_create_access_point_token_reuse_conflict.py line 44:
   assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
   AssertionError: expected message None, got ''
   Context: CreateFileSystem -> wait available -> CreateAccessPoint(ClientToken=T, FileSystemId, RootDirectory={'Path': '/idempotent-test'}) -> repeat the IDENTICAL call. The repeat is correctly refused: `assert code == "AccessPointAlreadyExists"` passed (HTTP 409, error class right). Only the presence of the Message member diverges: AWS sends none (botocore -> None), the emulator sends an empty one (botocore -> '').

2) [mismatch] efs.create-access-point-token-reuse-mismatched-params-rejected —
   check_efs_create_access_point_token_reuse_mismatched_params_rejected.py line 47:
   AssertionError: expected message None, got ''
   Context: same file system, CreateAccessPoint(ClientToken=T, RootDirectory={'Path': '/original-path'}) then CreateAccessPoint(ClientToken=T, RootDirectory={'Path': '/different-path'}). Again the code assertion AccessPointAlreadyExists passed and only the Message member's presence diverged. Both checks annotate the expectation in-line: `expected_msg = None  # AWS sends this error with no Message member`.

Source read: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/create_access_point.rs lines 257-266 construct AccessPointAlreadyExists { error_code, message: None, access_point_id }, so the empty string is introduced downstream of the handler, on the error serialization path.

Model consulted: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json — AccessPointAlreadyExists (errorKind client, httpError 409; required ErrorCode and AccessPointId; optional Message -> ErrorMessage).

Generated access-point / file-system ids, creation tokens, hosts and timestamps in these checks are environment-specific examples, not requirements; the 409 status, the AccessPointAlreadyExists code and the ABSENCE of the Message member are.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 against it (run_behavioral_suite) and look at the efs service group.
3. Cause 1: run check efs.create-access-point-token-reuse-conflict. It calls CreateFileSystem(CreationToken=...), polls DescribeFileSystems until LifeCycleState=available, calls CreateAccessPoint(ClientToken=T, FileSystemId=fs, RootDirectory={'Path': '/idempotent-test'}), then repeats that identical call.
4. Observe the repeat is refused with HTTP 409 / AccessPointAlreadyExists (correct) but botocore's e.response['Error'].get('Message') is '' where AWS leaves the member out entirely (None). Equivalently, inspect the raw error body: it carries "Message": "" instead of omitting Message.
5. Cause 1, second witness: run efs.create-access-point-token-reuse-mismatched-params-rejected — reuse the same ClientToken with a different RootDirectory Path; same 409/AccessPointAlreadyExists, same empty Message member.
6. After the fix, both checks must pass: the refusal body must contain ErrorCode and the required AccessPointId (naming the access point that holds the token) and NO Message member, with the status still 409.
