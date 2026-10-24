Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: b4572519ed5f295b6edc2ad3f0f0ced732a31b98

# CreateAutomationRule invents values for description and priority members the caller never sent

_Filed: 2026-10-24T06:11:05Z_
_Operation: CreateAutomationRule | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `b4572519ed5f295b6edc2ad3f0f0ced732a31b98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A create that omits the optional `description` or `priority` does not leave them unset: the handler materialises `description = ""` (state::DEFAULT_RULE_DESCRIPTION) and `priority = "1"` (state::DEFAULT_RULE_PRIORITY) into the stored rule (src/ops/create_automation_rule.rs lines 334-342), so CreateAutomationRule, GetAutomationRule, ListAutomationRules and UpdateAutomationRule all report `"description": ""` and `"priority": "1"` for a rule whose create named neither. Both members are optional in every output shape, and the awsJson1_0 projection of a member the service holds no value for is to omit it. `priority` in particular is a made-up precedence value — the state module's own comment records it as a CHOSEN value with no basis in the model or the docs — and it is the value that decides which rule claims a recommended action when several match, so a client reading it back is told the rule has a precedence it never requested.

Expected: a rule created without `description`/`priority` reports those members absent (or whatever default the service documents), not an emulator-chosen literal.

## Evidence

Spec: `description` and `priority` are optional members of CreateAutomationRuleRequest and of every rule output shape (OPERATIONS.md CreateAutomationRule/GetAutomationRule), and `AutomationRule$priority` documents that the LOWEST priority value wins when several rules match — so a fabricated value changes documented rule-selection semantics. Handler/state: src/state.rs lines 99-119 define DEFAULT_RULE_DESCRIPTION = "" and DEFAULT_RULE_PRIORITY = "1" with the comment "CHOSEN value: neither the model nor the operation docs give the default"; src/ops/create_automation_rule.rs applies them via `unwrap_or_else`. Tests: tests/all/automation_rule_lifecycle.rs asserts only that a supplied priority is echoed; no test pins the unsupplied-member projection.

## Steps to Reproduce

1. UpdateEnrollmentConfiguration {"status":"Active"}
2. POST CreateAutomationRule {"name":"r1","ruleType":"AccountRule","recommendedActionTypes":["UpgradeEbsVolumeType"],"schedule":{"scheduleExpression":"30 12 * * ? *"},"status":"Active"} with no description and no priority
3. Observe the response body carries "description": "" and "priority": "1"
4. POST GetAutomationRule {"ruleArn":<arn>} — observe the same invented values reported back
