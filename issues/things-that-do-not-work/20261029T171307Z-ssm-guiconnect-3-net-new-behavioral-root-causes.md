Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm-guiconnect.delete-not-configured-is-idempotent ssm-guiconnect.get-not-configured-raises-resource-not-found ssm-guiconnect.get-denied-without-iam-permission ssm-guiconnect.update-access-denied-reason-differs-for-iam-vs-jit-gate ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-name ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-owner
Issue-Shape-Id: com.amazonaws.ssmguiconnect#SSMGuiConnect
Issue-Key: ssm-guiconnect.delete-not-configured-is-idempotent|run#5|AssertionError|assert request_id, "a successful Delete response must carry a RequestId in ResponseMetadata"
Issue-Key: ssm-guiconnect.get-not-configured-raises-resource-not-found|run#8|AssertionError|assert error1.response.get("ResponseMetadata", {}).get("RequestId"), (
Issue-Key: ssm-guiconnect.get-denied-without-iam-permission|run#13|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got {code}"
Issue-Key: ssm-guiconnect.update-access-denied-reason-differs-for-iam-vs-jit-gate|run#22|AssertionError|assert message == expected, f"expected the IAM-permission denial {expected!r}, got: {message!r}"
Issue-Key: ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-name|run#10|AssertionError|assert bad_message == _BAD_BUCKET_NAME, f"unexpected ValidationException message: {bad_message!r}"
Issue-Key: ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-owner|run#10|AssertionError|assert message == _BAD_BUCKET_OWNER, f"unexpected ValidationException message: {message!r}"
Issue-Commit: ff63bbd76f191efb9689374a4f98215607f4bd70

# ssm-guiconnect: 3 net-new behavioral root causes — no ResponseMetadata RequestId on any response, IAM identity-based authorization never evaluated, and @pattern ValidationException messages use the framework envelope instead of the service's "isn't valid" sentence

> _Note: this issue was identified against the code base at commit `ff63bbd76f191efb9689374a4f98215607f4bd70`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateConnectionRecordingPreferences

## Description
Three independent root causes in the ssm-guiconnect (SSM GuiConnect, restJson1, sdkId "SSM GuiConnect") crate, grounded in 6 failing behavioral checks. All three are in one service and are filed as one bundled work item; each must be closed before submitting.

=== Cause 1 of 3: no request id on any response, success or error, so ResponseMetadata.RequestId is absent ===

AWS: Every ssm-guiconnect response — the 200 of DeleteConnectionRecordingPreferences and the 404 ResourceNotFoundException of GetConnectionRecordingPreferences alike — carries a service request id, which botocore surfaces as response["ResponseMetadata"]["RequestId"] (and, for a ClientError, error.response["ResponseMetadata"]["RequestId"]). Smithy: the service is restJson1 (protocols: ["restJson1"]) with no modeled output member for the id, so the id is a protocol-level envelope value carried in the response headers (x-amzn-RequestId), not a modeled member — recorded behavior only for the header, and the recordings for both the 200 Delete path and the 404 Get path show a populated RequestId. The VALUE is per-run and is not a requirement; its PRESENCE and well-formedness are.

Emulator: ResponseMetadata carries no RequestId at all. ssm-guiconnect.delete-not-configured-is-idempotent reaches the correct success path (the Delete on an unconfigured account does succeed, which is right) and then fails on `assert request_id, "a successful Delete response must carry a RequestId in ResponseMetadata"`. ssm-guiconnect.get-not-configured-raises-resource-not-found gets the correct code (ResourceNotFoundException) and the correct message ("The following  resource cannot be found: 'ConnectionRecordingPreferences'", doubled space included) and then fails on `assert error1.response.get("ResponseMetadata", {}).get("RequestId")`. So both the success serializer and the error serializer omit the id — it is one omission on the shared response path, not two.

