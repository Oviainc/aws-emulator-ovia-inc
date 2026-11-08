Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation secretsmanager.rotation-after-cancel-rotate-secret-resumes-the-stuck-token-instead-of-the-requested-one secretsmanager.rotate-secret-lambda-without-invoke-permission-denied secretsmanager.rotate-secret-while-in-progress-rejected secretsmanager.rotation-rules-validation-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation|_poll#17|AssertionError|raise AssertionError(f"waited {timeout_s}s for {description}; last observation: {last!r}")
Issue-Key: secrets-manager|RotateSecret|InvalidRequestException
Issue-Key: secretsmanager.rotate-secret-lambda-without-invoke-permission-denied|run#15|AssertionError|assert message_malformed == expected_malformed, f"unexpected malformed-ARN message: {message_malformed!r}"
Issue-Key: secretsmanager.rotate-secret-while-in-progress-rejected|run#19|AssertionError|assert message == f"Secrets Manager can't find the specified secret value for VersionId: {intruder}", message
Issue-Key: secretsmanager.rotation-rules-validation-rejected|run#12|AssertionError|assert message == expected_message, (
Issue-Commit: dd999dbcd4bb2e7cddb5a2ba7d5e91a60410d27b

# secretsmanager: 5 net-new causes — a deferred (RotateImmediately=false) rotation never test-invokes the function, RotateSecret after CancelRotateSecret still refuses the stuck rotation, and three refusal messages diverge from AWS's wording

