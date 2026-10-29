Issue-Source: trace-sweep-divergence
Issue-Trace: redshift-resource-policy-and-reserved-node-exchange-preconditions
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshift-resource-policy-and-reserved-node-exchange-preconditions redshift-resource-policy-put-get-delete-lifecycle redshift-qev2-idc-application-lifecycle redshift-redshift-idc-application-lifecycle redshift-event-subscription-validation-and-event-catalog
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: redshift|GetResourcePolicy|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: redshift|GetResourcePolicy|4xx->4xx|status
Issue-Key: redshift|PutResourcePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|PutResourcePolicy|4xx->2xx|status
Issue-Key: redshift|DeleteResourcePolicy|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|DeleteResourcePolicy|4xx->2xx|status
Issue-Key: redshift|PutResourcePolicy|2xx->2xx|$.PutResourcePolicyResponse.PutResourcePolicyResult.ResourcePolicy.Policy
Issue-Key: redshift|GetResourcePolicy|2xx->2xx|$.GetResourcePolicyResponse.GetResourcePolicyResult.ResourcePolicy.Policy
Issue-Key: redshift|CreateQev2IdcApplication|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|CreateQev2IdcApplication|4xx->2xx|status
Issue-Key: redshift|CreateRedshiftIdcApplication|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|CreateRedshiftIdcApplication|4xx->2xx|status
Issue-Key: redshift|CreateEventSubscription|4xx->2xx|$.ErrorResponse
Issue-Key: redshift|CreateEventSubscription|4xx->2xx|status
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# Redshift resource-policy, IDC-application and event-subscription operations accept requests AWS refuses

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourcePolicy

## Description
8 root causes: the resource-policy ARN gate is missing, the policy document is reserialized instead of echoed, and three create operations that depend on another service never run their dependency checks.

Cause 1 of 8: GetResourcePolicy for a ResourceArn whose resource type does not support resource policies. AWS validates the ARN's resource type FIRST and refuses 400 InvalidParameterValue with the message "Tagging is not supported for this type of resource: '<resource-type>' (the ARN is potentially malformed, please check the ARN documentation for more information)" — fixed text with the ARN's resource-type token single-quoted as the only variable part. The emulator skips that gate, goes straight to the policy store and refuses 404 ResourceNotFoundFault "The requested resource policy doesn't exist." Smithy: GetResourcePolicy declares both InvalidPolicyFault (400) and ResourceNotFoundFault (404), so both classes exist — the emulator picks the wrong one. Why: a client probing which resources support policies is told the policy is merely absent, so it retries a PutResourcePolicy that can never succeed; status class and error code are both wrong. Fix: GetResourcePolicy input validation must check the ResourceArn's resource type before the policy lookup. Traces: redshift-resource-policy-and-reserved-node-exchange-preconditions#task#1.

Cause 2 of 8: PutResourcePolicy for an unsupported resource type is ACCEPTED. AWS: 400 InvalidParameterValue, "Tagging is not supported for this type of resource: '<resource-type>' (the ARN is potentially malformed, please check the ARN documentation for more information)". Emulator: 200, and the policy is stored. Why: the emulator accepts and persists a policy on a resource that cannot carry one, so subsequent reads report a policy AWS would never have. Fix: PutResourcePolicy validation must run the same ARN resource-type gate. Traces: redshift-resource-policy-and-reserved-node-exchange-preconditions#task#2.

Cause 3 of 8: DeleteResourcePolicy for an unsupported resource type is ACCEPTED. AWS: 400 InvalidParameterValue with the same fixed message. Emulator: 200. Fix: DeleteResourcePolicy validation must run the ARN resource-type gate. Traces: redshift-resource-policy-and-reserved-node-exchange-preconditions#task#3.

Cause 4 of 8: PutResourcePolicy returns the policy document in a different serialization from AWS's. Smithy: ResourcePolicy.Policy is a String. AWS returns the policy canonically re-rendered in its own pretty-printed form — two-space indentation, a space before each colon ("\"Version\" : \"2012-10-17\""), Statement as a bracketed multi-line array — regardless of how the caller formatted the input. The emulator echoes a minified single-line JSON document with no whitespace. Why: a client that hashes, diffs or textually compares the returned policy against AWS's rendering sees every policy as changed. Fix: the PutResourcePolicy response serializer must render Policy in AWS's canonical form. Traces: redshift-resource-policy-put-get-delete-lifecycle#task#2.

Cause 5 of 8: GetResourcePolicy likewise returns the minified document instead of AWS's canonical pretty-printed rendering, so the defect is observable on the read path too. Fix: as cause 4, in the shared ResourcePolicy serializer/state. Traces: redshift-resource-policy-put-get-delete-lifecycle#task#3.

