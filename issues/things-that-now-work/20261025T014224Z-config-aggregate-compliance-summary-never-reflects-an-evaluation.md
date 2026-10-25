Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.aggregate-compliance-summary-reflects-source-evaluations config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied config.start-remediation-execution-missing-configuration-rejected config.service-linked-configuration-recorder-lifecycle
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.aggregate-compliance-summary-reflects-source-evaluations|_poll_until#40|Timeout|time.sleep(interval_s)
Issue-Key: config-service|GetComplianceDetailsByConfigRule|ValidationException
Issue-Key: config.start-remediation-execution-missing-configuration-rejected|run#15|AssertionError|assert start_code == "NoSuchRemediationConfigurationException", (
Issue-Key: config.service-linked-configuration-recorder-lifecycle|run#25|AssertionError|assert last_message == _ARN_NOT_FOUND_MESSAGE, (
Issue-Commit: d6e9bd85505b2fcf81ff15dd47c3206271b87270

# Config: aggregate compliance summary never reflects an evaluation, conformance-pack rule names exceed the 64-char read limit, StartRemediationExecution mis-codes a missing remediation configuration, and an ARN lookup is refused as a name (4 causes)

> _Note: this issue was identified against the code base at commit `d6e9bd85505b2fcf81ff15dd47c3206271b87270`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAggregateConfigRuleComplianceSummary

## Description
FOUR net-new root causes in the Config emulator, all grounded in failing behavioral checks of this suite. FOUR CAUSES must be closed before submitting.

Cause 1 of 4: the aggregate compliance SUMMARY never reflects a verdict a rule produced, so every flow that polls it runs to the harness ceiling.
AWS: recorded behavior only, with model grounding. The recorded flow is: create a CUSTOM_LAMBDA-backed Config rule whose function calls PutEvaluations, wait for the rule to become ACTIVE, read the verdict through GetComplianceDetailsByConfigRule, then read the same verdict through the aggregator surfaces — GetAggregateComplianceDetailsByConfigRule returns the per-source-account/region result and GetAggregateConfigRuleComplianceSummary's NON_COMPLIANT count for Filters={AccountId, AwsRegion} rises by exactly one. Smithy (com-amazonaws-configservice-starlingdoveservice.json): GetAggregateConfigRuleComplianceSummary models only client/400 errors (InvalidLimitException, InvalidNextTokenException, NoSuchConfigurationAggregatorException, ValidationException), so a healthy aggregator never answers a request about an existing rule with a permanently unchanged summary.
Emulator: the check is killed with 'Check timed out after 60.0s' while polling for the verdict; no evaluation result and no change to the aggregate summary ever materialises. The timeout report carries no stack frame, so the FIRST non-converging step is not pinned by the replay; the check polls (a) DescribeConfigRules for ConfigRuleState==ACTIVE, (b) GetComplianceDetailsByConfigRule for a NON_COMPLIANT result, (c) GetAggregateComplianceDetailsByConfigRule, (d) GetAggregateConfigRuleComplianceSummary, and a fixer should run those legs separately to see which never converges.
Why: a client that builds a compliance dashboard on the aggregate summary can never observe a verdict, and cannot distinguish 'nothing is non-compliant' from 'evaluation and aggregation are not implemented' — no call ever fails, the numbers simply never move.
Fix: the custom-rule evaluation pipeline and the aggregate read path in Config (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_evaluations.rs plus the compliance state it writes, and src/ops/get_aggregate_config_rule_compliance_summary.rs / get_aggregate_compliance_details_by_config_rule.rs).
Related open issue, named for overlap but NOT relied on as cover (it does not carry this check id): 20261024T114729Z-config-never-converges-on-an-evaluation-or-a.md, whose Cause 1 is the same non-convergence seen through GetAggregateComplianceDetailsByConfigRule. This check additionally pins the SUMMARY surface.
Traces: config.aggregate-compliance-summary-reflects-source-evaluations.