Why: a client that logs or correlates the request id (every AWS SDK retry/diagnostic path, and any caller echoing the id into its own logs for support) gets None instead of an id. Anything asserting on the presence of ResponseMetadata.RequestId — the normal shape of an AWS response envelope — breaks against the emulator even when the status, code and message are all correct.

Fix: the response-envelope serialization for this crate — the shared restJson1 response path the generated ServiceDefinition renders through (reached via services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/service.rs `definition()`), for BOTH the Ok and the Err arm. No handler change: neither src/ops/delete_connection_recording_preferences.rs nor src/ops/get_connection_recording_preferences.rs can set an envelope header.

Traces: ssm-guiconnect.delete-not-configured-is-idempotent (success path), ssm-guiconnect.get-not-configured-raises-resource-not-found (error path).

=== Cause 2 of 3: IAM identity-based authorization is never evaluated, so a principal with zero ssm-guiconnect permissions is served ===

AWS: a caller whose identity has NO ssm-guiconnect identity-based permission is refused with the modeled AccessDeniedException (Smithy: AccessDeniedException, errorKind client, httpError 403, member `message` required; it is a service-common error bound to all three operations) BEFORE the operation runs — so before the resource is looked up and before the account-level just-in-time-node-access gate is consulted. The message is the standard IAM denial sentence; its FIXED text is "User: " / " is not authorized to perform: " / " on resource: " / " because no identity-based policy allows the " / " action", and its VARIABLE parts come from the request and from state: the calling principal's ARN, the action `ssm-guiconnect:<OperationName>` (named twice — once after "perform:" and once before " action"), and the resource ARN `arn:<partition>:ec2:<region>:<account>:instance/*` built from the caller's partition, the request's Region and the caller's account id. Granting that same principal an `ssm-guiconnect:*` allow policy must restore the identical call to the same outcome the permissioned principal gets. Recorded behavior for the message template (the model documents only "You do not have sufficient access to perform this action."); the recordings for both Get and Update show exactly that template.

Emulator: the caller's identity is ignored entirely; every request runs as if fully permissioned.
  * GetConnectionRecordingPreferences from a brand-new IAM user with no policies attached returns 404 ResourceNotFoundException "The following  resource cannot be found: 'ConnectionRecordingPreferences'" instead of 403 AccessDeniedException — ssm-guiconnect.get-denied-without-iam-permission fails with "expected AccessDeniedException, got ResourceNotFoundException". This also LEAKS the existence (here, non-existence) of the preferences singleton to an unauthorized caller.
  * UpdateConnectionRecordingPreferences from the same unpermissioned principal returns 403 AccessDeniedException but with the JIT-gate message "Just-in-time node access is not enabled. You must set up just-in-time node access to record RDP connections." instead of the IAM denial sentence — ssm-guiconnect.update-access-denied-reason-differs-for-iam-vs-jit-gate fails with "expected the IAM-permission denial 'User: … is not authorized to perform: ssm-guiconnect:UpdateConnectionRecordingPreferences on resource: arn:aws:ec2:<region>:<account>:instance/* because no identity-based policy allows the ssm-guiconnect:UpdateConnectionRecordingPreferences action', got: 'Just-in-time node access is not enabled. …'". The code happens to match only because the JIT gate also denies; the cause is collapsed. The two distinct denial causes must not share a message.

Why: an unauthorized caller is served (and, on Get, told about resource state it has no permission to see). A client or test that proves least-privilege — "this role is denied, grant it the policy, now it works" — passes against the emulator for the wrong reason or fails outright, and a caller branching on the AccessDeniedException message cannot tell an IAM-permission problem (fix the policy) from the account-level JIT enrollment gate (enroll the account), which are completely different remediations.

Fix: an authorization check ahead of each handler's body in this crate — the shared gate belongs in src/state.rs next to the existing just_in_time_node_access_enabled gate and must run BEFORE it (and before the resource lookup) in src/ops/get_connection_recording_preferences.rs, src/ops/update_connection_recording_preferences.rs and src/ops/delete_connection_recording_preferences.rs; it needs the calling principal from the request context (`ctx`, smithy_server_runtime::OperationContext) resolved against the IAM identity-based policies the emulator already stores for that user.

