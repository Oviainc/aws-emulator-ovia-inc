Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.overly-permissive-aggregator-invalid-account-id config.put-aggregation-authorization-malformed-account-rejected config.tag-key-and-value-contract-limits-rejected config.tag-resource-lifecycle config.overly-permissive-tag-resource-unknown-arn config.tag-resource-nonexistent-arn-rejected config.op-delete-retention-configuration-nonexistent config.retention-configuration-lifecycle config.get-custom-rule-policy-non-policy-rule-rejected config.get-custom-rule-policy-on-managed-rule-rejected config.aggregator-source-exclusivity-rejected config.config-rule-custom-policy-details-validated
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.overly-permissive-aggregator-invalid-account-id|run#12|AssertionError|assert message == (
Issue-Key: config.put-aggregation-authorization-malformed-account-rejected|run#12|AssertionError|assert message[: len(header)] == header, message
Issue-Key: config.tag-key-and-value-contract-limits-rejected|run#16|AssertionError|assert message == expected_message, (
Issue-Key: config.tag-resource-lifecycle|run#21|AssertionError|assert _mask_java_identities(message) == expected_message, (
Issue-Key: config.overly-permissive-tag-resource-unknown-arn|run#16|AssertionError|assert message == not_found, message
Issue-Key: config.tag-resource-nonexistent-arn-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: config.op-delete-retention-configuration-nonexistent|run#11|AssertionError|assert message == f"Cannot find retention configuration with specified name '{name}'.", message
Issue-Key: config.retention-configuration-lifecycle|run#17|AssertionError|assert message == f"Cannot find retention configuration with specified name '{name}-wrong'.", message
Issue-Key: config.get-custom-rule-policy-non-policy-rule-rejected|run#15|AssertionError|assert message == (
Issue-Key: config.get-custom-rule-policy-on-managed-rule-rejected|run#15|AssertionError|assert message == (
Issue-Key: config.aggregator-source-exclusivity-rejected|run#14|AssertionError|assert message == expected_message, (
Issue-Key: config.config-rule-custom-policy-details-validated|run#17|AssertionError|assert message == expected_message, (
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Config refusal contract: validation messages report one violation with the element path and summarized values, and five not-found/validation messages use emulator-specific text (6 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Every failure here is a refusal whose CODE already matches and whose MESSAGE is produced by a known rule — either AWS's constraint-validation template (built from the request: the offending value, its member path and the violated constraints) or a fixed sentence AWS returns for that leg, which is the only signal separating legs that share one error code. SIX CAUSES must be closed.

Cause 1 of 6: the constraint-validation message builder reports only one violation, uses the element path instead of the list path, and summarizes values instead of echoing them.
AWS: refusals from the Smithy-generated validation layer follow the template "<n> validation error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint: <constraint>", with every violated constraint of the member listed (joined by "; "), the value echoed in full, and a list-valued member reported at the LIST path with its members rendered. Recorded examples: for AccountAggregationSources.AccountIds=["12345"] AWS answers 400 ValidationException "1 validation error detected: Value '[12345]' at 'accountAggregationSources.1.member.accountIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: \\d{12}]"; for AuthorizedAccountId='not-an-account-id' it answers "2 validation errors detected: " plus BOTH the pattern and the max-length violation of 'authorizedAccountId'; for a 257-character tag value it echoes the whole value; for a 51-entry tag list it renders the list's members.
Emulator: reports a single violation (the pattern only) at the ELEMENT path 'accountAggregationSources.1.member.accountIds.1.member'; reports "1 validation error detected" where AWS reports 2; truncates the echoed value ("...[257 bytes]"); and renders a list as a count summary ("Value '51 items' at 'tags'").
Why: the message is the only part of these refusals that identifies WHICH member and WHICH constraint failed — all four legs share ValidationException. A client that parses it to point a user at the offending field gets the wrong path, learns only one of several violations, and cannot see the offending value.
Fix: the shared constraint-validation message builder used by the Config request validators (services/service-com-amazonaws-configservice-starlingdoveservice/src/ — the validation layer behind put_configuration_aggregator.rs, put_aggregation_authorization.rs and tag_resource.rs).
Traces: config.overly-permissive-aggregator-invalid-account-id, config.put-aggregation-authorization-malformed-account-rejected, config.tag-key-and-value-contract-limits-rejected, config.tag-resource-lifecycle.

Cause 2 of 6: the tag-API not-found message.
AWS: TagResource/ListTagsForResource model ResourceNotFoundException (client/400, doc "You have specified a resource that does not exist"). Recorded template, seen for a config-rule ARN and for a stored-query ARN: "Resource <arn> does not exist for account <account-id>" — built from the request ARN and the caller's account.
Emulator: same code, template "The resource '<arn>' provided in the request does not exist." (no account).
Why: the account component is part of AWS's message rule (it tells the caller the ARN was looked up in THEIR account); clients matching the documented/recorded sentence do not match.
Fix: the ResourceNotFoundException message in the tag-API handlers (.../src/ops/tag_resource.rs, .../src/ops/list_tags_for_resource.rs).
Traces: config.overly-permissive-tag-resource-unknown-arn, config.tag-resource-nonexistent-arn-rejected.

Cause 3 of 6: the retention-configuration not-found message.
AWS: recorded template for DeleteRetentionConfiguration/DescribeRetentionConfigurations with an unknown name: "Cannot find retention configuration with specified name '<name>'." (NoSuchRetentionConfigurationException, client/400).
Emulator: same code, "Cannot find retention configuration with the specified name '<name>'. Please check the retentionConfigurationName."
Why: two recordings with different names ("default", "default-wrong") confirm the template; the emulator's wording differs from the only sentence AWS produces for this leg.
Fix: the NoSuchRetentionConfigurationException message in the retention-configuration handlers (.../src/ops/delete_retention_configuration.rs, .../src/ops/describe_retention_configurations.rs).
Traces: config.op-delete-retention-configuration-nonexistent, config.retention-configuration-lifecycle.

Cause 4 of 6: GetCustomRulePolicy on a rule that is not a custom-policy rule.
AWS: GetCustomRulePolicy answers NoSuchConfigRuleException (client/400) and, recorded for both a managed (Owner=AWS) rule and a process-check rule, reuses the generic rule-name template: "The ConfigRule '<name>' provided in the request is invalid. Please check the configRule name."
Emulator: same code, but a different rule: "The ConfigRule '<name>' provided in the request is invalid. Please check that the rule is a Config Custom Policy rule."
Why: AWS deliberately does not distinguish "wrong rule type" from "no such rule" in this message; the emulator leaks a different classification, so a client cannot reproduce AWS's (deliberately coarse) behavior and message-based branching diverges.
Fix: the NoSuchConfigRuleException message in the GetCustomRulePolicy handler (.../src/ops/get_custom_rule_policy.rs).
Traces: config.get-custom-rule-policy-non-policy-rule-rejected, config.get-custom-rule-policy-on-managed-rule-rejected.

Cause 5 of 6: PutConfigurationAggregator's AllAwsRegions/AwsRegions exclusivity refusal.
AWS: recorded behavior — AccountAggregationSources carrying both AllAwsRegions=true and an explicit AwsRegions list is refused with the fixed sentence "Your configuration aggregator contains a list of regions and also specifies the use of all regions. You must choose one of these options." All three refusal legs of this operation share one error code, so the sentence is the only discriminator.
Emulator: "AccountAggregationSources contains both AllAwsRegions and AwsRegions; specify only one of them and try again".
Why: a client cannot tell this leg from the other legs that share the code.
Fix: the exclusivity-check message in the PutConfigurationAggregator handler (.../src/ops/put_configuration_aggregator.rs).
Traces: config.aggregator-source-exclusivity-rejected.

Cause 6 of 6: PutConfigRule's missing-CustomPolicyDetails refusal.
AWS: recorded behavior — a ConfigRule with Source.Owner=CUSTOM_POLICY and no CustomPolicyDetails is refused InvalidParameterValueException (client/400) with the fixed sentence "CustomPolicyDetails is required when Owner is CUSTOM_POLICY". Two of the three legs of this check share InvalidParameterValueException, so the sentence is the discriminator.
Emulator: "ConfigRule.Source.CustomPolicyDetails is required when Source.Owner is CUSTOM_POLICY".
Why: same as Cause 5 — the leg is unidentifiable to a client keyed on AWS's text.
Fix: the InvalidParameterValueException message in PutConfigRule's source validation (.../src/ops/put_config_rule.rs).
Traces: config.config-rule-custom-policy-details-validated.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed). In every failure below the error CODE matched and only the message diverged.

