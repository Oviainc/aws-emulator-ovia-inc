Issue-Source: trace-sweep-divergence
Issue-Trace: iotdeviceadvisor-suite-definition-version-read-serves-current-config
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdeviceadvisor-suite-definition-version-read-serves-current-config iotdeviceadvisor-tag-resource-list-tags-roundtrip iotdeviceadvisor-untag-resource-removes-tags
Issue-Shape-Id: com.amazonaws.iotdeviceadvisor#IotSenateService
Issue-Key: iotdeviceadvisor|UpdateSuiteDefinition|2xx->2xx|$.suiteDefinitionArn
Issue-Key: iotdeviceadvisor|GetSuiteDefinition|2xx->2xx|$.suiteDefinitionConfiguration.devices
Issue-Key: iotdeviceadvisor|GetSuiteDefinition|2xx->2xx|$.suiteDefinitionConfiguration.isLongDurationTest
Issue-Key: iotdeviceadvisor|GetSuiteDefinition|2xx->2xx|$.suiteDefinitionConfiguration.protocol
Issue-Key: iotdeviceadvisor|GetSuiteDefinition|2xx->2xx|$.suiteDefinitionConfiguration.rootGroup
Issue-Key: iotdeviceadvisor|GetSuiteDefinition|2xx->2xx|$.tags
Issue-Key: iotdeviceadvisor|TagResource|2xx->2xx|status
Issue-Key: iotdeviceadvisor|UntagResource|2xx->2xx|status
Issue-Commit: f5ba8024c64455531a4bd3d9779b46ec0fe104e4

# Suite-definition and tag responses diverge from AWS: extra ARN, dropped configuration defaults, 200 instead of 204

