Issue-Source: trace-sweep-divergence
Issue-Trace: dlm-ebs-snapshot-policy-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dlm-ebs-snapshot-policy-lifecycle dlm-create-policy-invalid-description-characters-rejected dlm-create-policy-with-multiple-schedules-round-trip dlm-cross-region-copy-policy dlm-get-lifecycle-policies-filter-by-target-tags dlm-list-policies-filtering dlm-image-management-policy dlm-update-policy-error-paths
Issue-Shape-Id: com.amazonaws.dlm#dlm_20180112
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.DateCreated
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.DateModified
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.PolicyLanguage
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.ResourceLocations
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.Schedules[*].CreateRule.Location
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.Schedules[*].RetainRule.Interval
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.StatusMessage
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.Schedules[*].CopyTags
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.Tags
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.Schedules[*].CrossRegionCopyRules[*].CopyTags
Issue-Key: dlm|GetLifecyclePolicies|2xx->2xx|$.Policies[*].Tags
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.Schedules[*].CreateRule.Interval
Issue-Key: dlm|GetLifecyclePolicy|2xx->2xx|$.Policy.PolicyDetails.Schedules[*]
Issue-Commit: a95ddc9c00c4f94c9371e90406864e89cbfb6e44

# DLM policy read responses omit defaulted fields and serialize timestamps as epoch numbers

