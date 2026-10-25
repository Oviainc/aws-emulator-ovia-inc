Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.blueprint-tag-tag-resource-past-fifty-tags-rejected lambda.delete-layer-version-idempotent-on-missing lambda.layer-get-rejected-for-an-unknown-layer-while-list-returns-empty lambda.esm-crud lambda.list-event-source-mappings-reflects lambda.remove-permission lambda.remove-permission-unknown-statement-not-found lambda.resource-not-found-error-consistency lambda.permission-revisionid-precondition lambda.stale-revision-id lambda.publish-version-stale-sha-precondition-failed
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.blueprint-tag-tag-resource-past-fifty-tags-rejected|run#8|AssertionError|assert msg == "Number of tags exceeds resource tag limit.", (
Issue-Key: lambda.delete-layer-version-idempotent-on-missing|run#13|AssertionError|assert code_msg == expected_code_msg, (
Issue-Key: lambda.layer-get-rejected-for-an-unknown-layer-while-list-returns-empty|run#8|AssertionError|assert msg == "The resource you requested does not exist.", (
Issue-Key: lambda.esm-crud|run#21|AssertionError|assert gone_msg == "The resource you requested does not exist.", f"get message: {gone_msg!r}"
Issue-Key: lambda.list-event-source-mappings-reflects|run#24|AssertionError|assert get_code_msg == "The resource you requested does not exist.", (
Issue-Key: lambda.remove-permission|run#9|AssertionError|assert msg == "The resource you requested does not exist.", f"expected the exact emptied-policy message, got {msg!r}"
Issue-Key: lambda|GetPolicy|ResourceNotFoundException
Issue-Key: lambda.resource-not-found-error-consistency|run#20|AssertionError|assert alias_msg == expected, f"GetAlias: expected message {expected!r}, got {alias_msg!r}"
Issue-Key: lambda.permission-revisionid-precondition|run#9|AssertionError|assert stale_add_code_msg == "The Revision Id provided does not match the latest Revision Id. Call the GetPolicy API to retrieve the latest Revision Id", (
Issue-Key: lambda.stale-revision-id|run#13|AssertionError|assert message == expected, (
Issue-Key: lambda.publish-version-stale-sha-precondition-failed|run#16|AssertionError|assert code_msg == (
Issue-Commit: bb4602e7ae983d0b86fd9b19deccaf384a5ade46

# lambda: eight refusal-message rules diverge — ResourceNotFoundException, PreconditionFailedException and PublishVersion CodeSha256 refusals carry generated prose instead of AWS's fixed sentences (8 causes)

> _Note: this issue was identified against the code base at commit `bb4602e7ae983d0b86fd9b19deccaf384a5ade46`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetPolicy

## Description
Eight distinct net-new root causes in the Lambda control plane. In every one the HTTP status and the error CODE are correct, but the refusal's `Message` is prose the emulator generates from the request (it interpolates the resource ARN/name and the offending values) where AWS returns a FIXED sentence for that condition. These are message-RULE divergences, not cosmetic text differences: AWS's sentence for each of these conditions is a constant that does not vary with the resource, so the rule itself ("emit this constant") is what the emulator does not implement. No existing open issue covers Lambda refusal-message rules — the nearest open items (20261025T022656Z-lambda-ten-control-plane-writes-are-accepted-that.md, 20261025T022656Z-lambda-ten-read-representation-and-data-plane-divergences.md, 20261025T022656Z-lambda-ten-operations-answer-notimplemented-where-aws-answers.md) are about accepted-but-invalid writes, read representation and NotImplemented stubs respectively, and none of them carries any of the replay keys below.

Cause 1 of 8: TagResource past the 50-tag limit returns a computed sentence instead of AWS's constant.
  AWS: Smithy: Lambda `TagResource` models `TooManyRequestsException`/`InvalidParameterValueException`/`ResourceNotFoundException`; the tag-count ceiling refusal is 400 `TooManyTagsException`-class behaviour whose message AWS fixes as the constant `Number of tags exceeds resource tag limit.` (recorded behavior: the check asserts that exact sentence, independent of function or tag names).
  Emulator: answers the same code but the message `Tags may not exceed 50 per resource. The request would leave 'arn:aws:lambda:us-west-2:000000000000:function:<fn>' with 51 tags.` — generated from the request, quoting the ARN and the resulting count.
  Why: a client that branches on the documented tag-limit sentence (or logs/compares it) never matches, and the ARN leaking into the message makes the text non-deterministic across resources.
  Fix: the TagResource handler's tag-count validation — emit the constant, drop the interpolation.
  Traces: lambda.blueprint-tag-tag-resource-past-fifty-tags-rejected

