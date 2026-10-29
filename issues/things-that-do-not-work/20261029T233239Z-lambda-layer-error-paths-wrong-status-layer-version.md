Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-layer-delete-and-notfound-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-layer-delete-and-notfound-errors lambda-layer-error-paths lambda-layer-lifecycle-and-permissions lambda-layer-list-with-filters lambda-layer-multiple-versions lambda-layer-not-found-and-idempotent-delete lambda-layer-publish-and-permissions lambda-layer-permission-error-paths lambda-layer-version-permission-policy lambda-layer-permission-account-grant lambda-layer-permission-lifecycle lambda-layer-permission-organization-grant lambda-layer-version-permission-lifecycle lambda-layer-version-permission-sharing lambda-layer-permission-revision-precondition
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|GetLayerVersion|4xx->4xx|$.Type
Issue-Key: lambda|GetLayerVersionByArn|4xx->4xx|$.Type
Issue-Key: lambda|RemoveLayerVersionPermission|4xx->4xx|$.Type
Issue-Key: lambda|AddLayerVersionPermission|4xx->4xx|$.Type
Issue-Key: lambda|GetLayerVersionPolicy|4xx->4xx|$.Type
Issue-Key: lambda|GetLayerVersionPolicy|4xx->4xx|$.Message
Issue-Key: lambda|AddLayerVersionPermission|4xx->4xx|$.Message
Issue-Key: lambda|RemoveLayerVersionPermission|4xx->4xx|$.Message
Issue-Key: lambda|RemoveLayerVersionPermission|4xx->4xx|$.message
Issue-Key: lambda|GetLayerVersionByArn|4xx->4xx|$.Message
Issue-Key: lambda|GetLayerVersionByArn|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetLayerVersionByArn|4xx->4xx|status
Issue-Commit: 6fcb2038be76f91f904ab31cfab5a8d60c9030f7

# Lambda layer error paths: wrong status, layer version named without its ARN, and Type omitted

> _Note: this issue was identified against the code base at commit `6fcb2038be76f91f904ab31cfab5a8d60c9030f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLayerVersion

## Description
Shared fault: the layer-permission/layer-version error paths are built from the function-scoped error helpers, so they lose `Type`, name the layer version by bare name instead of its ARN, cite the wrong recovery API, and turn a cross-account authorization failure into a not-found. Ten net-new clusters.

AWS: Smithy ResourceNotFoundException {Type, Message} httpError 404, ResourceConflictException {Type, message} httpError 409, PreconditionFailedException {Type, message} httpError 412; all layer operations bind them. Recorded behavior: (a) a missing layer version is refused with Message `Layer version <layerVersionArn> does not exist.` — fixed text with the FULL layer version ARN and a terminating period, `<layerVersionArn>` derived from the request and the caller's account/region; (b) a revision-id mismatch on a layer permission is refused 412 with `The Revision Id provided does not match the latest Revision Id. Call the GetLayerPolicy API to retrieve the latest Revision Id` — the recovery API named is GetLayerPolicy for layer permissions (Smithy's PreconditionFailedException doc distinguishes the permission-operation recovery call from the GetFunction/GetAlias one); (c) reading a layer version owned by another account with no resource policy allowing it is refused 403 AccessDeniedException, x-amzn-errortype `AccessDeniedException`, Message `User: <callerArn> is not authorized to perform: lambda:GetLayerVersion on resource: <layerVersionArn> because no resource-based policy allows the lambda:GetLayerVersion action` — fixed text with the caller principal ARN, the target ARN and the action name as variables.

Emulator: `Type` is dropped everywhere; the not-found Message is `Layer version <name>:<version> does not exist` (bare name, no ARN, no period); the layer revision-mismatch message points at `GetFunction/GetAlias`; the cross-account read answers 404 ResourceNotFoundException `The resource you requested does not exist.` instead of 403 AccessDeniedException.

Why: a client cannot tell "not yours" from "not there" — the 403→404 substitution makes a permissions problem look like a missing resource, so retry/provisioning logic takes the wrong branch; callers parsing the layer ARN out of the message or following the named recovery API are misdirected.

Fix: the layer handlers need their own error builders (not the function ones): resolve-and-format the full layer version ARN in the not-found message, name GetLayerPolicy in the layer precondition message, raise AccessDeniedException (403) for a cross-account layer version with no allowing policy, and emit `Type` from the shared serializer.

Cause 1 of 10: GetLayerVersion, 404 — fixture lambda-layer-delete-and-notfound-errors step task#4; `Type` missing.
Cause 2 of 10: GetLayerVersionByArn, 404 — fixture lambda-layer-delete-and-notfound-errors step task#5; `Type` missing.
Cause 3 of 10: RemoveLayerVersionPermission, 404 — fixture lambda-layer-error-paths step task#8; `Type` missing.
Cause 4 of 10: AddLayerVersionPermission, 409 — fixture lambda-layer-lifecycle-and-permissions step task#8; `Type` missing.
Cause 5 of 10: GetLayerVersionPolicy, 404 — fixture lambda-layer-lifecycle-and-permissions step task#10; `Type` missing.
Cause 6 of 10: GetLayerVersionPolicy, 404 — fixture lambda-layer-error-paths step task#5; Message must carry the full layer version ARN and the trailing period; emulator emits the bare `<name>:<version>` form.
Cause 7 of 10: AddLayerVersionPermission, 404 — fixture lambda-layer-error-paths step task#6; same message defect.
Cause 8 of 10: RemoveLayerVersionPermission, 404 — fixture lambda-layer-error-paths step task#7; same message defect.
Cause 9 of 10: RemoveLayerVersionPermission, 412 — fixture lambda-layer-permission-revision-precondition step task#3; message names GetFunction/GetAlias instead of GetLayerPolicy; `Type` missing.
Cause 10 of 10: GetLayerVersionByArn — fixture lambda-layer-error-paths step task#4; expected 403 AccessDeniedException with the not-authorized message, emulator answered 404 ResourceNotFoundException `The resource you requested does not exist.`

## Evidence
Cluster details: four clusters `$.Type: missing (expected 'User', got None)`; three clusters `$.Message: value_mismatch (expected 'Layer version arn:aws:lambda:us-west-2:000000000000:layer:<name>:1 does not exist.', got 'Layer version <name>:1 does not exist')`; permission-revision cluster `$.message: value_mismatch (expected '... Call the GetLayerPolicy API ...', got '... Call the GetFunction/GetAlias API ...')`; cross-account cluster `status: expected 403, got 404` with `$headers.x-amzn-errortype: value_mismatch (expected 'AccessDeniedException', got 'ResourceNotFoundException')`.

## Steps to Reproduce
Replay the corpus. Fixture lambda-layer-error-paths step task#4: GetLayerVersionByArn for a layer version in another account — expect 403 AccessDeniedException, observe 404. Step task#5: GetLayerVersionPolicy for a missing layer version and compare the Message text.
