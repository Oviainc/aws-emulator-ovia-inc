Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.remediation-configuration-and-exception-lifecycle config.put-resource-config-rejects-unregistered-type config.put-resource-config-and-list-discovered config.delete-config-rule-with-attached-remediation-rejected config.stored-query-lifecycle
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.remediation-configuration-and-exception-lifecycle|_error#19|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: config-service|ListDiscoveredResources|ValidationException
Issue-Key: config.put-resource-config-and-list-discovered|run#24|AssertionError|assert batch["unprocessedResourceKeys"] == [], batch
Issue-Key: config.delete-config-rule-with-attached-remediation-rejected|run#32|AssertionError|assert message == (
Issue-Key: config.stored-query-lifecycle|run#7|AssertionError|assert message == "syntax error at line 1, column 1", message
Issue-Commit: 0ba4a6d0611b8ec8e3b3a33f291da7a865261cbb

# config: 5 net-new behavioral root causes — PutRemediationConfigurations accepts an unknown SSM document, ListDiscoveredResources refuses a custom resource type, BatchGetResourceConfig echoes unknown keys, and two refusal messages are not AWS's wording

> _Note: this issue was identified against the code base at commit `0ba4a6d0611b8ec8e3b3a33f291da7a865261cbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRemediationConfigurations

## Description
Five distinct net-new root causes in the AWS Config emulator, all grounded in failing behavioral checks of the `config` service. Each cause below must be closed before this issue is submitted.

Related but NOT covering: open issue 20261025T223403Z-config-putresourceconfig-accepts-an-unregistered-cloudformation-resource-type.md covers PutResourceConfig's own acceptance of an unregistered type; in this run PutResourceConfig's ValidationException assertion passes and the check now fails one call later on ListDiscoveredResources (Cause 2). None of the five causes below is carried by any open issue's replay key.

=== Cause 1 of 5: PutRemediationConfigurations stores a remediation whose SSM automation document does not exist ===
AWS: Smithy: PutRemediationConfigurations errors = {InsufficientPermissionsException, InvalidParameterValueException} (both 400); output has only FailedBatches. Recorded behavior: with a healthy rule and an assumable AutomationAssumeRole, a `TargetType: SSM_DOCUMENT` whose `TargetId` names no existing SSM automation document is refused with HTTP 400, error code `InvalidParameterValueException`, message `The remediation target for <ConfigRuleName> could not be found. Specify the appropriate remediation target and try again.` — fixed text is everything but `<ConfigRuleName>`, which is the rule name taken from the request's RemediationConfigurations[].ConfigRuleName. The rejected configuration is not persisted (DescribeRemediationConfigurations stays empty).
Emulator: returns HTTP 200 with `{"FailedBatches": []}` — the remediation configuration is accepted and stored with a dangling target. The positive control in the same check (an existing AWS-owned document, AWS-PublishSNSNotification) also succeeds, so the acceptance is indiscriminate: the TargetId is never resolved against SSM.
Why: a client that relies on AWS refusing a typo'd or deleted automation document gets a silent, permanently broken remediation: StartRemediationExecution later fails with no indication at configuration time, and error handling written around InvalidParameterValueException never runs.
Fix: the PutRemediationConfigurations handler and its target validation (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_remediation_configurations.rs) must resolve SSM_DOCUMENT TargetId against SSM's document registry and refuse with the modeled InvalidParameterValueException before touching state.
Traces: config.remediation-configuration-and-exception-lifecycle (check source line 236).

=== Cause 2 of 5: ListDiscoveredResources refuses a CloudFormation-registry (custom) resource type instead of returning an empty list ===
AWS: Smithy models `ListDiscoveredResources.resourceType` as the closed `ResourceType` enum (required) with ValidationException among its errors, but recorded behavior shows AWS accepting a non-enum, CloudFormation-registry-style type name and answering HTTP 200 with `resourceIdentifiers: []` when nothing of that type was ever recorded. Recorded behavior only; Hypothesis: Config relaxed this member to accept registry type names (`<Org>::<Service>::<Resource>`) when custom-resource recording shipped, so the service-side enum check is looser than the model.
Emulator: answers HTTP 400 `ValidationException` with `1 validation error detected: Value '<resourceType>' at 'resourceType' failed to satisfy constraint: Member must satisfy enum value set: [AWS::EC2::CustomerGateway, AWS::EC2::EIP, ...]` for a three-part custom type name, so the call raises instead of returning a list.
Why: the "nothing was recorded" assertion a client makes after a rejected or deleted custom resource cannot be made at all — the probe itself errors. Any tool that enumerates custom (CFN-registered) resource types against Config breaks on the first call.
Fix: the ListDiscoveredResources handler's input validation (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/list_discovered_resources.rs) should accept registry-style type names not in the ResourceType enum and return an empty `resourceIdentifiers` list rather than enforcing the closed enum.
Traces: config.put-resource-config-rejects-unregistered-type.

=== Cause 3 of 5: BatchGetResourceConfig echoes an unknown resource key back as unprocessed ===
AWS: Smithy: BatchGetResourceConfig output = {baseConfigurationItems, unprocessedResourceKeys}; `unprocessedResourceKeys` carries keys the service could not process. Recorded behavior: a resourceKey naming a resource that was never recorded is simply ABSENT from both lists — the response is HTTP 200 with `baseConfigurationItems: []` and `unprocessedResourceKeys: []`.
Emulator: answers HTTP 200 with `baseConfigurationItems: []` and `unprocessedResourceKeys: [{"resourceType": <requested type>, "resourceId": <requested id>}]`, i.e. it re-emits every key it has no configuration item for.
Why: unprocessedResourceKeys is a retry signal — clients loop on it. Against the emulator a key for a nonexistent resource is retried forever, and a client cannot distinguish "not recorded" (absent) from "try again" (unprocessed).
Fix: the BatchGetResourceConfig handler's lookup/response assembly (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/batch_get_resource_config.rs) must omit keys with no configuration item instead of placing them in unprocessedResourceKeys.
Traces: config.put-resource-config-and-list-discovered (check source line 387).

=== Cause 4 of 5: DeleteConfigRule's ResourceInUseException message is not AWS's wording ===
AWS: Smithy: DeleteConfigRule errors = {NoSuchConfigRuleException, ResourceInUseException} (400). Recorded behavior: deleting a rule that still has a remediation configuration is refused with HTTP 400 `ResourceInUseException` and message `The rule <ConfigRuleName> currently has a RemediationConfiguration. Please delete the RemediationConfiguration and try again.` — all fixed text except `<ConfigRuleName>`, which is the rule name from the request.
Emulator: the status and error code are correct, but the message is `A remediation action is associated with the Config rule '<ConfigRuleName>' and Config cannot delete this rule. Delete the remediation action associated with the rule before deleting the rule and try your request again later.` — different fixed text, and the rule name is quoted where AWS does not quote it.
Why: clients and IaC tools match on this message text (and surface it to operators); a reworded refusal breaks message assertions and log-based triage even though the code matches.
Fix: the refusal message in the DeleteConfigRule handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/delete_config_rule.rs) must use AWS's fixed text with the request's rule name interpolated unquoted.
Traces: config.delete-config-rule-with-attached-remediation-rejected (check source line 132).

=== Cause 5 of 5: PutStoredQuery's malformed-expression ValidationException message leaks a SQL-parser diagnostic ===
AWS: Smithy: PutStoredQuery errors include ValidationException (400). Recorded behavior: a StoredQuery whose Expression is not valid Config SQL is refused with HTTP 400 `ValidationException` and message `syntax error at line <line>, column <column>` — the fixed text is `syntax error at line ` / `, column `, and the line and column are the position of the first offending token in the submitted expression.
Emulator: the code is right but the message is the internal parser's text, e.g. `The expression is not a valid Config query: the expression is not a SQL SELECT command: sql parser error: Expected: an SQL statement, found: NOT at Line: 1, Column: 1.` — it prepends two wrapper phrases, names the parser, echoes the offending token and renders the position as `Line: N, Column: N`.
Why: a client parsing the refusal for the error position (editors and query consoles do) cannot read it, and message assertions on the documented AWS wording fail; it also exposes emulator internals as API surface.
Fix: the expression validation in the PutStoredQuery handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_stored_query.rs) must map any parse failure to AWS's `syntax error at line <line>, column <column>` wording instead of forwarding the parser's own Display output.
Traces: config.stored-query-lifecycle (check source line 43).

## Evidence
Cause 1 — config.remediation-configuration-and-exception-lifecycle:
  expected: ClientError InvalidParameterValueException, message "The remediation target for <rule> could not be found. Specify the appropriate remediation target and try again."
  actual:   AssertionError: expected the call to be rejected, but it succeeded and returned {'FailedBatches': [], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for TargetId 'sim2real-behav-NoSuchAutomationDocument'.

Cause 2 — config.put-resource-config-rejects-unregistered-type:
  expected: ListDiscoveredResources(resourceType='<Org>::<Svc>::<Res>', resourceIds=[id]) -> 200, resourceIdentifiers == []
  actual:   botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the ListDiscoveredResources operation: 1 validation error detected: Value 'Sim2Real::FidelityProbe::Widget' at 'resourceType' failed to satisfy constraint: Member must satisfy enum value set: [AWS::EC2::CustomerGateway, AWS::EC2::EIP, ... AWS::WAF::Web...]
  (the preceding ListDiscoveredResources(resourceType='AWS::S3::Bucket') and the PutResourceConfig ValidationException assertion both passed, so the data plane is otherwise live)

Cause 3 — config.put-resource-config-and-list-discovered:
  expected: BatchGetResourceConfig(resourceKeys=[{AWS::S3::Bucket, <never-recorded id>}]) -> baseConfigurationItems == [] AND unprocessedResourceKeys == []
  actual:   AssertionError: {'baseConfigurationItems': [], 'unprocessedResourceKeys': [{'resourceType': 'AWS::S3::Bucket', 'resourceId': 'sim2real-behav-widget-28a59fb1110f'}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 4 — config.delete-config-rule-with-attached-remediation-rejected:
  code assertion passed (ResourceInUseException); message assertion failed.
  expected: "The rule <rule> currently has a RemediationConfiguration. Please delete the RemediationConfiguration and try again."
  actual:   "A remediation action is associated with the Config rule '<rule>' and Config cannot delete this rule. Delete the remediation action associated with the rule before deleting the rule and try your request again later."

Cause 5 — config.stored-query-lifecycle:
  code assertion passed (ValidationException); message assertion failed.
  expected: "syntax error at line 1, column 1"
  actual:   "The expression is not a valid Config query: the expression is not a SQL SELECT command: sql parser error: Expected: an SQL statement, found: NOT at Line: 1, Column: 1."
  (request Expression was "NOT VALID CONFIG SQL ((()")

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks against the emulator:

1. config.remediation-configuration-and-exception-lifecycle — create a running recorder, PutConfigRule, PutRemediationConfigurations with an AWS-owned SSM document (positive control succeeds), DeleteRemediationConfiguration, then PutRemediationConfigurations with TargetType=SSM_DOCUMENT and TargetId naming a nonexistent document and a valid AutomationAssumeRole. Expect 400 InvalidParameterValueException with AWS's "remediation target ... could not be found" message and nothing persisted; the emulator returns 200 {FailedBatches: []}.
2. config.put-resource-config-rejects-unregistered-type — with a recorder running, call ListDiscoveredResources(resourceType="Sim2Real::FidelityProbe::Widget", resourceIds=[id]). Expect 200 with an empty resourceIdentifiers list; the emulator raises 400 ValidationException on the enum constraint.
3. config.put-resource-config-and-list-discovered — call BatchGetResourceConfig with one resourceKey for a resource that was never recorded (e.g. {AWS::S3::Bucket, <unused id>}). Expect both baseConfigurationItems and unprocessedResourceKeys empty; the emulator echoes the key under unprocessedResourceKeys.
4. config.delete-config-rule-with-attached-remediation-rejected — PutConfigRule, attach a remediation configuration, then DeleteConfigRule and read the error message off the ResourceInUseException.
5. config.stored-query-lifecycle — PutStoredQuery with Expression="NOT VALID CONFIG SQL ((()" and read the ValidationException message.
