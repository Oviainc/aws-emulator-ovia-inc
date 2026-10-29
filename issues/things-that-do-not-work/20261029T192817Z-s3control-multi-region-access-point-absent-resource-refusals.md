Issue-Source: trace-sweep-divergence
Issue-Trace: s3control-multi-region-access-point-absent-envelopes
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3control-multi-region-access-point-absent-envelopes s3c-multi-region-access-point-create-validation-boundary
Issue-Shape-Id: com.amazonaws.s3control#AWSS3ControlServiceV20180820
Issue-Key: s3control|GetMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|GetMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.MultiRegionAccessPointName
Issue-Key: s3control|GetMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetMultiRegionAccessPointPolicy|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|GetMultiRegionAccessPointPolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|GetMultiRegionAccessPointPolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetMultiRegionAccessPointPolicy|4xx->4xx|$.ErrorResponse.Error.MultiRegionAccessPointName
Issue-Key: s3control|GetMultiRegionAccessPointPolicy|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetMultiRegionAccessPointPolicy|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetMultiRegionAccessPointPolicy|4xx->4xx|status
Issue-Key: s3control|GetMultiRegionAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|GetMultiRegionAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|GetMultiRegionAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetMultiRegionAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.Error.MultiRegionAccessPointName
Issue-Key: s3control|GetMultiRegionAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetMultiRegionAccessPointPolicyStatus|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|GetMultiRegionAccessPointPolicyStatus|4xx->4xx|status
Issue-Key: s3control|GetMultiRegionAccessPointRoutes|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|GetMultiRegionAccessPointRoutes|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|GetMultiRegionAccessPointRoutes|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|GetMultiRegionAccessPointRoutes|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DeleteMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.AccountId
Issue-Key: s3control|DeleteMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|DeleteMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.Error.MultiRegionAccessPointName
Issue-Key: s3control|DeleteMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|DeleteMultiRegionAccessPoint|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|4xx->4xx|$.ErrorResponse.HostId
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|4xx->4xx|$.ErrorResponse.RequestId
Issue-Key: s3control|DescribeMultiRegionAccessPointOperation|4xx->4xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# s3control Multi-Region Access Point absent-resource refusals mis-route the greedy name label

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMultiRegionAccessPoint

## Description
7 net-new root causes on the s3control Multi-Region Access Point (MRAP) surface, all in the absent-resource path. Three sub-resource routes apply the MRAP name pattern to the whole greedy path segment and so answer a shape-validation error where AWS answers not-found, two not-found messages interpolate the name, and two refusals use the wrong code. Smithy declares no errors for these operations (errors: []); the codes and messages are recorded behavior only.

