Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.put-resource-config-rejects-unregistered-type config.resource-config-history-chronological-ordering config.resource-config-unicode-and-binary-blob-round-trip config.stored-query-lifecycle config.remediation-configuration-and-exception-lifecycle config.remediation-configuration-lifecycle config.delete-config-rule-with-attached-remediation-rejected config.put-resource-config-and-list-discovered
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.put-resource-config-rejects-unregistered-type|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: config.resource-config-history-chronological-ordering|run#6|AssertionError|raise AssertionError(f"expected PutResourceConfig with unregistered type {bogus_type!r} to be rejected")
Issue-Key: config.resource-config-unicode-and-binary-blob-round-trip|_error_code_and_message#8|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: config.stored-query-lifecycle|_error#5|AssertionError|raise AssertionError(f"expected rejection but call succeeded with {result!r}")
Issue-Key: config.remediation-configuration-and-exception-lifecycle|_error#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: config-service|PutRemediationConfigurations|InvalidParameterValueException
Issue-Key: config.delete-config-rule-with-attached-remediation-rejected|run#23|KeyError|after_untag = cfg.list_tags_for_resource(ResourceArn=rule_arn)["Tags"]
Issue-Key: config.put-resource-config-and-list-discovered|run#14|AssertionError|assert pre_code_message == (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Config write paths unvalidated or wrongly refused: PutResourceConfig/PutStoredQuery/PutRemediationExceptions accept what AWS rejects, PutRemediationConfigurations rejects what AWS accepts, ListTagsForResource drops Tags (6 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutResourceConfig

## Description
Six independent write/read-path contract defects on operations that do answer. SIX CAUSES must be closed.

Cause 1 of 6: PutResourceConfig accepts an unregistered custom ResourceType.
AWS: PutResourceConfig requires ResourceType (@length 1..196), SchemaVersionId, ResourceId and Configuration, returns an empty body, and models InsufficientPermissionsException, MaxActiveResourcesExceededException, NoRunningConfigurationRecorderException and ValidationException, all client/400 (Smithy: PutResourceConfig). Recorded behavior: a ResourceType that is not a registered CloudFormation resource type (e.g. "Sim2Real::Behavioral::Widget<id>") is refused with a 400 client error and nothing is recorded.
Emulator: answers HTTP 200 with an empty body ({}) and records the item.
Why: a client can inject configuration items for resource types that do not exist, and the registration precondition AWS enforces is unobservable — code that handles the rejection never sees it, and the recorded inventory is polluted with types AWS would refuse.
Fix: ResourceType/SchemaVersionId validation against the registered resource-type set in the PutResourceConfig handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_resource_config.rs).
Traces: config.put-resource-config-rejects-unregistered-type, config.resource-config-history-chronological-ordering, config.resource-config-unicode-and-binary-blob-round-trip.

Cause 2 of 6: PutStoredQuery accepts a StoredQuery whose Expression is not a valid Config query.
AWS: PutStoredQuery takes a required StoredQuery (QueryName required, Expression @length 1..4096) and models ValidationException (client/400), whose doc lists the PutStoredQuery cases explicitly: "There are missing required fields" / "The input value fails the validation" (Smithy: PutStoredQuery, ValidationException). Recorded behavior: a stored query carrying a malformed expression is refused 400 and no query ARN is created.
Emulator: answers 200 and returns a QueryArn (e.g. arn:aws:config:<region>:<account>:stored-query/<name>/query-<id>), so the bad query is persisted.
Why: the query-validation boundary is gone; a client only discovers the expression is invalid much later (or never, since the select path is itself unimplemented), and a resource exists that AWS would not have created.
Fix: StoredQuery.Expression validation in the PutStoredQuery handler (.../src/ops/put_stored_query.rs).
Traces: config.stored-query-lifecycle.

Cause 3 of 6: PutRemediationExceptions succeeds for a rule with no remediation configuration.
AWS: PutRemediationExceptions requires ConfigRuleName and ResourceKeys and models InsufficientPermissionsException and InvalidParameterValueException, both client/400 (Smithy: PutRemediationExceptions). Recorded behavior: with no remediation configuration attached to the rule, the call is refused 400 InvalidParameterValueException with a message naming the rule (recorded example: "No RemediationConfiguration found for <rule>").
Emulator: answers HTTP 200 with {"FailedBatches": []}, i.e. it reports the exception as accepted.
Why: remediation exceptions are silently created against a rule that cannot remediate anything; a client's error handling for the "no remediation configuration" precondition is dead code, and a later read shows exceptions AWS would never have stored.
Fix: the remediation-configuration precondition check in the PutRemediationExceptions handler (.../src/ops/put_remediation_exceptions.rs).
Traces: config.remediation-configuration-and-exception-lifecycle.

Cause 4 of 6: PutRemediationConfigurations refuses a valid SSM_DOCUMENT remediation because it demands an assume-role parameter the API does not require.
AWS: RemediationConfiguration requires only ConfigRuleName, TargetType and TargetId; Parameters is an optional map (0..25) and nothing in the model requires an AutomationAssumeRole/AssumeRole entry (Smithy: RemediationConfiguration). Recorded behavior: PutRemediationConfigurations with TargetType=SSM_DOCUMENT, TargetId=AWS-PublishSNSNotification, TargetVersion=1 and Parameters {Message, TopicArn} is accepted and returns FailedBatches=[].
Emulator: rejects the call with 400 InvalidParameterValueException ("RemediationConfiguration for Config rule '<rule>' must provide the role the remediation runs as: Parameters carries no value for AutomationAssumeRole or AssumeRole"), so the whole remediation lifecycle is unreachable.
Why: this is an over-rejection — valid client input is refused, so remediation cannot be configured at all for documents that need no assume-role parameter, and the emulator invents a required parameter that no SDK or template supplies.
Fix: remove the invented precondition from the PutRemediationConfigurations handler's validation; validate only the modeled requirements (.../src/ops/put_remediation_configurations.rs).
Traces: config.remediation-configuration-lifecycle.

