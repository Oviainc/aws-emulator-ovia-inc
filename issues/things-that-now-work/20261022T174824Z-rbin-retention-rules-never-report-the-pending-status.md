Issue-Source: trace-sweep-divergence
Issue-Trace: rbin-list-rules-pagination-and-filter
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rbin-list-rules-pagination-and-filter rbin-lock-conflict-tag-level-rule rbin-region-level-rule-with-exclude-tags rbin-tag-level-rule-lifecycle rbin-update-and-delete-conflict-when-locked
Issue-Shape-Id: com.amazonaws.rbin#AmazonRecycleBin
Issue-Key: rbin|CreateRule|2xx->2xx|$.Status
Issue-Key: rbin|CreateRule|2xx->2xx|$.Tags
Issue-Key: rbin|UpdateRule|2xx->2xx|$.Status
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# rbin retention rules never report the pending status and CreateRule serializes an empty Tags list

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
TWO root causes in the rbin (Amazon Recycle Bin) rule-record responses. Both show up on successful calls (status matches: 201 for CreateRule, 200 for UpdateRule) and both must be closed before submitting.

Related but NOT covering: the open issue 20261022T161637Z-rbin-retention-rule-fidelity-createrule-skips-the-pending.md describes the missing pending status from the BEHAVIORAL corpus and carries only rbin.* check ids; it names none of the wire fixture steps below and does not mention UpdateRule's Status or the Tags serialization at all, so these causes are recorded here with their own wire replay keys.

Cause 1 of 2: a rule's Status is 'available' immediately and 'pending' is never observable, on CreateRule and on UpdateRule.
AWS: Smithy: CreateRuleResponse.Status and UpdateRuleResponse.Status both target the RuleStatus enum with values [pending, available], documented as "The state of the retention rule. Only retention rules that are in the available state retain resources." Recorded behavior: the CreateRule 201 for a freshly minted rule carries Status 'pending', and an UpdateRule 200 on a rule likewise reports Status 'pending' after the modification, i.e. a rule that has just been created or changed is pending before it reaches available.
Emulator: Status is 'available' unconditionally. services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/create_rule.rs sets `status: "available".to_string()` on the stored Rule (with a comment asserting there is "no `pending` interval here"), and UpdateRule echoes that same stored value, so 'pending' is never returned by any operation. Failing steps: CreateRule — fixture rbin-list-rules-pagination-and-filter step setup#1 (expected Status 'pending', got 'available'); this cluster covers 5 steps across 4 fixtures (rbin-list-rules-pagination-and-filter#setup#1 and #setup#2, rbin-lock-conflict-tag-level-rule#task#1, rbin-region-level-rule-with-exclude-tags#task#1, rbin-tag-level-rule-lifecycle#task#1). CreateRule again — fixture rbin-update-and-delete-conflict-when-locked step setup#1 (Status only). UpdateRule — fixture rbin-tag-level-rule-lifecycle step task#4 (expected Status 'pending', got 'available').
Why: the documented pending->available transition does not exist in the emulator, so a client that waits for available before relying on retention, or that distinguishes "rule created but not yet retaining" from "rule enforcing", is validated against a state machine the real service does not have; the wait loop is never exercised and the client treats a pending AWS rule as enforcing in production.
Fix: the rule status in this crate's state plus the CreateRule and UpdateRule handlers — a freshly created or just-updated rule starts 'pending' and becomes 'available', with GetRule/ListRules reading the transitioned value.
Traces: rbin-list-rules-pagination-and-filter#setup#1 (representative of a 5-step cluster), rbin-update-and-delete-conflict-when-locked#setup#1, rbin-tag-level-rule-lifecycle#task#4.

