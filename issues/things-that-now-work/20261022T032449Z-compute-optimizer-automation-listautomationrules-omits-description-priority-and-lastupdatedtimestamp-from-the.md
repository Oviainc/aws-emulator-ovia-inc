Issue-Source: trace-sweep-divergence
Issue-Trace: compute-optimizer-automation-list-automation-rules-reflects-created-rule
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: compute-optimizer-automation-list-automation-rules-reflects-created-rule
Issue-Shape-Id: com.amazonaws.computeoptimizerautomation#ComputeOptimizerAutomationService
Issue-Key: compute-optimizer-automation|ListAutomationRules|2xx->2xx|$.automationRules[*].description
Issue-Key: compute-optimizer-automation|ListAutomationRules|2xx->2xx|$.automationRules[*].lastUpdatedTimestamp
Issue-Key: compute-optimizer-automation|ListAutomationRules|2xx->2xx|$.automationRules[*].priority
Issue-Commit: f81229b87ab2500fc3bfee437baf8654766d98fe

# ListAutomationRules omits description, priority and lastUpdatedTimestamp from the AutomationRule it returns

> _Note: this issue was identified against the code base at commit `f81229b87ab2500fc3bfee437baf8654766d98fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListAutomationRules

## Description
This issue has 1 cause.

Cause 1 of 1: the AutomationRule records ListAutomationRules projects are incomplete — three members that AWS always serializes for a rule that exists are absent from the emulator's response.
AWS: ListAutomationRules returns automationRules, a list of AutomationRule. Smithy: com.amazonaws.computeoptimizerautomation#AutomationRule carries description (String, "A description of the automation rule"), priority (String, a decimal between 0 and 1 that orders rules when several match the same recommended action) and lastUpdatedTimestamp (Timestamp, "The timestamp when the automation rule was last updated"). The recording, taken after a rule was created in the same fixture, carries all three on the listed rule: description present (empty string, because the create supplied none), priority present, lastUpdatedTimestamp present as an epoch-seconds Timestamp. The status is 200 on both sides — this is a response-shape divergence, not an error-path one.
Emulator: status 200, but $.automationRules[0].description, $.automationRules[0].priority and $.automationRules[0].lastUpdatedTimestamp are all absent from the serialized rule. The rule is otherwise found and listed, so the create/list round-trip itself works; only these members are dropped.
Why: a client reading the rule set back cannot order rules, because priority is what breaks ties when several rules match the same recommended action — an absent priority makes a multi-rule configuration non-deterministic for any caller that sorts on it. An absent lastUpdatedTimestamp breaks change detection and drift reconciliation (tooling that compares last-updated times sees nothing to compare), and an absent description means a round-trip of a rule through list-then-update loses the description. Typed SDKs surface these as null/None, so code that dereferences them faults rather than reading the value AWS sends.
Fix: the ListAutomationRules projection — either the AutomationRule serializer is not emitting these members, or the created rule's state record never stores them. If the rule state does hold them, this is the serializer for AutomationRule; if it does not, the CreateAutomationRule state write must default description to the empty string and set priority and lastUpdatedTimestamp at creation so the listing can project them.
Traces: compute-optimizer-automation-list-automation-rules-reflects-created-rule#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1; state_reset_failures 0, so this fixture ran against its own state and the preceding create in the same fixture is the rule being listed.

Fixture compute-optimizer-automation-list-automation-rules-reflects-created-rule, step task#6, ListAutomationRules, status 200 expected and 200 actual:
- $.automationRules[0].description: missing (expected '' — the empty string AWS returns for a rule created without a description)
- $.automationRules[0].lastUpdatedTimestamp: missing (recorded as an epoch-seconds Timestamp; the recorded instant itself is environment-specific and not a requirement — the requirement is that the member is present and is the rule's last-update time)
- $.automationRules[0].priority: missing (recorded as present with a shaped, non-asserted value)

Model evidence: Smithy AutomationRule declares description (String), priority (String, decimal 0..1 governing rule precedence) and lastUpdatedTimestamp (Timestamp) as members of the structure ListAutomationRulesResponse.automationRules lists.

This divergence carried no stub_path and no owned_by, so it is neither filed as a stub nor owned by an open issue's replay key. The two resolved compute-optimizer-automation issues concern error payloads and 501 stubs, not this projection.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture compute-optimizer-automation-list-automation-rules-reflects-created-rule from trace set simparity-wire-aws-v1.
2. The fixture creates an automation rule (without a description) and then calls ListAutomationRules at step task#6.
3. Inspect automationRules[0] in the 200 response: description, priority and lastUpdatedTimestamp are absent.
4. Expected after fix: automationRules[0] carries description (the empty string when none was supplied at create), priority (the rule's decimal precedence string) and lastUpdatedTimestamp (the rule's last-update Timestamp), alongside the members already returned.
