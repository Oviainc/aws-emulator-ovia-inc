Issue-Source: trace-sweep-divergence
Issue-Trace: config-configuration-recorder-and-delivery-channel-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-configuration-recorder-and-delivery-channel-lifecycle config-recorder-resource-type-association config-resource-config-and-evaluation-validation config-resource-config-discovery-and-query config-resource-tagging-and-compliance-summaries config-tag-resource-nonexistent-arn-rejected config-retention-configuration-boundary-values config-retention-configuration-workflow config-start-evaluation-invalid-configuration config-third-party-slr-access-denied
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|DescribeDeliveryChannelStatus|4xx->2xx|$.__type
Issue-Key: config|DescribeDeliveryChannelStatus|4xx->2xx|$.message
Issue-Key: config|DescribeDeliveryChannelStatus|4xx->2xx|status
Issue-Key: config|AssociateResourceTypes|4xx->4xx|$.message
Issue-Key: config|DeleteServiceLinkedConfigurationRecorder|4xx->4xx|$.__type
Issue-Key: config|DeleteServiceLinkedConfigurationRecorder|4xx->4xx|$.message
Issue-Key: config|StartConfigRulesEvaluation|4xx->4xx|$.message
Issue-Key: config|GetResourceConfigHistory|4xx->4xx|$.__type
Issue-Key: config|GetResourceConfigHistory|4xx->4xx|$.message
Issue-Key: config|ListTagsForResource|4xx->4xx|$.message
Issue-Key: config|DeleteRetentionConfiguration|4xx->4xx|$.message
Issue-Key: config|StartResourceEvaluation|4xx->4xx|$.message
Issue-Key: config|TagResource|4xx->4xx|$.message
Issue-Key: config|PutThirdPartyServiceLinkedConfigurationRecorder|4xx->4xx|$.message
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# Config recorder, tagging and retention refusals answer wrong class, wording or 200

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryChannelStatus

## Description
Ten distinct causes on this service's refusal paths: one operation does not refuse at all, two raise the wrong error class, and seven serve emulator-authored prose instead of AWS's fixed message text. TEN CAUSES must be closed.

