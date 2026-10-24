Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-automation-list-automation-rules-reflects-created-rule
Issue-Step: setup#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-automation-list-automation-rules-reflects-created-rule
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation|CreateAutomationRule|2xx->4xx|status
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# CreateAutomationRule refuses a valid create with 400 InvalidParameterValueException instead of minting the rule

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAutomationRule

## Description
Cause 1 of 1: CreateAutomationRule rejects a create request that the recording shows AWS accepting, so no rule is ever stored and every later step of the fixture that depends on the created rule cannot run (2 steps blocked in this replay).

AWS: CreateAutomationRule "Creates a new automation rule to apply recommended actions to resources based on specified criteria" and, on success, answers HTTP 200 with the created rule (Smithy: CreateAutomationRule -> CreateAutomationRuleResponse with ruleArn/ruleId/name/ruleType/ruleRevision/recommendedActionTypes/schedule/status/createdTimestamp; SUCCESS_STATUS 200). The only client-input refusal the operation models for a bad parameter value is InvalidParameterValueException (HTTP 400), and the required members are name (RuleName, @length 0..128, pattern ^[a-zA-Z0-9_-]*$), ruleType (RuleType: AccountRule|OrganizationRule), recommendedActionTypes (RecommendedActionTypeList), schedule (Schedule) and status (RuleStatus: Active|Inactive). The recorded step sends such a request and the recording's response status is 200.

Emulator: the same request is answered HTTP 400. The step diverges on status only (expected 200, got 400), so the request got past deserialization and the model-derived boundary validator and was refused by handler-local validation: services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/create_automation_rule.rs returns InvalidParameterValueException from invalid_parameter() before the store is touched. That function enforces seven rules the Smithy constraints do not carry (non-empty name, organizationConfiguration required for OrganizationRule, priority decimal in 0..1, non-empty recommendedActionTypes, schedule_violation, criteria_violation, duplicate tag keys), and at least one of them is stricter than the service it emulates.

Hypothesis: the schedule clause is the strictest of the seven and the most likely to refuse an accepted create. crate::state::schedule_violation requires EXACTLY CRON_FIELD_COUNT = 6 whitespace-separated fields and requires the minutes and hours fields to be digits only (cron_field_is_one_value rejects any field containing a non-digit character). The member's own documentation gives the example "(30 12 * * *)" - five fields - and clients commonly send the expression wrapped as "cron(30 12 * * ? *)", in which the first and last fields carry non-digit characters. Either form is refused by the emulator while the service accepts it. The same style of over-strictness applies to the priority clause (priority_is_valid admits only 0, 0.<=30 digits> and 1/1.000...), which would refuse any other decimal a caller sends.

Why: a client that creates an automation rule exactly as AWS documents gets a 400 validation refusal and no rule. Nothing downstream can work: ListAutomationRules reports an empty list, and every rule-scoped call (Get/Update/DeleteAutomationRule, the preview operations, StartAutomationEvent) has no rule to address. The emulator is unusable for the core create-then-use workflow of this service, and the failure surfaces as a client-side validation error, so a caller blames its own request rather than the emulator.

Fix: belongs in CreateAutomationRule's handler-local validation - invalid_parameter() in src/ops/create_automation_rule.rs and the helpers it calls in src/state.rs (schedule_violation / cron_field_is_one_value, priority_is_valid, criteria_violation). Each clause must admit every value the service accepts: accept the documented schedule-expression forms (including the 5-field form the member's own doc example shows and a cron(...)-wrapped expression) and keep a 400 InvalidParameterValueException only for values the service itself refuses. UpdateAutomationRule shares these helpers, so the same relaxation must hold there.

Traces: fixture compute-optimizer-automation-list-automation-rules-reflects-created-rule, step setup#5 (CreateAutomationRule, expected 200, got 400); the same fixture's remaining 2 steps are blocked by it.

## Evidence
Replay of trace set simparity-wire-ga-prod-v1 against the emulator (emulator_up: true, /__admin/health 200): divergence_key "service=compute-optimizer-automation|CreateAutomationRule|status=200->400|keys=status", verdict mismatch, detail "status: expected 200, got 400" - expected_status 200, actual_status 400, fixture compute-optimizer-automation-list-automation-rules-reflects-created-rule, step setup#5. affected: ["compute-optimizer-automation-list-automation-rules-reflects-created-rule#setup#5"]. Summary for the fixture: steps_total 7, steps_passed 4, steps_mismatched 1, steps_blocked 2 (the blocked steps are the ones that need the rule this create never stored). No stub_path and no owned_by, so the operation is implemented and no open issue carries this replay key. Expected per Smithy: HTTP 200 with CreateAutomationRuleResponse (ruleArn, ruleId, name, ruleType, ruleRevision, recommendedActionTypes, schedule, status, createdTimestamp). Actual: HTTP 400, the only 400 the operation models being InvalidParameterValueException, which src/ops/create_automation_rule.rs raises from invalid_parameter() ahead of any store write.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire fixture corpus (replay_wire_corpus).
2. Fixture compute-optimizer-automation-list-automation-rules-reflects-created-rule, step setup#5: send the recorded CreateAutomationRule request (name, ruleType, recommendedActionTypes, schedule, status as recorded) to the compute-optimizer-automation endpoint.
3. Observe HTTP 400 (InvalidParameterValueException) instead of the recorded HTTP 200 carrying ruleArn/ruleId; the rule is not stored, and the fixture's following 2 steps are blocked.
4. Vary the schedule expression to isolate the clause: a 6-field digits-in-minutes/hours expression such as "30 12 * * ? *" is accepted, while the documented 5-field example "30 12 * * *" and the wrapped form "cron(30 12 * * ? *)" are both refused with 400 by crate::state::schedule_violation.
