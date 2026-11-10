Issue-Source: trace-sweep-divergence
Issue-Trace: bedrock-data-automation-project-error-paths
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: bedrock-data-automation-project-error-paths
Issue-Shape-Id: com.amazonaws.bedrockdataautomation#AmazonBedrockKeystoneBuildTimeService
Issue-Key: bedrock-data-automation|UpdateDataAutomationProject|4xx->4xx|$.message
Issue-Commit: b2388491b42b5ba2f7f64857139a543aed866088

# UpdateDataAutomationProject not-found refusal returns emulator prose instead of AWS wording

> _Note: this issue was identified against the code base at commit `b2388491b42b5ba2f7f64857139a543aed866088`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDataAutomationProject

## Description
UpdateDataAutomationProject's ResourceNotFoundException path returns the correct HTTP status (404) and the correct error type, but the emulator's own prose as the message, and that prose interpolates the requested project ARN where AWS's wording is fixed text with nothing taken from the request. One net-new cause.

Note on adjacent open issues: 20261029T170936Z-bedrock-data-automation-blueprint-and-project-error-messages-diverge-from-aws covers the same wording fault on CreateBlueprint, GetBlueprint, DeleteBlueprint, CreateDataAutomationProject and GetDataAutomationProject. It does NOT cover UpdateDataAutomationProject -- that operation appears in none of its Issue-Keys and in none of its causes -- so this write path can be (and is) left diverging after that issue is closed. Fixing the two together is natural, but this cause needs its own closure and its own replay.

=== Cause 1 of 1: UpdateDataAutomationProject not-found message echoes the requested project ARN ===
AWS: Smithy: UpdateDataAutomationProject (PUT /data-automation-projects/{projectArn}/, @idempotent) models ResourceNotFoundException, errorKind client, @httpError(404), a single member message -> NonBlankString (pattern ^[\s\S]+$), doc "This exception is thrown when a resource referenced by the operation does not exist". Its projectArn is an @httpLabel, @required, @length(max 128), pattern ^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:(aws|[0-9]{12}):data-automation-project/[a-zA-Z0-9-]{12,36}$. The model does not state the message text; the recorded 404 response gives it as exactly "Project Arn not found." -- the whole message is FIXED text: there are no variable parts, and in particular no ARN, name or id from the request or from state appears in it. This is the same wording the recording shows for GetDataAutomationProject's 404, i.e. the service words the project-not-found refusal identically across the project operations.
Emulator: status 404 and the right error type (correct), but message "no data automation project exists with ARN <projectArn>" -- different fixed text, plus the requested project ARN interpolated into it.
Why: ResourceNotFoundException here carries no code member, so the message is the only discriminator a client has for this refusal. A client that updates a project and keys its recreate-vs-fail decision off the service's not-found wording never matches this string, so a project that is genuinely gone surfaces as an unclassified error and the recreate path is never taken; retry wrappers that whitelist the service's not-found text to stop retrying a doomed PUT keep retrying instead. Interpolating the ARN also makes the message a function of the request, which the service's fixed wording never is, so no client or test can assert the text at all.
Fix: the UpdateDataAutomationProject handler's not-found refusal wording -- emit the fixed "Project Arn not found." and interpolate nothing. Single-home it with the other project not-found refusals so the write and read paths cannot drift apart again.
Traces: bedrock-data-automation-project-error-paths#task#5.

## Evidence
Cause 1 (fixture bedrock-data-automation-project-error-paths, step task#5, UpdateDataAutomationProject, expected status 404, actual status 404):
  $.message: value_mismatch -- expected 'Project Arn not found.', got 'no data automation project exists with ARN <projectArn>'.
  In this run the emulator's interpolated value was arn:aws:bedrock:us-west-2:000000000000:data-automation-project/3a6437ef-0000000000000000; that ARN is a per-run generated value and is NOT part of the requirement. The requirement is the fixed message text 'Project Arn not found.' with no variable parts at all. Cluster size 1 (affected_count 1).
  Status and x-amzn-errortype matched; the message is the only diverging field.

Model citations (services/service-com-amazonaws-bedrockdataautomation-amazonbedrockkeystonebuildtimeservice): UpdateDataAutomationProject errors = AccessDeniedException(403), ConflictException(409), InternalServerException(500), ResourceNotFoundException(404), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400); http PUT /data-automation-projects/{projectArn}/ code 200, @idempotent. ResourceNotFoundException: errorKind client, @httpError(404), members {message -> NonBlankString}, doc "This exception is thrown when a resource referenced by the operation does not exist". UpdateDataAutomationProjectRequest.projectArn -> DataAutomationProjectArn, @httpLabel @required @length(max 128). The message text itself is recorded behavior only (the model does not specify prose); it is taken from the recorded 404 response.

## Steps to Reproduce
1. Replay the wire corpus simparity-wire-aws-v1 against the emulator (replay_wire_corpus).
2. Cause 1: fixture bedrock-data-automation-project-error-paths, step task#5 -- call UpdateDataAutomationProject with a syntactically valid data-automation-project ARN that no project in state corresponds to (the fixture deletes the project it created, then updates it), supplying the required standardOutputConfiguration.
3. Observe the 404 body: the emulator returns message 'no data automation project exists with ARN <the requested ARN>'.
4. Expected: status 404 with the ResourceNotFoundException message exactly 'Project Arn not found.' -- fixed text, with no ARN or other request/state value interpolated.
5. Re-replay that fixture/step to confirm the fix; the step passes only when the message is byte-identical to 'Project Arn not found.'.
