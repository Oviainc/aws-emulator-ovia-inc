Issue-Source: trace-sweep-divergence
Issue-Trace: xray-sampling-rule-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: xray-sampling-rule-lifecycle xray-create-sampling-rule-invalid-fixed-rate-fails xray-sampling-statistics-and-targets
Issue-Shape-Id: com.amazonaws.xray#AWSXRay
Issue-Key: xray|GetSamplingRules|2xx->2xx|$.SamplingRuleRecords[*]
Issue-Key: xray|CreateSamplingRule|2xx->2xx|$.SamplingRuleRecord.SamplingRule.Attributes
Issue-Key: xray|DeleteSamplingRule|2xx->2xx|$.SamplingRuleRecord.SamplingRule.Attributes
Issue-Key: xray|DeleteSamplingRule|2xx->2xx|$.SamplingRuleRecord.SamplingRule.FixedRate
Issue-Key: xray|DeleteSamplingRule|2xx->2xx|$.SamplingRuleRecord.SamplingRule.Priority
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# X-Ray sampling rules: built-in Default rule absent, Attributes map omitted, UpdateSamplingRule not persisted

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSamplingRules

## Description
Three distinct root causes in the X-Ray sampling-rule control plane of services/service-com-amazonaws-xray-awsxray, all from wire fixture replay (trace set simparity-wire-aws-v1, state_reset_failures=0, so no cross-fixture state contamination). None is marked ALREADY FILED or OWNED in the replay report. The open issue 20261022T045628Z-xray-x-ray-account-config-surfaces-are-overly-permissive.md covers neighbouring X-Ray account-config surfaces and carries none of the replay keys below.

Cause 1 of 3: the account's built-in "Default" sampling rule does not exist, so GetSamplingRules never returns it.
AWS: every account is seeded with a service-managed sampling rule named "Default" that GetSamplingRules always returns as a SamplingRuleRecord, with RuleName "Default", RuleARN arn:aws:xray:<region>:<account>:sampling-rule/Default (no generated id suffix), Priority 10000, FixedRate 0.05, ReservoirSize 1, ResourceARN/ServiceName/ServiceType/Host/HTTPMethod/URLPath all "*", Version 1, Attributes {}. It is returned both on an otherwise-empty account and alongside user rules, ordered after the user rule in the recording. Smithy: GetSamplingRules -> GetSamplingRulesResult.SamplingRuleRecords (SamplingRuleRecordList of SamplingRuleRecord{SamplingRule, CreatedAt, ModifiedAt}); the model does not describe the seeded rule, so the Default rule itself is recorded behavior only.
Emulator: GetSamplingRules returns only the rules the caller created — on an account with no user rules SamplingRuleRecords[0] is absent entirely, and after one CreateSamplingRule the response has exactly one record and no SamplingRuleRecords[1].
Why: every X-Ray SDK/agent calls GetSamplingRules at startup and uses the Default rule as its fallback sampling policy; with no Default rule the agent has no catch-all match and either samples nothing or falls back to local rules, and any client that reads the account's effective sampling configuration (or expects Default to be undeletable) sees an empty policy set.
Fix: the sampling-rule state seed for the X-Ray service (the state module that initialises the per-account sampling-rule store at boot) must contain the Default rule, and its ARN must be built without the generated id suffix used for user rules.
Traces: xray-create-sampling-rule-invalid-fixed-rate-fails#task#2 (GetSamplingRules on an account with no user rule: SamplingRuleRecords[0] missing), xray-sampling-rule-lifecycle#task#3 and #task#5 (one user rule present: SamplingRuleRecords[1], the Default record, missing).

Cause 2 of 3: the SamplingRule serializer omits the empty Attributes map instead of emitting {}.
AWS: SamplingRule.Attributes is always present in a serialized SamplingRule, as {} when the rule carries no attributes. It appears in CreateSamplingRule, GetSamplingRules and DeleteSamplingRule responses alike.
Smithy: SamplingRule.Attributes -> AttributeMap, @length min 0 max 5 (a map whose minimum length is 0 — the empty map is a legal, modeled value, not an absent one).
Emulator: $.SamplingRuleRecord.SamplingRule.Attributes is missing (null) on CreateSamplingRule and DeleteSamplingRule, and $.SamplingRuleRecords[*].SamplingRule.Attributes is missing on GetSamplingRules, for a rule created without attributes. HTTP status is 200 and every other SamplingRule member matches.
Why: a client that reads rule.Attributes to match a request against the rule (what the sampling agents do) gets null where AWS gives an empty map, so strongly-typed SDKs and attribute-matching loops hit a null dereference or must special-case the emulator.
Fix: the SamplingRule serializer in the X-Ray service — emit the modeled map even when empty (same for every operation returning a SamplingRuleRecord).
Traces: xray-sampling-rule-lifecycle#task#1 (CreateSamplingRule) and #task#6 (DeleteSamplingRule), xray-sampling-statistics-and-targets#teardown#4 (DeleteSamplingRule), xray-sampling-rule-lifecycle#task#3 (GetSamplingRules).

