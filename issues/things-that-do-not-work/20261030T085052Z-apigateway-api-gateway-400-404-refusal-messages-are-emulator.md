Issue-Source: trace-sweep-divergence
Issue-Trace: apigateway-documentation-version-invalid-restapi
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigateway-documentation-version-invalid-restapi apigateway-stage-canary-settings-promote-and-remove apigateway-stage-method-settings-path-grammar apigw-patch-deployment-stage-invalid-path apigateway-tag-boundary-length-and-character-validation apigateway-tag-isolation-across-resource-types apigateway-tag-operations-error-paths apigateway-tag-stage-inheritance-vs-direct-tag-precedence apigateway-usage-plan-lifecycle-tied-to-deployed-stage apigw-patch-account-cloudwatchrolearn-op-support apigw-patch-model-validator-authorizer-invalid-path
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway|UpdateDocumentationVersion|4xx->4xx|$.message
Issue-Key: apigateway|UpdateStage|4xx->4xx|$.message
Issue-Key: apigateway|TagResource|4xx->4xx|$.message
Issue-Key: apigateway|CreateUsagePlan|4xx->4xx|$.message
Issue-Key: apigateway|UpdateAccount|4xx->4xx|$.message
Issue-Key: apigateway|UpdateAuthorizer|4xx->4xx|$.message
Issue-Commit: ba1310c994fc47a47280538753d01db9a79290df

# API Gateway 400/404 refusal messages are emulator-authored instead of AWS's text (6 causes)