Cause 2 of 4: a conformance-pack-deployed Config rule is given a generated name longer than 64 characters, so Config's own compliance read paths then refuse to accept that name.
AWS: Smithy — ConfigRule.ConfigRuleName targets ConfigRuleName (@length 1..128), but GetComplianceDetailsByConfigRule.ConfigRuleName targets StringWithCharLimit64 (@length 1..64); the same 64-char ceiling applies on the other compliance read paths. Real AWS suffixes a conformance-pack-managed rule name (recorded behavior: '<rule>-conformance-pack-<short random>') and the result stays inside 64 characters, so a rule a pack deployed is always readable through GetComplianceDetailsByConfigRule.
Emulator: PutConformancePack deploys the template's rule under a name with a long generated suffix ('-conformance-pack-<16 chars>'), producing a 69-character rule name; the very next GetComplianceDetailsByConfigRule for that name is refused 400 ValidationException '1 validation error detected: Value \'sim2real-process-check-...-conformance-pack-000391gcnfweg5vg\' at \'configRuleName\' failed to satisfy constraint: Member must have length less than or equal to 64' (message quoted as an example; the status 400 and code ValidationException are the requirement). The emulator thus emits an identifier its own API cannot accept.
Why: every process-check / conformance-pack flow is unusable — a client that deploys a rule through a conformance pack, discovers its name via DescribeConfigRules and then reads or clears its evaluations is hard-blocked, and the failure looks like a client input error when the name came from the service.
Fix: the conformance-pack rule-name generation (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_conformance_pack.rs and the pack-deployment state that materialises the rule) must keep the generated name within the 64-char limit the compliance read paths enforce.
Traces: config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied.

Cause 3 of 4: StartRemediationExecution answers ValidationException for a rule with no remediation configuration instead of NoSuchRemediationConfigurationException.
AWS: Smithy — StartRemediationExecution(ConfigRuleName @length 1..128 pattern \S, ResourceKeys 1..100) models exactly three errors: InsufficientPermissionsException, InvalidParameterValueException and NoSuchRemediationConfigurationException (all errorKind client, httpError 400). ValidationException is not modeled on this operation at all. NoSuchRemediationConfigurationException is documented 'You specified an Config rule without a remediation configuration.', which is precisely this request: an ACTIVE rule that exists but has no remediation configuration attached. Recorded message example: 'There is no applicable RemediationConfiguration setup for AWS Config rule <rule>'.
Emulator: the call is refused with code 'ValidationException' — an error the operation does not model — so the caller receives a 400 whose code says the request was malformed rather than that no remediation configuration exists.
Why: a client cannot tell 'this rule has no remediation configuration' (retry after PutRemediationConfigurations) from 'my request was invalid' (never retry). Error-code based branching, which is the only reliable discriminator, takes the wrong branch.
Fix: the refusal classification in the operation's handler/validation (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_remediation_execution.rs) — look up the remediation configuration for the named rule and raise NoSuchRemediationConfigurationException when absent. Note: an already-filed issue covers StartRemediationExecution being an unimplemented scaffold stub for a different check; this cause is about the error CODE the call currently returns and must be closed on its own.
Traces: config.start-remediation-execution-missing-configuration-rejected.

Cause 4 of 4: a recorder looked up by ARN is refused with the name-variant refusal, so the refusal does not identify the member the caller supplied.
AWS: Smithy — DescribeConfigurationRecorders takes three alternative selectors (ConfigurationRecorderNames, ServicePrincipal, Arn @length 1..1000) and models NoSuchConfigurationRecorderException (client/400, documented 'You have specified a configuration recorder that does not exist.'). Recorded behavior: the refusal message is built from the selector that was used — a lookup by Arn is refused with the arn-variant ('The arn of the configuration recorder you entered cannot be found. Verify the configuration recorder arn, and try again. ') and a lookup by name with the name-variant. The requirement here is that rule (name the selector the request carried), not the recorded text.
Emulator: after DeleteServiceLinkedConfigurationRecorder, DescribeConfigurationRecorders(Arn=<deleted arn>) is refused with the correct code NoSuchConfigurationRecorderException but with the NAME-variant message ('The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again.') even though no name was supplied — the handler emits one hard-coded refusal regardless of which selector was used.
Why: the caller is told to verify an input it never sent; for a service-linked recorder, which is addressed only by ARN, the guidance is unactionable and suggests the ARN selector was not consulted at all.
Fix: derive the refusal from the selector in the request in the operation's handler (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/describe_configuration_recorders.rs).
Traces: config.service-linked-configuration-recorder-lifecycle.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 49/115 checks passed).

