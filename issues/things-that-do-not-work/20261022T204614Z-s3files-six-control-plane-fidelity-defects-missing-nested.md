Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3files.create-access-point-invalid-root-directory-path-rejected s3files.create-file-system-token-reuse-different-params-rejected s3files.create-file-system-unknown-role-not-found s3files.get-file-system-policy-not-found-when-absent s3files.create-access-point-unknown-file-system-not-found s3files.delete-file-system-transitions-to-deleted s3files.put-synchronization-configuration-multiple-expiration-rules-rejected
Issue-Shape-Id: com.amazonaws.s3files#S3Files
Issue-Key: s3files.create-access-point-invalid-root-directory-path-rejected|_error#5|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3files.create-file-system-token-reuse-different-params-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: s3files.create-file-system-unknown-role-not-found|run#9|AssertionError|assert final["status"] == "error", (
Issue-Key: s3files.get-file-system-policy-not-found-when-absent|run#8|AssertionError|assert get_code == "ResourceNotFoundException", f"expected ResourceNotFoundException for a fs with no policy, got {get_code!r}"
Issue-Key: s3files.create-access-point-unknown-file-system-not-found|run#8|AssertionError|assert message == "File system does not exist.", f"expected message 'File system does not exist.', got {message!r}"
Issue-Key: s3files.delete-file-system-transitions-to-deleted|run#8|AssertionError|assert message == "File system does not exist.", f"expected message 'File system does not exist.', got {message!r}"
Issue-Key: s3files.put-synchronization-configuration-multiple-expiration-rules-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: f973f78f74c99aad4e85f2736f1fde89ed7327e3

# s3files: six control-plane fidelity defects — missing nested-member validation, clientToken reuse not refused, no role validation in lifecycle, GetFileSystemPolicy still a 501 stub, wrong not-found message, collection-summarizing validation message

> _Note: this issue was identified against the code base at commit `f973f78f74c99aad4e85f2736f1fde89ed7327e3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAccessPoint, CreateFileSystem, GetFileSystem, GetFileSystemPolicy, PutSynchronizationConfiguration

## Description
Six distinct root causes in the s3files emulator, all grounded in failing behavioral checks of this suite. Each must be closed before submitting.

NOTE on scope: checks blocked by unimplemented scaffold stubs (DeleteAccessPoint, ListAccessPoints, PutFileSystemPolicy, PutSynchronizationConfiguration, TagResource, ec2 CreateSubnet) are marked ALREADY FILED by the suite and are NOT part of this issue. The open issue '20261022T110621Z-s3files-createfilesystem-accepts-a-nonexistent-bucket-arn-and-creates.md' (CreateFileSystem accepting a nonexistent bucket ARN) already owns s3files.create-file-system-unknown-bucket-not-found; Cause 3 below is a DIFFERENT CreateFileSystem defect (the roleArn is never dereferenced) and Cause 2 is a third one (clientToken dedupe), so neither is covered by it.

--- Cause 1 of 6: nested structure-member constraints (@pattern/@length) are not enforced on CreateAccessPoint.rootDirectory.path ---
AWS: Smithy model com.amazonaws.s3files#RootDirectory member `path` targets `Path` with @length min 1 max 100 and @pattern ^(\/|(\/(?!\.)+[^$#<>;;`|&?{}^*/\n]+){1,4})$. A constraint violation is the service-common `ValidationException` (errorKind client, httpError 400). The recording shows a non-absolute path rejected with ValidationException and the standard constraint message naming the member and the regex, e.g. "1 validation error detected: Value 'not-an-absolute-path' at 'rootDirectory.path' failed to satisfy constraint: Member must satisfy regular expression pattern: ..." (that text is an example; the requirement is 400 + ValidationException raised by the modeled pattern).
Emulator: CreateAccessPoint with rootDirectory={"path":"not-an-absolute-path"} returns HTTP 200 and CREATES the access point, echoing rootDirectory.path back verbatim (accessPointId fsap-..., status 'creating'). The top-level list-length constraint on PutSynchronizationConfiguration IS enforced (see Cause 6), so the constraint walk is not reaching members of nested structures.
Why: a client that relies on the service refusing malformed POSIX root paths gets an access point whose rootDirectory can never be mounted, and the error it codes a retry/repair path for never arrives.
Fix: the centralized deserialize→validate step for this service's operations must recurse into nested structure members (CreateAccessPoint input validation), not only top-level members.
Traces: s3files.create-access-point-invalid-root-directory-path-rejected

