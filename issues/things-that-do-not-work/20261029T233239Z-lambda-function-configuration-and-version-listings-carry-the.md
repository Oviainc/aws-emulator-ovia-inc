Issue-Source: trace-sweep-divergence
Issue-Trace: lam-err-code-signing-warn-then-enforce
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lam-err-code-signing-warn-then-enforce lam-err-revision-id-function-mutations lambda-alias-traffic-shifting lambda-function-core-lifecycle lambda-function-crud-with-tag-round-trip lambda-function-lifecycle-crud lambda-version-alias-routing lambda-account-and-global-reads lambda-function-tagging-lifecycle lambda-publish-version-then-invoke-pinned-version lambda-resource-policy-conditional-update-and-stale-revision lambda-resource-policy-nonexistent-function lambda-create-tracing-config-active-roundtrip lambda-esm-error-paths lambda-permissions-and-resource-policy lambda-resource-policy-error-paths lambda-zip-function-invoke-publish-lifecycle lam-page-list-versions-by-function
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|UpdateFunctionCode|2xx->2xx|$.LastUpdateStatusReason
Issue-Key: lambda|UpdateFunctionCode|2xx->2xx|$.LastUpdateStatusReasonCode
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->2xx|$.LastUpdateStatusReason
Issue-Key: lambda|UpdateFunctionConfiguration|2xx->2xx|$.LastUpdateStatusReasonCode
Issue-Key: lambda|CreateFunction|2xx->2xx|$.RuntimeVersionConfig.RuntimeVersionArn
Issue-Key: lambda|PublishVersion|2xx->2xx|$.RuntimeVersionConfig.RuntimeVersionArn
Issue-Key: lambda|ListVersionsByFunction|2xx->2xx|$.Versions[*].FunctionArn
Issue-Key: lambda|ListVersionsByFunction|2xx->2xx|$.Versions[*].LastUpdateStatus
Issue-Key: lambda|ListVersionsByFunction|2xx->2xx|$.Versions[*].RuntimeVersionConfig
Issue-Key: lambda|ListVersionsByFunction|2xx->2xx|$.Versions[*].State
Issue-Key: lambda|ListVersionsByFunction|2xx->2xx|$.Versions[*]
Issue-Key: lambda|PublishVersion|4xx->2xx|$.Type
Issue-Key: lambda|PublishVersion|4xx->2xx|$.message
Issue-Key: lambda|PublishVersion|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|PublishVersion|4xx->2xx|status
Issue-Related: 20261029T233239Z-lambda-does-not-enforce-the-timeout-and-memorysize.md
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda function configuration and version listings carry the wrong members and skip revision checks

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunctionConfiguration

## Description
Shared fault: the FunctionConfiguration serializer drops the in-flight update reason members, synthesizes a runtime-version ARN of the wrong form, renders ListVersionsByFunction entries with live-state members AWS leaves out and an unqualified $LATEST ARN, and PublishVersion skips the RevisionId precondition. Ten net-new clusters.

AWS: Smithy FunctionConfiguration declares LastUpdateStatus/LastUpdateStatusReason/LastUpdateStatusReasonCode and RuntimeVersionConfig; while an update is in flight the recordings return `LastUpdateStatusReason: "The function is being created."` with `LastUpdateStatusReasonCode: "Creating"`. The runtime version ARN is `arn:aws:lambda:<region>::runtime:<digest>` where the digest is the 64-hex-character runtime-version identifier (Smithy RuntimeVersionArn @length 26..2048) — the digest is an opaque service value, but its form is part of the contract. In ListVersionsByFunction the `$LATEST` entry's FunctionArn carries the `:$LATEST` qualifier, and version entries do NOT carry State/StateReason/StateReasonCode/LastUpdateStatus/RuntimeVersionConfig (those live-state members are rendered only by the per-function reads). PublishVersion binds PreconditionFailedException (httpError 412, {Type, message}): a stale RevisionId is refused with `The Revision Id provided does not match the latest Revision Id. Call the GetFunction/GetAlias API to retrieve the latest Revision Id` (wholly fixed text).

