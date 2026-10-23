Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: entityresolution.generate-match-id-dedup-across-calls entityresolution.batch-delete-unique-id-empty-list-rejected
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution|GenerateMatchId|ValidationException
Issue-Commit: 048778663a26e363156c6db724999c4fb229af36

# entityresolution GenerateMatchId rejects rule-based workflows that omit resolutionTechniques.enableRealTimeMatching

> _Note: this issue was identified against the code base at commit `048778663a26e363156c6db724999c4fb229af36`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GenerateMatchId

## Description
One root cause in the entityresolution emulator, reached through an operation the emulator does answer.

Cause 1 of 1: GenerateMatchId refuses any matching workflow whose CreateMatchingWorkflow request did not explicitly set resolutionTechniques.enableRealTimeMatching, so the real-time matching data plane is unreachable for the default RULE_MATCHING workflow shape.
Failing check ids: entityresolution.generate-match-id-dedup-across-calls, entityresolution.batch-delete-unique-id-empty-list-rejected.

AWS: Smithy model (services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model/com-amazonaws-entityresolution-awsveniceservice.json) GenerateMatchId POST /matchingworkflows/{workflowName}/generateMatches, input {workflowName (@required, @length 1-255, @pattern ^[a-zA-Z_0-9-]*$), records (@required, @length 1-1), processingType (ProcessingType: CONSISTENT|EVENTUAL|EVENTUAL_NO_LOOKUP)}, output {matchGroups (@required), failedRecords (@required)}, success status 200; modeled errors AccessDeniedException 403, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429, ValidationException 400. ResolutionTechniques.enableRealTimeMatching is an OPTIONAL Boolean with no modeled default; its doc says only "Specifies whether real-time matching is enabled for the rule-based matching workflow. When you enable real-time matching, you can use the GenerateMatchId operation with the workflow." The model states no rule that an ABSENT enableRealTimeMatching forbids GenerateMatchId. Recorded behavior: a workflow created with resolutionTechniques {resolutionType: RULE_MATCHING, ruleBasedProperties {attributeMatchingModel ONE_TO_ONE, matchPurpose IDENTIFIER_GENERATION, rules [emailRule on [email]]}} and NO enableRealTimeMatching member accepts GenerateMatchId and answers 200 with a non-empty matchGroups and an empty failedRecords; two records sharing the rule's matching key resolve to the same matchId, an unrelated record gets a different one, and the resulting match data is readable through GetMatchId.
Emulator: every such call is refused with HTTP 400 ValidationException, example message "GenerateMatchId requires real-time matching: MatchingWorkflow <name> was created with resolutionTechniques.enableRealTimeMatching disabled". The handler gates on crate::state::real_time_matching_enabled(&workflow) and treats an absent member as disabled, so no record is ever indexed and matchGroups is never produced.
Why: a client that creates a rule-based IDENTIFIER_GENERATION workflow exactly as AWS accepts it, then calls GenerateMatchId, gets a hard 400 instead of a match group. The whole real-time matching plane (GenerateMatchId -> GetMatchId -> BatchDeleteUniqueId) is unreachable, and downstream checks that merely need a populated match index as a positive control cannot run at all: entityresolution.batch-delete-unique-id-empty-list-rejected fails on the GenerateMatchId positive control before it can ever exercise BatchDeleteUniqueId's empty-uniqueIds rejection, so that validation path is also unverifiable today. The required status/code contract diverged (AWS 200 vs emulator 400/ValidationException); the quoted message is an example only.
Fix: the GenerateMatchId handler's real-time-matching precondition and the predicate it calls — services/service-com-amazonaws-entityresolution-awsveniceservice/src/ops/generate_match_id.rs (the `if !crate::state::real_time_matching_enabled(&workflow)` refusal) and crate::state::real_time_matching_enabled in src/state.rs. An absent enableRealTimeMatching on a RULE_MATCHING workflow must not refuse the call; the refusal belongs only where the member is explicitly false (and the non-RULE_MATCHING refusal already above it stays).
Traces: entityresolution.generate-match-id-dedup-across-calls (GenerateMatchId on workflow sim2real-er-rt-wf-* raised ValidationException instead of returning matchGroups), entityresolution.batch-delete-unique-id-empty-list-rejected (same refusal on workflow sim2real-er-bdu-wf-*, blocking the positive control).

## Evidence
entityresolution.generate-match-id-dedup-across-calls:
  expected: GenerateMatchId(workflowName=<rule-based IDENTIFIER_GENERATION workflow>, records=[{uniqueId 'rec-a', recordAttributeMap {email 'dup@example.com'}}], processingType omitted) -> HTTP 200 with matchGroups[0].matchId; a second call with uniqueId 'rec-b' and the same email returns the SAME matchId; a third, unrelated record returns a DIFFERENT matchId.
  actual:   botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GenerateMatchId operation: GenerateMatchId requires real-time matching: MatchingWorkflow sim2real-er-rt-wf-2d3f017c7c5a was created with resolutionTechniques.enableRealTimeMatching disabled (HTTP 400).

entityresolution.batch-delete-unique-id-empty-list-rejected:
  expected: the same GenerateMatchId call succeeds (no failedRecords, non-empty matchGroups) so the workflow holds a real record; the check then asserts BatchDeleteUniqueId(uniqueIds=[]) -> ValidationException while BatchDeleteUniqueId(uniqueIds=['rec-1']) -> status COMPLETED/ACCEPTED.
  actual:   botocore.errorfactory.ValidationException: ... GenerateMatchId requires real-time matching: MatchingWorkflow sim2real-er-bdu-wf-70ef6e9319b2 was created with resolutionTechniques.enableRealTimeMatching disabled (HTTP 400) — the check aborts at the positive control; nothing downstream is exercised.

## Steps to Reproduce
1. entityresolution CreateSchemaMapping with mappedInputFields [{fieldName id, type UNIQUE_ID}, {fieldName email, type EMAIL_ADDRESS, matchKey email}].
2. CreateMatchingWorkflow with that schema in inputSourceConfig, an outputSourceConfig S3 path, a roleArn, incrementalRunConfig {incrementalRunType IMMEDIATE}, and resolutionTechniques = {resolutionType: RULE_MATCHING, ruleBasedProperties: {attributeMatchingModel: ONE_TO_ONE, matchPurpose: IDENTIFIER_GENERATION, rules: [{ruleName: emailRule, matchingKeys: [email]}]}} — do NOT set enableRealTimeMatching (as AWS allows).
3. Call GenerateMatchId(workflowName=<that workflow>, records=[{inputSourceARN: <glue table arn>, uniqueId: 'rec-a', recordAttributeMap: {email: 'dup@example.com'}}]).
4. Observe HTTP 400 ValidationException naming enableRealTimeMatching instead of HTTP 200 with a matchGroups entry.
5. Repeat with uniqueId 'rec-b' and the same email: on AWS both calls return the same matchId and GetMatchId resolves the records; on the emulator neither call is ever accepted.
