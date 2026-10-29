Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-listener-rule-batch-update
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-listener-rule-batch-update
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|GetRule|2xx->2xx|$.match.httpMatch.headerMatches
Issue-Key: vpc-lattice|GetRule|2xx->2xx|$.priority
Issue-Commit: 8d5a4f803a514b21e009b483e221bcf8b3c8087d

# vpc-lattice GetRule returns stale rule state: priority not updated and headerMatches omitted

> _Note: this issue was identified against the code base at commit `8d5a4f803a514b21e009b483e221bcf8b3c8087d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRule

## Description
Reading a listener rule back after it has been created and batch-updated diverges from AWS in two ways on the same response. Both causes are in the one replayed divergence cluster (service=vpc-lattice|GetRule|status=200->200|keys=$.match.httpMatch.headerMatches,$.priority), which covers three GetRule reads in the fixture (task#7, task#9, task#11). THIS ISSUE HAS 2 CAUSES; both must be closed.

Cause 1 of 2: GetRule omits match.httpMatch.headerMatches instead of returning an empty list (fixture vpclattice-listener-rule-batch-update, step task#7).
AWS: GetRule (Smithy: GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, 200; output member `match` -> union RuleMatch with member `httpMatch` -> structure HttpMatch{method, pathMatch, headerMatches: HeaderMatchList @length min 1 max 5}) answers 200 and, for a rule whose HTTP match carries no header matches, serialises `match.httpMatch.headerMatches` as a PRESENT, EMPTY JSON array. The model states no read-time projection rule for this member, so this is recorded behavior only: the recorded 200 response body has `"headerMatches": []` inside `match.httpMatch`.
Emulator: answers 200 but leaves the member out of the body entirely, so `match.httpMatch` has no `headerMatches` key at all (replay: "$.match.httpMatch.headerMatches: missing (expected [], got None)").
Why: a client that reads the list to decide whether header matching is configured — iterating it, taking its length, or round-tripping the match back into BatchUpdateRule/UpdateRule — gets a null/absent member where AWS guarantees a list. SDKs that distinguish absent from empty surface None/null and the caller either faults or re-sends a match that drops the (empty) header-match configuration.
Fix: the read projection for a rule's match — either normalise an absent headerMatches to an empty list where the rule record is persisted, or fill it in when the stored match is projected into the response. Both the create path and the read path hand the match through verbatim, so the normalisation is missing in exactly one of them.
Traces: vpclattice-listener-rule-batch-update#task#7 (also task#9, task#11); code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/get_rule.rs:250 returns `rule.r#match.clone()` unchanged, and src/ops/create_rule.rs:384 stores `input.r#match.clone()` unchanged.