Emulator: LastUpdateStatusReason/LastUpdateStatusReasonCode are absent; the runtime-version ARN ends in a 16-hex-character id; ListVersionsByFunction entries add State/LastUpdateStatus/RuntimeVersionConfig and drop the `:$LATEST` qualifier from the $LATEST entry's FunctionArn (and omit the explicitly-rendered optional members of the recorded entry); PublishVersion ignores the RevisionId and answers 201.

Why: a client polling LastUpdateStatusReason/Code to learn why a function is not yet updatable gets nothing; a client pinning a runtime version gets an ARN of a form AWS never issues; a client distinguishing the $LATEST entry from a numbered version by ARN cannot; and optimistic-concurrency callers lose their guard — PublishVersion publishes from a revision the caller did not intend.

Fix: the FunctionConfiguration serializer (reason members, runtime-version ARN generation), the ListVersionsByFunction entry serializer (qualified $LATEST ARN, omit live-state members) and PublishVersion's precondition check.

Cause 1 of 10: UpdateFunctionCode, 200 — fixture lam-err-code-signing-warn-then-enforce step task#4; `LastUpdateStatusReason` and `LastUpdateStatusReasonCode` omitted.
Cause 2 of 10: UpdateFunctionConfiguration, 200 — fixture lam-err-revision-id-function-mutations step task#5; the same two members omitted.
Cause 3 of 10: GetFunctionConfiguration, 200 — fixture lam-err-code-signing-warn-then-enforce step task#102; `RuntimeVersionConfig.RuntimeVersionArn` ends in a 16-hex id instead of the 64-hex runtime-version digest form.
Cause 4 of 10: CreateFunction, 201 — fixture lambda-create-tracing-config-active-roundtrip step task#2; same runtime-version ARN form defect.
Cause 5 of 10: PublishVersion, 201 — fixture lambda-publish-version-then-invoke-pinned-version step task#4; same runtime-version ARN form defect.
Cause 6 of 10: ListVersionsByFunction, 200 — fixture lam-page-list-versions-by-function step task#11; the $LATEST entry's FunctionArn lacks `:$LATEST` and the entries add State/LastUpdateStatus/RuntimeVersionConfig.
Cause 7 of 10: ListVersionsByFunction, 200 — fixture lam-page-list-versions-by-function step task#12; whole-entry divergence, same defect across all four versions.
Cause 8 of 10: ListVersionsByFunction, 200 — fixture lambda-function-core-lifecycle step task#6; same entry-shape divergence.
Cause 9 of 10: ListVersionsByFunction, 200 — fixture lambda-version-alias-routing step task#16; same entry-shape divergence.
Cause 10 of 10: PublishVersion — fixture lam-err-revision-id-function-mutations step task#14; a stale RevisionId expected 412 PreconditionFailedException with the revision-mismatch message, emulator answered 201 and published a version.

## Evidence
Cluster details: `$.LastUpdateStatusReason: missing (expected 'The function is being created.', got None)` and `$.LastUpdateStatusReasonCode: missing (expected 'Creating', got None)` on UpdateFunctionCode/UpdateFunctionConfiguration; `$.RuntimeVersionConfig.RuntimeVersionArn: value_mismatch (expected 'arn:aws:lambda:us-west-2::runtime:36aeb19f...d863ae' (64 hex), got 'arn:aws:lambda:us-west-2::runtime:a5e88e82e3fdc7cd' (16 hex))`; ListVersionsByFunction `$.Versions[0].FunctionArn: value_mismatch (expected '...:$LATEST', got '...')` plus `$.Versions[n].State / LastUpdateStatus / RuntimeVersionConfig: type_mismatch (expected None, got ...)`; PublishVersion task#14 `status: expected 412, got 201` with the revision-mismatch message missing. Smithy: PreconditionFailedException httpError 412 bound to PublishVersion; RuntimeVersionArn @length 26..2048.

## Steps to Reproduce
Replay the corpus. Fixture lam-err-revision-id-function-mutations step task#14: PublishVersion with a RevisionId that is not current — expect 412, observe 201. Fixture lam-page-list-versions-by-function step task#11: ListVersionsByFunction and compare the $LATEST entry's FunctionArn and member set.