--- Cause 2 of 6: CreateFileSystem clientToken is not a dedupe guard — token reuse creates a second file system instead of 409 ConflictException ---
AWS: Smithy: CreateFileSystemRequest.clientToken (CreationToken, @idempotencyToken, @length 1..64); CreateFileSystem models `ConflictException` (errorKind client, httpError 409). Recorded behavior: once a clientToken has been consumed by a successful CreateFileSystem, any later CreateFileSystem carrying that same token is rejected with 409 ConflictException — both when the other parameters DIFFER and when they are IDENTICAL (it is a single-use dedupe guard, not an idempotent replay). The message is built from state and names the owning file system, e.g. "File system 'fs-<id>' already exists with this clientToken."
Emulator: the second CreateFileSystem with the same clientToken and a different bucket/roleArn returns HTTP 201 and creates a NEW file system (new fileSystemId, new ARN), with the reused clientToken echoed in the response. No conflict detection on clientToken at all.
Why: a retried or duplicated create silently provisions duplicate file systems against different buckets; a client that treats 409 as "already created, reuse it" instead ends up with orphaned resources and a wrong fileSystemId.
Fix: CreateFileSystem's handler/state must record consumed clientTokens per account and refuse a reuse with ConflictException naming the owning fileSystemId.
Traces: s3files.create-file-system-token-reuse-different-params-rejected

--- Cause 3 of 6: a nonexistent roleArn is never dereferenced during provisioning — the file system reaches 'available' instead of 'error' ---
AWS: Smithy: CreateFileSystemRequest.roleArn is required with pattern ^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$; GetFileSystemResponse.status targets LifeCycleState whose values include 'error' and 'misconfigured', alongside the optional `statusMessage` ("Additional information about the file system status"). Recorded behavior: a well-formed but nonexistent IAM role ARN is accepted synchronously (the role is not dereferenced at create time), then the file system transitions asynchronously to status 'error' with a non-empty statusMessage describing the role validation/access-denied failure — it never reaches 'available'.
Emulator: GetFileSystem reports status 'available' for a file system created with a role ARN that does not exist, and no statusMessage.
Why: the whole 'error'/statusMessage branch of the lifecycle is unreachable, so a client (or waiter) that polls for a terminal state to surface a misconfigured role sees a healthy file system and proceeds to mount a file system that can never sync.
Fix: the file-system lifecycle state machine (CreateFileSystem's async status progression in this service's state) must validate roleArn existence and drive the resource to 'error' with statusMessage instead of unconditionally to 'available'.
Traces: s3files.create-file-system-unknown-role-not-found

--- Cause 4 of 6: GetFileSystemPolicy is still a 501 NotImplemented scaffold, so policy-absence cannot be expressed ---
AWS: Smithy: GetFileSystemPolicy GET /file-systems/{fileSystemId}/policy, output GetFileSystemPolicyResponse{fileSystemId, policy both required}, errors ResourceNotFoundException (httpError 404). Recorded behavior: on a file system that never had a resource policy, GetFileSystemPolicy is rejected 404 ResourceNotFoundException (recorded message example: "File system policy does not exist."), and DeleteFileSystemPolicy on such a file system is idempotent (no error).
Emulator: GetFileSystemPolicy answers HTTP 501 with error code 'NotImplemented' ("operation GetFileSystemPolicy is scaffolded but not implemented in this emulator"). The suite did NOT mark this check as covered by a stub issue — the auto-filed stub issue for this check's service names PutFileSystemPolicy, not GetFileSystemPolicy — so this operation is uncovered.
Why: a client cannot distinguish "no policy set" from "operation unavailable"; a 501 is not retryable or interpretable, and policy-absence handling (the common "get, 404 means create" flow) cannot be exercised at all.
Fix: implement the handler at services/service-com-amazonaws-s3files-s3files/src/ops/get_file_system_policy.rs (currently returns crate::scaffolded::Scaffolded::not_implemented, see its SCAFFOLDED marker at the file head) to read the policy from state and return ResourceNotFoundException when absent.
Traces: s3files.get-file-system-policy-not-found-when-absent