> _Note: this issue was identified against the code base at commit `f5ba8024c64455531a4bd3d9779b46ec0fe104e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateSuiteDefinition

## Description
Four net-new wire divergences in the iotdeviceadvisor emulator, all of the same family: the response the emulator serves for a Device Advisor call is not the response AWS serves — a member AWS does not send is added, the members AWS always sends on a definition read are dropped, and the two tag mutations answer the wrong success status. FOUR causes; all four must be closed before this issue is submitted.

Cause 1 of 4 — UpdateSuiteDefinition emits a suiteDefinitionArn AWS does not send (fixture iotdeviceadvisor-suite-definition-version-read-serves-current-config, step task#3).
AWS: Smithy: UpdateSuiteDefinition PATCH /suiteDefinitions/{suiteDefinitionId}, output UpdateSuiteDefinitionResponse; every output member (suiteDefinitionId, suiteDefinitionArn [AmazonResourceName, @length 20..2048], suiteDefinitionName, suiteDefinitionVersion, createdAt, lastUpdatedAt) is OPTIONAL — the model does not require any of them on the wire. Recorded behavior only: the recorded 200 response for this update carries NO suiteDefinitionArn member at all; the update reply identifies the definition by id/name/version and timestamps, leaving the ARN to the read operations (GetSuiteDefinition/ListSuiteDefinitions). Hypothesis: Device Advisor serializes this reply without the ARN member.
Emulator: answers 200 but adds $.suiteDefinitionArn, a suitedefinition ARN built from the request region, account and the suiteDefinitionId. Replay reports type_mismatch at $.suiteDefinitionArn: expected the member to be absent, got a suitedefinition ARN value.
Why: a client that treats presence of suiteDefinitionArn as a signal (for example, round-tripping only the fields AWS returns, or diffing an update reply against a recorded one) sees a field AWS never sends; response-shape assertions and golden-response comparisons fail against the emulator even though the update itself succeeded.
Fix: the UpdateSuiteDefinition handler's output construction / serializer — it must stop populating the ARN member for this operation (services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/ops/update_suite_definition.rs, output built at the end of handle, suite_definition_arn set from the stored record).

Cause 2 of 4 — GetSuiteDefinition serves a stripped suiteDefinitionConfiguration and omits tags (same fixture, steps task#4 and task#5).
AWS: Smithy: GetSuiteDefinition GET /suiteDefinitions/{suiteDefinitionId}?suiteDefinitionVersion=..., output members suiteDefinitionConfiguration (SuiteDefinitionConfiguration) and tags (TagMap, @length 0..50). SuiteDefinitionConfiguration documents devices (DeviceUnderTestList, @length 0..2), isLongDurationTest (boolean), protocol (enum Protocol), rootGroup (@length 0..2048, required) and states the rootGroup rule: "if intendedForQualification is set to true, rootGroup can be an empty string ... If rootGroup is empty, and intendedForQualification is set to true, all the qualification tests are included, and the configuration is default." Recorded behavior: AWS answers the read with the NORMALIZED stored configuration — devices present as an empty list, isLongDurationTest present as false, protocol present (MqttV3_1_1 in the recording, i.e. the service's resolved protocol value), rootGroup present and NON-EMPTY (the materialized default qualification root group, not the empty string the caller sent), and tags present as an empty map for an untagged definition. Hypothesis: the service fills these optional members with their resolved defaults when it stores the configuration, so a read always echoes them.
Emulator: answers 200 but omits $.suiteDefinitionConfiguration.devices (expected [], got absent), omits $.suiteDefinitionConfiguration.isLongDurationTest (expected false, got absent), omits $.suiteDefinitionConfiguration.protocol (expected MqttV3_1_1, got absent), serves $.suiteDefinitionConfiguration.rootGroup as the empty string "" where AWS serves a non-empty root group, and omits $.tags entirely (expected {}, got absent — crate::state tags_of returns None for a resource with no tag row, so the member is dropped instead of serialized as an empty map).
Why: a client reading back a definition it just wrote gets None/null where AWS gives a value, so unwrapping config.devices, config.protocol, config.isLongDurationTest or response.tags panics or takes a "not configured" branch; an empty rootGroup also makes a qualification suite read as having no test group, so tooling that re-submits the read configuration to UpdateSuiteDefinition is then refused by the emulator's own rootGroup validation.
Fix: the suite-definition state record plus the GetSuiteDefinition reader — normalize the configuration on write (fill devices, isLongDurationTest, protocol, and materialize the default root group when intendedForQualification is true and rootGroup is empty) and serialize tags as an empty map rather than dropping the member (services/service-com-amazonaws-iotdeviceadvisor-iotsenateservice/src/state.rs SuiteDefinition::record_version and tags_of; src/ops/get_suite_definition.rs output construction).

Cause 3 of 4 — TagResource answers 200 instead of 204 (fixture iotdeviceadvisor-tag-resource-list-tags-roundtrip, step task#3).
AWS: Smithy: TagResource POST /tags/{resourceArn}, output TagResourceResponse has NO members; the model carries no explicit success code, so 200 is only the protocol default. Recorded behavior only: AWS answers a successful TagResource with HTTP 204 and no content.
Emulator: answers HTTP 200 (replay: "status: expected 204, got 200").
Why: a client or proxy that branches on the status of an empty-bodied mutation (204 == applied, no content to parse) sees 200 and tries to parse a body, and status assertions in recorded-contract tests fail on an otherwise correct tagging.
Fix: the TagResource handler's success status (SUCCESS_STATUS on the operation impl) — it must be 204 for this empty-output operation. Unrelated open issue 20261028T223907Z-iotdeviceadvisor-tagresource-wrong-behavior.md concerns the audit outcome recorded for a no-op tagging and does not cover this status divergence.

Cause 4 of 4 — UntagResource answers 200 instead of 204 (fixture iotdeviceadvisor-untag-resource-removes-tags, step task#2).
AWS: Smithy: UntagResource DELETE /tags/{resourceArn}?tagKeys=..., output UntagResourceResponse has NO members; again no explicit success code in the model. Recorded behavior only: AWS answers a successful UntagResource with HTTP 204 and no content.
Emulator: answers HTTP 200 (replay: "status: expected 204, got 200").
Why: same client impact as cause 3 — an empty-bodied untag reads as a 200 with a body to parse, so status-driven clients and recorded-contract checks diverge from AWS.
Fix: the UntagResource handler's success status (SUCCESS_STATUS on the operation impl) must be 204.

Traces: iotdeviceadvisor-suite-definition-version-read-serves-current-config#task#3 (cause 1), iotdeviceadvisor-suite-definition-version-read-serves-current-config#task#4 and #task#5 (cause 2), iotdeviceadvisor-tag-resource-list-tags-roundtrip#task#3 (cause 3), iotdeviceadvisor-untag-resource-removes-tags#task#2 (cause 4).

## Evidence
Replay of trace set simparity-wire-aws-v1 (16 steps, 11 passed, 5 mismatched, score 68.8%), all five mismatches on iotdeviceadvisor:

Cause 1 — UpdateSuiteDefinition, fixture iotdeviceadvisor-suite-definition-version-read-serves-current-config step task#3: expected status 200, actual 200; detail "$.suiteDefinitionArn: type_mismatch (expected None, got 'arn:aws:iotdeviceadvisor:<region>:<account>:suitedefinition/<id>')". Expected: member absent. Actual: member present with a suitedefinition ARN.

Cause 2 — GetSuiteDefinition, same fixture step task#4 (cluster also covers task#5): expected 200, actual 200; detail:
  $.suiteDefinitionConfiguration.devices: missing (expected [], got None)
  $.suiteDefinitionConfiguration.isLongDurationTest: missing (expected False, got None)
  $.suiteDefinitionConfiguration.protocol: missing (expected 'MqttV3_1_1', got None)
  $.suiteDefinitionConfiguration.rootGroup: value_mismatch (expected a non-empty shaped root group, got '')
  $.tags: missing (expected {}, got None)

Cause 3 — TagResource, fixture iotdeviceadvisor-tag-resource-list-tags-roundtrip step task#3: "status: expected 204, got 200".

Cause 4 — UntagResource, fixture iotdeviceadvisor-untag-resource-removes-tags step task#2: "status: expected 204, got 200".

Model evidence: UpdateSuiteDefinitionResponse members are all optional; SuiteDefinitionConfiguration declares devices/isLongDurationTest/protocol/rootGroup and documents the empty-rootGroup-with-intendedForQualification default-configuration rule; GetSuiteDefinitionResponse declares tags (TagMap @length 0..50); TagResourceResponse and UntagResourceResponse declare no members and no explicit success code.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus); all five mismatched steps are iotdeviceadvisor.
2. Cause 1 and 2: replay fixture iotdeviceadvisor-suite-definition-version-read-serves-current-config. Create a suite definition, then PATCH /suiteDefinitions/{suiteDefinitionId} (UpdateSuiteDefinition) with a configuration whose rootGroup is empty and intendedForQualification true (step task#3) — the reply contains suiteDefinitionArn, which the recorded AWS reply omits. Then GET /suiteDefinitions/{suiteDefinitionId} and GET it with ?suiteDefinitionVersion=<an earlier label> (steps task#4, task#5) — suiteDefinitionConfiguration.devices, .isLongDurationTest and .protocol are absent, .rootGroup is "", and tags is absent, where AWS returns [], false, the resolved protocol, a non-empty default root group and {}.
3. Cause 3: replay fixture iotdeviceadvisor-tag-resource-list-tags-roundtrip and POST /tags/{resourceArn} with a tag map (step task#3) — the emulator answers 200 where AWS answers 204.
4. Cause 4: replay fixture iotdeviceadvisor-untag-resource-removes-tags and DELETE /tags/{resourceArn}?tagKeys=<key> (step task#2) — the emulator answers 200 where AWS answers 204.