Cause 2 of 8: layer-version ResourceNotFoundException names the layer instead of AWS's constant.
  AWS: Smithy: `GetLayerVersion`/`DeleteLayerVersion` model `ResourceNotFoundException` at httpError 404. AWS's `Message` for an absent layer version is the constant `The resource you requested does not exist.`
  Emulator: 404 with the right code but `Layer version <layer-name>:1 does not exist`.
  Why: a client keying on Lambda's single canonical not-found sentence (the same text every Lambda 404 carries) cannot recognise the refusal; and the idempotent-delete contract is graded on that sentence.
  Fix: the layer-version lookup error construction shared by get_layer_version / delete_layer_version.
  Traces: lambda.delete-layer-version-idempotent-on-missing, lambda.layer-get-rejected-for-an-unknown-layer-while-list-returns-empty

Cause 3 of 8: event-source-mapping ResourceNotFoundException names the mapping ARN instead of AWS's constant.
  AWS: Smithy: `GetEventSourceMapping` models `ResourceNotFoundException` at httpError 404; AWS's `Message` is `The resource you requested does not exist.`
  Emulator: `Event source mapping not found: arn:aws:lambda:us-west-2:000000000000:event-source-mapping:<uuid>`.
  Why: same canonical-sentence break as cause 2, on the ESM read path that a delete-then-read lifecycle depends on.
  Fix: the event-source-mapping lookup error construction in the ESM handlers.
  Traces: lambda.esm-crud, lambda.list-event-source-mappings-reflects

Cause 4 of 8: GetPolicy on a function with no (or emptied) resource policy appends the resource ARN to AWS's constant.
  AWS: Smithy: `GetPolicy` models `ResourceNotFoundException` at httpError 404. AWS answers exactly `The resource you requested does not exist.` when the function exists but carries no policy statements.
  Emulator: `The resource you requested does not exist. Resource: arn:aws:lambda:us-west-2:000000000000:function:<fn>` — AWS's sentence plus an appended `Resource: <arn>` clause.
  Why: the emulator is one suffix away from correct, so string-equality clients and the empty-policy contract both fail while a substring check would pass — a silent divergence.
  Fix: the GetPolicy handler's empty-policy refusal; remove the appended `Resource:` clause.
  Traces: lambda.remove-permission, lambda.remove-permission-unknown-statement-not-found

Cause 5 of 8: GetAlias not-found uses the wrong fixed sentence.
  AWS: Smithy: `GetAlias` models `ResourceNotFoundException` at httpError 404. For a missing alias AWS's message rule is `Cannot find alias arn: <alias ARN>` (built from the request, so the rule — not a constant — is the requirement).
  Emulator: `Alias not found: <alias ARN>` — the right ARN under a different sentence.
  Why: the check exists to prove Lambda's not-found messages are consistent per resource KIND; a client that distinguishes alias-not-found from function-not-found by the leading phrase mis-classifies.
  Fix: the GetAlias handler's not-found error construction.
  Traces: lambda.resource-not-found-error-consistency

Cause 6 of 8: AddPermission/RemovePermission with a stale RevisionId returns generated prose for PreconditionFailedException.
  AWS: Smithy: `AddPermission`/`RemovePermission` model `PreconditionFailedException` at httpError 412. AWS's message is the constant `The Revision Id provided does not match the latest Revision Id. Call the GetPolicy API to retrieve the latest Revision Id` — note it names GetPolicy, the policy-side read.
  Emulator: 412 with `The RevisionId (00000000-0000-0000-0000-000000000000) provided does not match the latest RevisionId for the resource arn:aws:lambda:...:function:<fn>. Call GetPolicy to retrieve the latest RevisionId for your resource.` — the supplied id and the function ARN interpolated in.
  Why: optimistic-concurrency clients retry on this precondition and often match the sentence; interpolating the caller's own stale id also echoes request data into an error AWS keeps constant.
  Fix: the resource-policy revision precondition check in the AddPermission/RemovePermission handlers.
  Traces: lambda.permission-revisionid-precondition

Cause 7 of 8: UpdateFunctionConfiguration with a stale RevisionId returns generated prose, and names only GetFunction.
  AWS: Smithy: `UpdateFunctionConfiguration` models `PreconditionFailedException` at httpError 412; AWS's message is the constant `The Revision Id provided does not match the latest Revision Id. Call the GetFunction/GetAlias API to retrieve the latest Revision Id` — the configuration-side sentence names GetFunction/GetAlias, i.e. it is a DIFFERENT constant from cause 6.
  Emulator: `The RevisionId provided (<uuid>) does not match the latest RevisionId for the Lambda function. Call the GetFunction API to retrieve the latest RevisionId for your resource.`
  Why: the emulator collapses two distinct AWS precondition sentences into one generated template, so a client cannot tell a policy-revision conflict from a configuration-revision conflict by its message.
  Fix: the configuration revision precondition check in the UpdateFunctionConfiguration handler.
  Traces: lambda.stale-revision-id