Cause 5 of 6: ListTagsForResource omits the Tags member when a resource has no tags.
AWS: ListTagsForResource returns Tags (TagList) and NextToken and models InvalidLimitException, InvalidNextTokenException, ResourceNotFoundException and ValidationException (Smithy: ListTagsForResource). Recorded behavior: for an existing resource with no tags the response still carries the Tags member (an empty list), so clients can read it unconditionally.
Emulator: the response has no Tags key at all, which makes a plain response["Tags"] read fail (KeyError: 'Tags').
Why: every SDK/CLI consumer that reads Tags after untagging — the normal way to confirm an untag took effect — breaks with a missing-key error instead of seeing an empty list.
Fix: always serialize Tags in the ListTagsForResource response (.../src/ops/list_tags_for_resource.rs).
Traces: config.delete-config-rule-with-attached-remediation-rejected.

Cause 6 of 6: the PutResourceConfig no-recorder refusal message.
AWS: PutResourceConfig models NoRunningConfigurationRecorderException (client/400); recorded behavior is the fixed text "You must create and start a configuration recorder before you call the PutResourceConfig API."
Emulator: same precondition, different text: "there is no configuration recorder running; use StartConfigurationRecorder to turn one on before recording a resource configuration".
Why: the sentence AWS returns names the API and the required action in a fixed form clients and runbooks match on; the emulator's paraphrase does not match.
Fix: the NoRunningConfigurationRecorderException message in the PutResourceConfig handler (.../src/ops/put_resource_config.rs).
Traces: config.put-resource-config-and-list-discovered.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed).

Cause 1:
  config.put-resource-config-rejects-unregistered-type: AssertionError: expected the call to be rejected, but it succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}
  config.resource-config-history-chronological-ordering: AssertionError: expected PutResourceConfig with unregistered type 'Sim2Real::Behavioral::Widget<id>' to be rejected
  config.resource-config-unicode-and-binary-blob-round-trip: AssertionError: expected the call to be rejected, but it succeeded and returned {... 'HTTPStatusCode': 200 ...}

Cause 2:
  config.stored-query-lifecycle: AssertionError: expected rejection but call succeeded with {'QueryArn': 'arn:aws:config:us-west-2:000000000000:stored-query/sim2real-behav-cfg-badquery-<id>/query-<id>', 'HTTPStatusCode': 200}

Cause 3:
  config.remediation-configuration-and-exception-lifecycle: AssertionError: expected the call to be rejected, but it succeeded and returned {'FailedBatches': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}  (call: put_remediation_exceptions(ConfigRuleName=<rule with no remediation configuration>, ResourceKeys=[{AWS::S3::Bucket, <id>}]); expected 400 InvalidParameterValueException "No RemediationConfiguration found for <rule>")

Cause 4:
  config.remediation-configuration-lifecycle: botocore.errorfactory.InvalidParameterValueException: An error occurred (InvalidParameterValueException) when calling the PutRemediationConfigurations operation: RemediationConfiguration for Config rule 'sim2real-behav-cfg-rule-<id>' must provide the role the remediation runs as: Parameters carries no value for AutomationAssumeRole or AssumeRole
  The request the check sent: RemediationConfigurations=[{ConfigRuleName, TargetType: "SSM_DOCUMENT", TargetId: "AWS-PublishSNSNotification", TargetVersion: "1", Parameters: {Message: {StaticValue: {...}}, TopicArn: {StaticValue: {...}}}}] — accepted by AWS with FailedBatches=[].

Cause 5:
  config.delete-config-rule-with-attached-remediation-rejected: KeyError: 'Tags' at `cfg.list_tags_for_resource(ResourceArn=rule_arn)["Tags"]`

Cause 6:
  config.put-resource-config-and-list-discovered: AssertionError: there is no configuration recorder running; use StartConfigurationRecorder to turn one on before recording a resource configuration — asserted against the expected "You must create and start a configuration recorder before you call the PutResourceConfig API."

Model grounding: com-amazonaws-configservice-starlingdoveservice.json — PutResourceConfig errors (InsufficientPermissionsException, MaxActiveResourcesExceededException, NoRunningConfigurationRecorderException, ValidationException) and PutRemediationExceptions errors (InsufficientPermissionsException, InvalidParameterValueException) are client/400; RemediationConfiguration requires only ConfigRuleName/TargetType/TargetId with Parameters optional (0..25); ListTagsForResource output carries Tags.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline; replay simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
2. Cause 1: start a recorder, then config put-resource-config --resource-type 'Sim2Real::Behavioral::Widget1' --schema-version-id <id> --resource-id x --configuration '{}' — 200 instead of a 400 client error.
3. Cause 2: config put-stored-query with StoredQuery.Expression set to a malformed query — 200 with a QueryArn instead of 400 ValidationException.
4. Cause 3: config put-remediation-exceptions --config-rule-name <rule with no remediation configuration> --resource-keys ... — 200 {"FailedBatches":[]} instead of 400 InvalidParameterValueException.
5. Cause 4: config put-remediation-configurations with TargetType SSM_DOCUMENT / TargetId AWS-PublishSNSNotification and Parameters carrying only Message and TopicArn — rejected InvalidParameterValueException instead of accepted.
6. Cause 5: tag then untag a config rule and call config list-tags-for-resource — the response omits Tags entirely.
7. Cause 6: with no recorder, call config put-resource-config and compare the NoRunningConfigurationRecorderException message.