Traces: ssm-guiconnect.get-denied-without-iam-permission, ssm-guiconnect.update-access-denied-reason-differs-for-iam-vs-jit-gate.

=== Cause 3 of 3: a @pattern violation on an S3Bucket member is worded with the framework's aggregated envelope (and echoes the offending value) instead of this service's "isn't valid" sentence ===

AWS: ssm-guiconnect refuses a malformed S3Bucket member of UpdateConnectionRecordingPreferences with ValidationException (Smithy: ValidationException, errorKind client, httpError 400, `message` required) worded as ONE sentence that names the member by its MODEL path and withholds the offending value:
  "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName isn't valid."
  "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketOwner isn't valid."
Fixed text: "The value provided for " and " isn't valid." — the only variable part is the dotted model member path of the member that failed, in the model's own PascalCase spelling and with no list index. The constraints themselves are modeled: S3Bucket$BucketName @pattern "(?=^.{3,63}$)(?!^(\\d+\\.)+\\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])\\.)*([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])$)" (required) and S3Bucket$BucketOwner -> AccountId @pattern "^[0-9]{12}$" (required). This refusal precedes the account-level just-in-time-node-access gate: the positive control in both checks confirms a well-shaped request does reach that gate and is denied 403 AccessDeniedException with the JIT message, so the ValidationException is attributable to the member's shape. Recorded behavior for the sentence; the model declares only the code and the patterns.

Emulator: both members are refused with the correct code (ValidationException) but with the framework's aggregated envelope, which also echoes the value and the raw regex:
  BucketName "192.168.1.1" -> "1 validation error detected: Value '192.168.1.1' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.1.member.bucketName' failed to satisfy constraint: Member must satisfy regular expression pattern: (?=^.{3,63}$)(?!^(\\d+\\.)+\\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])\\.)*([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])$)"
  BucketOwner "abcdefghijkl" -> "1 validation error detected: Value 'abcdefghijkl' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.1.member.bucketOwner' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]{12}$"
Three divergences in one wording: the "N validation error detected" prefix AWS does not use, the lowerCamel framework path with the ".1.member." list-index segment instead of the model's dotted PascalCase path, and the echo of the caller's value plus the server-side regex. This is one root cause reachable by two code paths that both end at the same renderer: crate::state::constraint_message (src/state.rs) words only @length violations and returns None for ViolationKind::Pattern, so service.rs's override_refusal keeps the framework default for the boundary-enforced BucketOwner pattern; and crate::state::refuse_preferences (src/state.rs, the hand-rolled BucketName check, since that pattern does not compile) deliberately builds its message with the framework's `envelope(&violations)` renderer, value echo included.

Why: a client that matches or displays the ValidationException message — and any caller that parses which member it got wrong out of the message, since the message is the only place the member is named — sees a message AWS never emits, with a different member path spelling. The value echo additionally reflects caller input and the server's internal regex back into an error body AWS keeps free of both.

