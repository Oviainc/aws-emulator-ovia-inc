Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.aggregator-visibility-flow config.config-rule-deploy-triggers-evaluation config.conformance-pack-deploy-compliance-summary config.deploy-conformance-pack-enforces-rules-via-cloudformation config.remediation-auto-remediation-flow config.remediation-exceptions-empty-vs-absent-message-preserved
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.aggregator-visibility-flow|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: config.config-rule-deploy-triggers-evaluation|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: config.conformance-pack-deploy-compliance-summary|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Key: config.deploy-conformance-pack-enforces-rules-via-cloudformation|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: config.remediation-auto-remediation-flow|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Key: config.remediation-exceptions-empty-vs-absent-message-preserved|run#23|Timeout|time.sleep(10.0)
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Config never records or evaluates: a resource created under a running recorder never appears in ListDiscoveredResources and no rule ever produces a compliance verdict

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartConfigurationRecorder

## Description
ONE CAUSE must be closed.

Cause 1 of 1: with a configuration recorder created and started (recordingGroup allSupported) and a delivery channel in place, a resource created afterwards is never discovered, so nothing downstream of the recorded inventory ever happens.
AWS: recorded behavior only — after StartConfigurationRecorder, creating an in-scope resource (an S3 bucket in these checks) makes it observable through Config's read surface within the checks' budgets: ListDiscoveredResources(resourceType="AWS::S3::Bucket", resourceIds/resourceName=<bucket>) returns a resourceIdentifiers entry, and a managed rule scoped to it then reports a terminal verdict (DescribeComplianceByConfigRule / DescribeConformancePackCompliance report COMPLIANT or NON_COMPLIANT rather than staying empty or INSUFFICIENT_DATA). Model grounding only confirms the shapes involved (ListDiscoveredResources.resourceIdentifiers; ComplianceType enum COMPLIANT | NON_COMPLIANT | NOT_APPLICABLE | INSUFFICIENT_DATA).
Emulator: the recorder is created and reports recording, but the inventory stays empty: every check's first poll — ListDiscoveredResources for the bucket it just created — never satisfies its predicate and the check runs to its deadline ("Check timed out after 60.0s"). The same emptiness blocks the dependent polls: no rule compliance verdict, no conformance-pack rule row, no auto-remediation execution.
Why: the recorded inventory is the substrate of the whole service. A client cannot distinguish "not recorded yet" from "recording is not implemented", every compliance verdict, aggregator view, conformance-pack summary and auto-remediation trigger is permanently absent, and any code that waits for Config to observe a resource hangs instead of failing.
Fix: the configuration-recorder state machine — StartConfigurationRecorder must begin producing configuration items for in-scope resource types into Config's recorded state, and the rule-evaluation path must consume them to produce terminal verdicts (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/start_configuration_recorder.rs, the recorder/inventory state module, list_discovered_resources.rs and the rule-evaluation driver).
Related: open issue 20261022T190340Z-config-has-no-recording-delivery-side-effects-and.md records the same absence reached through CloudFormation-provisioned recorders and through the S3 delivery leg (its Causes 2 and 3). The six checks listed here are not carried by that issue's replay keys: they create the recorder directly through Config's own API, so the defect is reproducible without CloudFormation and must be closed for these checks too.
Traces: config.aggregator-visibility-flow, config.config-rule-deploy-triggers-evaluation, config.conformance-pack-deploy-compliance-summary, config.deploy-conformance-pack-enforces-rules-via-cloudformation, config.remediation-auto-remediation-flow, config.remediation-exceptions-empty-vs-absent-message-preserved.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed).

All six checks end with: Error: Check timed out after 60.0s.

The polls that never satisfy (from the checks' own source):
  config.aggregator-visibility-flow: _poll_until(cfg.list_discovered_resources(resourceType="AWS::S3::Bucket", resourceIds=[bucket]), ...) then the aggregator's discovered-resources view.
  config.config-rule-deploy-triggers-evaluation: the same ListDiscoveredResources poll ("Wait for the recorder to discover both buckets before evaluating"), then _poll_until(compliance in ("COMPLIANT","NON_COMPLIANT")).
  config.conformance-pack-deploy-compliance-summary and config.deploy-conformance-pack-enforces-rules-via-cloudformation: the same ListDiscoveredResources poll, then ConformancePackState/ConformancePackRuleComplianceList.
  config.remediation-auto-remediation-flow: the same ListDiscoveredResources poll, then _first_compliance in ("COMPLIANT","NON_COMPLIANT").
  config.remediation-exceptions-empty-vs-absent-message-preserved: waits for the bucket to be evaluated NON_COMPLIANT under S3_BUCKET_VERSIONING_ENABLED ("bucket was never evaluated NON_COMPLIANT").

No exception is raised by the probes — ListDiscoveredResources answers successfully with an empty resourceIdentifiers list, so the inventory is reachable and simply never populated.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. config put-configuration-recorder (recordingGroup allSupported) + config put-delivery-channel, then config start-configuration-recorder.
3. s3 create-bucket <bucket>.
4. Poll config list-discovered-resources --resource-type AWS::S3::Bucket --resource-ids <bucket> — resourceIdentifiers stays empty indefinitely.
5. config put-config-rule with Source {Owner: AWS, SourceIdentifier: S3_BUCKET_VERSIONING_ENABLED}, then poll config describe-compliance-by-config-rule — no COMPLIANT/NON_COMPLIANT verdict ever appears.
6. Replay simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids; each times out on one of these polls.
