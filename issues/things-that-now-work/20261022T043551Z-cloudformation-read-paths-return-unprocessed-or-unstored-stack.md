Issue-Source: trace-sweep-divergence
Issue-Trace: cfnrep-get-template-processed-transform
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cfnrep-get-template-processed-transform cloudformation-stack-lifecycle-basic-crud cloudformation-stack-policy-and-termination-protection cloudformation-stack-events-and-errors
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation|GetTemplate|2xx->2xx|$.GetTemplateResponse.GetTemplateResult.TemplateBody
Issue-Key: cloudformation|GetStackPolicy|2xx->2xx|$.GetStackPolicyResponse.GetStackPolicyResult
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*]
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*].ClientRequestToken
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*].LogicalResourceId
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*].ResourceProperties
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*].ResourceStatus
Issue-Key: cloudformation|DescribeStackEvents|2xx->2xx|$.DescribeStackEventsResponse.DescribeStackEventsResult.StackEvents.member[*].ResourceType
Issue-Commit: bffba5cba440022693dbdf16c8135b8d2c175737

# CloudFormation read paths return unprocessed or unstored stack data: templates, stack policy, events

> _Note: this issue was identified against the code base at commit `bffba5cba440022693dbdf16c8135b8d2c175737`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTemplate

## Description
FOUR net-new causes, all HTTP 200 responses whose bodies are wrong because the data the read returns was never processed or never stored: the Processed template stage, the stack policy body, and resource-level stack events.