Cause 2 of 2: CreateRule emits Tags as an empty list when the request carried no tags, instead of omitting the member.
AWS: Smithy: CreateRuleResponse.Tags targets TagList (length 0..200) and is an OPTIONAL member documented as "Information about the tags assigned to the retention rule." Recorded behavior: the CreateRule 201 for a rule created without Tags has NO Tags member in the body at all (replay: expected None).
Emulator: the handler always serializes the member — create_rule.rs builds the output with `tags: Some(stored.tags.clone())` over a `input.tags.unwrap_or_default()` store field, so an untagged rule comes back with `"Tags": []`. Failing step: fixture rbin-list-rules-pagination-and-filter step setup#1 ($.Tags type_mismatch, expected None, got []).
Why: an absent optional member and a present empty collection are different on the wire. A client that tests for the member's presence to decide whether to read tags (or that round-trips the response into a later request) sees a key AWS does not send, and any strict diff of the response against AWS's shape fails.
Fix: the CreateRule handler's response construction in this crate — leave the Tags member unset when the rule has no tags rather than serializing an empty list.
Traces: rbin-list-rules-pagination-and-filter#setup#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (8 fixtures, 38 steps, 19 passed, 19 mismatched; emulator up, 0 state_reset_failures). All steps below returned the EXPECTED status; only body members diverge.

Cause 1:
  rbin-list-rules-pagination-and-filter#setup#1 CreateRule, 201 expected / 201 returned: $.Status value_mismatch (expected 'pending', got 'available'). Cluster affected_count 5: rbin-list-rules-pagination-and-filter#setup#1, rbin-list-rules-pagination-and-filter#setup#2, rbin-lock-conflict-tag-level-rule#task#1, rbin-region-level-rule-with-exclude-tags#task#1, rbin-tag-level-rule-lifecycle#task#1.
  rbin-update-and-delete-conflict-when-locked#setup#1 CreateRule, 201/201: $.Status value_mismatch (expected 'pending', got 'available').
  rbin-tag-level-rule-lifecycle#task#4 UpdateRule, 200/200: $.Status value_mismatch (expected 'pending', got 'available').
  Source: services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/create_rule.rs line 267 — `status: "available".to_string()`, preceded by the comment at lines 265-266 claiming there is no pending interval.

Cause 2:
  rbin-list-rules-pagination-and-filter#setup#1 CreateRule, 201/201: $.Tags type_mismatch (expected None, got []).
  Source: services/service-com-amazonaws-rbin-amazonrecyclebin/src/ops/create_rule.rs line 264 (`tags: input.tags.unwrap_or_default()`) and line 290 (`tags: Some(stored.tags.clone())`).

Model references used: smithy_query shape=CreateRuleResponse,UpdateRuleResponse,RuleStatus on services/service-com-amazonaws-rbin-amazonrecyclebin/smithy-model/com-amazonaws-rbin-amazonrecyclebin.json — RuleStatus values [pending, available]; Tags optional, TagList length 0..200.

## Steps to Reproduce
1. Cause 1 (CreateRule): POST /rules with ResourceType EBS_SNAPSHOT and RetentionPeriod {RetentionPeriodValue: 7, RetentionPeriodUnit: DAYS} -> expect 201 with Status 'pending', then a later GetRule reaching 'available'; observe Status 'available' in the create response. Reproduces on tag-level, Region-level and exclude-tags creates alike (rbin-list-rules-pagination-and-filter#setup#1 and #setup#2, rbin-lock-conflict-tag-level-rule#task#1, rbin-region-level-rule-with-exclude-tags#task#1, rbin-tag-level-rule-lifecycle#task#1, rbin-update-and-delete-conflict-when-locked#setup#1).
2. Cause 1 (UpdateRule): PATCH /rules/{Identifier} on an existing unlocked rule (e.g. change Description or RetentionPeriod) -> expect 200 with Status 'pending'; observe 'available'. (rbin-tag-level-rule-lifecycle#task#4)
3. Cause 2: POST /rules WITHOUT a Tags member -> expect a 201 body with no Tags key; observe "Tags": []. (rbin-list-rules-pagination-and-filter#setup#1)