> _Note: this issue was identified against the code base at commit `ba1310c994fc47a47280538753d01db9a79290df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDocumentationVersion (also UpdateStage, TagResource, CreateUsagePlan, UpdateAccount, UpdateAuthorizer)

## Description
Six net-new wire divergences in the API Gateway control plane share one fault: the refusal is classified correctly (the HTTP status and the x-amzn-errortype both match the recording) but the `message` field carries prose the emulator's own validators author, not the sentence API Gateway emits for that error class — and in two of the six the emulator also picks the wrong validation step, so it names a different resource/condition than AWS does. All six are BadRequestException (400) or NotFoundException (404) bodies on PATCH/POST/PUT control-plane operations; all are message-only or message-plus-wrong-check, never a status or error-code divergence. Fixing them is one job in this service's validation/refusal-message layer: each refusal site must emit API Gateway's fixed sentence for its error class, filling only the variable parts from the request and from stored state.

Cause 1 of 6 — UpdateDocumentationVersion names the REST API instead of the documentation version.
AWS: Smithy (com.amazonaws.apigateway) UpdateDocumentationVersion PATCH /restapis/{restApiId}/documentation/versions/{documentationVersion}, success 200, models NotFoundException with httpError 404. The model carries no message text, so for the text this is recorded behavior only: the recording shows AWS answering HTTP 404 / NotFoundException with the message `Invalid Documentation version specified` — wholly fixed text, no variable part — when the targeted documentation version cannot be resolved, even though the `restApiId` label is also unresolvable. AWS reports the leaf child of the URI, not the parent.
Emulator: answers 404 / NotFoundException (both correct) with `Invalid API identifier specified 000000000000:nonexistent1` — a different fixed sentence (`Invalid API identifier specified ` plus the variable `<accountId>:<restApiId>` taken from the caller's account and the URI label), i.e. it resolves and refuses on the parent restApiId before ever looking at the documentation-version label.
Why: a client that distinguishes "this API is gone" from "this documentation version is gone" — a CLI/SDK error handler, a CDK/CFN importer, or a retry loop that recreates the version — takes the wrong recovery branch, because the emulator blames the API id for a missing documentation version.
Fix: the refusal/lookup order in UpdateDocumentationVersion's handler validation: resolve the documentationVersion label and emit AWS's fixed `Invalid Documentation version specified` sentence for this operation rather than deferring to the generic restApiId lookup message.
Traces: apigateway-documentation-version-invalid-restapi#task#4.

Cause 2 of 6 — UpdateStage's invalid copy-patch refusal does not state the valid copy:path / copy:from sets.
AWS: Smithy UpdateStage PATCH /restapis/{restApiId}/stages/{stageName}, success 200, input patchOperations: ListOfPatchOperation, models BadRequestException httpError 400. Model text only; the recorded refusal for a `copy` whose `from` is not a legal source is HTTP 400 / BadRequestException with the message pattern `Invalid copy operation with path: <path> and from <from>. Valid copy:path are [/deploymentId, /variables] and valid copy:from are [/canarySettings/deploymentId, /canarySettings/stageVariableOverrides]`. Fixed text: `Invalid copy operation with path: `, ` and from `, and the whole trailing `. Valid copy:path are [...] and valid copy:from are [...]` enumeration, which is a constant of the Stage patch grammar. Variable parts: only the `path` and `from` values copied verbatim from the rejected patch operation.
Emulator: answers 400 / BadRequestException with `a copy of /deploymentId takes from /canarySettings/deploymentId, got "/description"` — emulator-authored lowercase prose that inverts the structure, quotes the offending `from`, and omits both valid-value enumerations.
Why: the valid-set enumeration is the only machine-usable part of this refusal; a caller building stage patches (promote-canary tooling, IaC providers) cannot discover the legal copy:path/copy:from pairs from the emulator's text, and anything asserting the AWS sentence fails.
Fix: the patchOperations grammar validation for Stage (the copy-operation branch) in UpdateStage's validation layer.
Traces: apigateway-stage-canary-settings-promote-and-remove#task#12 (also apigateway-stage-method-settings-path-grammar#task#14, #task#15, apigw-patch-deployment-stage-invalid-path#task#9, #task#10).

Cause 3 of 6 — TagResource's tag-validation refusal uses its own per-violation prose instead of AWS's single fixed sentence.
AWS: Smithy TagResource PUT /tags/{resourceArn} (success 204), tags: MapOfStringToString required, models BadRequestException httpError 400. Model text only; the recording shows one invariant sentence for every tag key/value violation — oversize key, oversize value, or illegal character — HTTP 400 / BadRequestException, message `Invalid tag key/value. Valid characters are letters, numbers, spaces, and the following: _ . : / = + - @. Tag key can be up to 128 characters. Tag value can be up to 256 characters.` This message is entirely fixed text: it has NO variable part, and in particular does not name the offending key, nor its length, nor which of the three rules was broken.
Emulator: answers 400 / BadRequestException with `a tag key may be at most 128 characters, got 129` — a per-violation sentence that both omits AWS's fixed text and interpolates the offending length, a value AWS never includes.
Why: callers that match the documented tag-validation sentence (or that surface it to users) see unrecognised text; and because the emulator varies its message by violation, a client cannot treat the refusal as the single tag-contract error AWS defines.
Fix: the tag key/value validation shared by TagResource (and the tags input of the create operations) in this service's validation layer — collapse all three violations onto AWS's one fixed sentence.
Traces: apigateway-tag-boundary-length-and-character-validation#task#8 (also #task#9, apigateway-tag-isolation-across-resource-types#task#13, apigateway-tag-operations-error-paths#task#6, apigateway-tag-stage-inheritance-vs-direct-tag-precedence#task#13).

Cause 4 of 6 — CreateUsagePlan refuses an unresolvable apiStages entry by naming the apiId:stage pair instead of the account-qualified REST API id.
AWS: Smithy CreateUsagePlan POST /usageplans (success 201), apiStages: ListOfApiStage, models NotFoundException httpError 404. Model text only; the recording shows AWS resolving the entry's `apiId` FIRST and refusing an unknown REST API id with HTTP 404 / NotFoundException, message fixed text `Invalid API identifier specified ` followed by the variable `<accountId>:<restApiId>` — the caller's account id and the apiId copied from the request's apiStages entry. The requested stage name does not appear at all, and no usage plan is created.
Emulator: answers 404 / NotFoundException (both correct) with `API Stage not found: nonexistentapi123:nostage` — wrong fixed text, no account id, and an extra stage value AWS never mentions, because the handler resolves (apiId, stage) as one unit.
Why: a client cannot distinguish an unknown REST API id from an unknown stage of a known API, and loses the account scoping AWS provides, so usage-plan provisioning logic mis-reports or mis-retries.
Fix: the apiStages resolution in CreateUsagePlan's handler (the shared no-such-api-stage message helper in this service's state module): check the apiId against the account's stored REST APIs first and emit AWS's account-qualified sentence naming only that id; keep a stage-level refusal for an apiId that does resolve. Note: open issue 20261030T045621Z-apigateway-api-gateway-a-usage-plan-apistages-entry-naming.md describes this same refusal from the BEHAVIORAL corpus, but it carries no wire replay key, so this fixture step is not gated by it; closing both together is expected.
Traces: apigateway-usage-plan-lifecycle-tied-to-deployed-stage#task#8.

Cause 5 of 6 — UpdateAccount validates the cloudwatchRoleArn VALUE before rejecting an unsupported patch op on that path.
AWS: Smithy UpdateAccount PATCH /account (success 200), patchOperations: ListOfPatchOperation, models BadRequestException httpError 400. Model text only; the recording shows that for an `add` on /cloudwatchRoleArn — a path that only supports `replace` — AWS refuses on the op/path grammar BEFORE inspecting the value, with HTTP 400 / BadRequestException and the message pattern `Invalid patch path  '<path>' specified for op '<op>'. Must be one of: [<allowed paths for that op>]` (note the doubled space after `path`). Fixed text: `Invalid patch path  `, `' specified for op '`, `'. Must be one of: [`; variable parts: the rejected operation's `path` and `op` copied from the request, and the allowed-path list for that op on the Account resource (here `[/cloudwatchRoleArn]`).
Emulator: answers 400 / BadRequestException with `Invalid ARN specified in the request` — a value-level refusal, so the patch-grammar violation is never reported and neither the rejected path/op nor the allowed-path list appears.
Why: a caller that fixes the reported problem fixes the wrong one: it rewrites the role ARN and retries, when the request is actually refused for using `add` where only `replace` is supported, and the emulator never tells it which paths that op accepts.
Fix: ordering and content of the patchOperations grammar validation in UpdateAccount's validation layer — reject the op/path combination (with AWS's allowed-path enumeration) before any value-shape/ARN check.
Traces: apigw-patch-account-cloudwatchrolearn-op-support#task#3.

