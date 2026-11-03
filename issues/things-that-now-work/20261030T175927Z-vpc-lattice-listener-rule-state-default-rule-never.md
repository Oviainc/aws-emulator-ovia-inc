Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-listener-rule-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-listener-rule-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|ListRules|2xx->2xx|$.items[*]
Issue-Key: vpc-lattice|CreateRule|2xx->2xx|$.arn
Issue-Key: vpc-lattice|CreateRule|2xx->2xx|$.id
Issue-Key: vpc-lattice|CreateRule|2xx->2xx|$.match.httpMatch.headerMatches
Issue-Key: vpc-lattice|GetRule|2xx->2xx|$.arn
Issue-Related: 20261029T192412Z-vpc-lattice-getrule-returns-stale-rule-state-priority.md
Issue-Commit: aa6eac2cd77e70e1001710733620f31a870d8297

# vpc-lattice listener rule state: default rule never created, empty headerMatches omitted, UpdateRule not persisted

> _Note: this issue was identified against the code base at commit `aa6eac2cd77e70e1001710733620f31a870d8297`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRules

## Description
BUNDLE OF 4 CAUSES in the listener-rule read/write path of one service. All four must be closed before submitting.

RELATION TO AN EXISTING OPEN ISSUE: 20261029T192412Z-vpc-lattice-getrule-returns-stale-rule-state-priority.md reports the headerMatches omission and a stale priority for GetRule in the vpclattice-listener-rule-BATCH-UPDATE fixture, and carries only that cluster's replay key. The four clusters below are reported with an EMPTY owned_by (they are a different fixture and, for causes 1 and 2, different operations), so that issue's replay pointer does not exercise them; they are filed here. Causes 3 and 4 may clear together with it once the projection is normalised, but cause 1 (CreateListener) and cause 2 (CreateRule) are outside its scope entirely.

Cause 1 of 4 — ListRules on a freshly created listener, fixture vpclattice-listener-rule-lifecycle step task#5 (HTTP 200 in both).
AWS: creating a listener materialises that listener's implicit DEFAULT rule, so ListRules (Smithy: GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 200; output `items` -> RuleSummaryList, REQUIRED; RuleSummary{arn,id,name,isDefault,priority,createdAt,lastUpdatedAt}) returns at least one item for a listener that has had no rule created on it: the default rule, with `isDefault` true, `name` "default", `priority` 99999 and an `arn` of the form arn:<partition>:vpc-lattice:<region>:<account>:service/svc-<17 hex>/listener/listener-<17 hex>/rule/rule-<17 hex>. The model documents `isDefault` as "Indicates whether this is the default listener rule" but states no auto-creation rule, so the auto-creation, the name "default" and the priority 99999 are RECORDED BEHAVIOR ONLY: the recorded 200 body carries exactly that single item.
Emulator: answers 200 with an EMPTY `items` list — replay: "$.items[0]: missing (expected {'arn': ... 'isDefault': True, ... 'name': 'default', 'priority': 99999}, got None)". CreateListener stores only the Listener record and never inserts a Rule, so nothing can be listed.
Why: a client enumerating a listener's rules to find or skip the default rule, to pick a free priority, or simply to read items[0], sees a listener with no rules at all; priority allocation can collide with the default rule's 99999 and traffic-routing logic that depends on a default rule being present behaves as if the listener were unconfigured.
Fix: CreateListener's handler plus the rule store in the service's state module — the create must insert the default rule record (isDefault true, name "default", priority 99999, rule ARN nested under the listener) alongside the listener. ListRules' own filtering, ordering and paging are correct.
Traces: vpclattice-listener-rule-lifecycle#task#5; code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_listener.rs:365-380 inserts only a Listener, and src/ops/list_rules.rs:240-245 lists store.rules filtered by listener_id, so the listing is empty by construction.

