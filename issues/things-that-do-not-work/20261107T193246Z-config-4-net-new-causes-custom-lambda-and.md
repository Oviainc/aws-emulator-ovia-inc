Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.aggregate-config-rule-compliance-summary-matches-details config.aggregate-conformance-pack-compliance-summary-reflects-pack-status config.conformance-pack-compliance-details-and-summary-consistent config.delivery-channel-bucket-policy-statement-required config.deploy-configuration-recorder-cloudformation-actually-records
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.aggregate-config-rule-compliance-summary-matches-details|run#15|AssertionError|raise AssertionError(
Issue-Key: config.aggregate-conformance-pack-compliance-summary-reflects-pack-status|run#18|AssertionError|raise AssertionError(
Issue-Key: config.conformance-pack-compliance-details-and-summary-consistent|run#16|AssertionError|raise AssertionError(
Issue-Key: config.delivery-channel-bucket-policy-statement-required|_poll_until#25|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: config.deploy-configuration-recorder-cloudformation-actually-records|_poll_recorder_exists#10|AssertionError|raise AssertionError(f"configuration recorder {recorder_name} never appeared via CloudFormation: {last_exc}")
Issue-Commit: 9db3208a9fd65df95d2d1fd8f8c29dab9eab51df

# config: 4 net-new causes — custom-Lambda and conformance-pack rules are never evaluated, PutDeliveryChannel skips the bucket-policy pre-check, and CFN AWS::Config::ConfigurationRecorder is never materialised

> _Note: this issue was identified against the code base at commit `9db3208a9fd65df95d2d1fd8f8c29dab9eab51df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetComplianceDetailsByConfigRule

## Description
Four distinct net-new root causes in the AWS Config emulator, all surfaced by checks the suite lists under `### config` and none of them marked ALREADY FILED or OWNED. They are bundled because they share the service, not because they share a fix — a fixer must close ALL FOUR before submitting.

Cause 1 of 4: a scheduled CUSTOM_LAMBDA config rule is never evaluated, so the rule has no compliance detail at all.
AWS: Smithy (com-amazonaws-configservice-starlingdoveservice) models PutConfigRule with Source.Owner=CUSTOM_LAMBDA + SourceDetails/MaximumExecutionFrequency, PutEvaluations(Evaluations, ResultToken required; errors InvalidParameterValueException / InvalidResultTokenException / NoSuchConfigRuleException) and StartConfigRulesEvaluation. On AWS, creating a custom-Lambda rule (and explicitly calling StartConfigRulesEvaluation) causes Config to invoke the evaluator Lambda with a valid ResultToken; the Lambda's PutEvaluations verdict then becomes a row in GetComplianceDetailsByConfigRule for that rule, and after aggregation the same verdict is readable through GetAggregateComplianceDetailsByConfigRule (AggregateEvaluationResults, ComplianceType=NON_COMPLIANT, qualified by the evaluated resource id). Recorded behavior confirms the local detail row appears first, then the aggregate count and row.
Emulator: PutConfigRule and StartConfigRulesEvaluation are accepted (2xx), but the evaluator Lambda is never invoked and no PutEvaluations verdict is ever recorded: GetComplianceDetailsByConfigRule(ConfigRuleName=<rule>) returns EvaluationResults=[] for the whole 142 s poll budget, so the check never gets as far as the aggregate read. There is no refusal and no error — the evaluation pipeline simply has no effect.
Why: a client that creates a custom-Lambda rule and reads back its compliance sees a rule that is permanently un-evaluated. Nothing distinguishes "not yet evaluated" from "will never be evaluated", so compliance dashboards, gating logic and any loop that waits for a verdict hang forever or wrongly conclude the account is clean.
Fix: the Config rule-evaluation engine — the component that, on rule creation and on StartConfigRulesEvaluation, dispatches a CUSTOM_LAMBDA rule to its Source.SourceIdentifier Lambda with a ResultToken, plus the PutEvaluations handler/state that must land the returned verdict in the rule's evaluation-result store so GetComplianceDetailsByConfigRule and the aggregate reads can project it. This is distinct from the open issue 20261029T184030Z-config-compliance-and-aggregation-never-converge-conformance-pack.md (which names a CUSTOM_LAMBDA verdict among its causes but does not carry this check's replay key) — if that issue's fix covers the dispatch path, this check must still be replayed to confirm it.
Traces: config.aggregate-config-rule-compliance-summary-matches-details.