Cause 2 of 2: after a batch rule update, GetRule still reports the creation-time priority (fixture vpclattice-listener-rule-batch-update, step task#7).
AWS: BatchUpdateRule (Smithy: PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 200; input `rules` -> RuleUpdateList @length 1..10 of RuleUpdate{ruleIdentifier required, match, priority -> RulePriority @range 1..2000, action}; doc: "Updates the listener rules in a batch. You can use this operation to change the priority of listener rules. This can be useful when bulk updating or swapping rule priority.") persists each entry reported under `successful`, so a following GetRule on that rule reports the NEW priority supplied in the update (and a refreshed lastUpdatedAt). In the recording the rule read at this step carries the updated priority value, not the value it was created with.
Emulator: BatchUpdateRule is still a scaffold stub that answers 501 NotImplemented and persists nothing, so every later GetRule returns the create-time priority (replay: "$.priority: value_mismatch (expected 20, got 10)" — 20 is the priority the update assigned, 10 the priority the create assigned). The three reads in the fixture all report the pre-update value.
Why: a priority swap or bulk re-prioritisation is a silent no-op. Rule evaluation order never changes, and a client that confirms the swap by reading each rule back sees the old ordering — so it either retries forever, reports the update as failed, or proceeds believing an ordering that the emulator does not have.
Fix: BatchUpdateRule's handler, plus the rule record in the service's state module: apply each RuleUpdate's priority/match/action onto the stored rule (re-checking the documented "each rule for a specific listener must have a unique priority" rule across the whole batch), stamp lastUpdatedAt, and report per-entry `successful`/`unsuccessful`. GetRule itself already reads from state and needs no change for this cause. The operation's own 501 is tracked by its unimplemented-stub issue; this issue is the observable read divergence it leaves behind.
Traces: vpclattice-listener-rule-batch-update#task#7 (also task#9, task#11); code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/batch_update_rule.rs:175.

No existing open issue names vpc-lattice (checked by keyword 'vpc-lattice' / 'vpclattice'), so neither cause is covered elsewhere.

## Evidence
Replay cluster service=vpc-lattice|GetRule|status=200->200|keys=$.match.httpMatch.headerMatches,$.priority — fixture vpclattice-listener-rule-batch-update, representative step task#7, affected steps task#7, task#9, task#11 (affected_count 3), verdict mismatch, expected_status 200, actual_status 200, stub_path empty, owned_by empty.

Detail, verbatim:
  $.match.httpMatch.headerMatches: missing (expected [], got None)
  $.priority: value_mismatch (expected 20, got 10)

Expected (AWS, from the recording): HTTP 200 whose body carries match.httpMatch.headerMatches as a present empty array, and priority equal to the value the preceding BatchUpdateRule assigned to that rule.
Actual (emulator): HTTP 200 whose body omits match.httpMatch.headerMatches entirely and whose priority is still the value CreateRule assigned.

Supporting model/source facts:
  - Smithy HttpMatch.headerMatches -> HeaderMatchList (@length min 1, max 5); GetRule output member `match` -> union RuleMatch { httpMatch: HttpMatch }; GetRule http {GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, code 200}.
  - Smithy BatchUpdateRule http {PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, code 200}; input rules -> RuleUpdateList of RuleUpdate{ruleIdentifier(required), match, priority @range 1..2000, action}; output {successful, unsuccessful}.
  - services/.../src/ops/batch_update_rule.rs:175 — `Err(crate::scaffolded::Scaffolded::not_implemented("BatchUpdateRule"))` (SCAFFOLDED marker at file head), so no update is ever persisted.
  - services/.../src/ops/get_rule.rs:245-255 — output is built straight off the stored record (`r#match: Some(rule.r#match.clone())`, `priority: Some(rule.priority)`), confirming both divergences are state/projection, not serialisation of a recomputed value.

Note on values: 20 and 10 are the two priorities this fixture uses; the requirement is that the priority read back equals the one the batch update assigned, not these particular numbers. The empty array for headerMatches is the requirement; its contents are request-determined.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture vpclattice-listener-rule-batch-update (replay_wire_corpus; the cluster's representative step is task#7, other members task#9 and task#11).
2. Equivalent by hand against the vpc-lattice endpoint:
   a. POST /services  -> note the returned service id (svc-...).
   b. POST /services/{svcId}/listeners with a name, protocol and defaultAction -> note the listener id (listener-...).
   c. POST /services/{svcId}/listeners/{listenerId}/rules with priority 10, an action, and a `match` of {"httpMatch": {"pathMatch": {...}}} — i.e. NO headerMatches member -> note the rule id (rule-...).
   d. GET /services/{svcId}/listeners/{listenerId}/rules/{ruleId} and inspect the body: `match.httpMatch.headerMatches` is absent, where AWS returns [] (Cause 1).
   e. PATCH /services/{svcId}/listeners/{listenerId}/rules with body {"rules":[{"ruleIdentifier":"{ruleId}","priority":20}]} -> emulator answers 501 NotImplemented instead of 200 with the rule under `successful`.
   f. GET the same rule again: `priority` is still 10, where AWS reports 20 (Cause 2).
3. Both deviations are visible on the single GET in step (d)/(f), which is the replayed step task#7.
