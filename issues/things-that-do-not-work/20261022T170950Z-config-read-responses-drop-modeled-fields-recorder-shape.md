Issue-Source: trace-sweep-divergence
Issue-Trace: config-configuration-recorder-and-delivery-channel-lifecycle
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: config-configuration-recorder-and-delivery-channel-lifecycle config-configuration-recorder-update-role-arn config-config-rule-evaluation-lifecycle config-resource-tagging-and-compliance-summaries
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[name="sim2real-probe-config-recorder-e8ffebde"].arn
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[name="sim2real-probe-config-recorder-e8ffebde"].recordingGroup.exclusionByResourceTypes
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[name="sim2real-probe-config-recorder-e8ffebde"].recordingGroup.recordingStrategy
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[name="sim2real-probe-config-recorder-e8ffebde"].recordingMode
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[name="sim2real-probe-config-recorder-e8ffebde"].recordingScope
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[*].arn
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[*].recordingGroup.exclusionByResourceTypes
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[*].recordingGroup.recordingStrategy
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[*].recordingMode
Issue-Key: config|DescribeConfigurationRecorders|2xx->2xx|$.ConfigurationRecorders[*].recordingScope
Issue-Key: config|DescribeConfigRules|2xx->2xx|$.ConfigRules[*]
Issue-Key: config|GetComplianceSummaryByConfigRule|2xx->5xx|$.ComplianceSummary
Issue-Key: config|GetComplianceSummaryByConfigRule|2xx->5xx|status
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# Config read responses drop modeled fields: recorder shape and ARN, rule visibility, missing compliance summary

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeConfigurationRecorders

## Description
Four distinct causes on the AWS Config read path, all in this service: describe responses omit modeled members, the configuration-recorder ARN uses the wrong resource type, and one modeled read operation is not implemented at all (501, and it is NOT covered by a scaffold-stub issue — the replay reported no stub_path for it). No existing open issue covers the config service wire surface. FOUR CAUSES must be closed.