Cause 2 of 4: a conformance pack reaches CREATE_COMPLETE but its member rules are never evaluated, so the pack's compliance never resolves.
AWS: Smithy models DescribeConformancePackCompliance (output ConformancePackRuleComplianceList required), GetConformancePackComplianceDetails (ConformancePackRuleEvaluationResults) and GetConformancePackComplianceSummary. On AWS, a pack deployed from a template declaring an AWS::Config::ConfigRule with Source.Owner=AWS, SourceIdentifier=IAM_PASSWORD_POLICY creates that managed rule, Config evaluates it, and the pack's summary ConformancePackComplianceStatus settles on COMPLIANT or NON_COMPLIANT while the details read returns one evaluation result per member rule; the aggregator then reports exactly one pack in CompliantConformancePackCount + NonCompliantConformancePackCount for the account/region.
Emulator: PutConformancePack is accepted and DescribeConformancePackStatus does report CREATE_COMPLETE, but the pack's member managed rule is never evaluated: GetConformancePackComplianceSummary's ConformancePackComplianceStatus never becomes COMPLIANT or NON_COMPLIANT within 127 s, and GetConformancePackComplianceDetails yields no ConformancePackRuleEvaluationResults within 153 s. Both reads answer 200 with an unresolved/empty projection indefinitely, so the dependent aggregate read (GetAggregateConformancePackComplianceSummary) is never reached.
Why: a client deploying a conformance pack is told the pack is CREATE_COMPLETE and then can never learn whether it passes. Compliance reporting built on the pack summary/details pair reads as "no findings" forever, which is indistinguishable from a clean account.
Fix: the conformance-pack deployment path — it must materialise each template-declared AWS::Config::ConfigRule as a real config rule, run it through the same managed-rule evaluation engine as Cause 1, and have the pack compliance summary/details projections derive from those evaluation results instead of staying empty. Related open issue 20261029T184030Z-config-compliance-and-aggregation-never-converge-conformance-pack.md describes pack-rule non-convergence but does not carry these two checks; file/fix confirmation must replay them.
Traces: config.aggregate-conformance-pack-compliance-summary-reflects-pack-status, config.conformance-pack-compliance-details-and-summary-consistent.

Cause 3 of 4: PutDeliveryChannel performs no S3 bucket-policy pre-check, so a bucket missing the s3:GetBucketAcl grant for config.amazonaws.com is accepted instead of refused.
AWS: Smithy models PutDeliveryChannel with error InsufficientDeliveryPolicyException (client error, HTTP 400). AWS Config pre-checks the target bucket's policy before accepting the channel and refuses when the documented delivery policy is incomplete. With only the s3:GetBucketAcl statement removed (ListBucket and the PutObject grant still present, same bucket, same request), AWS answers 400 InsufficientDeliveryPolicyException with message `Insufficient delivery policy to s3 bucket: <bucket>, unable to get s3 bucket acl.` — the fixed text is `Insufficient delivery policy to s3 bucket: ` … `, unable to get s3 bucket acl.`, and the only variable part is the s3BucketName taken from the request. The refusal must also have no effect: DescribeDeliveryChannels stays empty and DescribeDeliveryChannelNames=[<name>] answers 400 NoSuchDeliveryChannelException with `Cannot find delivery channel with specified name '<name>'.` (fixed text, name from the request).
Emulator: the identical PutDeliveryChannel is accepted (2xx) every time for 120 s of retries after the s3:GetBucketAcl statement is dropped — no error code and no message are produced at all, and a delivery channel is created for a bucket Config could not actually read the ACL of.
Why: a client gets a delivery channel that AWS would have rejected, so a misconfigured bucket policy is discovered only as silently missing deliveries rather than at the API call that can be fixed. Error-handling code keyed on InsufficientDeliveryPolicyException is never exercised, and IaC that relies on the refusal to fail fast reports success.
Fix: PutDeliveryChannel's validation — add the bucket-policy pre-check (resolve the target bucket's policy from S3 state and require the config.amazonaws.com grants for s3:GetBucketAcl, s3:ListBucket and s3:PutObject) and raise InsufficientDeliveryPolicyException with AWS's wording before any channel is stored. Open issue 20261022T190340Z-config-putconfigrule-and-putdeliverychannel-are-overly-permissive-inconsistent.md covers other PutDeliveryChannel permissiveness (malformed ARNs, unvalidated bucket name) but does not carry this check's replay key or the bucket-policy grant check.
Traces: config.delivery-channel-bucket-policy-statement-required.