Cause 1 of 4 — GetTemplate(TemplateStage=Processed) returns the raw template (fixture cfnrep-get-template-processed-transform, task#3).
AWS: GetTemplate with TemplateStage=Processed returns the template AFTER transform processing — macro directives are expanded and intrinsic functions evaluated by the transform are resolved. Smithy: GetTemplate.TemplateStage -> enum TemplateStage (Original, Processed); output TemplateBody plus StagesAvailable. For this template (Transform AWS::LanguageExtensions with Fn::ForEach over ['Alpha','Beta'] and Fn::Length) the recorded Processed body is the expanded form: two resources HandleAlpha/HandleBeta and Outputs.HandleCount.Value 3, with no Transform member. The expansion semantics of the language-extensions transform are recorded behavior only.
Emulator: returns the ORIGINAL body verbatim — Transform member still present, Fn::ForEach::Handles unexpanded, Outputs.HandleCount.Value still {'Fn::Length': [...]}.
Why: Processed is the stage clients read to see what CloudFormation will actually deploy; returning the original makes the two stages indistinguishable, so macro/transform-based deployments cannot be inspected, diffed or validated, and anything asserting on expanded resource names sees none of them.
Fix: transform processing in the CloudFormation template pipeline and the GetTemplate handler's stage selection (services/service-com-amazonaws-cloudformation-cloudformation): store/compute the processed template and serve it for TemplateStage=Processed.
Traces: cfnrep-get-template-processed-transform#task#3.

Cause 2 of 4 — GetStackPolicy returns an empty result (cluster also covers cloudformation-stack-policy-and-termination-protection#task#7). AWS: after SetStackPolicy, GetStackPolicy returns 200 with GetStackPolicyResult.StackPolicyBody holding the policy document that was set (Smithy: GetStackPolicy output StackPolicyBody, length 1..16384; no modeled errors). Emulator: GetStackPolicyResult is the empty string — no StackPolicyBody member at all, a type mismatch against the modeled structure. Why: the stack policy is unreadable after being set, so a client cannot verify or round-trip the update-protection policy it installed, and policy-driven update guards cannot be reasoned about. Fix: persist the stack policy body in stack state on SetStackPolicy/CreateStack and serialize GetStackPolicyResult.StackPolicyBody. Traces: cloudformation-stack-lifecycle-basic-crud#task#8.

Cause 3 of 4 — DescribeStackEvents reports no resource-level events. AWS: for a stack containing one resource (AWS::SSM::Parameter, logical id ProbeParam), the event list contains the per-resource events (CREATE_IN_PROGRESS with ResourceStatusReason 'Resource creation Initiated', then CREATE_COMPLETE) alongside the stack-level events, each with ResourceType 'AWS::SSM::Parameter', LogicalResourceId 'ProbeParam', the ResourceProperties blob of the created resource, and ClientRequestToken set (Smithy: StackEvent members LogicalResourceId, PhysicalResourceId, ResourceType, ResourceStatus, ResourceStatusReason, ResourceProperties, ClientRequestToken — 'All events triggered by a given stack operation are assigned the same client request token'). Emulator: only stack-level events exist — member[1] carries LogicalResourceId = the stack name and ResourceType 'AWS::CloudFormation::Stack' with ResourceStatus CREATE_IN_PROGRESS where AWS has the resource's CREATE_COMPLETE, members[2..4] are absent entirely, and no event carries ClientRequestToken or ResourceProperties. Why: stack events are the primary diagnostic for a deployment; with no per-resource events a client cannot tell which resource was created or why one failed, and cannot correlate events by client request token as documented. Fix: emit per-resource events (with ResourceProperties and the operation's ClientRequestToken) from the stack provisioning path into event state, and serialize them in DescribeStackEvents. Traces: cloudformation-stack-events-and-errors#task#6.

Cause 4 of 4 — stack events omit ClientRequestToken even on the stack-level event. AWS: the CREATE_COMPLETE event for the stack itself carries ClientRequestToken (plus EventId, PhysicalResourceId, OperationId, Timestamp, ResourceType AWS::CloudFormation::Stack). Emulator: the event is otherwise correct but has no ClientRequestToken member. Why: as cause 3 — event-to-operation correlation is impossible. Fix: propagate the operation's client request token onto every emitted StackEvent. Traces: cloudformation-stack-lifecycle-basic-crud#task#4.

## Evidence
Replay of simparity-wire-aws-v1; all four clusters are expected_status 200 / actual_status 200 with body divergences.

Cause 1 — cfnrep-get-template-processed-transform#task#3, GetTemplate:
  $.GetTemplateResponse.GetTemplateResult.TemplateBody: value_mismatch
    expected '{"AWSTemplateFormatVersion":"2010-09-09",...,"Outputs":{"HandleCount":{"Value":3}},"Resources":{"HandleAlpha":{"Type":"AWS::CloudFormation::WaitConditionHandle"},"HandleBeta":{...}}}'
    got '{...,"Transform": "AWS::LanguageExtensions", "Resources": {"Fn::ForEach::Handles": ["Suffix", ["Alpha", "Beta"], {"Handle${Suffix}": {...}}]}, "Outputs": {"HandleCount": {"Value": {"Fn::Length": ["a", "b", "c"]}}}}'

Cause 2 — cloudformation-stack-lifecycle-basic-crud#task#8, GetStackPolicy (cluster: cloudformation-stack-policy-and-termination-protection#task#7):
  $.GetStackPolicyResponse.GetStackPolicyResult: type_mismatch (expected {'StackPolicyBody': '{"Statement": [{"Effect": "Allow", "Action": "Update:*", "Principal": "*", "Resource": "*"}]}'}, got '')

Cause 3 — cloudformation-stack-events-and-errors#task#6, DescribeStackEvents:
  member[0].ClientRequestToken: missing
  member[1].ResourceType: value_mismatch (expected 'AWS::SSM::Parameter', got 'AWS::CloudFormation::Stack')
  member[1].LogicalResourceId: value_mismatch (expected 'ProbeParam', got '<stack name>')
  member[1].ResourceStatus: value_mismatch (expected 'CREATE_COMPLETE', got 'CREATE_IN_PROGRESS')
  member[1].ResourceProperties: missing (expected '{"Type":"String","Value":"probe-value"}')
  member[1].ClientRequestToken: missing
  member[2], member[3], member[4]: missing (expected the ProbeParam CREATE_IN_PROGRESS events, ResourceStatusReason 'Resource creation Initiated')

Cause 4 — cloudformation-stack-lifecycle-basic-crud#task#4, DescribeStackEvents:
  member[0]: value_mismatch — expected object includes ClientRequestToken; the emulator's object has every other member (StackId, EventId, StackName, OperationId, LogicalResourceId, PhysicalResourceId, ResourceType, Timestamp, ResourceStatus CREATE_COMPLETE) but no ClientRequestToken.

EventIds, physical ids, timestamps and ARNs in the expected documents are environment-specific examples; the requirements asserted are the processed template content, the presence of StackPolicyBody, the presence of per-resource events with their ResourceType/LogicalResourceId/ResourceStatus/ResourceProperties, and the presence of ClientRequestToken on events.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateStack with a Transform: AWS::LanguageExtensions template using Fn::ForEach and Fn::Length, then call GetTemplate(StackName=..., TemplateStage='Processed') (cfnrep-get-template-processed-transform#task#3): the emulator returns the original unexpanded body; AWS returns the expanded template (HandleAlpha/HandleBeta, HandleCount 3, no Transform member).
3. Call SetStackPolicy(StackName=..., StackPolicyBody='{"Statement":[{"Effect":"Allow","Action":"Update:*","Principal":"*","Resource":"*"}]}') then GetStackPolicy (cloudformation-stack-lifecycle-basic-crud#task#8): GetStackPolicyResult is empty instead of carrying StackPolicyBody.
4. CreateStack with a single AWS::SSM::Parameter resource (logical id ProbeParam) and a ClientRequestToken, then DescribeStackEvents (cloudformation-stack-events-and-errors#task#6): only stack-level events are returned — no ProbeParam CREATE_IN_PROGRESS/CREATE_COMPLETE events, no ResourceProperties, no ClientRequestToken.
5. DescribeStackEvents on any created stack (cloudformation-stack-lifecycle-basic-crud#task#4): the stack-level CREATE_COMPLETE event has no ClientRequestToken member.
6. Re-replay the representative plus the three bindings in Issue-Trace-Ids to confirm the fix.