Cause 6 of 6 — UpdateAuthorizer's invalid-patch-path refusal replaces AWS's allowed-path list with generic prose.
AWS: Smithy UpdateAuthorizer PATCH /restapis/{restApiId}/authorizers/{authorizerId} (success 200), patchOperations: ListOfPatchOperation, models BadRequestException httpError 400. Model text only; the recording shows HTTP 400 / BadRequestException with the same pattern as cause 5: `Invalid patch path  '<path>' specified for op '<op>'. Must be one of: [<allowed paths for that op>]`, where for an `add` on an Authorizer the allowed list is the multi-valued members `[/providerARNs, /identitySource]`. The variable parts are the rejected path and op from the request plus the per-resource, per-op allowed-path list; the rest is fixed text.
Emulator: answers 400 / BadRequestException and reproduces the prefix correctly (`Invalid patch path  '/sim2realNoSuchField' specified for op 'add'.`) but ends with `Please choose supported operations` instead of `Must be one of: [/providerARNs, /identitySource]` — the one part of the sentence that tells the caller what is legal.
Why: the allowed-path enumeration is the actionable payload of this refusal; without it a client (or an IaC provider generating authorizer patches) cannot correct the request, and anything asserting AWS's sentence fails on the tail.
Fix: the patchOperations grammar validation for Authorizer in UpdateAuthorizer's validation layer — emit the per-op allowed-path list rather than the generic trailing sentence. Shared with cause 5, which is the same sentence template on the Account resource.
Traces: apigw-patch-model-validator-authorizer-invalid-path#task#11.

Six causes must be closed before submitting: causes 1 and 4 (wrong resource/condition reported in a 404), cause 5 (wrong validation step reported in a 400), and causes 2, 3 and 6 (the 400 message text itself).