Cause 4 of 4: a CloudFormation stack declaring AWS::Config::ConfigurationRecorder reports no failure, yet the recorder never exists in Config.
AWS: recorded behavior only (the resource type is a CloudFormation resource, not a Config API shape). On AWS, a stack containing AWS::Config::ConfigurationRecorder (Name, RoleARN from an IAM role, RecordingGroup{AllSupported,IncludeGlobalResourceTypes}) calls Config's PutConfigurationRecorder during provisioning, so DescribeConfigurationRecorders(ConfigurationRecorderNames=[<name>]) resolves the recorder with that exact name, and it can then be started and will record.
Emulator: the stack is created and never enters any *_FAILED or ROLLBACK_* state (the check polls DescribeStacks for failure and finds none), but for 120 s DescribeConfigurationRecorders(ConfigurationRecorderNames=[<name>]) answers 400 NoSuchConfigurationRecorderException `The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again.` — i.e. the declared recorder was silently not materialised into Config state while CloudFormation reports progress/success.
Why: a client that provisions its Config baseline by CloudFormation (the normal way) is told the stack succeeded and then finds no recorder, so nothing is recorded and every downstream Config read is empty; worse, the stack's reported state disagrees with the service's state, so drift/rollback logic cannot detect it.
Fix: CloudFormation's resource provisioner for AWS::Config::ConfigurationRecorder — it must invoke the Config PutConfigurationRecorder path (and register the physical id) so the recorder appears to DescribeConfigurationRecorders, or fail the stack resource if it cannot. Open issue 20261022T190340Z-config-has-no-recording-delivery-side-effects-and.md names CFN-declared Config resources among its causes but does not carry this check's replay key; this check must be replayed to confirm any fix.
Traces: config.deploy-configuration-recorder-cloudformation-actually-records.

## Evidence
All evidence from the suite replay (115/177 passed; these 5 failing checks are the only `### config` failures not marked ALREADY FILED / OWNED).

Cause 1 — config.aggregate-config-rule-compliance-summary-matches-details [mismatch]:
  AssertionError: the Lambda-backed rule never delivered a PutEvaluations result within 142s
  Expected (AWS): GetComplianceDetailsByConfigRule(ConfigRuleName=<rule>) returns >=1 EvaluationResults with ComplianceType=NON_COMPLIANT after StartConfigRulesEvaluation on a CUSTOM_LAMBDA rule.
  Actual (emulator): EvaluationResults=[] for the whole poll window; PutConfigRule/StartConfigRulesEvaluation both 2xx, evaluator Lambda never invoked.

Cause 2 — config.aggregate-conformance-pack-compliance-summary-reflects-pack-status [mismatch]:
  AssertionError: the conformance pack's own rule never reached a final compliance status within 127s
  Expected: GetConformancePackComplianceSummary(...).ConformancePackComplianceSummaryList[0].ConformancePackComplianceStatus in {COMPLIANT, NON_COMPLIANT}.
  Actual: never reaches either value, although DescribeConformancePackStatus already reports ConformancePackState=CREATE_COMPLETE.
  config.conformance-pack-compliance-details-and-summary-consistent [mismatch]:
  AssertionError: the conformance pack's rule never produced an evaluation result within 153s
  Expected: GetConformancePackComplianceDetails returns ConformancePackRuleEvaluationResults for the pack's IAM_PASSWORD_POLICY rule.
  Actual: empty for the whole window.