Fix: src/state.rs — extend constraint_message to word a ViolationKind::Pattern violation as the "The value provided for <ModelMemberPath> isn't valid." sentence (so service.rs's existing override_refusal seam covers the boundary-validated BucketOwner), and rebuild refuse_preferences' message with the same words instead of the framework's envelope/value echo, mapping the framework violation path to the model's dotted PascalCase member path with the list-index segment dropped.

Traces: ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-name, ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-owner.

## Evidence
Suite: 9/15 passed (60.0%), 6 mismatched, 0 errored, 0 skipped, 0 unloadable. All 6 failures are in ssm-guiconnect; none was marked ALREADY FILED or OWNED by.

Cause 1 — missing ResponseMetadata.RequestId:
  [mismatch] ssm-guiconnect.delete-not-configured-is-idempotent
    AssertionError: a successful Delete response must carry a RequestId in ResponseMetadata
    expected: DeleteConnectionRecordingPreferences 200 with ResponseMetadata.RequestId present (any well-formed id)
    actual:   200, ResponseMetadata.RequestId absent/empty
  [mismatch] ssm-guiconnect.get-not-configured-raises-resource-not-found
    AssertionError: expected a RequestId in ResponseMetadata on the error response
    expected: 404 ResourceNotFoundException, message "The following  resource cannot be found: 'ConnectionRecordingPreferences'", ResponseMetadata.RequestId present
    actual:   correct code and correct message, ResponseMetadata.RequestId absent/empty

Cause 2 — IAM authorization never evaluated:
  [mismatch] ssm-guiconnect.get-denied-without-iam-permission
    AssertionError: expected AccessDeniedException, got ResourceNotFoundException
    expected: a freshly created IAM user with no policies calling GetConnectionRecordingPreferences -> 403 AccessDeniedException, message "User: <caller-arn> is not authorized to perform: ssm-guiconnect:GetConnectionRecordingPreferences on resource: arn:<partition>:ec2:<region>:<account>:instance/* because no identity-based policy allows the ssm-guiconnect:GetConnectionRecordingPreferences action"
    actual:   404 ResourceNotFoundException "The following  resource cannot be found: 'ConnectionRecordingPreferences'" — identical to the permissioned principal's answer, i.e. the identity is ignored and resource state is disclosed
  [mismatch] ssm-guiconnect.update-access-denied-reason-differs-for-iam-vs-jit-gate
    AssertionError: expected the IAM-permission denial 'User: arn:aws:iam::<account>:user/<user> is not authorized to perform: ssm-guiconnect:UpdateConnectionRecordingPreferences on resource: arn:aws:ec2:<region>:<account>:instance/* because no identity-based policy allows the ssm-guiconnect:UpdateConnectionRecordingPreferences action', got: 'Just-in-time node access is not enabled. You must set up just-in-time node access to record RDP connections.'
    expected: unpermissioned principal -> 403 AccessDeniedException with the IAM denial sentence, materially different from the JIT-gate sentence the permissioned principal gets
    actual:   403 AccessDeniedException with the JIT-gate sentence — both denial causes collapsed onto one message
    (the permissioned principal's positive controls in the same check PASSED: Get -> ResourceNotFoundException with the right message, Update -> AccessDeniedException with the JIT message)

Cause 3 — @pattern ValidationException wording:
  [mismatch] ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-name
    AssertionError: unexpected ValidationException message: "1 validation error detected: Value '192.168.1.1' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.1.member.bucketName' failed to satisfy constraint: Member must satisfy regular expression pattern: (?=^.{3,63}$)(?!^(\\d+\\.)+\\d+$)(^(([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])\\.)*([a-z0-9]|[a-z0-9][a-z0-9\\-]*[a-z0-9])$)"
    expected: 400 ValidationException, message "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName isn't valid."
  [mismatch] ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-owner
    AssertionError: unexpected ValidationException message: "1 validation error detected: Value 'abcdefghijkl' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.1.member.bucketOwner' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9]{12}$"
    expected: 400 ValidationException, message "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketOwner isn't valid."
    In both, the code (ValidationException) and the ordering (shape validation ahead of the JIT gate) are already correct; only the message wording diverges.

Supporting source (file/line citations only, no patch):
  services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs:337-339 — constraint_message words only @length violations; a ViolationKind::Pattern yields None.
  services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/state.rs:499-519 — refuse_preferences builds the BucketName refusal through the framework's envelope(&violations) with value: Some(echoed(..)).
  services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/service.rs:111-120 — override_refusal returns None for anything constraint_message does not word, keeping the framework envelope; authentication/authorization refusals are explicitly "not this service's to reword".
  services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/update_connection_recording_preferences.rs:168-193 — validation then the JIT gate; no caller-identity authorization check anywhere ahead of them.
  services/service-com-amazonaws-ssmguiconnect-ssmguiconnect/src/ops/delete_connection_recording_preferences.rs:133-205 — no caller-identity authorization check; returns Ok with no envelope request id.

## Steps to Reproduce
Boot the emulator from the checkout and point a botocore/boto3 ssm-guiconnect client at it in the suite's Region.

Cause 1 (RequestId):
  1. resp = guiconnect.delete_connection_recording_preferences()  -> 200. Inspect resp["ResponseMetadata"]; RequestId is absent. AWS always populates it.
  2. try: guiconnect.get_connection_recording_preferences() except ClientError as e: inspect e.response["ResponseMetadata"]; RequestId is absent, although Error.Code (ResourceNotFoundException) and Error.Message ("The following  resource cannot be found: 'ConnectionRecordingPreferences'") are correct.
  Replay: ssm-guiconnect.delete-not-configured-is-idempotent, ssm-guiconnect.get-not-configured-raises-resource-not-found.

Cause 2 (IAM authorization):
  1. With the suite's permissioned principal, confirm the positive control: guiconnect.get_connection_recording_preferences() raises ResourceNotFoundException (not AccessDenied).
  2. iam.create_user(UserName=<new>) ; iam.create_access_key(UserName=<new>) — attach NO policy.
  3. Build an ssm-guiconnect client with those credentials and call get_connection_recording_preferences(): the emulator answers 404 ResourceNotFoundException. AWS answers 403 AccessDeniedException "User: <arn> is not authorized to perform: ssm-guiconnect:GetConnectionRecordingPreferences on resource: arn:<partition>:ec2:<region>:<account>:instance/* because no identity-based policy allows the ssm-guiconnect:GetConnectionRecordingPreferences action".
  4. With the same unpermissioned credentials call update_connection_recording_preferences(ConnectionRecordingPreferences={"RecordingDestinations":{"S3Buckets":[{"BucketName":<a valid bucket name>,"BucketOwner":<12-digit account>}]},"KMSKeyArn":<a key arn>}, ClientToken=<=64 chars): the emulator answers 403 AccessDeniedException with the JIT-gate message; AWS answers 403 with the UpdateConnectionRecordingPreferences IAM denial sentence, which must differ from the JIT-gate sentence.
  5. iam.put_user_policy(..., PolicyDocument={Allow ssm-guiconnect:* on *}) and repeat step 3: AWS now returns the same outcome as step 1.
  Replay: ssm-guiconnect.get-denied-without-iam-permission, ssm-guiconnect.update-access-denied-reason-differs-for-iam-vs-jit-gate.

Cause 3 (@pattern message wording):
  1. Positive control: update_connection_recording_preferences with a well-shaped BucketName, a 12-digit BucketOwner and any KMSKeyArn -> 403 AccessDeniedException "Just-in-time node access is not enabled. You must set up just-in-time node access to record RDP connections." (this already passes).
  2. Repeat with BucketName="192.168.1.1" (IP-shaped, excluded by the modeled pattern): expect 400 ValidationException "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketName isn't valid."; the emulator returns the "1 validation error detected: Value '192.168.1.1' at 'connectionRecordingPreferences.recordingDestinations.s3Buckets.1.member.bucketName' failed to satisfy constraint: …" envelope.
  3. Repeat with BucketOwner="abcdefghijkl": expect "The value provided for ConnectionRecordingPreferences.RecordingDestinations.S3Buckets.BucketOwner isn't valid."; the emulator returns the analogous "1 validation error detected: … ^[0-9]{12}$" envelope.
  4. Confirm neither attempt created state: get_connection_recording_preferences() still raises ResourceNotFoundException with the standard message.
  Replay: ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-name, ssm-guiconnect.update-shape-validation-precedes-jit-gate-for-bucket-owner.