--- Cause 1 of 7: GetMultiRegionAccessPoint's not-found message interpolates the name and drops its members ---
AWS: recorded behavior only — 404 Code `NoSuchMultiRegionAccessPoint`, Message exactly `The specified Multi-Region Access Point does not exist.` (fixed text, trailing period, no name in it), with `<MultiRegionAccessPointName>` (the name from the request) and `<AccountId>` (the request's account) plus RequestId/HostId.
Emulator: answers `The specified Multi-Region Access Point does not exist: <name>` and omits MultiRegionAccessPointName, AccountId, RequestId and HostId.
Why: the message does not match AWS's wording, and the members clients read the offending name/account from are absent.
Fix: the GetMultiRegionAccessPoint refusal construction plus the shared s3control envelope serializer.
Traces: s3control-multi-region-access-point-absent-envelopes#task#2.

--- Cause 2 of 7: GetMultiRegionAccessPointPolicy treats the `/policy` suffix as part of the name ---
AWS: recorded behavior only — Smithy binds this operation to `GET /v20180820/mrap/instances/{Name+}/policy` with Name @pattern ^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$; for an MRAP that does not exist the answer is 404 `NoSuchMultiRegionAccessPoint` / `The specified Multi-Region Access Point does not exist.` with MultiRegionAccessPointName and AccountId.
Emulator: answers 400 `ValidationException` with `1 validation error detected: Value '<name>/policy' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$` — the greedy `{Name+}` label swallowed the `/policy` suffix.
Why: the operation is unreachable for every name: even an existing MRAP's policy can never be read, because the name always fails validation with the suffix appended.
Fix: the s3control MRAP route table — match the `/policy` suffix before binding the greedy `{Name+}` label, then look the MRAP up.
Traces: s3control-multi-region-access-point-absent-envelopes#task#3.

--- Cause 3 of 7: GetMultiRegionAccessPointPolicyStatus treats the `/policystatus` suffix as part of the name ---
AWS: recorded behavior only — 404 `NoSuchMultiRegionAccessPoint` / `The specified Multi-Region Access Point does not exist.` with MultiRegionAccessPointName and AccountId.
Emulator: answers 400 ValidationException naming `<name>/policystatus` as the value of `name`.
Why: as cause 2 — the policy-status read is unreachable for any name.
Fix: same route table — match `/policystatus` before the greedy label.
Traces: s3control-multi-region-access-point-absent-envelopes#task#4.

--- Cause 4 of 7: GetMultiRegionAccessPointRoutes treats the `/routes` suffix as part of the name ---
AWS: recorded behavior only — the refusal is 400 with Code `InvalidRequest` and Message `Request invalid` (fixed text) plus RequestId/HostId.
Emulator: answers 400 ValidationException naming `<name>/routes` as the value of `name`.
Why: as cause 2 for the routes read, and the code a client branches on is wrong even though the status matches.
Fix: same route table plus the refusal mapping for this operation.
Traces: s3control-multi-region-access-point-absent-envelopes#task#5.

--- Cause 5 of 7: DeleteMultiRegionAccessPoint's not-found message interpolates the name ---
AWS: recorded behavior only — 404 Message `The specified Multi-Region Access Point does not exist.` with MultiRegionAccessPointName and AccountId plus RequestId/HostId.
Emulator: answers `The specified Multi-Region Access Point does not exist: <name>` and omits those members.
Why: as cause 1.
Fix: the DeleteMultiRegionAccessPoint refusal construction plus the envelope serializer.
Traces: s3control-multi-region-access-point-absent-envelopes#task#7.

--- Cause 6 of 7: DescribeMultiRegionAccessPointOperation's unknown-token refusal uses the wrong code and message ---
AWS: recorded behavior only — an async request token that names no operation is refused with 404, Code `NoSuchAsyncRequest` and Message `The specified request was not found.` (fixed text), plus RequestId/HostId.
Emulator: answers 404 with Code `NotFoundException` and Message `The specified request token names no asynchronous request: <requestTokenARN>`, and omits RequestId/HostId.
Why: a client branching on `NoSuchAsyncRequest` does not recognize the refusal, and the message leaks the submitted ARN into text AWS keeps fixed.
Fix: the DescribeMultiRegionAccessPointOperation refusal construction plus the envelope serializer.
Traces: s3control-multi-region-access-point-absent-envelopes#task#9.

--- Cause 7 of 7: a non-ARN request token is a validation error instead of 404 NoSuchAsyncRequest ---
AWS: recorded behavior only — Smithy binds RequestTokenARN as a greedy httpLabel with @pattern ^arn:.+$, but the recorded wire answers 404 `NoSuchAsyncRequest` / `The specified request was not found.` for a token that is not an ARN: the token is resolved, not shape-checked.
Emulator: answers 400 `ValidationException` with `1 validation error detected: Value '<token>' at 'requestTokenARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.+$`, and omits RequestId/HostId.
Why: a client that polls an async operation by a token it did not construct gets a 400 it treats as its own bug instead of the 404 that means the operation is unknown; poll loops keyed on 404 never terminate.
Fix: the DescribeMultiRegionAccessPointOperation input handling — resolve the token and refuse an unknown one with 404 NoSuchAsyncRequest rather than failing label validation.
Traces: s3c-multi-region-access-point-create-validation-boundary#task#7.

## Evidence
Replay of wire corpus simparity-wire-aws-v1 against the emulator:
- GetMultiRegionAccessPoint 404: `Message: expected 'The specified Multi-Region Access Point does not exist.', got 'The specified Multi-Region Access Point does not exist: <name>'`; `Error.MultiRegionAccessPointName: missing`; `Error.AccountId: missing`; RequestId/HostId missing — s3control-multi-region-access-point-absent-envelopes#task#2.
- GetMultiRegionAccessPointPolicy: `status: expected 404, got 400`; `Code: expected 'NoSuchMultiRegionAccessPoint', got 'ValidationException'`; `Message: ... got "1 validation error detected: Value '<name>/policy' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-z0-9][-a-z0-9]{1,48}[a-z0-9]$"` — same fixture task#3.
- GetMultiRegionAccessPointPolicyStatus: identical, with `<name>/policystatus` — same fixture task#4.
- GetMultiRegionAccessPointRoutes: `Code: expected 'InvalidRequest', got 'ValidationException'`; `Message: expected 'Request invalid'`, got the same pattern error with `<name>/routes` — same fixture task#5.
- DeleteMultiRegionAccessPoint 404: interpolated message, MultiRegionAccessPointName/AccountId/RequestId/HostId missing — same fixture task#7.
- DescribeMultiRegionAccessPointOperation (unknown token): `Code: expected 'NoSuchAsyncRequest', got 'NotFoundException'`; `Message: expected 'The specified request was not found.', got 'The specified request token names no asynchronous request: <arn>'` — same fixture task#9.
- DescribeMultiRegionAccessPointOperation (non-ARN token): `status: expected 404, got 400`; `Code: expected 'NoSuchAsyncRequest', got 'ValidationException'`; `Message: ... got "1 validation error detected: Value 'sim2real-probe-not-an-arn' at 'requestTokenARN' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.+$"` — s3c-multi-region-access-point-create-validation-boundary#task#7.

## Steps to Reproduce
1. Replay s3control-multi-region-access-point-absent-envelopes#task#2 and #task#7: GET and DELETE /v20180820/mrap/instances/<absent name> — the message interpolates the name and the envelope lacks MultiRegionAccessPointName/AccountId/RequestId/HostId.
2. Same fixture task#3/#task#4/#task#5: GET /v20180820/mrap/instances/<name>/policy, /policystatus and /routes — the emulator validates `<name>/policy` (etc.) against the MRAP name pattern and answers 400 ValidationException instead of the recorded 404 NoSuchMultiRegionAccessPoint (or 400 InvalidRequest for routes).
3. Same fixture task#9: GET /v20180820/async-requests/mrap/<ARN naming no operation> — Code NotFoundException with an interpolated message instead of NoSuchAsyncRequest / `The specified request was not found.`
4. Replay s3c-multi-region-access-point-create-validation-boundary#task#7: GET /v20180820/async-requests/mrap/<non-ARN token> — 400 ValidationException instead of 404 NoSuchAsyncRequest.
