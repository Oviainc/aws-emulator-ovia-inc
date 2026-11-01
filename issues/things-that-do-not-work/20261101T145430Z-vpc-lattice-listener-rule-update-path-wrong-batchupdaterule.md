Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-listener-rule-batch-update
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-listener-rule-batch-update vpclattice-listener-rule-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|BatchUpdateRule|2xx->2xx|$.unsuccessful[*].failureMessage
Issue-Key: vpc-lattice|GetRule|2xx->2xx|$.priority
Issue-Related: 20261030T175927Z-vpc-lattice-listener-rule-state-default-rule-never.md
Issue-Commit: 7c3ecfa1a25692ce41ad054c5d4a0fbbc15f7c4b

# vpc-lattice listener rule update path: wrong BatchUpdateRule per-entry failure message and UpdateRule priority never persisted

> _Note: this issue was identified against the code base at commit `7c3ecfa1a25692ce41ad054c5d4a0fbbc15f7c4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateRule

## Description
BUNDLE OF 2 CAUSES in the vpc-lattice listener-rule UPDATE path (BatchUpdateRule and UpdateRule). Both must be closed before submitting.

RELATION TO EXISTING OPEN ISSUES: 20261030T175927Z-vpc-lattice-listener-rule-state-default-rule-never.md describes a 'UpdateRule not persisted' cause for the same fixture, but its Issue-Trace-Ids carries only the bare fixture token 'vpclattice-listener-rule-lifecycle' with NO step, so the replay reports this run's GetRule#task#12 cluster with an EMPTY owned_by -- its replay pointer does not exercise this cluster and it is re-filed here as cause 2. 20261029T223134Z-vpc-lattice-batchupdaterule-stub.md (open) is the unimplemented-stub issue for BatchUpdateRule; the operation now answers 200 with both result lists, so cause 1 is a fidelity defect in the implemented handler, not that stub. 20261030T175927Z-vpc-lattice-mints-rule-and-target-group-ids.md owns the id alphabet and is NOT part of this bundle.

Cause 1 of 2: BatchUpdateRule reports an entry that names no rule of the listener with the single-rule 404 message text instead of AWS's batch-entry wording. Fixture vpclattice-listener-rule-batch-update step task#10 (HTTP 200 expected and actual).
AWS: BatchUpdateRule (Smithy: PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, success 200; input rules -> RuleUpdateList @length 1..10; output successful -> RuleUpdateSuccessList, unsuccessful -> RuleUpdateFailureList of RuleUpdateFailure{ruleIdentifier, failureCode, failureMessage}) does not refuse the whole batch for an entry whose ruleIdentifier names no rule of the listener: it reports that entry under `unsuccessful` with a failureMessage whose FIXED text is "Rule with identifier " ... " doesn't exist" and whose single VARIABLE part is the ruleIdentifier exactly as the caller submitted it for that entry. The model types failureMessage only as a string (com.amazonaws.vpclattice#FailureMessage, no constraints, doc "The failure message."), so the wording is RECORDED BEHAVIOR ONLY: the recorded 200 body carries "Rule with identifier rule-0123456789abcdef0 doesn't exist", where rule-0123456789abcdef0 is that run's submitted identifier. Note this is deliberately NOT the wording of the modeled ResourceNotFoundException (message/resourceId/resourceType, HTTP 404), which AWS words "Rule <id> cannot be found." for the single-rule paths -- the batch-entry text is its own template.
Emulator: answers 200 with the entry present under `unsuccessful` but with the 404 exception's text: replay detail "$.unsuccessful[0].failureMessage: value_mismatch (expected \"Rule with identifier rule-0123456789abcdef0 doesn't exist\", got 'Rule rule-0123456789abcdef0 cannot be found.')". The handler builds the per-entry message from the shared not-found construction (crate::state::ResourceKind::Rule.not_found_message(..)), so it reuses the exception wording for a channel that has a different one. failureCode ("ResourceNotFoundException"), ruleIdentifier, the 200 status and the list placement all match.
Why: the batch channel carries no HTTP status or error code a client can branch on, so failureMessage is the only text distinguishing a non-existent rule from any other per-entry refusal. A caller that matches, logs or surfaces AWS's "...doesn't exist" text sees an unrecognised string, so it classifies the entry as an unknown failure, retries it forever or reports a misleading diagnostic to its user.
Fix: BatchUpdateRule's handler -- the per-entry failure construction must use the batch-entry message template rather than the shared exception message; the shared ResourceNotFoundException wording must stay as it is for GetRule/UpdateRule/DeleteRule. Nothing about the status, the failureCode or the request-order placement of the two lists needs to change.
Traces: vpclattice-listener-rule-batch-update#task#10; code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/batch_update_rule.rs:270-274 (failure(..) built from ResourceKind::Rule.not_found_message) and src/ops/batch_update_rule.rs:125-133.

Cause 2 of 2: UpdateRule stores nothing, so GetRule still reports the creation-time priority after a re-prioritisation. Fixture vpclattice-listener-rule-lifecycle step task#12 (HTTP 200 expected and actual).
AWS: UpdateRule (Smithy: PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules/{ruleIdentifier}, success 200; input match -> RuleMatch, priority -> RulePriority @range 1..2000, action -> RuleAction; output arn/id/name/isDefault/match/priority/action) applies the supplied members to the stored rule, so a following GetRule (GET .../rules/{ruleIdentifier}, 200; output includes priority and lastUpdatedAt) reports the priority the UPDATE assigned and a refreshed lastUpdatedAt. The persistence rule is not stated in the model text, so it is RECORDED BEHAVIOR ONLY: in the recording the rule read back at this step carries the updated priority, not the created one.
Emulator: GetRule answers 200 with the create-time priority -- replay detail "$.priority: value_mismatch (expected 15, got 10)", where 15 is the priority the preceding UpdateRule assigned and 10 the one CreateRule assigned. The cause is upstream: UpdateRule is still an unimplemented scaffold that answers 501 NotImplemented and writes nothing to the rule record. (The same cluster also prints "$.arn: value_mismatch" with expected and actual rendered identically -- the harness aliases the recorded rule id for display; that difference is the id alphabet, owned by the mints-ids issue, and is not part of this bundle.)
Why: a re-prioritisation is a silent no-op. Rule evaluation order never changes, and a client that confirms the change by reading the rule back sees the stale priority, so it retries, reports failure, or proceeds believing an ordering the emulator does not have. BatchUpdateRule already writes priority onto the same records, so the two paths disagree about the same state.
Fix: UpdateRule's handler plus the rule record in the service's state module -- apply the request's priority/match/action onto the stored rule (re-checking the documented rule that each rule of a listener holds a unique priority, as BatchUpdateRule's conflict check already does), stamp lastUpdatedAt, and return the updated projection. GetRule already reads from state and needs no change. The bare 501 belongs to UpdateRule's own stub issue; THIS cause is the observable read divergence the stub leaves behind, which no stub issue carries a replay key for.
Traces: vpclattice-listener-rule-lifecycle#task#12; code: src/ops/update_rule.rs:190 (Scaffolded::not_implemented("UpdateRule")), and src/ops/batch_update_rule.rs:353-380 for the write that UpdateRule should perform.

Values note: rule-0123456789abcdef0 is the identifier this fixture submits and 10/15 are the priorities it uses. The requirements are the message TEMPLATE "Rule with identifier <submitted ruleIdentifier> doesn't exist" for a batch entry naming no rule, and that a read-back priority equals the one the last update assigned -- not these particular values.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up true; 110 steps, 59 passed, 51 mismatched, 0 errored, 0 blocked; score 53.6%; 21 divergence clusters; 30 not-implemented operations). Two net-new vpc-lattice mismatch clusters with stub_path empty and owned_by empty:

1. BatchUpdateRule, fixture vpclattice-listener-rule-batch-update, step task#10 -- expected status 200, actual 200. Detail verbatim:
   $.unsuccessful[0].failureMessage: value_mismatch (expected "Rule with identifier rule-0123456789abcdef0 doesn't exist", got 'Rule rule-0123456789abcdef0 cannot be found.')
   divergence_key: service=vpc-lattice|BatchUpdateRule|status=200->200|keys=$.unsuccessful[0].failureMessage

2. GetRule (after UpdateRule), fixture vpclattice-listener-rule-lifecycle, step task#12 -- expected status 200, actual 200. Detail verbatim:
   $.arn: value_mismatch (expected 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-lne7wwsr7kyw43uza/listener/listener-bcv94y414oc3x6v4d/rule/rule-kq0jwp53h79cpiqqx', got 'arn:aws:vpc-lattice:us-west-2:000000000000:service/svc-lne7wwsr7kyw43uza/listener/listener-bcv94y414oc3x6v4d/rule/rule-kq0jwp53h79cpiqqx')  [rendered alike: harness aliases the recorded id; owned by the id-alphabet issue]
   $.priority: value_mismatch (expected 15, got 10)
   divergence_key: service=vpc-lattice|GetRule|status=200->200|keys=$.arn,$.priority

Expected (AWS, from the recordings): a batch entry naming no rule of the listener is reported under `unsuccessful` with failureMessage "Rule with identifier <submitted identifier> doesn't exist"; and a rule read after UpdateRule reports the priority that update assigned.
Actual (emulator): the per-entry failureMessage is the single-rule ResourceNotFoundException text "Rule <id> cannot be found."; and GetRule reports the creation-time priority because UpdateRule answers 501 and persists nothing.

Model facts used: BatchUpdateRule http {PATCH /services/{serviceIdentifier}/listeners/{listenerIdentifier}/rules, 200}, input rules -> RuleUpdateList @length 1..10 required, output unsuccessful -> RuleUpdateFailureList; RuleUpdateFailure{ruleIdentifier, failureCode -> FailureCode (plain string), failureMessage -> FailureMessage (plain string, no constraints)}; ResourceNotFoundException{message, resourceId, resourceType all required, httpError 404}; UpdateRule http {PATCH .../rules/{ruleIdentifier}, 200} with priority -> RulePriority @range 1..2000; GetRule http {GET .../rules/{ruleIdentifier}, 200} with priority and lastUpdatedAt. Neither the batch-entry message wording nor the persistence rule is stated in the model, so both are recorded behavior only.

Code: services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/batch_update_rule.rs:125-133 and :270-274 (per-entry failure message taken from the shared not-found construction), src/ops/update_rule.rs:190 (Scaffolded::not_implemented("UpdateRule")), src/ops/batch_update_rule.rs:353-380 (the equivalent write BatchUpdateRule already performs).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the corpus (replay_wire_corpus). Representative step: fixture vpclattice-listener-rule-batch-update step task#10; the other bundled cause is fixture vpclattice-listener-rule-lifecycle step task#12 (listed in trace_ids).
2. Cause 1 by hand against the vpc-lattice endpoint: create a service, create a listener on it, create one rule; then PATCH /services/{svcId}/listeners/{listenerId}/rules with a `rules` list containing one entry for the real rule and one entry whose ruleIdentifier is a well-formed but non-existent rule id (rule- plus 17 chars). The response is 200 with the bogus entry under `unsuccessful`; its failureMessage reads "Rule <id> cannot be found." where AWS returns "Rule with identifier <id> doesn't exist".
3. Cause 2 by hand: create a rule with priority 10, then PATCH /services/{svcId}/listeners/{listenerId}/rules/{ruleId} with {"priority":15} -- the emulator answers 501 NotImplemented instead of 200 with the updated rule -- then GET that rule: `priority` is still 10 where AWS reports 15 (and lastUpdatedAt is unchanged).
4. Confirm the fix by re-replaying the representative step AND every step named in trace_ids; both clusters must come back clean.