Cause 1 of 10 - DescribeDeliveryChannelStatus does not raise NoSuchDeliveryChannelException for a named, absent channel (fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#7).
AWS: DescribeDeliveryChannelStatus models exactly one error, NoSuchDeliveryChannelException (client, httpError 400, defaulted) (Smithy: DescribeDeliveryChannelStatus.errors). The recording shows 400 with that code when DeliveryChannelNames names a channel that does not exist, message fixed text "Cannot find delivery channel with specified name '" + the requested name + "'."
Emulator: answers 200 and omits __type and message entirely - a named-but-absent channel is treated as an empty result set.
Why: the standard AWS probe (name a channel, catch NoSuchDeliveryChannelException) silently succeeds, so a client cannot tell "no channel" from "channel with nothing to report".
Fix: DescribeDeliveryChannelStatus' handler/state lookup - raise the modeled 400 for any explicitly requested name not in state. NOTE: the sibling operations DescribeDeliveryChannels / DeleteDeliveryChannel / PutDeliveryChannel are already owned by open issue 20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and.md; DescribeDeliveryChannelStatus is NOT covered there and must be fixed here.

Cause 2 of 10 - AssociateResourceTypes rejects an unsupported resource type with the generic enum-constraint message (fixture config-recorder-resource-type-association, step task#5).
AWS: AssociateResourceTypes models ValidationException (client, 400) (Smithy: AssociateResourceTypes.errors). The recording shows 400 with the fixed, request-independent sentence "The resource type you specified is not supported by AWS Config. Verify the resource type, and try again." - the offending value is not echoed and the enum is not enumerated.
Emulator: same status/class but emits the generic validation rendering "1 validation error detected: Value '<type>' at 'resourceTypes.1.member' failed to satisfy constraint: Member must satisfy enum value set: [AWS::EC2::CustomerGateway, ...]", spelling out the whole ResourceType enum.
Why: clients matching AWS's short sentence do not match, and the response body balloons with a model enum AWS never returns.
Fix: AssociateResourceTypes' input validation - special-case the ResourceTypes member to AWS's fixed sentence instead of the shared enum-constraint formatter.

Cause 3 of 10 - DeleteServiceLinkedConfigurationRecorder raises the wrong error class for an unknown service principal (fixture config-recorder-resource-type-association, step task#7).
AWS: the operation models ValidationException, NoSuchConfigurationRecorderException and ConflictException (Smithy: DeleteServiceLinkedConfigurationRecorder.errors), and ValidationException's own documentation covers "You have provided a service principal for service-linked configuration recorder that is not valid." The recording shows 400 __type ValidationException with the fixed sentence "The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again."
Emulator: 400 __type NoSuchConfigurationRecorderException with its own sentence "Configuration recorder (AccountId: <acct>, Service Principal: <principal>) cannot be found".
Why: the error code is part of the contract - retry/branch logic keyed on ValidationException versus NoSuchConfigurationRecorderException takes the wrong branch.
Fix: DeleteServiceLinkedConfigurationRecorder's handler - classify an unresolvable ServicePrincipal as ValidationException with AWS's fixed sentence.

Cause 4 of 10 - StartConfigRulesEvaluation's not-found message is the plural/generic form (fixture config-resource-config-and-evaluation-validation, step task#6).
AWS: StartConfigRulesEvaluation models NoSuchConfigRuleException (client, 400) (Smithy: StartConfigRulesEvaluation.errors). The recording shows the message names the single offending rule: fixed text "The ConfigRule '" + rule name from the request + "' provided in the request is invalid. Please check the configRule name."
Emulator: "One or more ConfigRules provided in the request are invalid. Please check the configRule names." - the offending name is dropped.
Why: a caller submitting several rule names cannot tell which one was rejected, which is exactly what AWS's message carries.
Fix: StartConfigRulesEvaluation's handler error construction - name the offending rule in the singular sentence.

Cause 5 of 10 - GetResourceConfigHistory raises the wrong error class for a never-recorded resource (fixture config-resource-config-discovery-and-query, step task#5).
AWS: the operation models both ResourceNotDiscoveredException and NoAvailableConfigurationRecorderException (Smithy: GetResourceConfigHistory.errors). The recording shows that for a resource that was never discovered AWS answers 400 __type ResourceNotDiscoveredException with the fixed text "Resource " + resourceId + " of resourceType:" + resourceType + " is unknown or has not been discovered" (both values from the request).
Emulator: 400 __type NoAvailableConfigurationRecorderException, message "there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one" - it short-circuits on recorder availability and never reaches the discovery check.
Why: callers that catch ResourceNotDiscoveredException to mean "this resource has no history" see an unrelated code, and the recorder-absence code is raised in a state AWS does not raise it.
Fix: GetResourceConfigHistory's handler - perform the per-resource discovery check and emit ResourceNotDiscoveredException with AWS's sentence.

Cause 6 of 10 - ListTagsForResource's not-found message is reworded (fixture config-resource-tagging-and-compliance-summaries, step teardown#10).
AWS: ListTagsForResource models ResourceNotFoundException (client, 400) (Smithy: ListTagsForResource.errors). The recording shows the fixed text "Resource " + the ARN from the request + " does not exist for account " + the caller's account id - the ARN is unquoted and the account id is named.
Emulator: "The resource '<arn>' provided in the request does not exist." - quotes the ARN and omits the account id.
Why: clients matching AWS's sentence (or reading the account id out of it) do not match.
Fix: the shared config tagging not-found error construction used by ListTagsForResource.

Cause 7 of 10 - DeleteRetentionConfiguration's not-found message is reworded (fixture config-retention-configuration-boundary-values, step task#8).
AWS: DeleteRetentionConfiguration models NoSuchRetentionConfigurationException (client, 400) (Smithy: DeleteRetentionConfiguration.errors). The recording shows the fixed text "Cannot find retention configuration with specified name '" + the requested name + "'."
Emulator: "Cannot find retention configuration with the specified name '<name>'. Please check the retentionConfigurationName." - inserts "the" and appends a hint sentence AWS does not send.
Why: assertion on AWS's exact sentence fails; the wording also differs from the delivery-channel family's identical "with specified name" phrasing.
Fix: DeleteRetentionConfiguration's handler error construction.

Cause 8 of 10 - StartResourceEvaluation's invalid-JSON refusal is emulator prose (fixture config-start-evaluation-invalid-configuration, step task#1).
AWS: StartResourceEvaluation models InvalidParameterValueException (client, 400) (Smithy: StartResourceEvaluation.errors). The recording shows the fixed, request-independent sentence "The provided resource configuration is not a valid JSON. Provide a valid JSON and try again."
Emulator: "The ResourceDetails.ResourceConfiguration provided in the request is invalid. It must be the resource definition as a JSON object, as per the CFN_RESOURCE_SCHEMA resource configuration schema type."
Why: clients distinguishing "malformed JSON" from other parameter failures by message do not match AWS.
Fix: StartResourceEvaluation's ResourceConfiguration parsing/validation - emit AWS's fixed sentence for a JSON parse failure.

Cause 9 of 10 - TagResource's not-found message is reworded (fixture config-tag-resource-nonexistent-arn-rejected, step task#2).
AWS: 400 with the same fixed text as cause 6 - "Resource " + the ARN from the request + " does not exist for account " + the account id (recorded behavior; the model states only the error class).
Emulator: "The resource '<arn>' provided in the request does not exist."
Why/Fix: as cause 6, from TagResource's handler; both entry points must use the same AWS sentence.

Cause 10 of 10 - PutThirdPartyServiceLinkedConfigurationRecorder refuses with the wrong sentence (fixture config-third-party-slr-access-denied, step task#1).
AWS: the recording shows 400 with the fixed text "The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again." - the refusal is about the service principal and names no connector.
Emulator: "The connector '<connectorArn>' provided in the request does not exist." - it refuses on connector existence and echoes the ARN.
Why: the request is refused for a different reason than AWS gives, so a client cannot learn what to correct; message-keyed handling diverges.
Fix: PutThirdPartyServiceLinkedConfigurationRecorder's handler validation - validate (and word the refusal about) the service principal, as AWS does.

No existing open issue covers causes 2-10; cause 1's sibling operations are covered by 20261022T170950Z-config-error-paths-unenforced-delivery-channel-validation-and.md but DescribeDeliveryChannelStatus itself is not.

Traces: config-configuration-recorder-and-delivery-channel-lifecycle#task#7, config-recorder-resource-type-association#task#5 and #task#7, config-resource-config-and-evaluation-validation#task#6, config-resource-config-discovery-and-query#task#5, config-resource-tagging-and-compliance-summaries#teardown#10, config-retention-configuration-boundary-values#task#8, config-start-evaluation-invalid-configuration#task#1, config-tag-resource-nonexistent-arn-rejected#task#2, config-third-party-slr-access-denied#task#1.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false).

Cause 1 - DescribeDeliveryChannelStatus, config-configuration-recorder-and-delivery-channel-lifecycle#task#7:
  status: expected 400, got 200
  $.__type: missing (expected 'NoSuchDeliveryChannelException'); $.message: missing (expected "Cannot find delivery channel with specified name '<channelName>'.")

Cause 2 - AssociateResourceTypes, config-recorder-resource-type-association#task#5 (400 both sides):
  $.message expected 'The resource type you specified is not supported by AWS Config. Verify the resource type, and try again.'
  got "1 validation error detected: Value 'AWS::NotAReal::ResourceType' at 'resourceTypes.1.member' failed to satisfy constraint: Member must satisfy enum value set: [AWS::EC2::CustomerGateway, AWS::EC2::EIP, ...]" (full enum inlined)

Cause 3 - DeleteServiceLinkedConfigurationRecorder, config-recorder-resource-type-association#task#7 (400 both sides):
  $.__type expected 'ValidationException', got 'NoSuchConfigurationRecorderException'
  $.message expected 'The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again.', got 'Configuration recorder (AccountId: 000000000000, Service Principal: securityhub.amazonaws.com) cannot be found'

Cause 4 - StartConfigRulesEvaluation, config-resource-config-and-evaluation-validation#task#6 (400 both sides):
  $.message expected "The ConfigRule '<ruleName>' provided in the request is invalid. Please check the configRule name.", got 'One or more ConfigRules provided in the request are invalid. Please check the configRule names.'

Cause 5 - GetResourceConfigHistory, config-resource-config-discovery-and-query#task#5 (400 both sides):
  $.__type expected 'ResourceNotDiscoveredException', got 'NoAvailableConfigurationRecorderException'
  $.message expected 'Resource <resourceId> of resourceType:AWS::S3::Bucket is unknown or has not been discovered', got 'there are no customer managed configuration recorders available; use PutConfigurationRecorder to create one'

Cause 6 - ListTagsForResource, config-resource-tagging-and-compliance-summaries#teardown#10 (400 both sides, 2 steps in cluster):
  $.message expected 'Resource <arn> does not exist for account 000000000000', got "The resource '<arn>' provided in the request does not exist."

Cause 7 - DeleteRetentionConfiguration, config-retention-configuration-boundary-values#task#8 (400 both sides, 2 steps in cluster):
  $.message expected "Cannot find retention configuration with specified name 'default'.", got "Cannot find retention configuration with the specified name 'default'. Please check the retentionConfigurationName."

Cause 8 - StartResourceEvaluation, config-start-evaluation-invalid-configuration#task#1 (400 both sides):
  $.message expected 'The provided resource configuration is not a valid JSON. Provide a valid JSON and try again.', got 'The ResourceDetails.ResourceConfiguration provided in the request is invalid. It must be the resource definition as a JSON object, as per the CFN_RESOURCE_SCHEMA resource configuration schema type.'

Cause 9 - TagResource, config-tag-resource-nonexistent-arn-rejected#task#2 (400 both sides):
  $.message expected 'Resource <arn> does not exist for account 000000000000', got "The resource '<arn>' provided in the request does not exist."

Cause 10 - PutThirdPartyServiceLinkedConfigurationRecorder, config-third-party-slr-access-denied#task#1 (400 both sides):
  $.message expected 'The service principal of the configuration recorder you entered cannot be found. Verify the configuration recorder service principal, and try again.', got "The connector '<connectorArn>' provided in the request does not exist."

(ARNs, channel/rule/resource names and the account id are per-run fixture values; the requirements are the statuses, codes and message templates.)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): DescribeDeliveryChannelStatus models only NoSuchDeliveryChannelException (400); AssociateResourceTypes and DeleteServiceLinkedConfigurationRecorder both model ValidationException (400); StartConfigRulesEvaluation models NoSuchConfigRuleException; GetResourceConfigHistory models ResourceNotDiscoveredException and NoAvailableConfigurationRecorderException; ListTagsForResource models ResourceNotFoundException; DeleteRetentionConfiguration models NoSuchRetentionConfigurationException; StartResourceEvaluation models InvalidParameterValueException.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: config-configuration-recorder-and-delivery-channel-lifecycle#task#7 - DescribeDeliveryChannelStatus naming an absent channel; 200 instead of 400 NoSuchDeliveryChannelException.
4. Cause 2: config-recorder-resource-type-association#task#5 - AssociateResourceTypes with an unsupported resource type.
5. Cause 3: config-recorder-resource-type-association#task#7 - DeleteServiceLinkedConfigurationRecorder with an unknown ServicePrincipal.
6. Cause 4: config-resource-config-and-evaluation-validation#task#6 - StartConfigRulesEvaluation naming an unknown rule.
7. Cause 5: config-resource-config-discovery-and-query#task#5 - GetResourceConfigHistory for a never-recorded resource.
8. Cause 6: config-resource-tagging-and-compliance-summaries#teardown#10 - ListTagsForResource on an absent stored-query ARN.
9. Cause 7: config-retention-configuration-boundary-values#task#8 - DeleteRetentionConfiguration for an absent configuration.
10. Cause 8: config-start-evaluation-invalid-configuration#task#1 - StartResourceEvaluation with a non-JSON ResourceConfiguration.
11. Cause 9: config-tag-resource-nonexistent-arn-rejected#task#2 - TagResource on an absent ARN.
12. Cause 10: config-third-party-slr-access-denied#task#1 - PutThirdPartyServiceLinkedConfigurationRecorder.
13. Re-replay all ten fixture#step bindings to confirm the fix.