## Evidence
All six are from the simparity-wire-aws-v1 replay against this checkout. Status and x-amzn-errortype matched the recording in every one; only `$.message` diverged (plus, in causes 1, 4 and 5, which check produced it).

Cause 1 — UpdateDocumentationVersion, fixture apigateway-documentation-version-invalid-restapi step task#4:
  status 404 expected, 404 actual (match)
  $.message expected 'Invalid Documentation version specified'
  $.message actual   'Invalid API identifier specified 000000000000:nonexistent1'
  Message anatomy (AWS): all fixed text, no variable part. Emulator: fixed 'Invalid API identifier specified ' + variable '<accountId>:<restApiId>' (account from the caller, restApiId from the URI label) — the wrong sentence for this lookup.

Cause 2 — UpdateStage, fixture apigateway-stage-canary-settings-promote-and-remove step task#12 (cluster of 5: + apigateway-stage-method-settings-path-grammar#task#14, #task#15, apigw-patch-deployment-stage-invalid-path#task#9, #task#10):
  status 400 expected, 400 actual (match)
  $.message expected 'Invalid copy operation with path: /deploymentId and from /description. Valid copy:path are [/deploymentId, /variables] and valid copy:from are [/canarySettings/deploymentId, /canarySettings/stageVariableOverrides]'
  $.message actual   'a copy of /deploymentId takes from /canarySettings/deploymentId, got "/description"'
  Message anatomy (AWS): fixed 'Invalid copy operation with path: ' + VARIABLE path + ' and from ' + VARIABLE from + fixed '. Valid copy:path are [/deploymentId, /variables] and valid copy:from are [/canarySettings/deploymentId, /canarySettings/stageVariableOverrides]'. The two valid-value lists are constants of the Stage patch grammar, not recording values; '/deploymentId' and '/description' here are the request's own patch path and from.

Cause 3 — TagResource, fixture apigateway-tag-boundary-length-and-character-validation step task#8 (cluster of 5: + #task#9, apigateway-tag-isolation-across-resource-types#task#13, apigateway-tag-operations-error-paths#task#6, apigateway-tag-stage-inheritance-vs-direct-tag-precedence#task#13):
  status 400 expected, 400 actual (match)
  $.message expected 'Invalid tag key/value. Valid characters are letters, numbers, spaces, and the following: _ . : / = + - @. Tag key can be up to 128 characters. Tag value can be up to 256 characters.'
  $.message actual   'a tag key may be at most 128 characters, got 129'
  Message anatomy (AWS): entirely fixed text — no variable part at all. Emulator: emulator-authored text plus the variable observed length (129 in this run), which AWS never emits.

Cause 4 — CreateUsagePlan, fixture apigateway-usage-plan-lifecycle-tied-to-deployed-stage step task#8:
  status 404 expected, 404 actual (match)
  $.message expected 'Invalid API identifier specified 000000000000:nonexistentapi123'
  $.message actual   'API Stage not found: nonexistentapi123:nostage'
  Message anatomy (AWS): fixed 'Invalid API identifier specified ' + variable '<accountId>:<restApiId>' — the account id of the caller and the apiId from the request's apiStages entry ('nonexistentapi123' here); the stage name is NOT part of it. Emulator: fixed 'API Stage not found: ' + '<apiId>:<stage>'.

Cause 5 — UpdateAccount, fixture apigw-patch-account-cloudwatchrolearn-op-support step task#3:
  status 400 expected, 400 actual (match)
  $.message expected "Invalid patch path  '/cloudwatchRoleArn' specified for op 'add'. Must be one of: [/cloudwatchRoleArn]"
  $.message actual   'Invalid ARN specified in the request'
  Message anatomy (AWS): fixed "Invalid patch path  '" (two spaces after 'path') + variable path + fixed "' specified for op '" + variable op + fixed "'. Must be one of: [" + the allowed-path list for that op on the Account resource + ']'. '/cloudwatchRoleArn' and 'add' come from the request's patch operation.

