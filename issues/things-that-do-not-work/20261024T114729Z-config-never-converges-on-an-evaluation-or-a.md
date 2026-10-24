Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.aggregate-compliance-details-reflect-source-rule-evaluation config.overly-permissive-put-evaluations-invalid-result-token config.e2e-remediation-execution-resolves-noncompliant-resource config.concurrent-start-remediation-execution-dedup config.remediation-execution-retry-backoff-and-timeout config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.aggregate-compliance-details-reflect-source-rule-evaluation|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: config.overly-permissive-put-evaluations-invalid-result-token|_poll_until#32|Timeout|time.sleep(interval_s)
Issue-Key: config.e2e-remediation-execution-resolves-noncompliant-resource|_poll_compliance#17|Timeout|time.sleep(5)
Issue-Key: config.concurrent-start-remediation-execution-dedup|_poll_compliance#22|Timeout|time.sleep(5)
Issue-Key: config.remediation-execution-retry-backoff-and-timeout|run#29|Timeout|time.sleep(10)
Issue-Key: config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Config never converges on an evaluation or a delivery: Lambda-backed rule verdicts, managed-rule NON_COMPLIANT results and snapshot-delivery info never appear, so every flow that polls for them is killed at the harness ceiling (3 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAggregateComplianceDetailsByConfigRule

## Description
Three causes where Config accepts the control-plane calls but the asynchronous consequence AWS produces never happens, so every check that polls for the effect runs to the harness deadline instead of observing it. THREE CAUSES must be closed.

Cause 1 of 3: a Lambda-backed custom Config rule never produces an evaluation result, so nothing downstream of PutEvaluations ever materialises.
AWS: recorded behavior only, with model grounding. The recorded flow is: CreateFunction, poll GetFunctionConfiguration until State=Active, PutConfigRule with a Source of type CUSTOM_LAMBDA pointing at that function, StartConfigRulesEvaluation, and then the verdict the function delivered with PutEvaluations becomes readable — GetComplianceDetailsByConfigRule returns an EvaluationResults entry carrying the function's Annotation/ComplianceType, and the same verdict shows up through the aggregator read path. Smithy (com-amazonaws-configservice-starlingdoveservice.json): GetAggregateComplianceDetailsByConfigRule returns AggregateEvaluationResults and models only InvalidLimitException, InvalidNextTokenException, NoSuchConfigurationAggregatorException and ValidationException (all client/400) — for a healthy aggregator and an existing rule the modeled answer is a result list that becomes non-empty once a verdict exists, never a permanently empty one.
Emulator: all four checks are killed at the harness ceiling with 'Check timed out after 60.0s' while polling for that state; no verdict ever becomes readable. The timeout report carries no stack frame, so the FIRST non-converging step is not pinned by the replay. Hypothesis: either the created function never reports State=Active (config.overly-permissive-put-evaluations-invalid-result-token polls `lam.get_function_configuration(FunctionName=...)['State'] == 'Active'` with a 180s budget before it ever touches Config), or Config never invokes the rule's function on StartConfigRulesEvaluation so no evaluation result is ever recorded. A fixer should confirm which by running the two legs separately.
Why: custom (Lambda-backed) Config rules are inert. A client that builds its own compliance control cannot observe any verdict — not through GetComplianceDetailsByConfigRule, not through the aggregate views, and not as the precondition for remediation — and cannot distinguish 'not evaluated yet' from 'evaluation is not implemented', because the call never fails, it just never answers differently.
Fix: the custom-rule evaluation pipeline in Config (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_config_rules_evaluation.rs and the evaluation/compliance state behind put_evaluations.rs plus the aggregate read path get_aggregate_compliance_details_by_config_rule.rs), together with the Lambda function-state transition the first leg polls (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/get_function_configuration.rs's State).
Traces: config.aggregate-compliance-details-reflect-source-rule-evaluation, config.overly-permissive-put-evaluations-invalid-result-token, config.e2e-remediation-execution-resolves-noncompliant-resource, config.concurrent-start-remediation-execution-dedup.

Cause 2 of 3: no evaluation ever marks a recorded resource NON_COMPLIANT, so the remediation state machine is unreachable.
AWS: recorded behavior only — with a running customer-managed recorder and a rule in scope for the resource, the resource acquires a NON_COMPLIANT evaluation result, which is the precondition real AWS requires before StartRemediationExecution will drive an execution through the documented QUEUED -> IN_PROGRESS -> terminal transitions reported by DescribeRemediationExecutionStatus.
Emulator: the check is killed at 'Check timed out after 60.0s' while waiting for that NON_COMPLIANT result; it never even reaches StartRemediationExecution (which is separately unimplemented and already filed), so the stall is upstream of it, in evaluation.
Why: remediation can never be exercised end to end, and a client cannot tell a rule that found nothing from a rule that was never evaluated.
Fix: the rule-evaluation path that writes compliance results for recorded resources (.../src/ops/start_config_rules_evaluation.rs and the compliance state module feeding get_compliance_details_by_config_rule.rs / describe_remediation_execution_status.rs).
Traces: config.remediation-execution-retry-backoff-and-timeout.

Cause 3 of 3: DescribeDeliveryChannelStatus never reports snapshot delivery info, so no bound on the next delivery can ever be read.
AWS: recorded behavior only — once a delivery channel with deliveryFrequency set (One_Hour / TwentyFour_Hours) has delivered, DescribeDeliveryChannelStatus populates configSnapshotDeliveryInfo (including nextDeliveryTime), and re-putting the channel under the same name updates the frequency in place on the account/region singleton.
Emulator: the check is killed at 'Check timed out after 60.0s' polling DescribeDeliveryChannelStatus for that snapshot delivery info; it is never populated.
Why: a client that verifies its Config delivery cadence (the normal way to confirm snapshot wiring) has nothing to read and waits forever; the deliveryFrequency it configured has no observable effect.
Fix: the delivery pipeline behind the delivery channel and its status reporting (.../src/ops/put_delivery_channel.rs's channel state plus the snapshot-delivery task and .../src/ops/describe_delivery_channel_status.rs).
Traces: config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time.

Related open issues, named so a fixer can see the overlap but NOT relied on as cover (neither carries these check ids): 20261022T190340Z-config-has-no-recording-delivery-side-effects-and.md (its Cause 2/3 are the recorder/S3-delivery side effects; Cause 3 here is the delivery-STATUS surface) and 20261023T140058Z-config-never-records-or-evaluates-a-resource-created.md.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 45/108 checks passed). All six checks below report exactly:

  Error: Check timed out after 60.0s

Cause 1:
  config.aggregate-compliance-details-reflect-source-rule-evaluation — polls GetAggregateComplianceDetailsByConfigRule for the per-resource verdict a CUSTOM_LAMBDA rule delivered with PutEvaluations (check declares _OVERALL_BUDGET_S = 900.0; resource type AWS::::Account).
  config.overly-permissive-put-evaluations-invalid-result-token — first polls `lam.get_function_configuration(FunctionName=<fn>)['State'] == 'Active'` (timeout_s=180), then DescribeConfigRules for ConfigRuleState == 'ACTIVE' (timeout_s=60), then GetComplianceDetailsByConfigRule for the positive control's own Annotation (timeout_s=180). Never gets past these polls, so its InvalidResultTokenException assertion on a forged ResultToken is never even reached.
  config.e2e-remediation-execution-resolves-noncompliant-resource — polls for the NON_COMPLIANT verdict a Lambda rule delivered, then for the COMPLIANT verdict after remediation.
  config.concurrent-start-remediation-execution-dedup — races two StartRemediationExecution calls for the same ConfigRuleName + ResourceKey after the rule has flagged the resource.

Cause 2:
  config.remediation-execution-retry-backoff-and-timeout — PutRemediationConfigurations persists RetryAttemptSeconds / MaximumAutomaticAttempts, then the check waits for the target bucket to actually hold a NON_COMPLIANT evaluation result (the precondition real AWS requires) before driving the QUEUED -> IN_PROGRESS -> terminal transitions. It is killed while waiting for that result.

Cause 3:
  config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time — polls DescribeDeliveryChannelStatus for configSnapshotDeliveryInfo.nextDeliveryTime with deliveryFrequency=One_Hour, then again after an in-place change to TwentyFour_Hours. No snapshot delivery info is ever reported.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): GetAggregateComplianceDetailsByConfigRule(input ConfigurationAggregatorName, ConfigRuleName, AccountId, AwsRegion required; Limit 0..100) -> AggregateEvaluationResults, errors InvalidLimitException / InvalidNextTokenException / NoSuchConfigurationAggregatorException / ValidationException, all errorKind client, httpError 400 — i.e. a healthy aggregator never answers a request about an existing rule with a permanently empty result list.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay the check ids in trace_ids; each reports 'Check timed out after 60.0s'.
3. Cause 1 (isolate the two legs): (a) lambda create-function with a small zip, then poll lambda get-function-configuration and watch whether State ever becomes 'Active'; (b) with a function that is usable, config put-config-rule with Source.Owner=CUSTOM_LAMBDA pointing at it, config start-config-rules-evaluation, then poll config get-compliance-details-by-config-rule — no EvaluationResults entry ever appears, and config get-aggregate-compliance-details-by-config-rule against an aggregator over this account/region stays empty.
4. Cause 2: start a customer-managed recorder, create a rule in scope for an S3 bucket, attach a remediation configuration, then poll config get-compliance-details-by-resource for the bucket — no NON_COMPLIANT result ever appears, so the remediation state machine is never entered.
5. Cause 3: config put-delivery-channel with configSnapshotDeliveryProperties.deliveryFrequency=One_Hour at a policied bucket, start the recorder, then poll config describe-delivery-channel-status — configSnapshotDeliveryInfo (and nextDeliveryTime) is never populated.