Cause 1 of 4 — DescribeConfigurationRecorders omits recordingGroup.exclusionByResourceTypes, recordingGroup.recordingStrategy, recordingMode and recordingScope (fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#2).
AWS: ConfigurationRecorder models recordingGroup (RecordingGroup, with members exclusionByResourceTypes and recordingStrategy), recordingMode (RecordingMode: required recordingFrequency plus recordingModeOverrides) and recordingScope (enum RecordingScope: INTERNAL|PAID) — Smithy: ConfigurationRecorder, RecordingGroup, RecordingMode, RecordingScope. The recording shows AWS populating all of them for a customer managed recorder created with an explicit resourceTypes list: recordingGroup.exclusionByResourceTypes {resourceTypes: []}, recordingGroup.recordingStrategy {useOnly: 'INCLUSION_BY_RESOURCE_TYPES'} (derived from the recording group that was put), recordingMode {recordingFrequency: 'CONTINUOUS', recordingModeOverrides: []} and recordingScope 'PAID' — i.e. the server materializes the defaults rather than echoing only what the caller sent.
Emulator: returns the recorder with those four members absent/null.
Why: a client reading the recorder to learn its recording strategy, frequency or billing scope gets None from the emulator and a concrete value from AWS; code that branches on recordingStrategy.useOnly or recordingMode.recordingFrequency takes the wrong branch, and round-tripping a described recorder back into PutConfigurationRecorder loses the strategy.
Fix: the configuration-recorder state defaulting at Put time plus the DescribeConfigurationRecorders serializer — derive recordingStrategy from the recording group, default exclusionByResourceTypes, recordingMode (CONTINUOUS, empty overrides) and recordingScope, and emit them.

Cause 2 of 4 — configuration-recorder ARN uses the wrong resource type and omits the recorder id segment (fixture config-configuration-recorder-update-role-arn, step task#5).
AWS: ConfigurationRecorder.arn is an AmazonResourceName (Smithy: ConfigurationRecorder.arn). Recorded behavior: the ARN has the form arn:aws:config:<region>:<account>:configuration-recorder/<recorder-name>/<recorder-id> — resource type 'configuration-recorder' followed by the name and a server-assigned id segment. The region, account and the id value itself are environment-specific; the resource-type token and the presence of the id segment are the contract.
Emulator: emits arn:aws:config:<region>:<account>:config-recorder/<recorder-name> — resource type 'config-recorder' and no id segment. The same step also shows the four missing members of Cause 1.
Why: clients and policies that parse or match Config recorder ARNs (IAM resource patterns, ARN splitting on resource type) do not recognise the emulator's ARN, so authorization/identification logic that works against AWS fails against the emulator.
Fix: the recorder ARN construction in the configuration-recorder state/serializer.

Cause 3 of 4 — DescribeConfigRules omits RuleEvaluationVisibility and echoes the custom policy body back (fixture config-config-rule-evaluation-lifecycle, step task#4).
AWS: ConfigRule models RuleEvaluationVisibility (enum RuleEvaluationVisibility, documented as EXTERNAL when Evaluations can be retrieved for the rule, INTERNAL when they cannot) — Smithy: ConfigRule.RuleEvaluationVisibility. The recording shows DescribeConfigRules returning RuleEvaluationVisibility 'EXTERNAL' for a CUSTOM_POLICY rule, and returning Source.CustomPolicyDetails.PolicyText as the empty string (recorded behavior only: AWS does not return the stored guard policy body on this read path).
Emulator: omits RuleEvaluationVisibility entirely and returns the full PolicyText that was supplied to PutConfigRule.
Why: a client that checks RuleEvaluationVisibility before calling GetComplianceDetailsByConfigRule sees None and cannot make the decision AWS supports; and a client that reads PolicyText from DescribeConfigRules gets content on the emulator but an empty string on AWS, so logic built on the emulator (diffing or re-putting the policy text) breaks on the real service.
Fix: the DescribeConfigRules serializer / config-rule state — populate RuleEvaluationVisibility and suppress CustomPolicyDetails.PolicyText on the describe path.

Cause 4 of 4 — GetComplianceSummaryByConfigRule is not implemented (fixture config-resource-tagging-and-compliance-summaries, step task#6).
AWS: GetComplianceSummaryByConfigRule takes no input, has no modeled errors, and returns ComplianceSummary {CompliantResourceCount, NonCompliantResourceCount (each a ComplianceContributorCount with CapExceeded and CappedCount), ComplianceSummaryTimestamp} — Smithy: GetComplianceSummaryByConfigRule, ComplianceSummary. The recording shows HTTP 200 with a ComplianceSummary whose counts are present (CapExceeded false, CappedCount 0) and a ComplianceSummaryTimestamp.
Emulator: answers HTTP 501 with no ComplianceSummary, so the operation is unusable.
Why: any client that polls Config compliance roll-ups fails outright against the emulator; a 501 is not a modeled outcome of this operation, so SDKs surface it as an unretryable protocol error rather than a service error.
Fix: implement the GetComplianceSummaryByConfigRule handler over the config-rule compliance state, returning the modeled ComplianceSummary (zeroed counts with CapExceeded false when nothing is evaluated).

Traces: config-configuration-recorder-and-delivery-channel-lifecycle#task#2 (DescribeConfigurationRecorders), config-configuration-recorder-update-role-arn#task#5 (DescribeConfigurationRecorders ARN), config-config-rule-evaluation-lifecycle#task#4 (DescribeConfigRules), config-resource-tagging-and-compliance-summaries#task#6 (GetComplianceSummaryByConfigRule).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0). None of these clusters carried stub_path or owned_by.

Cause 1 — DescribeConfigurationRecorders, config-configuration-recorder-and-delivery-channel-lifecycle#task#2 (200 on both sides):
  $.ConfigurationRecorders[name="...recorder-e8ffebde"].recordingGroup.exclusionByResourceTypes: missing (expected {'resourceTypes': []}, got None)
  ...recordingGroup.recordingStrategy: missing (expected {'useOnly': 'INCLUSION_BY_RESOURCE_TYPES'}, got None)
  ...recordingMode: missing (expected {'recordingFrequency': 'CONTINUOUS', 'recordingModeOverrides': []}, got None)
  ...recordingScope: missing (expected 'PAID', got None)
  (the same step's .arn comparison is against a fixture placeholder {{recorder_arn}} and is covered by Cause 2 instead)

Cause 2 — DescribeConfigurationRecorders, config-configuration-recorder-update-role-arn#task#5 (200 on both sides):
  $.ConfigurationRecorders[0].arn: value_mismatch
    expected 'arn:aws:config:us-west-2:000000000000:configuration-recorder/sim2real-probe-config-rec-uparn-61bc88ab/mk7tbf6f6cxjsvlz'
    got      'arn:aws:config:us-west-2:000000000000:config-recorder/sim2real-probe-config-rec-uparn-61bc88ab'
  plus the same four missing members (exclusionByResourceTypes, recordingStrategy, recordingMode, recordingScope)

Cause 3 — DescribeConfigRules, config-config-rule-evaluation-lifecycle#task#4 (200 on both sides):
  $.ConfigRules[0]: expected element carries RuleEvaluationVisibility 'EXTERNAL' and Source.CustomPolicyDetails.PolicyText '' ;
  actual element has no RuleEvaluationVisibility and PolicyText "rule tag_check when resourceType == 'AWS::S3::Bucket' {\n  configuration.tags != null\n}"

Cause 4 — GetComplianceSummaryByConfigRule, config-resource-tagging-and-compliance-summaries#task#6:
  status: expected 200, got 501
  $.ComplianceSummary: missing (expected {'ComplianceSummaryTimestamp': <timestamp>, 'CompliantResourceCount': {'CapExceeded': False, 'CappedCount': 0}, 'NonCompliantResourceCount': {'CapExceeded': False, 'CappedCount': 0}}, got None)

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): ConfigurationRecorder{arn,name,roleARN,recordingGroup,recordingMode,recordingScope}; RecordingGroup{...,exclusionByResourceTypes,recordingStrategy}; RecordingMode{recordingFrequency(required),recordingModeOverrides}; RecordingScope enum [INTERNAL,PAID]; ConfigRule.RuleEvaluationVisibility enum; GetComplianceSummaryByConfigRule -> GetComplianceSummaryByConfigRuleResponse.ComplianceSummary, no modeled errors.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: fixture config-configuration-recorder-and-delivery-channel-lifecycle, step task#2 — PutConfigurationRecorder with an explicit resourceTypes recording group, then DescribeConfigurationRecorders; recordingGroup.exclusionByResourceTypes, recordingGroup.recordingStrategy, recordingMode and recordingScope come back absent.
4. Cause 2: fixture config-configuration-recorder-update-role-arn, step task#5 — DescribeConfigurationRecorders; the arn is 'config-recorder/<name>' instead of 'configuration-recorder/<name>/<id>'.
5. Cause 3: fixture config-config-rule-evaluation-lifecycle, step task#4 — PutConfigRule with a CUSTOM_POLICY source, then DescribeConfigRules; RuleEvaluationVisibility is missing and CustomPolicyDetails.PolicyText is echoed back instead of empty.
6. Cause 4: fixture config-resource-tagging-and-compliance-summaries, step task#6 — GetComplianceSummaryByConfigRule returns 501 with no ComplianceSummary.
7. Re-replay all four fixture#step bindings above to confirm the fix.
