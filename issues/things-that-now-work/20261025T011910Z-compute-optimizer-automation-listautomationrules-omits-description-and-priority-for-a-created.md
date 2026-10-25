Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-automation-list-automation-rules-reflects-created-rule
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-ga-prod-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-automation-list-automation-rules-reflects-created-rule
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation|ListAutomationRules|2xx->2xx|$.automationRules[*].description
Issue-Key: compute-optimizer-automation|ListAutomationRules|2xx->2xx|$.automationRules[*].priority
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# ListAutomationRules omits description and priority for a created rule

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAutomationRules

## Description
Cause 1 of 1: a rule reported by ListAutomationRules is missing the AutomationRule members `description` and `priority`, so the listing does not reflect the rule the account actually holds.

AWS: ListAutomationRules returns `automationRules` as a list of `AutomationRule` (Smithy: ListAutomationRules -> ListAutomationRulesResponse$automationRules : AutomationRules, member AutomationRule; AutomationRule$description : String "A description of the automation rule"; AutomationRule$priority : String, "a string representation of a decimal number between 0 and 1 ... that determines the priority of the rule. When multiple rules match the same recommended [action] ..."). The model marks neither member as required and documents no default for either, so for the defaulting question this is recorded behavior only: the recording of this fixture shows the real service reporting BOTH members on a rule reached through ListAutomationRules — `description` present as the empty string, and `priority` present with a value (the recording asserts its shape, not a literal). Both the recorded response and the emulator answer HTTP 200; the divergence is in the body, not in the status or an error code.

Emulator: the replayed step answers 200 but the first element of `automationRules` carries neither member — replay reports `$.automationRules[0].description: missing (expected '', got None)` and `$.automationRules[0].priority: missing (expected '<shaped-not-asserted>', got None)`. The emulator's rule state keeps `description` and `priority` as optional values that are populated only when a request named them, and every read path propagates that absence instead of materialising the value the service reports (see Traces for the code path), so a rule created without those members is listed without them forever.

Why: a client that walks ListAutomationRules to render or diff its rule inventory sees no description field and, worse, no `priority`. Priority is the member that decides which rule claims a recommended action when several match, so a client that reads precedence from the listing gets `None` where AWS gives it a comparable decimal, and either treats every rule as unprioritised or errors on a member it expects to be present. Any round-trip (list, then UpdateAutomationRule with the listed values) also silently drops the description.

Fix: the service's rule state and the projection it hands the list handler — the stored rule must carry the `description` and `priority` the service reports for a rule whose create named neither (empty description, a default priority), and the `AutomationRule` projection must report both, so ListAutomationRules and the other read paths built from the same stored rule agree with the recording.

Note: the open issues for this service (compute-optimizer-automation error-message wording, the ListRecommendedActions behavior issue, the GetEnrollmentConfiguration organizationRuleMode issue, and the two scaffold stubs) do not cover this: none of them concerns the AutomationRule members reported by ListAutomationRules.

## Evidence
Fixture compute-optimizer-automation-list-automation-rules-reflects-created-rule, step task#6, operation ListAutomationRules.
Verdict: mismatch. Expected status 200, actual status 200.
Body detail from replay:
  $.automationRules[0].description: missing (expected '', got None)
  $.automationRules[0].priority: missing (expected '<shaped-not-asserted>', got None)
Divergence key: service=compute-optimizer-automation|ListAutomationRules|status=200->200|keys=$.automationRules[0].description,$.automationRules[0].priority
Affected steps in cluster (1): compute-optimizer-automation-list-automation-rules-reflects-created-rule#task#6

Model basis: AutomationRule$description (target String) and AutomationRule$priority (target String) both exist on the shape ListAutomationRules returns; neither is required and neither has a modelled default, so the recording is the authority for what the service reports when a create named neither.

Emulator code path: services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/list_automation_rules.rs:226 maps stored rules through crate::state::StoredRule::to_automation_rule; services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/state.rs:827-844 copies description/priority straight from the Option fields declared at state.rs:799 and state.rs:809, which CreateAutomationRule leaves None when the request omits them (src/ops/create_automation_rule.rs:333,337).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture compute-optimizer-automation-list-automation-rules-reflects-created-rule.
2. The fixture opts the account in, creates an automation rule, and then calls ListAutomationRules at step task#6.
3. Inspect automationRules[0] in the 200 response: the object has ruleArn, ruleId, name, ruleType, ruleRevision, accountId, recommendedActionTypes, schedule, status and the timestamps, but no `description` and no `priority` keys.
4. Expected (per the recording): `description` present (empty string) and `priority` present with a decimal-string value, as the real service reports for the same created rule.