Cause 1:
  config.overly-permissive-aggregator-invalid-account-id: got "1 validation error detected: Value '12345' at 'accountAggregationSources.1.member.accountIds.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: \\d{12}"; expected "1 validation error detected: Value '[12345]' at 'accountAggregationSources.1.member.accountIds' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 12, Member must have length greater than or equal to 12, Member must satisfy regular expression pattern: \\d{12}]".
  config.put-aggregation-authorization-malformed-account-rejected: got "1 validation error detected: Value 'not-an-account-id' at 'authorizedAccountId' ... pattern: \\d{12}"; expected header "2 validation errors detected: " with both the pattern and the "Member must have length less than or equal to 12" violations.
  config.tag-key-and-value-contract-limits-rejected: got "... Value 'vbbb...[257 bytes]' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256"; expected the full 257-character value echoed.
  config.tag-resource-lifecycle: got "1 validation error detected: Value '51 items' at 'tags' failed to satisfy constraint: Member must have length less than or equal to 50"; expected the list's members rendered (the check masks AWS's per-member identities as <hash>).

Cause 2:
  config.overly-permissive-tag-resource-unknown-arn: expected "Resource <config-rule arn> does not exist for account 000000000000", got "The resource '<arn>' provided in the request does not exist."
  config.tag-resource-nonexistent-arn-rejected: expected "Resource <stored-query arn> does not exist for account 000000000000", got the same emulator template.