--- Cause 5 of 6: ResourceNotFoundException for an unknown/deleted fileSystemId carries an internal diagnostic message instead of the service's fixed sentence ---
AWS: Smithy: ResourceNotFoundException (errorKind client, httpError 404). Recorded behavior: for an unknown or already-deleted fileSystemId the service returns a FIXED, condition-specific sentence that does not embed any request value — the identical message "File system does not exist." is returned by GetFileSystem after a delete and by CreateAccessPoint against the same dead/bogus id. The rule is therefore: one constant message per condition, independent of the requested id.
Emulator: returns ResourceNotFoundException (code correct) with a message that embeds the requested id and reads as an internal diagnostic — 'no file system fs-00000000000000000' and 'no file system fs-ec5389ae5026cd60b'.
Why: the message is request-dependent where AWS's is constant, so clients and test fixtures that branch or assert on the not-found reason see a different message per call and cannot normalize it; it also leaks internal phrasing instead of the service's API-surface wording.
Fix: the shared not-found error construction in this service's state/handler layer (GetFileSystem, CreateAccessPoint, and every operation that resolves a fileSystemId) should emit the fixed condition message rather than formatting the looked-up id.
Traces: s3files.create-access-point-unknown-file-system-not-found, s3files.delete-file-system-transitions-to-deleted

--- Cause 6 of 6: constraint-violation messages summarize a rejected collection ('2 items') instead of rendering the offending value ---
AWS: Smithy: PutSynchronizationConfigurationRequest.expirationDataRules targets ExpirationDataRuleList with @length min 1 max 1, required; violation is the service-common ValidationException (httpError 400). The message is built from the request by AWS's standard constraint formatter: "1 validation error detected: Value '<the rejected value>' at '<member path>' failed to satisfy constraint: Member must have length less than or equal to 1" — i.e. the actual value is rendered, e.g. '[ExpirationDataRule(daysAfterLastAccess=30), ExpirationDataRule(daysAfterLastAccess=60)]'. That rendering rule, not the literal recorded text, is the requirement.
Emulator: emits the same 400/ValidationException and the same template but substitutes a count summary for the value: "1 validation error detected: Value '2 items' at 'expirationDataRules' failed to satisfy constraint: Member must have length less than or equal to 1".
Why: the validation error no longer tells the caller WHICH values were rejected, so a client that surfaces or parses the offending value out of the message (the whole purpose of AWS embedding it) loses the diagnostic for every list- and map-typed constraint, not just this member.
Fix: the shared constraint-violation message formatter in this service's validation layer must render collection values element-wise, as it already does for scalars (compare Cause 1's expected scalar rendering).
Traces: s3files.put-synchronization-configuration-multiple-expiration-rules-rejected

## Evidence
Suite: simparity-behavioral-aws-v1, s3files 2/23 passed (21 failed).

Cause 1 — s3files.create-access-point-invalid-root-directory-path-rejected [mismatch]
  Expected: ValidationException (400) for rootDirectory.path 'not-an-absolute-path'.
  Actual: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'accessPointId': 'fsap-...', 'status': 'creating', 'rootDirectory': {'path': 'not-an-absolute-path'}}".

Cause 2 — s3files.create-file-system-token-reuse-different-params-rejected [mismatch]
  Expected: ConflictException (409), message "File system '<fs-id>' already exists with this clientToken."
  Actual: "expected the call to be rejected, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'fileSystemId': 'fs-7bacecd57feeccdca', 'clientToken': 'sim2real-s3files-token-...', 'status': 'creating'}" — a second file system against a different bucket with the same token.

Cause 3 — s3files.create-file-system-unknown-role-not-found [mismatch]
  Expected: GetFileSystem eventually reports status 'error' with a non-empty statusMessage.
  Actual: AssertionError: "expected a nonexistent role to drive the file system to 'error' status, got 'available'" (polled to 900s).