Cause 6 of 8: CreateQev2IdcApplication succeeds where AWS refuses on the IAM Identity Center dependency. AWS calls sso:CreateApplication on behalf of the caller and, when that call is denied, surfaces 403 DependentServiceAccessDenied whose Message wraps the downstream refusal: "User: <caller principal ARN> is not authorized to perform: sso:CreateApplication on resource: <sso application resource ARN> because no resource-based policy allows the sso:CreateApplication action (Service: SsoAdmin, Status Code: 400, Request ID: <id>) (SDK Attempt Count: 1)" — the fixed text is the authorization sentence and the parenthesised "(Service: SsoAdmin, Status Code: 400, ...)" envelope; the principal ARN, the application resource ARN and the downstream request id come from the request and the downstream call. Emulator: 200 with a created application and no ErrorResponse. Why: the whole dependent-service failure path of the Query-Editor-V2 IDC integration is unreachable, so a client cannot exercise or handle it. Fix: CreateQev2IdcApplication must perform the IAM Identity Center application creation through the emulator's SSO surface and translate its refusal into DependentServiceAccessDenied. Traces: redshift-qev2-idc-application-lifecycle#task#3.

Cause 7 of 8: CreateRedshiftIdcApplication succeeds where AWS refuses with the same 403 DependentServiceAccessDenied on the sso:CreateApplication dependency. Emulator: 200 with a created application. Fix: as cause 6, in CreateRedshiftIdcApplication. Traces: redshift-redshift-idc-application-lifecycle#task#3.

Cause 8 of 8: CreateEventSubscription accepts an SnsTopicArn that names no SNS topic. Smithy: CreateEventSubscription declares SNSTopicArnNotFoundFault with httpError 404 and awsQueryError code SNSTopicArnNotFound. AWS: 404 SNSTopicArnNotFound, Message "Failed to create Subscription. Topic: <SnsTopicArn> not found." — fixed text "Failed to create Subscription. Topic: " + the request's SnsTopicArn + " not found." Emulator: 200 and the subscription is stored. Why: the subscription is created pointing at a topic that does not exist, so events are silently dropped and the caller never learns the ARN was wrong. Fix: CreateEventSubscription validation must resolve SnsTopicArn against SNS before creating the subscription. Open issue 20261025T194435Z-redshift-createeventsubscription-accepts-an-snstopicarn-naming-a-nonexistent.md describes the same acceptance but does not carry this fixture/step as its replay key. Traces: redshift-event-subscription-validation-and-event-catalog#task#1.

## Evidence
Replay of simparity-wire-aws-v1; all eight clusters verdict=mismatch.

GetResourcePolicy (redshift-resource-policy-and-reserved-node-exchange-preconditions#task#1):
  status: expected 400, got 404
  Error.Code: expected 'InvalidParameterValue', got 'ResourceNotFoundFault'
  Error.Message: expected "Tagging is not supported for this type of resource: 'datashare' (the ARN is potentially malformed, please check the ARN documentation for more information)", got 'The requested resource policy doesn\u2019t exist.'
PutResourcePolicy (#task#2) and DeleteResourcePolicy (#task#3):
  status: expected 400, got 200; $.ErrorResponse: missing — expected Error.Type 'Sender', Code 'InvalidParameterValue', the same "Tagging is not supported for this type of resource: 'datashare' ..." message.
PutResourcePolicy (redshift-resource-policy-put-get-delete-lifecycle#task#2) and GetResourcePolicy (#task#3), 200 vs 200:
  ResourcePolicy.Policy: expected '{\n  "Version" : "2012-10-17",\n  "Statement" : [ {\n    "Sid" : ...\n  } ]\n}', got '{"Version":"2012-10-17","Statement":[{"Sid":...}]}'
CreateQev2IdcApplication (redshift-qev2-idc-application-lifecycle#task#3) and CreateRedshiftIdcApplication (redshift-redshift-idc-application-lifecycle#task#3):
  status: expected 403, got 200; $.ErrorResponse: missing — expected Error.Code 'DependentServiceAccessDenied' with the 'is not authorized to perform: sso:CreateApplication ... (Service: SsoAdmin, Status Code: 400, ...)' message.
CreateEventSubscription (redshift-event-subscription-validation-and-event-catalog#task#1):
  status: expected 404, got 200; $.ErrorResponse: missing — expected Error.Code 'SNSTopicArnNotFound', Message 'Failed to create Subscription. Topic: arn:aws:sns:us-west-2:000000000000:sim2real-probe-topic-missing not found.'

The 'datashare' token, the topic ARN, the principal ARN and the downstream request id are request/environment values filling the templates; the status classes, error codes, fixed wording and the canonical policy rendering are the requirements.

## Steps to Reproduce
1. Replay fixture redshift-resource-policy-and-reserved-node-exchange-preconditions; inspect task#1 (GetResourcePolicy), task#2 (PutResourcePolicy) and task#3 (DeleteResourcePolicy), each with a ResourceArn whose resource type does not support resource policies. Observe 404/200 instead of 400 InvalidParameterValue.
2. Replay fixture redshift-resource-policy-put-get-delete-lifecycle; inspect task#2 and task#3 and compare the returned ResourcePolicy.Policy string's formatting with the recording.
3. Replay redshift-qev2-idc-application-lifecycle step task#3 and redshift-redshift-idc-application-lifecycle step task#3; observe 200 instead of 403 DependentServiceAccessDenied.
4. Replay redshift-event-subscription-validation-and-event-catalog step task#1: CreateEventSubscription with an SnsTopicArn naming no topic. Observe 200 instead of 404 SNSTopicArnNotFound.