Cause 3:
  config.op-delete-retention-configuration-nonexistent: expected "Cannot find retention configuration with specified name 'default'.", got "Cannot find retention configuration with the specified name 'default'. Please check the retentionConfigurationName."
  config.retention-configuration-lifecycle: same divergence for name 'default-wrong'.

Cause 4:
  config.get-custom-rule-policy-non-policy-rule-rejected and config.get-custom-rule-policy-on-managed-rule-rejected: code NoSuchConfigRuleException matched; got "The ConfigRule '<name>' provided in the request is invalid. Please check that the rule is a Config Custom Policy rule.", expected "... Please check the configRule name."

Cause 5:
  config.aggregator-source-exclusivity-rejected: got 'AccountAggregationSources contains both AllAwsRegions and AwsRegions; specify only one of them and try again', expected 'Your configuration aggregator contains a list of regions and also specifies the use of all regions. You must choose one of these options.'

Cause 6:
  config.config-rule-custom-policy-details-validated: got 'ConfigRule.Source.CustomPolicyDetails is required when Source.Owner is CUSTOM_POLICY', expected 'CustomPolicyDetails is required when Owner is CUSTOM_POLICY'.

Model grounding: com-amazonaws-configservice-starlingdoveservice.json — ValidationException, ResourceNotFoundException, NoSuchConfigRuleException and InvalidParameterValueException are all errorKind client with httpError 400 (defaulted); the account-id members carry pattern \\d{12} and length 12, TagsList is @length 0..50 and tag values @length 0..256, which is what AWS's validation template reports.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline; replay simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
2. Cause 1: config put-configuration-aggregator --account-aggregation-sources '[{"AccountIds":["12345"],"AllAwsRegions":true}]'; config put-aggregation-authorization --authorized-account-id not-an-account-id --authorized-aws-region us-west-2; config tag-resource with a 257-character tag value; config tag-resource with 51 tags — compare each ValidationException message with AWS's template (violation count, member path, value rendering).
3. Cause 2: config tag-resource / list-tags-for-resource against a forged config-rule or stored-query ARN.
4. Cause 3: config delete-retention-configuration --retention-configuration-name default.
5. Cause 4: config get-custom-rule-policy --config-rule-name <a managed Owner=AWS rule>.
6. Cause 5: the aggregator call in step 2 with both AllAwsRegions and AwsRegions set.
7. Cause 6: config put-config-rule with Source.Owner=CUSTOM_POLICY and no CustomPolicyDetails.
