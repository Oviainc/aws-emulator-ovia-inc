Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 09acb2538adf9fea9bce04c8d04105f42be236df

# Bootstrap the com-amazonaws-configservice-starlingdoveservice emulator

_Filed: 2026-10-21T21:38:20Z_
_Operation: service-com-amazonaws-configservice-starlingdoveservice | Class: stub_

> _Note: this issue was identified against the code base at commit `09acb2538adf9fea9bce04c8d04105f42be236df`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 102 operations in `src/ops/` is an unmodified scaffold: each file carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))`, i.e. HTTP 501 `NotImplemented`, and `src/state.rs` holds no service data. No client workflow can run.

Implement a FIRST, SMALL, stateful slice: the AWS Config setup-and-rule loop, which is the primary resource lifecycle of this service (wire protocol `awsJson1_1`, `POST /` with `X-Amz-Target: StarlingDoveService.<Operation>`).

### Happy path to implement (in order)

1. **PutConfigurationRecorder** — Input `ConfigurationRecorder` (required; members `name`, `roleARN`, `recordingGroup`, `recordingMode`, `recordingScope`, `arn`), `Tags`. Output has no members. Create-or-update the recorder under its `name` (default the name to `"default"` when absent, as the real service does), assign and persist an `arn` (`arn:aws:config:<region>:<account>:config-recorder/<name>`), and store the recording group/mode verbatim so a later describe returns them. Reject a missing/empty `roleARN` with `InvalidRoleException` and a second distinct recorder name with `MaxNumberOfConfigurationRecordersExceededException` (real Config allows exactly one customer-managed recorder per region).
2. **DescribeConfigurationRecorders** — Input `ConfigurationRecorderNames`, `ServicePrincipal`, `Arn`. Output `ConfigurationRecorders` (list<ConfigurationRecorder>). Must return the recorder(s) read back out of state with the exact `name`, `roleARN`, `recordingGroup`, `recordingMode` and generated `arn` that were persisted — not an echo of the current request. An unknown name in `ConfigurationRecorderNames` must raise `NoSuchConfigurationRecorderException`; an empty store returns an empty list.
3. **PutDeliveryChannel** — Input `DeliveryChannel` (required; members `name`, `s3BucketName`, `s3KeyPrefix`, `s3KmsKeyArn`, `snsTopicARN`, `configSnapshotDeliveryProperties`). Output has no members. Persist by `name`; raise `NoAvailableConfigurationRecorderException` when no recorder exists yet, `InvalidDeliveryChannelNameException` on an empty name, and `MaxNumberOfDeliveryChannelsExceededException` on a second distinct channel. `DescribeDeliveryChannels` should read this same map back (cheap companion).
4. **StartConfigurationRecorder** / **StopConfigurationRecorder** — Input `ConfigurationRecorderName` (required). Output has no members. Flip the persisted `recording` flag and stamp `lastStartTime`/`lastStopTime`/`lastStatus`. Raise `NoSuchConfigurationRecorderException` for an unknown name and `NoAvailableDeliveryChannelException` when no delivery channel has been put yet. **DescribeConfigurationRecorderStatus** must then report `ConfigurationRecordersStatus` with `name`, `arn`, `recording: true/false`, `lastStartTime`, `lastStopTime`, `lastStatus` computed from that stored status record (recording must be `false` before the first start and `true` after it).
5. **PutConfigRule** — Input `ConfigRule` (required; `ConfigRuleName`, `Source` (required), `Description`, `Scope`, `InputParameters`, `MaximumExecutionFrequency`, `EvaluationModes`), `Tags`. Output has no members. Create-or-update the rule keyed by `ConfigRuleName`; on create assign and persist `ConfigRuleArn` (`arn:aws:config:<region>:<account>:config-rule/config-rule-<id>`), `ConfigRuleId`, and `ConfigRuleState = ACTIVE`. Raise `NoAvailableConfigurationRecorderException` when no recorder exists and `InvalidParameterValueException` when `ConfigRule.Source` / `ConfigRuleName` are absent.
6. **DescribeConfigRules** — Input `ConfigRuleNames` (len 0..25), `Filters`, `NextToken`. Output `ConfigRules`, `NextToken`. Return rules from state, filtered by `ConfigRuleNames` when given, each carrying the stored `ConfigRuleArn`/`ConfigRuleId`/`ConfigRuleState` and the inputs as persisted. Unknown requested name → `NoSuchConfigRuleException`; a malformed `NextToken` → `InvalidNextTokenException`. The operation is `@paginated(inputToken=NextToken, outputToken=NextToken, items=ConfigRules)` — page over a stable ordering and only emit `NextToken` when more rules remain.
7. **DeleteConfigRule** — Input `ConfigRuleName` (required). Output has no members. Remove the rule from state so the next `DescribeConfigRules` no longer returns it; unknown name → `NoSuchConfigRuleException`. **DeleteConfigurationRecorder** / `DeleteDeliveryChannel` should likewise remove their entries so the describes go back to empty.

### Rules for this slice

- NEVER STUBS: every one of these operations must compute its response from persisted state. Hardcoded bodies, echoing the request input back, or minting an ARN/ID without storing it are failures — a create followed by a describe must return exactly what was created, including server-generated ARNs/IDs/timestamps.
- Errors must use the modeled shapes above (all HTTP 400 with the `awsJson1_1` error envelope), not a generic 500.
- Everything outside this list keeps returning 501 and will be filed as individual gap issues later (aggregators, conformance packs, organization rules, remediation, evaluations, SelectResourceConfig, resource config history, stored queries, connectors, tagging, …). Do not grow this slice.
- Done = a client script running steps 1→7 against the emulator succeeds end-to-end with correct, stateful bodies (not merely 2xx).

## State Model

Store all of the below in `ComAmazonawsConfigserviceStarlingdoveserviceState` (reached via `crate::state::service_state(&*self.engine)`, mutated under `state.lock()`):

- `recorders: HashMap<String /*name*/, RecorderEntry>` where `RecorderEntry { name: String, arn: String, role_arn: String, recording_group: Option<RecordingGroup>, recording_mode: Option<RecordingMode>, recording_scope: Option<RecordingScope>, service_principal: Option<String>, tags: Vec<Tag> }`
- `recorder_status: HashMap<String /*name*/, RecorderStatusEntry>` where `RecorderStatusEntry { name: String, arn: String, recording: bool, last_start_time: Option<DateTime>, last_stop_time: Option<DateTime>, last_status: Option<RecorderStatus>, last_status_change_time: Option<DateTime> }`
- `delivery_channels: HashMap<String /*name*/, DeliveryChannelEntry>` where `DeliveryChannelEntry { name: String, s3_bucket_name: Option<String>, s3_key_prefix: Option<String>, s3_kms_key_arn: Option<String>, sns_topic_arn: Option<String>, config_snapshot_delivery_properties: Option<ConfigSnapshotDeliveryProperties> }`
- `config_rules: BTreeMap<String /*ConfigRuleName*/, ConfigRuleEntry>` (BTreeMap so pagination ordering is stable) where `ConfigRuleEntry { name: String, arn: String, id: String, description: Option<String>, scope: Option<Scope>, source: Source, input_parameters: Option<String>, maximum_execution_frequency: Option<MaximumExecutionFrequency>, state: ConfigRuleState, evaluation_modes: Vec<EvaluationModeConfiguration>, created_at: DateTime, tags: Vec<Tag> }`
- `account_id: String` and `region: String` (or read from the request context) used to mint the recorder and rule ARNs consistently.

Pagination tokens for `DescribeConfigRules` should be derived from the last returned rule name against the `BTreeMap` ordering, so a token survives inserts/deletes between pages.

## Evidence

All 102 handler files under `services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/` are unmodified scaffolds returning `Scaffolded::not_implemented(..)` (HTTP 501) per the crate README ("Every generated stub starts out scaffolded, not implemented … answers HTTP 501 NotImplemented until you write it"), so no state exists and no workflow can run. Operation contracts cited from `services/service-com-amazonaws-configservice-starlingdoveservice/OPERATIONS.md`: PutConfigurationRecorder (L535-539: input `ConfigurationRecorder` required, errors InvalidConfigurationRecorderNameException/InvalidRoleException/MaxNumberOfConfigurationRecordersExceededException), DescribeConfigurationRecorders (L216-220: output `ConfigurationRecorders`, error NoSuchConfigurationRecorderException), DescribeConfigurationRecorderStatus (L222-226: output `ConfigurationRecordersStatus`), PutDeliveryChannel (L553-557: errors InvalidDeliveryChannelNameException/MaxNumberOfDeliveryChannelsExceededException/NoAvailableConfigurationRecorderException), StartConfigurationRecorder (L645-649: error NoAvailableDeliveryChannelException/NoSuchConfigurationRecorderException), StopConfigurationRecorder (L663-667), PutConfigRule (L523-527: errors NoAvailableConfigurationRecorderException/InvalidParameterValueException), DescribeConfigRules (L195-200: `@paginated(inputToken=NextToken, outputToken=NextToken, items=ConfigRules)`, error InvalidNextTokenException/NoSuchConfigRuleException), DeleteConfigRule (L51-55: error NoSuchConfigRuleException). Member names for the `ConfigurationRecorder` (`arn`, `name`, `roleARN`, `recordingGroup`, `recordingMode`, `recordingScope`, `servicePrincipal`), `ConfigurationRecorderStatus` (`recording`, `lastStartTime`, `lastStopTime`, `lastStatus`) and `ConfigRule` (`ConfigRuleName`, `ConfigRuleArn`, `ConfigRuleId`, `Source` required, `ConfigRuleState`) structures come from `smithy_query` on the model at `services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json`. Wire protocol `awsJson1_1`, target prefix `StarlingDoveService.` (OPERATIONS.md L8-15). No tests cover any of these operations (only the generated `tests/state_roundtrip.rs`).

## Steps to Reproduce

1. Start the emulator and point an AWS Config client at it (awsJson1_1, POST / with X-Amz-Target: StarlingDoveService.<Operation>).
2. PutConfigurationRecorder with ConfigurationRecorder={name: "default", roleARN: "arn:aws:iam::123456789012:role/config-role", recordingGroup: {allSupported: true}} — expect 200 (today: 501).
3. DescribeConfigurationRecorders with no filters — expect one recorder named "default" with the roleARN and recordingGroup just stored plus a server-assigned arn.
4. DescribeConfigurationRecorderStatus — expect name "default" with recording=false and no lastStartTime.
5. PutDeliveryChannel with DeliveryChannel={name: "default", s3BucketName: "config-bucket"} — expect 200; DescribeDeliveryChannels returns it.
6. StartConfigurationRecorder with ConfigurationRecorderName="default" — expect 200; DescribeConfigurationRecorderStatus now reports recording=true with a lastStartTime.
7. PutConfigRule with ConfigRule={ConfigRuleName: "s3-bucket-versioning-enabled", Source: {Owner: "AWS", SourceIdentifier: "S3_BUCKET_VERSIONING_ENABLED"}} — expect 200.
8. DescribeConfigRules with no filters — expect that rule with a server-assigned ConfigRuleArn, ConfigRuleId and ConfigRuleState=ACTIVE; DescribeConfigRules with ConfigRuleNames=["does-not-exist"] raises NoSuchConfigRuleException.
9. DeleteConfigRule with ConfigRuleName="s3-bucket-versioning-enabled" — expect 200; DescribeConfigRules now returns an empty ConfigRules list and a repeat delete raises NoSuchConfigRuleException.
10. StopConfigurationRecorder then DeleteConfigurationRecorder for "default" — expect 200 each; DescribeConfigurationRecorders returns an empty list and StartConfigurationRecorder("default") now raises NoSuchConfigurationRecorderException.
