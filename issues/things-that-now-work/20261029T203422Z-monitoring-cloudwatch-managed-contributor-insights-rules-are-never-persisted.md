Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-managed-insight-rules-for-resource
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-managed-insight-rules-for-resource cloudwatch-managed-insight-rules-lifecycle cloudwatch-managed-insight-rules-workflow cloudwatch-managed-insight-rules-validation cloudwatch-put-managed-insight-rules-invalid-template-name cloudwatch-list-managed-insight-rules-unsupported-resource cloudwatch-list-managed-insight-rules-missing-vs-malformed-arn cloudwatch-managed-insight-rules-error-paths
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|ListManagedInsightRules|2xx->2xx|$.ManagedRules[*]
Issue-Key: monitoring|PutManagedInsightRules|2xx->2xx|$.Failures[*]
Issue-Key: monitoring|PutManagedInsightRules|2xx->4xx|$.Failures
Issue-Key: monitoring|PutManagedInsightRules|2xx->4xx|status
Issue-Key: monitoring|PutManagedInsightRules|2xx->2xx|$.Failures[*].ExceptionType
Issue-Key: monitoring|PutManagedInsightRules|2xx->2xx|$.Failures[*].FailureDescription
Issue-Key: monitoring|ListManagedInsightRules|4xx->4xx|$.__type
Issue-Key: monitoring|ListManagedInsightRules|4xx->4xx|$.message
Issue-Key: monitoring|ListManagedInsightRules|4xx->4xx|$headers.x-amzn-query-error
Issue-Commit: 6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492

# CloudWatch managed Contributor Insights rules are never persisted or validated

> _Note: this issue was identified against the code base at commit `6bd726e14a5c3a202f9e89e1e854b8dfcc8d4492`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListManagedInsightRules

## Description
8 causes, one shared fault: the managed Contributor Insights subsystem has no template catalogue and no per-resource rule state, so PutManagedInsightRules never enables anything, ListManagedInsightRules is always empty, and the batch error surface is invented.

Cause 1 of 8: ListManagedInsightRules for a resource whose managed rules were just enabled. AWS: 200 with one ManagedRuleDescription per template the service supports for that resource type, each carrying TemplateName, the ResourceARN from the request and a RuleState — the enabled template's RuleState carries RuleName '<TemplateName-without-version>-<resource name>-<version>' and State 'ENABLED', the others an empty RuleState. Smithy: ListManagedInsightRules output ManagedRules targets ManagedRuleDescriptions; ManagedRuleDescription members TemplateName/ResourceARN/RuleState. Emulator: 200 with an EMPTY ManagedRules list. Why: a client enumerating which managed rules exist for a resource sees none, so enable/disable workflows and inventory reconciliation cannot work at all. Fix: managed-rule state in the CloudWatch crate plus the ListManagedInsightRules handler (it must enumerate the supported templates for the resource type, not only stored rows). Trace: cloudwatch-managed-insight-rules-for-resource#task#2.

Cause 2 of 8: ListManagedInsightRules after the managed rule is deleted. AWS: 200 and the template is STILL listed, with an empty RuleState (the catalogue entry survives; only the rule state clears). Emulator: empty ManagedRules. Same component as Cause 1 but the post-delete projection, so it does not close with Cause 1 alone. Trace: cloudwatch-managed-insight-rules-for-resource#teardown#4.

Cause 3 of 8: PutManagedInsightRules for a supported template on a supported resource. AWS: 200 with NO Failures entry. Emulator: 200 but a synthesized failure (FailureCode 'TEMPLATE_NAME', ExceptionType 'ResourceNotFoundException', FailureDescription 'Could not find template with the specified templateName.') because no template catalogue exists. Why: the enable looks like it failed, so callers abort before the rule is ever used. Fix: the template catalogue plus PutManagedInsightRules' per-item resolution. Trace: cloudwatch-managed-insight-rules-for-resource#task#1.

Cause 4 of 8: PutManagedInsightRules with a TemplateName that is not a real template. AWS: 200 (the operation itself succeeds) and reports the item in Failures with ExceptionType 'InvalidParameterException', FailureCode 'TEMPLATE_NAME', FailureDescription 'Invalid ARN.' and FailureResource echoing the submitted template name. Smithy: PutManagedInsightRules output Failures targets BatchFailures; it is a partial-failure batch operation. Emulator: refuses the WHOLE call with 400 and no Failures member. Why: a batch caller loses the per-item result and a single bad template name fails the entire batch, so valid items in the same request are not enabled. Fix: PutManagedInsightRules must validate per item and report through Failures instead of raising. Trace: cloudwatch-managed-insight-rules-validation#task#1.

Cause 5 of 8: PutManagedInsightRules with a malformed ResourceARN. AWS: 200 with a Failures entry whose ExceptionType is 'InvalidParameterException' and FailureDescription 'Invalid ResourceArn.' — fixed text naming which member was bad. Emulator: Failures entry with ExceptionType 'ResourceNotFoundException' and FailureDescription 'Could not find template with the specified templateName.' — it blames the template for a bad ARN. Why: the caller is pointed at the wrong request member and cannot repair the request. Fix: PutManagedInsightRules' per-item validation ordering (ARN shape before template lookup) and the failure classification. Trace: cloudwatch-managed-insight-rules-validation#task#2.