Cause 3 of 3: UpdateSamplingRule's changes are not persisted, so later reads of the rule return the pre-update values.
AWS: UpdateSamplingRule writes the submitted SamplingRuleUpdate members through to the stored rule; every subsequent operation that returns that rule (GetSamplingRules, DeleteSamplingRule) reflects the updated FixedRate/Priority and a bumped ModifiedAt. Smithy: UpdateSamplingRule(SamplingRuleUpdate) -> SamplingRuleRecord; SamplingRule.FixedRate @range 0..1, Priority @range 1..9999.
Emulator: after a rule is updated to FixedRate 0.5 / Priority 500 (the UpdateSamplingRule call itself replayed as a pass), the following DeleteSamplingRule returns the rule with FixedRate 0.05 and Priority 1000 — the values it had at creation. The update is acknowledged on the wire but dropped from state.
Why: silent loss of a control-plane write. A client that raises a rule's sampling rate believes the change took effect, while the service keeps sampling at the old rate and every read-back contradicts the earlier success response — the classic unobservable-config-drift failure.
Fix: the UpdateSamplingRule handler's write path into the sampling-rule store (it must mutate the stored record, not just the response it serializes) and the ModifiedAt bump that goes with it.
Traces: xray-sampling-rule-lifecycle#task#6 (DeleteSamplingRule echoes pre-update FixedRate/Priority); the same fixture's #task#5 GetSamplingRules is clustered under Cause 1/2 keys and should be re-checked for stale values once the write is fixed.

## Evidence
Cause 1: xray-create-sampling-rule-invalid-fixed-rate-fails#task#2 GetSamplingRules, HTTP 200 vs 200 — `$.SamplingRuleRecords[0]: missing (expected {'CreatedAt': 0.0, 'ModifiedAt': 0.0, 'SamplingRule': {'Attributes': {}, 'FixedRate': 0.05, 'HTTPMethod': '*', 'Host': '*', 'Priority': 10000, 'ReservoirSize': 1, 'ResourceARN': '*', 'RuleARN': 'arn:aws:xray:us-west-2:000000000000:sampling-rule/Default', 'RuleName': 'Default', 'ServiceName': '*', 'ServiceType': '*', 'URLPath': '*', 'Version': 1}}, got None)`. xray-sampling-rule-lifecycle#task#3: `$.SamplingRuleRecords[1]: missing (expected {... 'RuleName': 'Default', 'RuleARN': 'arn:aws:xray:us-west-2:000000000000:sampling-rule/Default', 'Priority': 10000 ...}, got None)` while SamplingRuleRecords[0], the user rule, was returned.
Cause 2: xray-sampling-rule-lifecycle#task#1 CreateSamplingRule 200 vs 200 — `$.SamplingRuleRecord.SamplingRule.Attributes: missing (expected {}, got None)`; identical diff on DeleteSamplingRule at xray-sampling-statistics-and-targets#teardown#4 and xray-sampling-rule-lifecycle#task#6, and on GetSamplingRules at #task#3 (`'Attributes': {}` expected in each record, absent in the emulator's record).
Cause 3: xray-sampling-rule-lifecycle#task#6 DeleteSamplingRule 200 vs 200 — `$.SamplingRuleRecord.SamplingRule.FixedRate: value_mismatch (expected 0.5, got 0.05)` and `$.SamplingRuleRecord.SamplingRule.Priority: value_mismatch (expected 500, got 1000)`; 0.05/1000 are the values the rule was created with at #task#1.

## Steps to Reproduce
1. git_clone, then replay_wire_corpus (trace set simparity-wire-aws-v1); the emulator answers (/__admin/health 200) and bootstrap_signal is false.
2. Cause 1: on a fresh account call GetSamplingRules — expect one record named 'Default' (RuleARN .../sampling-rule/Default, Priority 10000, FixedRate 0.05, ReservoirSize 1, all matchers '*', Version 1); observe an empty SamplingRuleRecords. Repeat after CreateSamplingRule and confirm Default is still returned alongside the user rule. Fixture xray-create-sampling-rule-invalid-fixed-rate-fails step task#2.
3. Cause 2: CreateSamplingRule with no Attributes, then GetSamplingRules and DeleteSamplingRule — inspect SamplingRule.Attributes in each response (expect {}, observe absent/null). Fixture xray-sampling-rule-lifecycle steps task#1, task#3, task#6.
4. Cause 3: CreateSamplingRule (FixedRate 0.05, Priority 1000), UpdateSamplingRule(SamplingRuleUpdate{FixedRate 0.5, Priority 500}) — note it answers 200 — then DeleteSamplingRule for the same rule and read SamplingRuleRecord.SamplingRule (expect 0.5/500, observe 0.05/1000). Fixture xray-sampling-rule-lifecycle step task#6.
5. Re-replay the fixtures/steps named in Issue-Trace/Issue-Step and trace_ids to confirm the fix.