Cause 4 — s3files.get-file-system-policy-not-found-when-absent [mismatch]
  Expected: ResourceNotFoundException (404) from GetFileSystemPolicy on a file system with no policy.
  Actual: AssertionError: "expected ResourceNotFoundException for a fs with no policy, got 'NotImplemented'" (HTTP 501 scaffold stub; see get_file_system_policy.rs line 162, `Scaffolded::not_implemented("GetFileSystemPolicy")`).

Cause 5 — s3files.create-access-point-unknown-file-system-not-found [mismatch]
  Expected message: 'File system does not exist.'  Actual: 'no file system fs-00000000000000000' (code ResourceNotFoundException was correct).
  s3files.delete-file-system-transitions-to-deleted [mismatch]
  Expected message: 'File system does not exist.'  Actual: 'no file system fs-ec5389ae5026cd60b'.
  Same constant sentence is expected from two different operations with different ids, so the message is condition-fixed and id-independent on AWS.

Cause 6 — s3files.put-synchronization-configuration-multiple-expiration-rules-rejected [mismatch]
  Expected: "1 validation error detected: Value '[ExpirationDataRule(daysAfterLastAccess=30), ExpirationDataRule(daysAfterLastAccess=60)]' at 'expirationDataRules' failed to satisfy constraint: Member must have length less than or equal to 1"
  Actual:   "1 validation error detected: Value '2 items' at 'expirationDataRules' failed to satisfy constraint: Member must have length less than or equal to 1"

Model references used: smithy_query digest + shapes RootDirectory, CreateFileSystemRequest, GetFileSystemResponse, LifeCycleState, PutSynchronizationConfigurationRequest, GetFileSystemPolicyResponse, ResourceNotFoundException, ConflictException, ValidationException in services/service-com-amazonaws-s3files-s3files/smithy-model/com-amazonaws-s3files-s3files.json.

## Steps to Reproduce
Boot the emulator and run the s3files behavioral checks (all steps use the s3files client against a versioned bucket + an s3files role, and a file system polled to 'available' unless stated).

Cause 1: create_access_point(fileSystemId=<available fs>, posixUser={'uid':1000,'gid':1000}, rootDirectory={'path':'not-an-absolute-path'}) -> observe 200 + a created access point; expect 400 ValidationException naming rootDirectory.path and the modeled pattern.

Cause 2: token = <unique>; create_file_system(bucket=<bucket1 arn>, roleArn=<role1>, clientToken=token) -> 201. Then create_file_system(bucket=<bucket2 arn>, roleArn=<role2>, clientToken=token) -> observe 201 and a second fileSystemId; expect 409 ConflictException. Repeat the call with the ORIGINAL identical params and the same token -> also expect 409.

Cause 3: create_file_system(bucket=<bucket arn>, roleArn='arn:aws:iam::<account>:role/<name-that-does-not-exist>') -> accepted. Poll get_file_system(fileSystemId=...) -> observe it settle on status 'available'; expect status 'error' with a non-empty statusMessage.

Cause 4: on a file system that never had a policy, get_file_system_policy(fileSystemId=...) -> observe HTTP 501 code 'NotImplemented'; expect 404 ResourceNotFoundException. Also delete_file_system_policy(fileSystemId=...) must succeed idempotently.

Cause 5: (a) create_access_point(fileSystemId='fs-00000000000000000', posixUser={'uid':1000,'gid':1000}, rootDirectory={'path':'/'}) -> ResourceNotFoundException with message 'no file system fs-00000000000000000'; expect 'File system does not exist.'. (b) delete_file_system(fileSystemId=<available fs>) then poll get_file_system on that id -> ResourceNotFoundException with message 'no file system <id>'; expect the same fixed sentence.

Cause 6: put_synchronization_configuration(fileSystemId=<fs>, importDataRules=[<one valid rule>], expirationDataRules=[{'daysAfterLastAccess':30},{'daysAfterLastAccess':60}]) -> observe the ValidationException message rendering the list as '2 items'; expect the rejected list value itself to be rendered in the message.