> _Note: this issue was identified against the code base at commit `dd999dbcd4bb2e7cddb5a2ba7d5e91a60410d27b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RotateSecret

## Description
Five distinct net-new root causes in Secrets Manager rotation and version-selector behaviour, each grounded in a failing behavioral check.

Cause 1 of 5: RotateSecret with RotateImmediately=false never performs the deferred test invocation of the rotation function.
AWS: Smithy: RotateSecret.RotateImmediately is a @boolean with no default; RotationLambdaARN and RotationRules configure the schedule. Recorded behavior: when RotateImmediately is false, Secrets Manager still invokes the associated rotation Lambda asynchronously once to validate it, with Step='testSecret' (the function's own log records that step), and the candidate version it briefly labels settles back so the staging map returns to {<current version>: ['AWSCURRENT']}.
Emulator: the function is never invoked at all — the set of logged steps stays empty for the whole 120 s window, so nothing with Step='testSecret' ever appears in the function's log group, even though DescribeSecret does report RotationEnabled=true with a NextRotationDate.
Why: the deferred test invocation is how AWS tells a client, at configuration time, that its rotation function is reachable and well-formed. A client that schedules rotation and waits for that validation never sees it, and cannot distinguish 'scheduled and healthy' from 'scheduled against a broken function'.
Fix: the RotateSecret handler's rotation scheduler — on a RotateImmediately=false request it must perform the one-shot testSecret invocation of the configured RotationLambdaARN. Related open issue 20261104T044754Z-secretsmanager-a-scheduled-but-unfired-rotation-never-runs.md describes the same area but does not carry this check's replay key.
Traces: secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation.

Cause 2 of 5: RotateSecret issued after CancelRotateSecret is still refused as an incomplete rotation instead of resuming the stuck token.
AWS: Smithy: RotateSecret errors are InternalServiceError / InvalidParameterException / InvalidRequestException / ResourceNotFoundException; its output echoes ARN/Name/VersionId. Recorded behavior: while a rotation is stuck (a candidate holding AWSPENDING beside an untouched AWSCURRENT and rotation still enabled), a new RotateSecret is refused 400 InvalidRequestException with the fixed message 'A previous rotation isn't complete. That rotation will be reattempted.' — but after CancelRotateSecret has disabled rotation, the NEXT RotateSecret is ACCEPTED and resumes the leftover candidate: the response's VersionId is the STUCK token, not the ClientRequestToken this request supplied, and no new version is minted.
Emulator: that post-cancel RotateSecret is still refused with InvalidRequestException and the same 'A previous rotation isn't complete. That rotation will be reattempted.' message, so the secret is permanently wedged — CancelRotateSecret provides no way out.
Why: this is the documented recovery path for a rotation whose function failed mid-protocol: cancel, repair the function, rotate again. In the emulator a client that follows it gets a 400 forever and has no way to complete or abandon the rotation, and it cannot learn that AWS re-drives the OLD token (so its own token echo assumptions are wrong too).
Fix: the RotateSecret handler's in-progress gate and the rotation state it consults — CancelRotateSecret must clear the 'rotation in progress' condition while leaving the AWSPENDING candidate in place, and the next RotateSecret must resume that candidate's token (returning it as VersionId) rather than refusing.
Traces: secretsmanager.rotation-after-cancel-rotate-secret-resumes-the-stuck-token-instead-of-the-requested-one.

Cause 3 of 5: a malformed RotationLambdaARN is refused with the wrong message (the 'no function associated' text).
AWS: Smithy: RotateSecret.RotationLambdaARN is @length 0-2048 with no pattern, so the ARN form is checked by the service; the refusal is InvalidRequestException (400). Recorded behavior: the message is the fixed text 'Invalid rotation Lambda ARN. Ensure the ARN matches format arn:aws:lambda:<region>:<account>:function:<functionName>.' — its only variable part is the region (the request's region); '<account>' and '<functionName>' are literal placeholders in AWS's text, not substituted values.
Emulator: the error code is right (InvalidRequestException) but the message is 'No Lambda rotation function ARN is associated with this secret.' — the text for a completely different condition (rotating a secret that has no rotation function at all), even though the request DID supply a RotationLambdaARN, just a malformed one.
Why: the two conditions need different operator responses (fix the ARN you passed vs. attach a rotation function), and the message is the only thing distinguishing them, since both are InvalidRequestException. A client that matches on the message mis-diagnoses its own request.
Fix: the RotateSecret handler's RotationLambdaARN validation — emit the malformed-ARN message when a supplied ARN fails the arn:<partition>:lambda:<region>:<account>:function:<name> form, and reserve the 'No Lambda rotation function ARN is associated with this secret.' text for a request that supplies none and whose secret has none.
Traces: secretsmanager.rotate-secret-lambda-without-invoke-permission-denied.

Cause 4 of 5: GetSecretValue for an unknown VersionId uses the wrong not-found message.
AWS: Smithy: GetSecretValue.VersionId is @length 32-64; ResourceNotFoundException is a modeled error (400). Recorded behavior: the message is the fixed text "Secrets Manager can't find the specified secret value for VersionId: " followed by the VersionId from the request.
Emulator: the code is right (ResourceNotFoundException) but the message is "Secrets Manager can't find the specified secret version: " + the version id — different fixed text (it drops 'secret value for VersionId:' in favour of 'secret version:').
Why: the message distinguishes 'this secret has no such VERSION VALUE' from the secret-level not-found text; callers and rotation functions branch on it, and an emulator-worded message breaks parity-sensitive clients and test assertions even though the code matches.
Fix: the GetSecretValue handler's version-selector not-found path (message construction). Open issue 20261107T212447Z-lambda-logs-secretsmanager-getsecretvalue-for-an-unknown-version-id-uses-the.md records the same wording defect for another service group's check but does not carry this check's replay key.
Traces: secretsmanager.rotate-secret-while-in-progress-rejected.

Cause 5 of 5: the request-validation message echoes the Smithy pattern with its anchors instead of AWS's unanchored rendering.
AWS: Smithy: RotationRulesType$Duration targets DurationType, @length 2-3, @pattern "^[0-9]+h$". Recorded behavior: AWS renders a @pattern violation with the anchors stripped — the fixed text is '1 validation error detected: Value ' + <the rejected value, quoted> + ' at ' + <the member path> + ' failed to satisfy constraint: Member must satisfy regular expression pattern: ' + <the pattern WITHOUT ^ and $>, i.e. '[0-9]+h' for this member. The variable parts are the rejected value and the member path ('rotationRules.duration').
Emulator: it emits the model's pattern verbatim, '… Member must satisfy regular expression pattern: ^[0-9]+h$', so the final token of every pattern-constraint message carries anchors AWS never shows.
Why: this is AWS's generic validation-message format; clients, SDK test suites and operators compare it literally, and the divergence applies to every @pattern-constrained member in the service, not just this one.
Fix: the shared request-validation/constraint-message serializer for secretsmanager — strip leading '^' and trailing '$' from the pattern when rendering 'Member must satisfy regular expression pattern:'.
Traces: secretsmanager.rotation-rules-validation-rejected.

## Evidence
Cause 1 — secretsmanager.cancel-rotate-secret-on-a-scheduled-but-unfired-rotation:
  AssertionError: waited 120.0s for RotateSecret's deferred test invocation to run and log testSecret; last observation: set()
  Expected: the rotation function's log shows Step='testSecret' after RotateSecret(RotateImmediately=False).
  Actual: no step is ever logged — the function is never invoked.

Cause 2 — secretsmanager.rotation-after-cancel-rotate-secret-resumes-the-stuck-token-instead-of-the-requested-one:
  botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the RotateSecret operation: A previous rotation isn't complete. That rotation will be reattempted.
  Expected: after CancelRotateSecret, RotateSecret(ClientRequestToken=<fresh>) succeeds and returns VersionId = <the stuck token>, minting no new version.
  Actual: 400 InvalidRequestException with the in-progress message.

Cause 3 — secretsmanager.rotate-secret-lambda-without-invoke-permission-denied:
  AssertionError: unexpected malformed-ARN message: 'No Lambda rotation function ARN is associated with this secret.'
  Expected message: 'Invalid rotation Lambda ARN. Ensure the ARN matches format arn:aws:lambda:<region>:<account>:function:<functionName>.'
  Code matches (InvalidRequestException); only the message diverges.

Cause 4 — secretsmanager.rotate-secret-while-in-progress-rejected:
  AssertionError: Secrets Manager can't find the specified secret version: <version id>
  Expected message: "Secrets Manager can't find the specified secret value for VersionId: <version id>"
  Code matches (ResourceNotFoundException); only the fixed text diverges.

Cause 5 — secretsmanager.rotation-rules-validation-rejected:
  AssertionError: RotationRules violating Duration pattern: expected "1 validation error detected: Value '<bad value>' at 'rotationRules.duration' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9]+h", got "… Member must satisfy regular expression pattern: ^[0-9]+h$"

## Steps to Reproduce
Replay the five behavioral checks listed in trace_ids:
1. cancel-rotate-secret-on-a-scheduled-but-unfired-rotation — CreateSecret, deploy a rotation Lambda that prints its Step, AddPermission for secretsmanager.amazonaws.com, then RotateSecret(RotationLambdaARN=…, RotationRules={AutomaticallyAfterDays:30}, RotateImmediately=False) and poll the function's log group for a testSecret step.
2. rotation-after-cancel-rotate-secret-resumes-the-stuck-token-… — rotate with a function that raises in setSecret so the candidate is stuck AWSPENDING, confirm the in-progress refusal, CancelRotateSecret, repair the function, then RotateSecret with a FRESH ClientRequestToken: it must succeed and echo the stuck token.
3. rotate-secret-lambda-without-invoke-permission-denied — RotateSecret(SecretId=<arn>, RotationLambdaARN='not-a-lambda-arn-at-all') and read the InvalidRequestException message.
4. rotate-secret-while-in-progress-rejected — GetSecretValue(SecretId=<name>, VersionId=<a uuid that was never minted>) and read the ResourceNotFoundException message.
5. rotation-rules-validation-rejected — RotateSecret(…, RotationRules={'Duration': '1x'}) and read the validation message's pattern token.
