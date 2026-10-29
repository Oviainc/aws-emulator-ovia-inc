Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore.recommendation-lifecycle bedrock-agentcore.deploy-code-interpreter-custom-resource-executes
Issue-Shape-Id: com.amazonaws.bedrockagentcore#AmazonBedrockAgentCore
Issue-Key: bedrock-agentcore|StartRecommendation|ValidationException
Issue-Key: bedrock-agentcore|CreateStack|ValidationError
Issue-Commit: 7240e32f2500d20910730c71d4ad0714ff1f3217

# StartRecommendation rejects RFC3339 timestamps modeled as @timestampFormat date-time, and CloudFormation does not recognize the AWS::BedrockAgentCore::CodeInterpreterCustom resource type

> _Note: this issue was identified against the code base at commit `7240e32f2500d20910730c71d4ad0714ff1f3217`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRecommendation

## Description
Two net-new causes that make whole AgentCore surfaces unreachable: a timestamp deserialization defect that refuses every well-formed StartRecommendation request, and a missing CloudFormation resource type that makes a template referencing an AgentCore code interpreter unsubmittable.

Cause 1 of 2: StartRecommendation refuses a request whose trace-window timestamps are serialized as RFC3339 date-times, answering ValidationException 'invalid timestamp: failed to parse int'.
AWS: Smithy: StartRecommendation POST /recommendations, @http code 202; required input recommendationConfig: RecommendationConfig, whose systemPromptRecommendationConfig.agentTraces is AgentTracesConfig; its cloudwatchLogs member is CloudWatchLogsTraceConfig with required startTime and endTime, both target Timestamp carrying @timestampFormat date-time. A date-time-formatted timestamp member is serialized in the JSON body as an RFC3339 string (what every AWS SDK sends for that trait), so a request carrying ISO-8601 strings for startTime/endTime is well-formed and AWS answers 202 with recommendationId/recommendationArn/status (RecommendationStatus: PENDING | IN_PROGRESS | COMPLETED | FAILED | DELETING). ValidationException is modeled @httpError 400 for input that actually violates a constraint — a correctly formatted date-time is not such input.
Emulator: 400 ValidationException with message 'invalid timestamp: failed to parse int' — its body deserializer ignores @timestampFormat date-time for these members and accepts only epoch-number timestamps, so the operation can never be called successfully by an SDK client. The message is also not AWS's: AWS's validation text for this service is the '1 validation error detected: Value at '<member>' failed to satisfy constraint: ...' form naming the offending member, never a lowercase parser message with no member path.
Why: the entire recommendation surface (StartRecommendation -> GetRecommendation -> DeleteRecommendation, and the PENDING/IN_PROGRESS -> terminal transition) is unreachable through any SDK, because the SDK serializes the modeled format and the emulator rejects exactly that format. Worse, the failure is reported as a client validation error, so a caller is told its own correct request is malformed and no member name is given to correct.
Fix: the bedrock-agentcore request deserializer for Timestamp members carrying @timestampFormat date-time (StartRecommendation's input parsing, services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/start_recommendation.rs and the shared JSON timestamp decoder) — accept the RFC3339 string form for date-time members, and when a timestamp truly is unparseable raise ValidationException with the service's '1 validation error detected: Value at '<member path>' ...' wording.
Traces: bedrock-agentcore.recommendation-lifecycle.

Cause 2 of 2: CloudFormation rejects a template that declares AWS::BedrockAgentCore::CodeInterpreterCustom — the resource type is not registered at all.
AWS: recorded behavior only (the AgentCore Smithy model declares the ARN reference types but CloudFormation's type registry is not part of it): CreateStack with a template whose Resources include Type: AWS::BedrockAgentCore::CodeInterpreterCustom is ACCEPTED — the stack is created and the resource is provisioned, reaching CREATE_COMPLETE, after which the created code interpreter is usable through the data plane. CreateStack's ValidationError 'Template format error: Unrecognized resource types: [...]' is AWS's answer only for a type that does not exist in its registry.
Emulator: CreateStack is refused outright with ClientError code 'ValidationError', message 'Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::CodeInterpreterCustom]', so the stack is never created and nothing downstream can be exercised.
Why: infrastructure-as-code is the normal way AgentCore resources are provisioned; a client whose template is valid against AWS cannot even submit it here, and the error claims the type does not exist rather than reporting an emulator gap. Any deployment-shaped test of AgentCore stops at CreateStack.
Note: the open issue 20261029T212239Z-bedrock-agentcore-control-cloudformation-does-not-provision-agentcore-resources-aws-bedrockagentcore.md covers AWS::BedrockAgentCore::Runtime/::Memory/::Gateway stacks COMPLETING without provisioning anything. This cause is the different and earlier failure mode — ::CodeInterpreterCustom is not in the type registry at all, so CreateStack is refused — and it carries no behavioral check id in that issue.
Fix: the CloudFormation resource-type registry and its AgentCore provisioner (the cloudformation service's supported-types table plus the handler that maps AWS::BedrockAgentCore::CodeInterpreterCustom onto bedrock-agentcore-control CreateCodeInterpreter), so the template validates and the resource is actually created.
Traces: bedrock-agentcore.deploy-code-interpreter-custom-resource-executes.

## Evidence
Cause 1: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartRecommendation operation: invalid timestamp: failed to parse int. The request sent recommendationConfig.systemPromptRecommendationConfig.agentTraces.cloudwatchLogs.startTime = now-7d and .endTime = now as datetimes, which the SDK serializes as RFC3339 strings because CloudWatchLogsTraceConfig.startTime/endTime are Timestamp @timestampFormat date-time. Expected: HTTP 202 with recommendationId, recommendationArn, name, type SYSTEM_PROMPT_RECOMMENDATION and status in PENDING/IN_PROGRESS/COMPLETED. The emulator's message ('invalid timestamp: failed to parse int') is itself divergent: it is lowercase parser text with no member path, where this service's ValidationException text is '1 validation error detected: Value at '<member>' failed to satisfy constraint: ...'.
Cause 2: botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::CodeInterpreterCustom]. Expected: CreateStack accepted, stack reaching CREATE_COMPLETE with the code interpreter provisioned (per the check's recorded AWS run).

## Steps to Reproduce
1. Build and boot the emulator, then replay the behavioral suite (simparity-behavioral-aws-v1).
2. Cause 1: run bedrock-agentcore.recommendation-lifecycle — create a CloudWatch Logs log group, then call StartRecommendation(name=..., type='SYSTEM_PROMPT_RECOMMENDATION', recommendationConfig={'systemPromptRecommendationConfig': {'systemPrompt': {'text': ...}, 'agentTraces': {'cloudwatchLogs': {'logGroupArns': [<arn>], 'serviceNames': [...], 'startTime': <datetime>, 'endTime': <datetime>}}, 'evaluationConfig': {...}}}). The emulator answers 400 ValidationException 'invalid timestamp: failed to parse int'; AWS answers 202 and the recommendation is then readable via GetRecommendation.
3. Cause 2: run bedrock-agentcore.deploy-code-interpreter-custom-resource-executes — CreateStack with a template whose Resources contain a Type: AWS::BedrockAgentCore::CodeInterpreterCustom entry. The emulator refuses the CreateStack call with ValidationError 'Template format error: Unrecognized resource types: [AWS::BedrockAgentCore::CodeInterpreterCustom]'; AWS accepts the template and provisions the resource.