> _Note: this issue was identified against the code base at commit `a95ddc9c00c4f94c9371e90406864e89cbfb6e44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLifecyclePolicy

## Description
The DLM read path (GetLifecyclePolicy, GetLifecyclePolicies) builds its response from only the members the client literally sent, so every server-defaulted/derived member of LifecyclePolicy and PolicyDetails is dropped, and the two timestamp members are emitted in the wrong wire form. There are 7 distinct causes in this bundle; all must be closed.

Cause 1 of 7 - GetLifecyclePolicy drops defaulted policy/schedule members (representative cause).
AWS: Smithy (com.amazonaws.dlm, restJson1) models LifecyclePolicy.StatusMessage, LifecyclePolicy.Tags (TagMap), PolicyDetails.ResourceLocations (ResourceLocationList, length 1..1), PolicyDetails.PolicyLanguage (PolicyLanguageValues, STANDARD for a custom policy), Schedule.CopyTags (CopyTags boolean, doc default false), CreateRule.Location (LocationValues, doc "Default: CLOUD") and RetainRule.Interval (StandardTierRetainRuleInterval, range min 0). AWS materializes these server-side defaults and echoes them back on a read, as the recording shows: StatusMessage 'ENABLED', ResourceLocations ['CLOUD'], PolicyLanguage 'STANDARD', Tags {}, Schedules[0].CopyTags false, Schedules[0].CreateRule.Location 'CLOUD', Schedules[0].RetainRule.Interval 0.
Emulator: returns 200 with all of those members absent (null) - it only serializes what CreateLifecyclePolicy received.
Why: a client that round-trips a policy (read, mutate, write) loses the location, retention interval, copy-tags and policy-language settings, and code that reads StatusMessage/State to decide whether a policy is live sees nothing. Clients using a strict deserializer for a non-sparse list see a missing ResourceLocations where the model requires 1 element.
Fix: the GetLifecyclePolicy handler's response projection plus the policy-creation defaulting in DLM state - the defaults must be stamped onto the stored policy at create time, not synthesized per read.
Traces: dlm-ebs-snapshot-policy-lifecycle#task#3 (same cluster also fails at task#6).

Cause 2 of 7 - DateCreated/DateModified serialized as epoch seconds instead of date-time.
AWS: Smithy LifecyclePolicy.DateCreated and .DateModified target Timestamp with timestampFormat date-time, i.e. an ISO-8601 string on the wire ('2026-09-06T00:43:31.236Z' in the recording).
Emulator: emits a JSON number of epoch seconds with fractional part (e.g. 1792662276.228).
Why: an AWS SDK deserializing a date-time member off a bare number either errors or yields a wrong instant, so any client that reads policy creation/modification time breaks. The exact instants are environment-specific and are not the requirement - the encoding is.
Fix: the DLM response serializer's timestamp encoding for these two members.
Traces: dlm-cross-region-copy-policy#task#2 (expected ISO strings, got epoch numbers); the same encoding fault rides on every GetLifecyclePolicy cluster in this bundle.

Cause 3 of 7 - round-trip read after a create that should have been rejected.
AWS: recorded behavior only - after the invalid-description create is rejected, the surviving valid policy reads back with the full defaulted projection (StatusMessage, ResourceLocations, PolicyLanguage, Tags {}, Schedules[0].CopyTags/CreateRule.Location/RetainRule.Interval).
Emulator: 200 with each of those members null and the timestamps as epoch numbers.
Why/Fix: as Cause 1.
Traces: dlm-create-policy-invalid-description-characters-rejected#task#4.

Cause 4 of 7 - multi-schedule policies drop the defaults on EVERY schedule.
AWS: the defaulting in Cause 1 applies per element of PolicyDetails.Schedules (ScheduleList, length 1..4); the recording shows Schedules[0] and Schedules[1] each carrying CopyTags false, CreateRule.Location 'CLOUD', RetainRule.Interval 0.
Emulator: both elements come back with those members null, so the omission is not limited to the first schedule.
Why: a client reading a 2+ schedule policy cannot tell where any schedule writes its snapshots.
Fix: the per-schedule defaulting in DLM state / the schedule serializer (it must map over the list, not just the head).
Traces: dlm-create-policy-with-multiple-schedules-round-trip#task#3.

Cause 5 of 7 - cross-region copy rules lose CopyTags.
AWS: Schedule.CrossRegionCopyRules elements carry CopyTags; the recording shows CrossRegionCopyRules[0].CopyTags true echoed back exactly as submitted.
Emulator: 200 with CrossRegionCopyRules[0].CopyTags absent.
Why: this one is a value the client SENT, so the emulator is losing submitted state, not just a default - a client cannot verify its own cross-region copy configuration.
Fix: the CrossRegionCopyRule branch of the DLM policy serializer / state storage.
Traces: dlm-cross-region-copy-policy#task#2.

Cause 6 of 7 - GetLifecyclePolicies summaries omit Tags.
AWS: the LifecyclePolicySummaryList elements returned by GetLifecyclePolicies (GET /policies) carry Tags; the recording shows Policies[0].Tags {} for an untagged policy - an empty map, not an absent member.
Emulator: 200 with Policies[0].Tags absent.
Why: a client filtering or displaying policies from the list call must issue a second per-policy read to learn that a policy has no tags, and cannot distinguish "no tags" from "not reported".
Fix: the GetLifecyclePolicies handler's summary projection.
Traces: dlm-get-lifecycle-policies-filter-by-target-tags#task#5 (same cluster also fails dlm-list-policies-filtering#task#4 and #task#6).

Cause 7 of 7 - IMAGE_MANAGEMENT policies show the same omissions, including CreateRule.Interval.
AWS: for an AMI policy the recording additionally shows Schedules[0].CreateRule.Interval 0 and RetainRule.Interval 0 present, plus the Cause 1 set.
Emulator: 200 with all of them null, so the defect is not specific to EBS_SNAPSHOT_MANAGEMENT.
Why/Fix: as Cause 1, in whichever policy-type branch the handler uses.
Traces: dlm-image-management-policy#task#3.

Note on dlm-update-policy-error-paths#task#8 (listed in trace_ids): besides the Cause 1 omissions it shows PolicyDetails.Schedules[0] holding the schedule from an UpdateLifecyclePolicy call that AWS rejected ('BadUpdateSched' with both Interval and CronExpression) instead of the previously stored 'UpdateErrSched'. The read-projection omissions on that step belong to this issue; the fact that a rejected update was persisted is the missing-validation defect filed as the companion DLM error-path issue.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up true, bootstrap_signal false, state_reset_failures 0; 56/77 steps passed).

dlm-ebs-snapshot-policy-lifecycle#task#3 GetLifecyclePolicy, expected 200 / actual 200:
  $.Policy.StatusMessage: expected 'ENABLED', got None
  $.Policy.PolicyDetails.ResourceLocations: expected ['CLOUD'], got None
  $.Policy.PolicyDetails.PolicyLanguage: expected 'STANDARD', got None
  $.Policy.PolicyDetails.Schedules[0].CreateRule.Location: expected 'CLOUD', got None
  $.Policy.PolicyDetails.Schedules[0].RetainRule.Interval: expected 0, got None
  $.Policy.DateCreated / .DateModified: got 1792662276.9223416 (epoch number) where a date-time string is expected

dlm-cross-region-copy-policy#task#2 GetLifecyclePolicy, 200/200:
  $.Policy.DateCreated: expected '2026-09-06T00:43:31.236Z', got 1792662276.2280343
  $.Policy.DateModified: expected '2026-09-06T00:43:31.339Z', got 1792662276.2280343
  $.Policy.PolicyDetails.Schedules[0].CrossRegionCopyRules[0].CopyTags: expected True, got None
  plus StatusMessage/ResourceLocations/PolicyLanguage/Tags/CopyTags/CreateRule.Location/RetainRule.Interval all None

dlm-create-policy-with-multiple-schedules-round-trip#task#3 GetLifecyclePolicy, 200/200:
  Schedules[0] and Schedules[1] each: CopyTags expected False got None, CreateRule.Location expected 'CLOUD' got None, RetainRule.Interval expected 0 got None

dlm-image-management-policy#task#3 GetLifecyclePolicy, 200/200:
  Schedules[0].CreateRule.Interval expected 0 got None; Schedules[0].RetainRule.Interval expected 0 got None; plus the common set

dlm-create-policy-invalid-description-characters-rejected#task#4 GetLifecyclePolicy, 200/200: common omission set + epoch timestamps

dlm-get-lifecycle-policies-filter-by-target-tags#task#5 GetLifecyclePolicies, 200/200:
  $.Policies[0].Tags: expected {}, got None (cluster also covers dlm-list-policies-filtering#task#4, #task#6)

dlm-update-policy-error-paths#task#8 GetLifecyclePolicy, 200/200:
  $.Policy.StatusMessage expected 'DISABLED' got None; PolicyDetails.ResourceLocations/PolicyLanguage/Tags None; Schedules[0] holds {'Name': 'BadUpdateSched', ...} instead of {'Name': 'UpdateErrSched', ...}

Model citations: Smithy com.amazonaws.dlm#LifecyclePolicy (StatusMessage, Tags, DateCreated/DateModified timestampFormat date-time), #PolicyDetails (ResourceLocations length 1..1, PolicyLanguage, Schedules length 1..4), #Schedule (CopyTags, CrossRegionCopyRules), #CreateRule (Location default CLOUD, Interval), #RetainRule (Interval range min 0); protocol restJson1.

## Steps to Reproduce
1. Replay trace set simparity-wire-aws-v1 against the emulator.
2. Representative: fixture dlm-ebs-snapshot-policy-lifecycle, step task#3 - CreateLifecyclePolicy with an EBS_SNAPSHOT_MANAGEMENT PolicyDetails, then GET /policies/{PolicyId}. The 200 body omits Policy.StatusMessage, Policy.Tags, PolicyDetails.ResourceLocations, PolicyDetails.PolicyLanguage, Schedules[0].CreateRule.Location and Schedules[0].RetainRule.Interval, and Policy.DateCreated/DateModified are JSON numbers rather than ISO-8601 strings.
3. Repeat for each bundled cause: dlm-create-policy-invalid-description-characters-rejected#task#4, dlm-create-policy-with-multiple-schedules-round-trip#task#3 (check BOTH Schedules elements), dlm-cross-region-copy-policy#task#2 (check CrossRegionCopyRules[0].CopyTags and the ISO timestamps), dlm-get-lifecycle-policies-filter-by-target-tags#task#5 (GET /policies, Policies[0].Tags must be {}), dlm-image-management-policy#task#3 (CreateRule.Interval), dlm-update-policy-error-paths#task#8.
4. All seven must pass before the issue is closed.
