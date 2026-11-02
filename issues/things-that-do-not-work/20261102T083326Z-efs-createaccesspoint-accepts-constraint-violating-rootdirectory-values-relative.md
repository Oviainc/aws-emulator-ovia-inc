Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: efs.efs-access-point-invalid-root-directory-rejected
Issue-Shape-Id: com.amazonaws.efs#MagnolioAPIService_v20150201
Issue-Key: efs.efs-access-point-invalid-root-directory-rejected|_error_code#5|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Commit: b1a4a993822b0574a1f24a3d622a9d42702ca481

# EFS CreateAccessPoint accepts constraint-violating RootDirectory values (relative Path, non-octal Permissions) with 200 instead of refusing with ValidationException

> _Note: this issue was identified against the code base at commit `b1a4a993822b0574a1f24a3d622a9d42702ca481`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint

## Description
1 cause.

Cause 1 of 1: `CreateAccessPoint` does not enforce the modeled constraints on the members of `RootDirectory`, so a malformed request is ACCEPTED, mints an access point and returns HTTP 200 where AWS refuses it.

AWS: the Smithy model (services/service-com-amazonaws-efs-magnolioapiservice-v20150201/smithy-model/com-amazonaws-efs-magnolioapiservice-v20150201.json) constrains the two members the check exercises:
  * `RootDirectory$Path` -> shape `com.amazonaws.efs#Path`, @length min 1 max 100, @pattern `^(\/|(\/(?!\.)+[^$#<>;`|&?{}^*/\n]+){1,4})$` — i.e. the path MUST be absolute (leading `/`) and may name at most four subdirectories.
  * `CreationInfo$Permissions` -> shape `com.amazonaws.efs#Permissions`, @length min 3 max 4, @pattern `^[0-7]{3,4}$` — an octal mode, so no digit 8 or 9.
