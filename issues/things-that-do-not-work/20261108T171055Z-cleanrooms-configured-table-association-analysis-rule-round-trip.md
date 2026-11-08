Issue-Source: trace-sweep-divergence
Issue-Trace: cleanrooms-configured-table-association-analysis-rule-round-trip
Issue-Step: task#11
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cleanrooms-configured-table-association-analysis-rule-round-trip
Issue-Shape-Id: com.amazonaws.cleanrooms#AWSBastionControlPlaneServiceLambda
Issue-Key: cleanrooms|CreateConfiguredTableAssociationAnalysisRule|2xx->4xx|$.analysisRule
Issue-Key: cleanrooms|CreateConfiguredTableAssociationAnalysisRule|2xx->4xx|status
Issue-Key: cleanrooms|GetConfiguredTableAssociationAnalysisRule|2xx->4xx|$.analysisRule
Issue-Key: cleanrooms|GetConfiguredTableAssociationAnalysisRule|2xx->4xx|status
Issue-Commit: c0b127c099d8543a6fa74c4b10d6d49cf92e859e

# cleanrooms configured-table-association analysis-rule round trip answers 404 instead of serving the rule

> _Note: this issue was identified against the code base at commit `c0b127c099d8543a6fa74c4b10d6d49cf92e859e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfiguredTableAssociationAnalysisRule

## Description
The configured-table-association analysis-rule plane does not round trip: the create of a rule on an association the same fixture has already created is refused with a 404 ResourceNotFoundException, so no rule is ever persisted and the read that follows it is refused the same way. Both causes are the SAME plane and the SAME membership/association pair, but they are two distinct operations a fixer must make answer, so they are numbered separately. 2 causes in this issue.

Cause 1 of 2: CreateConfiguredTableAssociationAnalysisRule answers 404 instead of 200 + the created rule (fixture cleanrooms-configured-table-association-analysis-rule-round-trip, step task#11).
AWS: Smithy: CreateConfiguredTableAssociationAnalysisRule, POST /memberships/{membershipIdentifier}/configuredTableAssociations/{configuredTableAssociationIdentifier}/analysisRule, http success code 200; output member `analysisRule` (ConfiguredTableAssociationAnalysisRule) is @required; inputs `membershipIdentifier` and `configuredTableAssociationIdentifier` are @required @httpLabel (length 36..36, uuid pattern), `analysisRuleType` (AGGREGATION | CUSTOM | LIST) and `analysisRulePolicy` are @required. ResourceNotFoundException (HTTP 404, "Request references a resource which does not exist.") is modeled, so it is reserved for a membership or an association that does not exist. For an association the membership DOES hold, AWS mints the rule and answers 200 with the whole rule computed from the stored association: `configuredTableAssociationId` and `configuredTableAssociationArn` from the persisted association, `membershipIdentifier` from the membership that holds it, `type` and `policy` as the caller sent them, and `createTime`/`updateTime` at the creation instant (the recorded response in this fixture shows exactly that shape — a `policy.v1.list.allowedResultReceivers` entry, `type` LIST, and equal create/update times; the concrete ids, arn and timestamps there are per-run values, not requirements).
Emulator: answers HTTP 404 for the create and sends no `analysisRule` member at all (replay: "status: expected 200, got 404" / "$.analysisRule: missing ... got None"), even though the membership, the configured table and the configured table association addressed by the path were created by earlier steps of the same fixture that all passed.
Why: a client that creates a collaboration analysis rule for an associated configured table gets a not-found refusal for a resource it just created, so it cannot complete the association-side configuration at all; the @required `analysisRule` is absent, so an SDK either throws the modeled ResourceNotFoundException or fails to deserialize a success it never receives. Nothing downstream that needs the association's `analysisRuleTypes` to be populated can proceed.
Fix: the CreateConfiguredTableAssociationAnalysisRule handler and the association lookup it depends on — the handler refuses before it reaches its persist step, so the association it resolves the path pair against is not found when it should be. Hypothesis: the pre-persist resolution of (membershipIdentifier, configuredTableAssociationIdentifier) against the stored associations, or the registration of the POST .../analysisRule route itself, is what yields the 404; neither the model nor the recording states which, so the requirement is only that an association the membership holds resolves and the create answers 200 with the persisted rule.
Traces: cleanrooms-configured-table-association-analysis-rule-round-trip#task#11 (expected 200, got 404).

Cause 2 of 2: GetConfiguredTableAssociationAnalysisRule answers 404 instead of 200 + the stored rule (fixture cleanrooms-configured-table-association-analysis-rule-round-trip, step task#12).
AWS: Smithy: GetConfiguredTableAssociationAnalysisRule, GET /memberships/{membershipIdentifier}/configuredTableAssociations/{configuredTableAssociationIdentifier}/analysisRule/{analysisRuleType}, @readonly, http success code 200; output member `analysisRule` is @required; ResourceNotFoundException (HTTP 404) is modeled for a membership, association or rule that does not exist. A read that follows a successful create of the same (association, analysisRuleType) pair reports exactly the rule that was created — same type, same policy, same association id/arn, same membership identifier and the same create/update instants the create reported.
Emulator: answers HTTP 404 with no `analysisRule` member (replay: "status: expected 200, got 404" / "$.analysisRule: missing ... got None"), because the create in cause 1 persisted nothing for the pair to read back.
Why: the read side of the plane is unusable — a client cannot confirm or re-read a rule it created, and a 404 is indistinguishable to it from "this association has no rule of that type", so it will retry the create and loop.
Fix: the GetConfiguredTableAssociationAnalysisRule handler plus the association-analysis-rule state it reads: once the create persists the rule under the (association id, analysis rule type) pair, this read must answer 200 from the store rather than the modeled not-found. Both causes must be closed together — making only the create answer 200 leaves this read unverified.
Traces: cleanrooms-configured-table-association-analysis-rule-round-trip#task#12 (expected 200, got 404).

Not covered by an existing open issue: the open cleanrooms issues concern tagging status codes (20261029T175308Z), UpdateConfiguredTable column-reference wording (20261030T044422Z), GetConfiguredTableAnalysisRule not-found wording (20261031T001457Z), analysis-template wording (20261102T062702Z, 20261102T105318Z) and the DeleteConfiguredTable stub (20261102T105318Z) — none of them touches the configured-table-ASSOCIATION analysis-rule plane or a 404-instead-of-200 success path.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false).

Cause 1 — fixture cleanrooms-configured-table-association-analysis-rule-round-trip, step task#11, operation CreateConfiguredTableAssociationAnalysisRule, verdict mismatch:
  expected_status 200, actual_status 404
  detail: "status: expected 200, got 404" and "$.analysisRule: missing (expected {configuredTableAssociationArn, configuredTableAssociationId, createTime, membershipIdentifier, policy: {v1: {list: {allowedResultReceivers: [...]}}}, type: 'LIST', updateTime}, got None)"
  divergence_key: service=cleanrooms|CreateConfiguredTableAssociationAnalysisRule|status=200->404|keys=$.analysisRule,status

Cause 2 — same fixture, step task#12, operation GetConfiguredTableAssociationAnalysisRule, verdict mismatch:
  expected_status 200, actual_status 404
  detail: "status: expected 200, got 404" and "$.analysisRule: missing (... got None)"
  divergence_key: service=cleanrooms|GetConfiguredTableAssociationAnalysisRule|status=200->404|keys=$.analysisRule,status

Neither cluster carries a stub_path (the operations are routed and implemented: services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/ops/create_configured_table_association_analysis_rule.rs and .../get_configured_table_association_analysis_rule.rs), and neither carries an owned_by, so no filed issue covers them. The association lookup both handlers use is CleanRoomsState::configured_table_association_in (services/service-com-amazonaws-cleanrooms-awsbastioncontrolplaneservicelambda/src/state.rs around line 1171), and the persist step is insert_configured_table_association_analysis_rule (same file, around line 1203). The per-run ids, arns and timestamps in the recorded expectation are environment values, not requirements; the requirements are the 200 status and the presence and composition of the @required `analysisRule`.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay fixture cleanrooms-configured-table-association-analysis-rule-round-trip from trace set simparity-wire-aws-v1.
2. The fixture's earlier steps create a collaboration, a membership, a configured table and a configured table association under that membership; they all pass.
3. Step task#11 sends POST /memberships/{membershipIdentifier}/configuredTableAssociations/{configuredTableAssociationIdentifier}/analysisRule with analysisRuleType=LIST and a matching analysisRulePolicy (v1.list.allowedResultReceivers). AWS answers 200 with the created analysisRule; the emulator answers 404 and no analysisRule.
4. Step task#12 sends GET /memberships/{membershipIdentifier}/configuredTableAssociations/{configuredTableAssociationIdentifier}/analysisRule/LIST. AWS answers 200 with the same rule; the emulator answers 404 and no analysisRule.
5. Re-replay both steps after the fix: task#11 must answer 200 with the persisted rule, and task#12 must answer 200 with that same rule.
