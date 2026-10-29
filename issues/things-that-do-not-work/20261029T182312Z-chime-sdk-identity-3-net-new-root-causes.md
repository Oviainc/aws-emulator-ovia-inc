Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-identity.create-app-instance-token-reuse-conflict-rejected chime-sdk-identity.create-app-instance-user-duplicate-id-conflict-rejected chime-sdk-identity.delete-app-instance-cascades-users
Issue-Shape-Id: com.amazonaws.chimesdkidentity#ChimeIdentityService
Issue-Key: chime-sdk-identity.create-app-instance-token-reuse-conflict-rejected|_error#7|AssertionError|raise AssertionError(
Issue-Key: chime-sdk-identity.create-app-instance-user-duplicate-id-conflict-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: chime-sdk-identity.delete-app-instance-cascades-users|_wait_for_not_found#11|Timeout|time.sleep(interval_s)
Issue-Commit: fc350f637d730e9c7a46ff0b276c9700aec9957e

# chime-sdk-identity: 3 net-new root causes — CreateAppInstance token reuse with different parameters is accepted, duplicate-id ConflictException message is bespoke prose, and reads/mutations of a deleted AppInstance are refused 400 NotFound instead of 403 ForbiddenException

> _Note: this issue was identified against the code base at commit `fc350f637d730e9c7a46ff0b276c9700aec9957e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppInstance

## Description
Three independent root causes in the chime-sdk-identity control plane, found by replaying the behavioral suite against the emulator. Each must be closed before this issue is done.

Cause 1 of 3: CreateAppInstance treats a reused ClientRequestToken as an unconditional replay and never compares the request against the stored instance, so a token reused with DIFFERENT parameters is accepted instead of refused.
AWS: Smithy — CreateAppInstance.ClientRequestToken is @idempotencyToken, @required, len 2..64, pattern ^[-_a-zA-Z0-9]*$; the operation models ConflictException (HTTP 409, "The request could not be processed because of conflict in the current state of the resource"). AWS's documented idempotency rule is that the same token replays the SAME request; a token already associated with a resource but presented with different parameters is refused ConflictException with the fixed text "The provided ClientRequestToken is already associated with an existing resource." (fully fixed text; it interpolates nothing from the request).
Emulator: the second CreateAppInstance (same token, different Name and Metadata) SUCCEEDS with HTTP 201 and returns a FRESH AppInstanceArn body, e.g. {"AppInstanceArn": "arn:aws:chime:<region>:<account>:app-instance/<uuid>"} — no 409 and no error code at all. The handler's own comment states the design: because an AppInstanceArn is a fresh uuid rather than a value derived from the request, it "cannot be told apart from a retry here and replays the first instance" (so the differing-parameter case is silently swallowed).
Why: a client that relies on AWS's idempotency contract to detect accidental token reuse — e.g. a retry wrapper or a provisioning tool that reuses a token across a changed request — gets a success and an ARN whose Name/Metadata are NOT the ones it just asked for. The divergence is overly permissive: code written against AWS will take an un-taken branch, and the mismatch is discovered only later when the instance's attributes do not match the request.
Fix: the CreateAppInstance handler's idempotency branch (services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/create_app_instance.rs) must record the request fingerprint (Name, Metadata, Tags) alongside the token, replay only on an identical request, and otherwise return the modeled ConflictException with AWS's fixed message.
Traces: chime-sdk-identity.create-app-instance-token-reuse-conflict-rejected.

Cause 2 of 3: CreateAppInstanceUser's duplicate-AppInstanceUserId ConflictException carries composed prose that interpolates the id and the parent ARN instead of AWS's fixed already-exists text.
AWS: Smithy — CreateAppInstanceUser.AppInstanceUserId is @required, len 1..64, pattern ^[A-Za-z0-9]([A-Za-z0-9:\-_.@]{0,62}[A-Za-z0-9])?$ and is unique within its AppInstance; the operation models ConflictException (HTTP 409). Recorded behavior: a second create of an id that already exists under the AppInstance (fresh ClientRequestToken, different Name) is refused 409 ConflictException with the message "The resource that you are trying to create already exists." — that whole sentence is FIXED text for this error class; it interpolates no id, ARN or name from the request or from state.
Emulator: the status and error code are right (409 / ConflictException) but the message is "AppInstanceUserId dup-user-<suffix> already exists in AppInstance arn:aws:chime:<region>:<account>:app-instance/<uuid>" — a bespoke template whose variable parts (the user id, the parent ARN) AWS does not put in this message at all.
Why: clients and test suites that match on the error message (or surface it to operators) see wording AWS never emits, and message-based assertions or log scrapers written against AWS fail. Note the adjacent token-mismatch branch in the same handler emits its own invented prose ("ClientRequestToken was already used to create AppInstanceUser <arn>") and should be reviewed against AWS's wording for that class in the same pass.
Fix: the ConflictException construction in the AlreadyExists arm of the CreateAppInstanceUser handler's validation (services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/create_app_instance_user.rs) must emit AWS's fixed already-exists sentence rather than a formatted template.
Traces: chime-sdk-identity.create-app-instance-user-duplicate-id-conflict-rejected.

Cause 3 of 3: after DeleteAppInstance, reads and mutations addressed to the dead AppInstance ARN (and to its cascaded-away AppInstanceUsers) are refused with BadRequestException/NotFound instead of ForbiddenException, so the deleted-resource refusal class is wrong in status, code and message.
AWS: Smithy — DescribeAppInstance (GET /app-instances/{AppInstanceArn}), DescribeAppInstanceUser and UpdateAppInstanceUser (PUT /app-instance-users/{AppInstanceUserArn}) all model ForbiddenException at HTTP 403 ("The client is permanently forbidden from making the request"); DescribeAppInstance models NO 404 shape. Recorded behavior: once the AppInstance itself has been deleted, a request against its now-dead ARN — and against a user ARN the delete cascaded away — is refused 403 ForbiddenException with the message "You are not allowed to make this request." (fully fixed text, nothing interpolated), because the deleted AppInstance can no longer authorize a read or a write against its own ARN. This differs from the generic unknown-ARN path other Describe* calls take.
Emulator: the delete-then-describe path returns the handler's unknown-ARN refusal instead — HTTP 400, error code "NotFound" in a BadRequestException, message "AppInstance <arn> does not exist" (composed prose interpolating the requested ARN). The check polls for the documented 403 and never observes it, so it hangs until the harness kills it ("Check timed out after 60.0s"); the same wrong class is served for DescribeAppInstanceUser and UpdateAppInstanceUser on the cascaded-away user.
Why: a client that distinguishes "gone" (403 permanent refusal) from "bad request" branches on status 400 vs 403 and on the error code, so cleanup/teardown logic, retry classification (400 is not retried the way a permission refusal is surfaced) and any poll-until-deleted loop written against AWS either take the wrong branch or never terminate against the emulator.
Fix: the post-delete refusal path shared by DescribeAppInstance, DescribeAppInstanceUser and UpdateAppInstanceUser (services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/describe_app_instance.rs, .../describe_app_instance_user.rs, .../update_app_instance_user.rs, with the deletion/cascade bookkeeping in .../src/state.rs and .../ops/delete_app_instance.rs) must refuse a request addressed to a deleted AppInstance or to a user cascaded away with it as the modeled ForbiddenException carrying AWS's fixed forbidden sentence, and must keep that distinct from the never-existed-ARN path.
Traces: chime-sdk-identity.delete-app-instance-cascades-users.

## Evidence
Cause 1 — chime-sdk-identity.create-app-instance-token-reuse-conflict-rejected [mismatch]
  Expected (AWS): HTTP 409, Error.Code "ConflictException", Error.Message "The provided ClientRequestToken is already associated with an existing resource." (fixed text, no interpolation), and NO second AppInstance materialized.
  Actual (emulator): the conflicting call SUCCEEDED — AssertionError: "expected ConflictException, but the conflicting create_app_instance call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'AppInstanceArn': 'arn:aws:chime:us-east-1:000000000000:app-instance/a4251cb3-5494-414e-a994-ccda3b24ea64'}". (The ARN/account/region in that line are per-run values, not part of the requirement.)
  Emulator source confirming the cause: create_app_instance.rs handle() replays unconditionally on token hit — `if let Some(existing) = state.idempotent_arn(Self::NAME, &input.client_request_token) { return Ok(...) }` with no comparison of Name/Metadata.

Cause 2 — chime-sdk-identity.create-app-instance-user-duplicate-id-conflict-rejected [mismatch]
  Expected (AWS): Error.Code "ConflictException" (HTTP 409) with Error.Message exactly "The resource that you are trying to create already exists." — the entire sentence is fixed text for this class.
  Actual (emulator): code ConflictException, but AssertionError: expected message 'The resource that you are trying to create already exists.', got 'AppInstanceUserId dup-user-0963cb2d956a already exists in AppInstance arn:aws:chime:us-east-1:000000000000:app-instance/6adf8abb-c9db-4215-aad8-c51808a1441a'. Fixed part of the emulator's template: "AppInstanceUserId " / " already exists in AppInstance "; variable parts taken from the request: the AppInstanceUserId and the parent AppInstanceArn (per-run values, not requirements).
  Emulator source: create_app_instance_user.rs, InsertUser::AlreadyExists arm — ConflictException { code: Some("Conflict"), message: Some(format!("AppInstanceUserId {} already exists in AppInstance {}", ...)) }.

Cause 3 — chime-sdk-identity.delete-app-instance-cascades-users [mismatch]
  Expected (AWS): after DeleteAppInstance succeeds, DescribeAppInstance(dead ARN), DescribeAppInstanceUser(cascaded user ARN) and UpdateAppInstanceUser(cascaded user ARN) each fail HTTP 403, Error.Code "ForbiddenException", Error.Message "You are not allowed to make this request." (fixed text).
  Actual (emulator): the poll for ForbiddenException never succeeds and the check is killed — "Error: Check timed out after 60.0s" (the check's own _wait_for_not_found budget is 90s). The refusal the emulator serves on that path is its unknown-ARN branch: HTTP 400, BadRequestException with Error.Code "NotFound" and Error.Message "AppInstance <requested arn> does not exist" — see describe_app_instance.rs, which comments "This operation models no 404 shape, so an unknown ARN is refused with the modelled BadRequestException carrying the NotFound error code" and has no deleted-vs-never-existed distinction at all.
  Smithy for all three operations lists ForbiddenException/403 among their modeled errors, so the correct shape is available and simply not used.

## Steps to Reproduce
All against the emulator with a chime-sdk-identity client pinned to us-east-1 (the service's control plane is us-east-1 only).

Cause 1 (create-app-instance-token-reuse-conflict-rejected):
1. token = <fresh 32-hex token>; CreateAppInstance(Name="orig-name", Metadata="original", ClientRequestToken=token) -> note the returned AppInstanceArn.
2. CreateAppInstance(Name="conflict-name", Metadata="different-metadata", ClientRequestToken=token) — the SAME token with different parameters.
3. Observe HTTP 201 and an AppInstanceArn instead of HTTP 409 ConflictException "The provided ClientRequestToken is already associated with an existing resource."

Cause 2 (create-app-instance-user-duplicate-id-conflict-rejected):
1. CreateAppInstance(...) -> instance_arn.
2. CreateAppInstanceUser(AppInstanceArn=instance_arn, AppInstanceUserId="dup-user-x", Name="original-owner", ClientRequestToken=<fresh>).
3. CreateAppInstanceUser(AppInstanceArn=instance_arn, AppInstanceUserId="dup-user-x", Name="impostor-owner", ClientRequestToken=<a different fresh token>).
4. Observe 409 ConflictException whose Message is the interpolated "AppInstanceUserId <id> already exists in AppInstance <arn>" rather than AWS's fixed "The resource that you are trying to create already exists."

Cause 3 (delete-app-instance-cascades-users):
1. CreateAppInstance(...) -> arn; CreateAppInstanceUser(AppInstanceArn=arn, AppInstanceUserId="user-x", Name="cascade-user", ClientRequestToken=<fresh>) -> user_arn.
2. Confirm DescribeAppInstance(arn) and DescribeAppInstanceUser(user_arn) both succeed.
3. DeleteAppInstance(AppInstanceArn=arn).
4. Call DescribeAppInstance(arn), then DescribeAppInstanceUser(user_arn), then UpdateAppInstanceUser(AppInstanceUserArn=user_arn, Name="ghost", Metadata="").
5. Observe HTTP 400 BadRequestException/NotFound with "AppInstance <arn> does not exist" instead of HTTP 403 ForbiddenException "You are not allowed to make this request."; a client polling for the 403 never terminates (the check is killed at the harness's 60s limit).