A request violating either constraint never reaches the operation on AWS: it is refused at constraint validation. Recorded behavior (the check's expectation, which is what the real-AWS recording produced) is error code `ValidationException` at HTTP 400. `ValidationException` is NOT one of this operation's modeled errors (`AccessPointAlreadyExists`, `AccessPointLimitExceeded`, `BadRequest`, `FileSystemNotFound`, `IncorrectFileSystemLifeCycleState`, `InternalServerError`, `ThrottlingException`) — it is the protocol-level constraint-violation refusal the framework owes for an input that fails an @pattern/@length trait, so it belongs in the request-validation stage, not in this handler's modeled error set. Its message is AWS's standard validation sentence, whose FIXED text is `N validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>` and whose VARIABLE parts are taken entirely from the request and the model: the count N of violations, the rejected value as sent, the lowerCamel member path (`rootDirectory.path`, `rootDirectory.creationInfo.permissions`) and the pattern from the model. The third case the check exercises — a well-formed `RootDirectory` against a non-existent `FileSystemId` — is already correct: the emulator answers `FileSystemNotFound` (HTTP 404).

Emulator: the call SUCCEEDS. The replay fails inside the check's `_error_code` helper with `AssertionError: the call succeeded; real AWS rejects it, so there is no error code`, raised on the `CreateAccessPoint` call in the case loop, so no refusal of any kind was produced. The handler validates only two things of its own invention — that a supplied `RootDirectory` carries a `Path` at all, and the tag list — and then walks straight to minting: the request's `RootDirectory` is stored verbatim and an `fsap-…` access point is committed to state. Neither the `Path` pattern nor the `Permissions` pattern is ever checked, by the handler or by the request walk ahead of it. Because the refusal is missing, the access point is also LEFT BEHIND, which breaks the check's second assertion (`refused requests left access points on <fs-id>`): the file system holds an access point that AWS would never have created, and the subsequent positive control, which requires the valid access point to be the ONLY one on the file system, cannot hold either.

Why: a client that relies on AWS rejecting a bad root directory gets silent corruption instead of an error. Code that validates user input by round-tripping it through `CreateAccessPoint` (Kubernetes EFS dynamic provisioning, IaC drift checks, admission controllers) sees success for a relative path such as `dynamic_provisioning/bad` or a non-octal mode such as `999`, so it records an access point id that cannot be mounted, and it never surfaces the `ValidationException` its error handling is written against. Worse, the emulator's state now diverges permanently: tests that assert 'a refused create leaves nothing behind' pass against AWS and fail here, and any later listing or count of access points on that file system is wrong.

Fix: the input-validation stage for `CreateAccessPoint` — the @pattern/@length constraints on the shapes reached through `RootDirectory` (`Path`, `CreationInfo$Permissions`, and for consistency the @range on `OwnerUid`/`OwnerGid`) must be enforced before `handle` runs, refusing with the protocol-level `ValidationException` at HTTP 400 and AWS's validation sentence, so no state is written. Enforcing it in the shared request walk (deserialize -> validate -> handle) rather than inside this one handler is what makes every other EFS operation inherit the same behavior; the handler's own hand-rolled `RootDirectory requires a Path when it is specified` BadRequest should be reconciled with that stage rather than duplicated beside it.

No open issue covers this: the filed EFS issues concern BackupPolicy/FileSystemPolicy/UpdateFileSystem 501s and messages (20261022T154034Z-efs-control-plane-backup-policy-file-system-policy.md, 20261029T173204Z-efs-updatefilesystem-badrequest-message-for-a-provisioned-mode.md), mount-target subnet state, replication not-found refusals, and the efs__lambda flows — none of them touch CreateAccessPoint input validation.

Traces: efs.efs-access-point-invalid-root-directory-rejected

## Evidence
Check: efs.efs-access-point-invalid-root-directory-rejected [mismatch]

The check drives three refusal cases against one available, encrypted file system and then a positive control:
  1. RootDirectory={"Path": "dynamic_provisioning/bad"}                      -> expected ValidationException
  2. RootDirectory={"Path": "/dynamic_provisioning/ok", "CreationInfo": {"OwnerUid": 1000, "OwnerGid": 1000, "Permissions": "999"}} -> expected ValidationException
  3. FileSystemId="fs-0123456789abcdef0" with a valid RootDirectory        -> expected FileSystemNotFound

Expected (real AWS): each CreateAccessPoint raises a ClientError; cases 1 and 2 carry Error.Code == "ValidationException" at HTTP 400 with the standard validation sentence, e.g. for case 1 the fixed text `1 validation error detected: Value '...' at 'rootDirectory.path' failed to satisfy constraint: Member must satisfy regular expression pattern: ...` with the value taken from the request and the pattern from the model; and DescribeAccessPoints on the file system then returns [] because a refused create writes nothing.

Actual (emulator): CreateAccessPoint returns HTTP 200 with a minted AccessPointId/AccessPointArn. Replay output:

  File "check_efs_efs_access_point_invalid_root_directory_rejected.py", line 33, in run
    code = _error_code(lambda t=target, r=root: efs.create_access_point(
  File "check_efs_efs_access_point_invalid_root_directory_rejected.py", line 57, in _error_code
    raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
  AssertionError: the call succeeded; real AWS rejects it, so there is no error code

No error code at all was produced, so the divergence is 200-vs-400 and a missing ValidationException, not a wrong code or wrong message. Because the call succeeded, the access point is committed to state (services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/create_access_point.rs:354-357 inserts the client token and the access point), which additionally breaks the check's `left == []` assertion and its 'the control must be the only access point' barrier.

Handler evidence: services/service-com-amazonaws-efs-magnolioapiservice-v20150201/src/ops/create_access_point.rs:220-233 is the ONLY inspection of RootDirectory — it checks `requested.path.is_none()` and otherwise passes the structure through untouched; lines 237-240 validate the tag list; lines 273-318 check file-system existence, lifecycle and the access-point cap. Nothing anywhere in the handler compares `Path` against `com.amazonaws.efs#Path`'s @pattern or `Permissions` against `com.amazonaws.efs#Permissions`'s `^[0-7]{3,4}$`.

## Steps to Reproduce
1. Call CreateFileSystem (Encrypted=true) and poll DescribeFileSystems until LifeCycleState == "available"; note <fs-id>.
2. Call CreateAccessPoint with FileSystemId=<fs-id>, a fresh ClientToken, and RootDirectory={"Path": "dynamic_provisioning/bad"} (a RELATIVE path, violating com.amazonaws.efs#Path's @pattern).
   - AWS: HTTP 400, Error.Code == "ValidationException", message is the standard validation sentence naming 'rootDirectory.path' and the pattern.
   - Emulator: HTTP 200 with a minted AccessPointId.
3. Call CreateAccessPoint with FileSystemId=<fs-id>, a fresh ClientToken, and RootDirectory={"Path": "/dynamic_provisioning/ok", "CreationInfo": {"OwnerUid": 1000, "OwnerGid": 1000, "Permissions": "999"}} (non-octal mode, violating com.amazonaws.efs#Permissions' ^[0-7]{3,4}$).
   - AWS: HTTP 400, Error.Code == "ValidationException" naming 'rootDirectory.creationInfo.permissions'.
   - Emulator: HTTP 200 with a minted AccessPointId.
4. Call DescribeAccessPoints(FileSystemId=<fs-id>) and page NextToken to the end.
   - AWS: AccessPoints == [] — both refused creates wrote nothing.
   - Emulator: the access points minted in steps 2 and 3 are listed.
5. Control that still passes: CreateAccessPoint with FileSystemId="fs-0123456789abcdef0" and a valid RootDirectory correctly yields FileSystemNotFound (HTTP 404) on both.
6. Replay: run_behavioral_suite and observe efs.efs-access-point-invalid-root-directory-rejected.