Cause 6 — UpdateAuthorizer, fixture apigw-patch-model-validator-authorizer-invalid-path step task#11:
  status 400 expected, 400 actual (match)
  $.message expected "Invalid patch path  '/sim2realNoSuchField' specified for op 'add'. Must be one of: [/providerARNs, /identitySource]"
  $.message actual   "Invalid patch path  '/sim2realNoSuchField' specified for op 'add'. Please choose supported operations"
  Message anatomy: identical template to cause 5; the prefix (including the doubled space) is already right, only the fixed tail 'Must be one of: [' + the per-resource/per-op allowed-path list + ']' is replaced by 'Please choose supported operations'. '/sim2realNoSuchField' and 'add' are the request's values; '[/providerARNs, /identitySource]' is the Authorizer add-path constant.

Neither of the other open API Gateway refusal bundles covers these steps: 20261029T193654Z-apigateway-api-gateway-refusal-contracts-part-1/-part-2/-part-3 and 20261029T220031Z-apigateway-notfoundexception-messages-do-not-match-aws-wording.md collect different causes and carry none of these six replay keys; 20261030T045621Z-apigateway-api-gateway-a-usage-plan-apistages-entry-naming.md describes cause 4's root cause from the behavioral corpus but carries no wire replay key.

## Steps to Reproduce
Boot the emulator from this checkout and drive the API Gateway control plane (endpoint-url at the emulator's data listener).

Cause 1: aws apigateway update-documentation-version --rest-api-id nonexistent1 --documentation-version v1 --patch-operations op=replace,path=/description,value=x
  -> 404 NotFoundException with 'Invalid API identifier specified <account>:nonexistent1'; AWS answers 'Invalid Documentation version specified'.

Cause 2: create a REST API and a deployed stage, then: aws apigateway update-stage --rest-api-id <apiId> --stage-name <stage> --patch-operations op=copy,path=/deploymentId,from=/description
  -> 400 BadRequestException with 'a copy of /deploymentId takes from ...'; AWS answers 'Invalid copy operation with path: /deploymentId and from /description. Valid copy:path are [/deploymentId, /variables] and valid copy:from are [/canarySettings/deploymentId, /canarySettings/stageVariableOverrides]'.

Cause 3: aws apigateway tag-resource --resource-arn <any valid apigateway ARN> --tags <129-character key>=v
  -> 400 with 'a tag key may be at most 128 characters, got 129'; AWS answers the single fixed 'Invalid tag key/value. Valid characters are ...' sentence. Repeat with a 257-character value and with an illegal character and observe the emulator varying its text where AWS does not.

Cause 4: aws apigateway create-usage-plan --name p1 --api-stages apiId=nonexistentapi123,stage=nostage
  -> 404 with 'API Stage not found: nonexistentapi123:nostage'; AWS answers 'Invalid API identifier specified <account>:nonexistentapi123'.

Cause 5: aws apigateway update-account --patch-operations op=add,path=/cloudwatchRoleArn,value=arn:aws:iam::000000000000:role/anything
  -> 400 'Invalid ARN specified in the request'; AWS refuses the op/path first: "Invalid patch path  '/cloudwatchRoleArn' specified for op 'add'. Must be one of: [/cloudwatchRoleArn]".

Cause 6: create an authorizer, then: aws apigateway update-authorizer --rest-api-id <apiId> --authorizer-id <authId> --patch-operations op=add,path=/sim2realNoSuchField,value=x
  -> 400 ending 'Please choose supported operations'; AWS ends 'Must be one of: [/providerARNs, /identitySource]'.

Then re-replay the wire corpus and confirm these steps pass: apigateway-documentation-version-invalid-restapi#task#4, apigateway-stage-canary-settings-promote-and-remove#task#12, apigateway-tag-boundary-length-and-character-validation#task#8, apigateway-usage-plan-lifecycle-tied-to-deployed-stage#task#8, apigw-patch-account-cloudwatchrolearn-op-support#task#3, apigw-patch-model-validator-authorizer-invalid-path#task#11.