Cause 3 — config.delivery-channel-bucket-policy-statement-required [mismatch]:
  AssertionError: PutDeliveryChannel refused once s3:GetBucketAcl is gone from the policy not met within 120s. Last result: None
  (the helper returns None for every ACCEPTED put, so 'None' means the call kept succeeding)
  Expected (AWS): 400 InsufficientDeliveryPolicyException, Message = "Insufficient delivery policy to s3 bucket: <s3BucketName>, unable to get s3 bucket acl." and no channel created (DescribeDeliveryChannels == [], and DescribeDeliveryChannels(DeliveryChannelNames=[<name>]) -> 400 NoSuchDeliveryChannelException "Cannot find delivery channel with specified name '<name>'.").
  Actual (emulator): PutDeliveryChannel returns success; no error code, no message; the channel is created.
  Control legs that DO pass: the same call with the complete policy is accepted, confirming only the policy pre-check is missing.

Cause 4 — config.deploy-configuration-recorder-cloudformation-actually-records [mismatch]:
  AssertionError: configuration recorder sim2real-recorder-<id> never appeared via CloudFormation: An error occurred (NoSuchConfigurationRecorderException) when calling the DescribeConfigurationRecorders operation: The name of the configuration recorder you entered cannot be found. Verify the configuration recorder name, and try again.
  Expected: after the stack declaring AWS::Config::ConfigurationRecorder is created, DescribeConfigurationRecorders(ConfigurationRecorderNames=[<name>]) returns exactly one recorder with name == the template's Name.
  Actual: 400 NoSuchConfigurationRecorderException for 120 s while the stack reports no ROLLBACK/FAILED status (the check aborts early on stack failure and did not).
  Note: the recorder name/id in the message is a per-run generated value, not part of the requirement.

## Steps to Reproduce
Cause 1: create an evaluator Lambda returning NON_COMPLIANT; PutConfigRule with Source.Owner=CUSTOM_LAMBDA, SourceIdentifier=<lambda arn> and a periodic trigger; call StartConfigRulesEvaluation(ConfigRuleNames=[<rule>]); poll GetComplianceDetailsByConfigRule(ConfigRuleName=<rule>) — EvaluationResults stays [] (AWS returns a NON_COMPLIANT result, and GetAggregateComplianceDetailsByConfigRule later returns exactly one).

Cause 2: ensure a running recorder + delivery channel; PutConformancePack(ConformancePackName=<pack>, TemplateBody declaring an AWS::Config::ConfigRule with Source.Owner=AWS, SourceIdentifier=IAM_PASSWORD_POLICY); wait for DescribeConformancePackStatus -> CREATE_COMPLETE; then poll GetConformancePackComplianceSummary(ConformancePackNames=[<pack>]) (status never becomes COMPLIANT/NON_COMPLIANT) and GetConformancePackComplianceDetails(ConformancePackName=<pack>) (ConformancePackRuleEvaluationResults stays empty).

Cause 3: create an S3 bucket; put the documented Config delivery policy (s3:GetBucketAcl + s3:ListBucket on the bucket and s3:PutObject on bucket/* for config.amazonaws.com); PutDeliveryChannel({name,s3BucketName}) -> accepted; DeleteDeliveryChannel; re-put the SAME policy with only the s3:GetBucketAcl statement removed; call the identical PutDeliveryChannel again — it is accepted instead of refused 400 InsufficientDeliveryPolicyException "Insufficient delivery policy to s3 bucket: <bucket>, unable to get s3 bucket acl.", and DescribeDeliveryChannels now lists the channel.

Cause 4: CreateStack with a template containing AWS::S3::Bucket + AWS::S3::BucketPolicy + AWS::IAM::Role (trusting config.amazonaws.com) + AWS::Config::ConfigurationRecorder{Name:<recorder>, RoleARN: Fn::GetAtt role Arn, RecordingGroup:{AllSupported:true, IncludeGlobalResourceTypes:false}}; poll DescribeStacks (no ROLLBACK/FAILED) and DescribeConfigurationRecorders(ConfigurationRecorderNames=[<recorder>]) — the latter answers 400 NoSuchConfigurationRecorderException indefinitely.