Cause 8 of 8: PublishVersion with a mismatched CodeSha256 returns the wrong sentence shape.
  AWS: Smithy: `PublishVersion` takes `CodeSha256` and models `InvalidParameterValueException` at httpError 400. AWS's message rule, built from the request, is `CodeSHA256 (<supplied sha>) is different from current CodeSHA256 in $LATEST (<actual sha>). Please try again with the CodeSHA256 in $LATEST.`
  Emulator: 400 `InvalidParameterValueException` with `CodeSha256 '<supplied>' does not match the SHA256 of the function's deployment package, which is '<actual>'` — both hashes present, but neither the `CodeSHA256` spelling, the `$LATEST` reference, nor the remediation clause.
  Why: the publish-precondition is the guard a deploy pipeline uses to refuse publishing a version it did not just upload; its message is how the pipeline reports which SHA to retry with.
  Fix: the PublishVersion handler's CodeSha256 precondition validation.
  Traces: lambda.publish-version-stale-sha-precondition-failed

## Evidence
All eight causes replayed against the booted emulator; status and error code matched AWS, message did not.

Cause 1 — expected `Number of tags exceeds resource tag limit.`; got `Tags may not exceed 50 per resource. The request would leave 'arn:aws:lambda:us-west-2:000000000000:function:sim2real-scn-bp-50-d98ea8d0f47c' with 51 tags.`
Cause 2 — expected `The resource you requested does not exist.`; got `Layer version sim2real-behav-lambda-layer-idem-2b534d266989:1 does not exist` (DeleteLayerVersion) and `Layer version sim2real-scn-lyr-ghost-9f2b59fb3ae0:1 does not exist` (GetLayerVersion).
Cause 3 — expected `The resource you requested does not exist.`; got `Event source mapping not found: arn:aws:lambda:us-west-2:000000000000:event-source-mapping:5de91baf-0557-491a-8186-351b273b270f`.
Cause 4 — expected `The resource you requested does not exist.`; got `The resource you requested does not exist. Resource: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-fn-8808aa0b421d`.
Cause 5 — expected `Cannot find alias arn: arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-fn-671f6cdbd1b4:alias-missing-9a4df225b70c`; got `Alias not found: <same arn>`.
Cause 6 — expected `The Revision Id provided does not match the latest Revision Id. Call the GetPolicy API to retrieve the latest Revision Id`; got `The RevisionId (00000000-0000-0000-0000-000000000000) provided does not match the latest RevisionId for the resource arn:aws:lambda:us-west-2:000000000000:function:sim2real-behav-fn-cef46344f96b. Call GetPolicy to retrieve the latest RevisionId for your resource.` (HTTP 412, PreconditionFailedException).
Cause 7 — expected `The Revision Id provided does not match the latest Revision Id. Call the GetFunction/GetAlias API to retrieve the latest Revision Id`; got `The RevisionId provided (9a503a20-88dc-4656-9c7b-5045d0499220) does not match the latest RevisionId for the Lambda function. Call the GetFunction API to retrieve the latest RevisionId for your resource.`
Cause 8 — expected `CodeSHA256 (<supplied>) is different from current CodeSHA256 in $LATEST (<actual>). Please try again with the CodeSHA256 in $LATEST.`; got `CodeSha256 'qXiK6JBqXyFFSu19/Ka/Ldh9gSBz5TA/o0Ckp8eUolA=' does not match the SHA256 of the function's deployment package, which is 'RJ6t/q0oTDRCUBjUSI3AwqMh6H2yjYge1dkj/k2ZWKE='` (HTTP 400, InvalidParameterValueException).

The quoted ARNs, names, UUIDs and hashes are per-run examples; the requirement is the sentence RULE in each case, not those values.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Re-run exactly the checks in trace_ids:
   - lambda.blueprint-tag-tag-resource-past-fifty-tags-rejected (TagResource with 51 tags)
   - lambda.delete-layer-version-idempotent-on-missing, lambda.layer-get-rejected-for-an-unknown-layer-while-list-returns-empty (Delete/GetLayerVersion on a never-published layer)
   - lambda.esm-crud, lambda.list-event-source-mappings-reflects (GetEventSourceMapping on a deleted UUID)
   - lambda.remove-permission, lambda.remove-permission-unknown-statement-not-found (GetPolicy after removing the last statement)
   - lambda.resource-not-found-error-consistency (GetAlias for an alias that does not exist)
   - lambda.permission-revisionid-precondition (AddPermission with RevisionId=00000000-0000-0000-0000-000000000000)
   - lambda.stale-revision-id (UpdateFunctionConfiguration with a superseded RevisionId)
   - lambda.publish-version-stale-sha-precondition-failed (PublishVersion with a CodeSha256 of other bytes)
3. In each response assert the error `Message` equals AWS's sentence for that condition. Status and Code already match; only Message diverges.
