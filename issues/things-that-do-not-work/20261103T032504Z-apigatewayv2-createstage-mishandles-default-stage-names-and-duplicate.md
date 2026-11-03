Issue-Source: trace-sweep-divergence
Issue-Trace: apigatewayv2-sstr-traj-default-stage-name-forms
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: apigatewayv2-sstr-traj-default-stage-name-forms
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.autoDeploy
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.createdDate
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.defaultRouteSettings
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.lastUpdatedDate
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.routeSettings
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.stageName
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.stageVariables
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|$.tags
Issue-Key: apigatewayv2|CreateStage|2xx->4xx|status
Issue-Key: apigatewayv2|CreateStage|4xx->4xx|$.message
Issue-Key: apigatewayv2|CreateStage|4xx->2xx|$.message
Issue-Key: apigatewayv2|CreateStage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: apigatewayv2|CreateStage|4xx->2xx|status
Issue-Commit: 2d2373ac96719fb51b225d122133a9d558f5af51

# apigatewayv2 CreateStage mishandles $Default stage names and duplicate stage conflicts

> _Note: this issue was identified against the code base at commit `2d2373ac96719fb51b225d122133a9d558f5af51`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStage

## Description
CreateStage's stage-name handling diverges three ways: a valid name is refused, an invalid name is refused with emulator-authored text, and a duplicate name is accepted instead of refused. 3 causes.

Cause 1 of 3: a valid stage name is rejected with 400 (fixture apigatewayv2-sstr-traj-default-stage-name-forms, step task#2; same cluster also task#4).
AWS: Smithy CreateStage — POST /v2/apis/{ApiId}/stages, httpResponseCode 201; input StageName targets StringWithLengthBetween1And128 and the only modeled refusals are BadRequestException(400)/ConflictException(409)/NotFoundException(404)/TooManyRequestsException(429). The recorded response for the '$Default' name form is 201 with stageName '$Default', defaultRouteSettings {detailedMetricsEnabled:false}, routeSettings {}, stageVariables {}, autoDeploy true, createdDate/lastUpdatedDate timestamps and tags {} — i.e. AWS accepts stage names containing the '$' sigil ('$default' and its case variants) and creates the stage.
Emulator: answers 400 and returns no stage body at all (every modeled output member reported missing), so the stage is never created.
Why: the quick-create/auto-deploy workflow names its stage '$default'. A client that creates it gets a hard 400 and cannot deploy the API at all; every later step that depends on the stage fails.
Fix: CreateStage's stage-name validation in the apigatewayv2 handler — accept the modeled 1..128-character name including the '$default' forms, then build the stage with the documented defaults.

Cause 2 of 3: the empty/absent stage-name refusal message is emulator-authored (step task#3).
AWS: 400 BadRequestException with message 'Stage name must be non-empty' (recorded response; the model only states StageName is required with length 1..128). Fixed text: the whole sentence 'Stage name must be non-empty' — it has no variable parts.
Emulator: 400 with message "1 validation error detected: Value null at 'stageName' failed to satisfy constraint: Member must not be null" — a generic constraint-framework sentence that neither the model nor AWS produces for this operation.
Why: clients and tests that match on the AWS sentence (or surface it to users) see different text for the same refusal, so error-handling branches keyed on the message do not fire.
Fix: CreateStage's input validation — emit the AWS sentence for a missing/empty stageName instead of the generic null-constraint text.

Cause 3 of 3: a duplicate stage name is accepted with 201 instead of refused 409 (step task#5).
AWS: 409 ConflictException (modeled on CreateStage, httpError 409) with message 'Stage already exists' and the error type carried in the x-amzn-errortype header. Fixed text: 'Stage already exists'; no variable parts in the recorded message.
Emulator: answers 201 and creates a second stage with the same name, sending no x-amzn-errortype header and no message.
Why: a client that relies on the 409 to detect 'already created' (idempotent deploy scripts, create-or-update logic) silently gets a duplicate stage, and the emulator's stage collection then holds two stages with one name — divergent state for every later Get/UpdateStage.
Fix: CreateStage's handler/state — look up the stage name within the API before insert and raise ConflictException with AWS's sentence.

Related but non-owning: 20261029T220031Z-apigatewayv2-request-validation-and-refusal-messages-diverge-from.md and 20261029T220031Z-apigatewayv2-responses-omit-defaulted-and-derived-members.md describe the same classes for other operations; neither carries these replay keys.

## Evidence
Replay divergence clusters (service=apigatewayv2, operation CreateStage, fixture apigatewayv2-sstr-traj-default-stage-name-forms):
- task#2 (and task#4): 'status: expected 201, got 400' plus missing $.stageName ('$Default'), $.defaultRouteSettings ({detailedMetricsEnabled:false}), $.routeSettings ({}), $.stageVariables ({}), $.autoDeploy (true), $.createdDate, $.lastUpdatedDate, $.tags ({}).
- task#3: '$.message: value_mismatch (expected "Stage name must be non-empty", got "1 validation error detected: Value null at \'stageName\' failed to satisfy constraint: Member must not be null")' on a 400/400.
- task#5: 'status: expected 409, got 201'; '$.message: missing (expected "Stage already exists")'; '$headers.x-amzn-errortype: missing'.

## Steps to Reproduce
1. Replay fixture apigatewayv2-sstr-traj-default-stage-name-forms.
2. task#2/task#4: POST /v2/apis/{apiId}/stages with stageName '$Default' -> emulator 400, AWS 201 with the stage body.
3. task#3: POST the same path with an empty/absent stageName -> emulator returns the generic 'N validation error(s) detected' sentence, AWS returns 'Stage name must be non-empty'.
4. task#5: POST a stageName that already exists on the API -> emulator 201 (duplicate created), AWS 409 ConflictException 'Stage already exists' with x-amzn-errortype.