Cause 6 of 8: PutManagedInsightRules against a resource type the service does not support managed rules for. AWS: 200 with a Failures entry whose ExceptionType is 'InternalFailure'. Emulator: ExceptionType 'ResourceNotFoundException'. Why: the entry's fault class flips from receiver-side to sender-side, so client retry policy treats an unsupported resource as a permanent not-found. Fix: the same per-item failure classification. Trace: cloudwatch-list-managed-insight-rules-unsupported-resource#task#1.

Cause 7 of 8: ListManagedInsightRules with ResourceARN omitted. AWS: 400 with __type 'com.amazon.coral.service#MissingRequiredParameterException', header 'x-amzn-query-error: MissingParameter;Sender' and the message 'MISSING_RESOURCE_ARN: The ResourceARN parameter must be present.' — fixed text for the class. Smithy: ListManagedInsightRules.ResourceARN is required and the operation models MissingRequiredParameterException (awsQueryError code MissingParameter). Emulator: 400 with __type 'ValidationException', no query-error header, and the generic "1 validation error detected: Value null at 'resourceARN' failed to satisfy constraint: Member must not be null". Why: a missing required parameter must surface as MissingParameter, not as a generic shape validation error; code-based branching mis-routes. Fix: required-member handling for ListManagedInsightRules. Trace: cloudwatch-list-managed-insight-rules-missing-vs-malformed-arn#task#1.

Cause 8 of 8: ListManagedInsightRules with a foreign NextToken. AWS: 400 with 'INVALID_TOKEN: Unsupported token version.' Emulator: 'NextToken is not a token this service issued'. Fix: the pagination-token validation on this operation. Trace: cloudwatch-managed-insight-rules-error-paths#task#2.

## Evidence
Replay (simparity-wire-aws-v1), 8 clusters.
1. ListManagedInsightRules, cloudwatch-managed-insight-rules-for-resource#task#2: $.ManagedRules[0..3] missing — expected 4 ManagedRuleDescription entries (VpcEndpointService-NewConnectionsByEndpointId-v1 with RuleState {RuleName '…-v1', State 'ENABLED'}, plus BytesByEndpointId/RstPacketsByEndpointId/ActiveConnectionsByEndpointId with RuleState {}), got None. 5 affected steps.
2. ListManagedInsightRules, cloudwatch-managed-insight-rules-for-resource#teardown#4: $.ManagedRules[0] missing (expected the template with RuleState {}).
3. PutManagedInsightRules, cloudwatch-managed-insight-rules-for-resource#task#1: $.Failures[0] extra (expected None, got {'FailureResource': 'VpcEndpointService-NewConnectionsByEndpointId-v1', 'ExceptionType': 'ResourceNotFoundException', 'FailureCode': 'TEMPLATE_NAME', 'FailureDescription': 'Could not find template with the specified templateName.'}). 3 affected steps.
4. PutManagedInsightRules, cloudwatch-managed-insight-rules-validation#task#1: status expected 200, got 400; $.Failures missing (expected [{'ExceptionType': 'InvalidParameterException', 'FailureCode': 'TEMPLATE_NAME', 'FailureDescription': 'Invalid ARN.', 'FailureResource': 'NotARealTemplateName'}]).
5. PutManagedInsightRules, cloudwatch-managed-insight-rules-validation#task#2: $.Failures[0].ExceptionType expected 'InvalidParameterException', got 'ResourceNotFoundException'; FailureDescription expected 'Invalid ResourceArn.', got 'Could not find template with the specified templateName.'
6. PutManagedInsightRules, cloudwatch-list-managed-insight-rules-unsupported-resource#task#1: $.Failures[0].ExceptionType expected 'InternalFailure', got 'ResourceNotFoundException'.
7. ListManagedInsightRules, cloudwatch-list-managed-insight-rules-missing-vs-malformed-arn#task#1: $.__type expected 'com.amazon.coral.service#MissingRequiredParameterException', got 'ValidationException'; $.message expected 'MISSING_RESOURCE_ARN: The ResourceARN parameter must be present.'; header expected 'MissingParameter;Sender', got None.
8. ListManagedInsightRules, cloudwatch-managed-insight-rules-error-paths#task#2: $.message expected 'INVALID_TOKEN: Unsupported token version.', got 'NextToken is not a token this service issued'.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the simparity-wire-aws-v1 corpus.
2. PutManagedInsightRules for a VPC endpoint service ARN with TemplateName VpcEndpointService-NewConnectionsByEndpointId-v1 (cloudwatch-managed-insight-rules-for-resource#task#1), then ListManagedInsightRules for the same ResourceARN (task#2) and after deletion (teardown#4).
3. PutManagedInsightRules with TemplateName 'NotARealTemplateName' (cloudwatch-managed-insight-rules-validation#task#1) and with a malformed ResourceARN (task#2); inspect the status and Failures entries.
4. PutManagedInsightRules against an unsupported resource type (cloudwatch-list-managed-insight-rules-unsupported-resource#task#1) and read Failures[0].ExceptionType.
5. Call ListManagedInsightRules with ResourceARN omitted (cloudwatch-list-managed-insight-rules-missing-vs-malformed-arn#task#1) and with a foreign NextToken (cloudwatch-managed-insight-rules-error-paths#task#2).
