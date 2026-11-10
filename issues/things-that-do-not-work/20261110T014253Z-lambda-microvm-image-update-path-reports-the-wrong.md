Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-microvms-image-lifecycle
Issue-Step: task#17
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-microvms-image-lifecycle
Issue-Shape-Id: com.amazonaws.lambdamicrovms#LambdaMicrovms
Issue-Key: lambda|GetMicrovmImage|2xx->2xx|$
Issue-Key: lambda|GetMicrovmImage|2xx->2xx|$.latestFailedImageVersion
Issue-Key: lambda|GetMicrovmImage|2xx->2xx|$.state
Issue-Key: lambda|UpdateMicrovmImageVersion|2xx->2xx|$.versionStateTimeBucket
Issue-Commit: d9137e240d6d86ce2d0eba5736851daf32b7e9eb

# lambda MicroVM image update path reports the wrong image state and omits versionStateTimeBucket

> _Note: this issue was identified against the code base at commit `d9137e240d6d86ce2d0eba5736851daf32b7e9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMicrovmImage

## Description
Two net-new root causes found by replaying the wire corpus against the emulator, both on the MicroVM-image UPDATE path of the lambda service token. CAUSE COUNT: 2. Both must be closed before this issue is submitted, and BOTH bindings below must be re-replayed (lambda-microvms-image-lifecycle#task#17 and lambda-microvms-image-lifecycle#task#18).

--- Cause 1 of 2: after an UpdateMicrovmImage that AWS refuses, GetMicrovmImage reports an UPDATE_* state and a new latestFailedImageVersion, so a creation poll never settles ---

AWS: Smithy com.amazonaws.lambdamicrovms#GetMicrovmImage -> GetMicrovmImageOutput: `state` (MicrovmImageState @required, enum CREATING | CREATED | CREATE_FAILED | UPDATING | UPDATED | UPDATE_FAILED | DELETING | DELETE_FAILED | DELETED), `latestActiveImageVersion` and `latestFailedImageVersion` (NonBlankString, len 1..2048, pattern ^[^\s]+$), `createdAt` @required, `updatedAt`, `tags`. UpdateMicrovmImage (PUT /2025-09-09/microvm-images/{imageIdentifier}, "Updates the configuration of a MicroVM image and triggers a new version build") models ValidationException (HTTP 400, service-common error). In the recording AWS REFUSES an update addressed at an image whose only version build failed: HTTP 400, header x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', message 'Cannot update MicroVM Image in its current state: <imageArn>' — the fixed text is 'Cannot update MicroVM Image in its current state: ' and the only variable part is the image ARN taken from the request/state. Because the update never starts, the image stays in the terminal state its CREATION settled in: state CREATE_FAILED, latestFailedImageVersion = the version whose build failed (the first version, '1.0' in the recording), latestActiveImageVersion absent. A client polling GetMicrovmImage for a terminal state therefore settles on CREATE_FAILED.

Emulator: the emulator ACCEPTS that same update (HTTP 200, no error envelope), mints a second version, and GetMicrovmImage then answers 200 with state 'UPDATE_FAILED' and latestFailedImageVersion '2.0'. The replay's poll for state in {CREATED, CREATE_FAILED} never converged — 37 attempts, capped at 180 s — because the record left the CREATE_* family entirely.

Why: a client that creates an image, sees the build fail, and then retries/updates it polls GetMicrovmImage for a terminal creation state. Against the emulator that poll never terminates, and the image's failure bookkeeping points at a version AWS never created, so error reporting and remediation logic ("which version failed?") reads the wrong version.

Fix: UpdateMicrovmImage's handler needs the current-state guard that refuses an update of an image in a terminal CREATE_FAILED state (its validation), and the image record's state/latestFailedImageVersion transition in the lambda-microvms service state must not be advanced by a refused update. NOTE FOR THE FIXER: the refusal itself (status, error class and message of that UpdateMicrovmImage call) is open issue 20261030T024426Z-lambda-microvm-image-refusal-paths-answer-the-wrong.md; the replay does NOT report task#17 as owned by it, because that issue carries the refusal's key and not this image-level state/poll consequence. Fixing the guard is likely to fix both, but task#17 must be re-replayed here regardless.

Traces: lambda-microvms-image-lifecycle#task#17.

--- Cause 2 of 2: UpdateMicrovmImageVersion's 200 response omits versionStateTimeBucket ---

AWS: recorded behavior only — versionStateTimeBucket is NOT a member of UpdateMicrovmImageVersionResponse in the Smithy model (com.amazonaws.lambdamicrovms#UpdateMicrovmImageVersion output members are baseImageArn, baseImageVersion, buildRoleArn, description, codeArtifact, logging, egressNetworkConnectors, cpuConfigurations, resources, additionalOsCapabilities, hooks, environmentVariables, imageArn, imageVersion, state, status, createdAt, updatedAt, stateReason, tags). The recorded AWS 200 response nevertheless carries an extra member `versionStateTimeBucket` whose value is a COMPOSITE key: the version's state token, a literal '#', then a compact time bucket — 'FAILED#26110509' in the recording, where 'FAILED' is the version's MicrovmImageVersionState and the digits are the bucket derived from the state-transition time (a per-run value, not a requirement; the required part is the '<STATE>#<bucket>' form and the member's presence).

Emulator: the member is absent from the response body entirely (replay: '$.versionStateTimeBucket: missing (expected 'FAILED#26110509', got None)'). The response projection builds no such member: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/update_microvm_image_version.rs:294-322 (`rendered`) emits only the modeled members.

Why: a client that reads the bucket key from the update response to index, sort or query versions by state-and-time gets nothing back and has to fall back to a read that, on this emulator, reports the key in a different format altogether.

Fix: the UpdateMicrovmImageVersion response serializer/projection, plus the shared version projection in the lambda-microvms service state that must compute the '<STATE>#<bucket>' value from the stored version. NOTE FOR THE FIXER: the read paths DO emit versionStateTimeBucket but as an ISO-8601 instant ('2026-11-10T01:00:00Z') rather than the '<STATE>#<bucket>' composite; those steps are already owned by 20261103T192714Z-lambda-microvms-getmicrovmimageversion-reports-a-failed-build-as.md and 20261108T125129Z-lambda-microvms-image-version-and-build-read-paths.md, so the same value computation is what both those issues and this cause need. The sibling of this cause on the lambda-microvms service token (UpdateMicrovmImageVersion omitting the member there too) is filed separately because an issue carries exactly one service.

Traces: lambda-microvms-image-lifecycle#task#18.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up=true, 104 steps, 78 passed, 26 mismatched, score 75.0%.

[mismatch] fixture lambda-microvms-image-lifecycle step task#17 — GetMicrovmImage, expected HTTP 200, got HTTP 200
  $.state: value_mismatch (expected 'CREATE_FAILED', got 'UPDATE_FAILED')
  $.latestFailedImageVersion: value_mismatch (expected '1.0', got '2.0')
  $.state: poll: not in ['CREATED', 'CREATE_FAILED'] after 37 attempt(s) (expected 'CREATE_FAILED', got 'UPDATE_FAILED'); $: poll capped at 180s
  Upstream in the same fixture, step task#16 recorded AWS refusing the UpdateMicrovmImage with HTTP 400, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/' and message 'Cannot update MicroVM Image in its current state: <imageArn>', while the emulator answered HTTP 200 with no error envelope (that step is owned by 20261030T024426Z-lambda-microvm-image-refusal-paths-answer-the-wrong.md).
  The recording's id, createdAt/updatedAt instants and the image name are per-run values and are NOT requirements; the requirement is state staying in the CREATE_* family with latestFailedImageVersion naming the version that actually failed.

[mismatch] fixture lambda-microvms-image-lifecycle step task#18 — UpdateMicrovmImageVersion, expected HTTP 200, got HTTP 200
  $.versionStateTimeBucket: missing (expected 'FAILED#26110509', got None)
  Source: services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/ops/update_microvm_image_version.rs:294 (`rendered` builds the response with no versionStateTimeBucket member). The digits '26110509' are a per-run time bucket; the requirement is the member's presence and its '<STATE>#<bucket>' form.

## Steps to Reproduce
Against the emulator (lambda / lambda-microvms stood up), using the MicroVM image API (base path /2025-09-09):

Cause 1:
1. CreateMicrovmImage(name=<name>, baseImageArn=<an AWS-managed base image>, codeArtifact={uri:'s3://<a bucket that does not exist>/code.zip'}, buildRoleArn=<role>) — the build fails.
2. Poll GetMicrovmImage(<imageArn>) until state leaves CREATING: AWS settles on state='CREATE_FAILED' with latestFailedImageVersion='1.0' and no latestActiveImageVersion.
3. UpdateMicrovmImage(<imageArn>, <a changed configuration>).
   Expected: HTTP 400 ValidationException, x-amzn-errortype 'ValidationException:http://internal.amazon.com/coral/com.amazonaws.lambda.microvms/', message 'Cannot update MicroVM Image in its current state: <imageArn>'.
   Observed: HTTP 200, a second version is minted.
4. Poll GetMicrovmImage(<imageArn>) for state in {CREATED, CREATE_FAILED}.
   Expected: settles immediately on CREATE_FAILED with latestFailedImageVersion='1.0'.
   Observed: state='UPDATE_FAILED', latestFailedImageVersion='2.0', and the poll never settles (capped at 180 s after 37 attempts).

Cause 2:
5. UpdateMicrovmImageVersion(imageIdentifier=<imageArn>, imageVersion='1.0', status='INACTIVE') — PATCH /2025-09-09/microvm-images/{imageIdentifier}/versions/{imageVersion}.
   Expected: HTTP 200 whose body includes versionStateTimeBucket of the form '<version state>#<time bucket>', e.g. 'FAILED#<bucket>' for a version whose build failed.
   Observed: HTTP 200 with no versionStateTimeBucket member at all.