Cause 2 of 4 — CreateRule, fixture vpclattice-listener-rule-lifecycle step task#6 (HTTP 201 in both). TWO faults on one response.
AWS: CreateRule (Smithy: POST /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 201; output arn/id/name/match/priority/action; `match` -> union RuleMatch{httpMatch -> HttpMatch{method,pathMatch,headerMatches -> HeaderMatchList @length 1..5}}) echoes the created rule's `match` with `httpMatch.headerMatches` serialised as a PRESENT, EMPTY JSON array when the request's match carries no header matches (recorded behavior only — the model states no projection rule; the recorded 201 body has "headerMatches": []). It also returns `id` matching ^rule-<17 hex>$ and an `arn` embedding it.
Emulator: answers 201 but (a) leaves `match.httpMatch.headerMatches` out of the body entirely — "$.match.httpMatch.headerMatches: missing (expected [], got None)" — and (b) returns a base36 rule id, so `$.id` fails its shape assertion and `$.arn` diverges.
Why: (a) an SDK that distinguishes absent from empty surfaces null where AWS guarantees a list, so a caller that takes its length, iterates it, or round-trips the returned match back into UpdateRule/BatchUpdateRule either faults or silently drops the (empty) header-match configuration; (b) the returned rule ARN/id is the handle the caller stores next.
Fix: (a) CreateRule's handler/serializer must normalise an absent headerMatches to an empty list when the match is stored or projected — it currently stores and echoes `input.match` verbatim; (b) the id-minting alphabet in the state module, which the companion issue titled "vpc-lattice mints rule and target-group ids from base36 instead of hex" owns.
Traces: vpclattice-listener-rule-lifecycle#task#6; code: src/ops/create_rule.rs stores input.r#match.clone() unchanged.

Cause 3 of 4 — GetRule, fixture vpclattice-listener-rule-lifecycle step task#7 (HTTP 200 in both).
AWS: GetRule (Smithy: GET .../rules/{ruleIdentifier}, 200) returns the stored rule's `match` with `httpMatch.headerMatches` present as an empty array for a rule configured with no header matches (recorded behavior only, as in cause 2).
Emulator: answers 200 and omits the member — "$.match.httpMatch.headerMatches: missing (expected [], got None)". The cluster also prints $.arn with expected and actual rendered alike because the harness aliases the recorded rule id to the emulator's for display; the substantive difference there is the id alphabet (cause 2b), not the region or account.
Why: same as cause 2a, on the read path a client polls after create.
Fix: the read projection for a rule's match in GetRule's handler, or the normalisation at the point the rule record is persisted (fixing it once at write time covers both causes 2a and 3).
Traces: vpclattice-listener-rule-lifecycle#task#7; code: src/ops/get_rule.rs returns rule.r#match.clone() unchanged.

Cause 4 of 4 — GetRule after UpdateRule, fixture vpclattice-listener-rule-lifecycle step task#12 (HTTP 200 in both).
AWS: UpdateRule (Smithy: PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, 200; input `match`, `priority` -> RulePriority @range 1..2000, `action`; output arn/id/name/isDefault/match/priority/action) PERSISTS the supplied priority, so a following GetRule reports the NEW priority and a refreshed lastUpdatedAt. In the recording the rule read at this step carries the priority the update assigned, not the one it was created with.
Emulator: GetRule still reports the creation-time priority — "$.priority: value_mismatch (expected 15, got 10)", where 15 is the value the update assigned and 10 the value the create assigned; the same response also omits headerMatches. UpdateRule is an unimplemented scaffold (src/ops/update_rule.rs:190 returns Scaffolded::not_implemented("UpdateRule")), so it answers 501 and persists nothing.
Why: a re-prioritisation is a silent no-op. Rule evaluation order never changes, and a client that confirms the change by reading the rule back sees the old priority — it retries forever, reports failure, or proceeds believing an ordering the emulator does not have.
Fix: UpdateRule's handler plus the rule record in the state module — apply the request's priority/match/action onto the stored rule (re-checking the documented rule that each rule of a listener has a unique priority), stamp lastUpdatedAt, and return the updated projection. GetRule already reads from state and needs no change for this cause. The 501 itself is owned by UpdateRule's own unimplemented-stub issue; THIS cause is the observable read divergence the stub leaves behind, which no stub issue carries a replay key for.
Traces: vpclattice-listener-rule-lifecycle#task#12; code: src/ops/update_rule.rs:190.

Values note: 10 and 15 are the priorities this fixture uses and 99999/"default" are the recorded default rule's values; the requirements are that a listener is created with its default rule, that headerMatches is a present empty list when no header match is configured, and that a read-back priority equals the one the last update assigned — not these particular numbers.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator_up true; 110 steps, 53 passed, 57 mismatched, 0 errored, 0 blocked; score 48.2%; 22 divergence clusters). Four net-new vpc-lattice mismatch clusters in fixture vpclattice-listener-rule-lifecycle, each with stub_path empty and owned_by empty:

1. ListRules #task#5 — expected 200, actual 200. Detail verbatim:
   $.items[0]: missing (expected {'arn': 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-2nyustd6302b2cdql/listener/listener-zeqri84hqxgic17bq/rule/rule-097e03279b222843a', 'createdAt': '<shaped-not-asserted>', 'id': '<shaped-not-asserted>', 'isDefault': True, 'lastUpdatedAt': '<shaped-not-asserted>', 'name': 'default', 'priority': 99999}, got None)
2. CreateRule #task#6 — expected 201, actual 201. Detail verbatim:
   $.arn: value_mismatch (expected '...rule/rule-05dfcacbf071a1a5c', got '...rule/rule-birmvby5npf2496f1')
   $.id: value_mismatch (expected '<shaped-not-asserted>', got 'rule-birmvby5npf2496f1')
   $.match.httpMatch.headerMatches: missing (expected [], got None)
3. GetRule #task#7 — expected 200, actual 200. Detail verbatim:
   $.arn: value_mismatch (expected '...rule/rule-birmvby5npf2496f1', got '...rule/rule-birmvby5npf2496f1')  [harness aliases the recorded id for display; the comparison fails on the embedded identifier]
   $.match.httpMatch.headerMatches: missing (expected [], got None)
4. GetRule #task#12 (after UpdateRule) — expected 200, actual 200. Detail verbatim:
   $.arn: value_mismatch (expected/actual rendered alike, as in cluster 3)
   $.match.httpMatch.headerMatches: missing (expected [], got None)
   $.priority: value_mismatch (expected 15, got 10)

Expected (AWS, from the recordings): a listener listing that contains the implicit default rule (isDefault true, name "default", priority 99999); rule responses whose match.httpMatch.headerMatches is a present empty array; and a priority on read equal to the one the preceding UpdateRule assigned.
Actual (emulator): an empty items list; rule bodies with no headerMatches key at all; and the create-time priority on read, because UpdateRule answers 501 and stores nothing.

Model facts used: ListRules http {GET /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 200}, output items -> RuleSummaryList REQUIRED, paginated {items: items}; RuleSummary{arn,id,name,isDefault,priority,createdAt,lastUpdatedAt}; HttpMatch.headerMatches -> HeaderMatchList @length 1..5; CreateRule http {POST .../rules, 201}; GetRule http {GET .../rules/{ruleIdentifier}, 200}; UpdateRule http {PATCH .../rules/{ruleIdentifier}, 200} with priority -> RulePriority @range 1..2000. None of these models the default-rule auto-creation, the empty-list projection or the persistence rule, so those three are recorded behavior only.

Code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_listener.rs:365-380 (listener inserted, no default rule), src/ops/list_rules.rs:240-245 (lists store.rules by listener_id), src/ops/create_rule.rs (stores input.r#match verbatim), src/ops/get_rule.rs (returns rule.r#match verbatim), src/ops/update_rule.rs:190 (Scaffolded::not_implemented("UpdateRule")).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture vpclattice-listener-rule-lifecycle (replay_wire_corpus); representative step task#5, other bundled steps task#6, task#7, task#12.
2. Equivalent by hand against the vpc-lattice endpoint:
   a. POST /services -> note the service id (svc-...).
   b. POST /services/{svcId}/listeners with a name, protocol and defaultAction -> note the listener id (listener-...).
   c. GET /services/{svcId}/listeners/{listenerId}/rules (task#5): `items` is empty, where AWS returns the listener's default rule with isDefault true, name "default" and priority 99999 (Cause 1).
   d. POST /services/{svcId}/listeners/{listenerId}/rules with priority 10, an action, and a match of {"httpMatch":{"pathMatch":{...}}} — i.e. NO headerMatches member (task#6): the 201 body omits match.httpMatch.headerMatches, where AWS returns [], and `id` is rule-<17 non-hex chars> (Cause 2).
   e. GET that rule (task#7): match.httpMatch.headerMatches is again absent (Cause 3).
   f. PATCH /services/{svcId}/listeners/{listenerId}/rules/{ruleId} with {"priority":15}: the emulator answers 501 NotImplemented instead of 200 with the updated rule; then GET the rule again (task#12): `priority` is still 10 where AWS reports 15 (Cause 4).
3. Confirm the fix by re-replaying task#5 and every step listed in trace_ids; all four clusters must come back clean.