Cause 1 — config.aggregate-compliance-summary-reflects-source-evaluations:
  Error: Check timed out after 60.0s
  The check creates an IAM role + Lambda that calls PutEvaluations, PutConfigRule with a CUSTOM_LAMBDA source, PutConfigurationAggregator over {AccountId, Region}, then polls: DescribeConfigRules for ACTIVE, GetComplianceDetailsByConfigRule for a NON_COMPLIANT result (120s), GetAggregateComplianceDetailsByConfigRule (180s) and finally GetAggregateConfigRuleComplianceSummary for baseline_noncompliant + 1. None of the polled states is ever reached.

Cause 2 — config.e2e-external-evaluation-persists-when-delete-evaluation-results-denied:
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the GetComplianceDetailsByConfigRule operation: 1 validation error detected: Value 'sim2real-process-check-776e61f4bb8d-conformance-pack-000391gcnfweg5vg' at 'configRuleName' failed to satisfy constraint: Member must have length less than or equal to 64
  The offending name was produced by the emulator itself: the check deploys an AWS_CONFIG_PROCESS_CHECK rule through PutConformancePack and then resolves the deployed name by prefix via DescribeConfigRules (_poll_rule_active_by_prefix), i.e. the 69-character name is the emulator's generated conformance-pack rule name. Model: GetComplianceDetailsByConfigRule.ConfigRuleName -> StringWithCharLimit64 (@length 1..64) vs ConfigRule.ConfigRuleName -> ConfigRuleName (@length 1..128).

Cause 3 — config.start-remediation-execution-missing-configuration-rejected:
  AssertionError: expected NoSuchRemediationConfigurationException for a rule with no remediation configuration, got 'ValidationException'
  Request: StartRemediationExecution(ConfigRuleName=<rule created by PutConfigRule with Source CLOUD_TRAIL_ENABLED, polled to ACTIVE, with NO remediation configuration>, ResourceKeys=[{resourceType: 'AWS::::Account', resourceId: <account>}]). A sibling rule in the same check does have a remediation configuration attached (PutRemediationConfigurations) and is the positive control. Model: StartRemediationExecution errors = InsufficientPermissionsException | InvalidParameterValueException | NoSuchRemediationConfigurationException (client/400); ValidationException is not modeled.

Cause 4 — config.service-linked-configuration-recorder-lifecycle:
  AssertionError: expected 'The arn of the configuration recorder you entered cannot be found. Verify the configuration recorder arn, and try again. ' on post-delete describe, got 'The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again.'
  Sequence that got there: PutServiceLinkedConfigurationRecorder(ServicePrincipal='telemetry-enablement.observabilityadmin.amazonaws.com') -> recorder visible via DescribeConfigurationRecorders(Arn=...) -> DeleteServiceLinkedConfigurationRecorder(Arn=...) -> DescribeConfigurationRecorders(Arn=...) refused with code NoSuchConfigurationRecorderException (correct) but the name-variant message.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay the four check ids in trace_ids.
3. Cause 1: create a Lambda that calls config:PutEvaluations, config put-config-rule with Source.Owner=CUSTOM_LAMBDA pointing at it, config put-configuration-aggregator over this account/region, then poll config get-compliance-details-by-config-rule and config get-aggregate-config-rule-compliance-summary --filters AccountId=<acct>,AwsRegion=<region> — the NON_COMPLIANT count never changes.
4. Cause 2: config put-conformance-pack with a template declaring one AWS::Config::ConfigRule (SourceIdentifier AWS_CONFIG_PROCESS_CHECK) whose ConfigRuleName is ~35 characters; wait for CREATE_COMPLETE; config describe-config-rules and note the deployed name is 69 characters; call config get-compliance-details-by-config-rule --config-rule-name <that name> and observe 400 ValidationException on the length constraint.
5. Cause 3: config put-config-rule for a managed rule with no remediation configuration, poll to ACTIVE, then config start-remediation-execution --config-rule-name <rule> --resource-keys resourceType=AWS::::Account,resourceId=<acct> — observe ValidationException instead of NoSuchRemediationConfigurationException.
6. Cause 4: config put-service-linked-configuration-recorder --service-principal telemetry-enablement.observabilityadmin.amazonaws.com, then config delete-service-linked-configuration-recorder --arn <arn>, then config describe-configuration-recorders --arn <arn> — the NoSuchConfigurationRecorderException message talks about a recorder NAME although only an ARN was supplied.
