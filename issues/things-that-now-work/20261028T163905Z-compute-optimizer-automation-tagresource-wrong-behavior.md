Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 63dbcd68d6fd7b54c4f6138d45d0f4dfd539b7cc

# A tag write rewrites the stored rule but leaves lastUpdatedTimestamp at its old value, the one write path in the crate that does not stamp "when this rule was last changed"

_Filed: 2026-10-28T16:39:05Z_
_Operation: TagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `63dbcd68d6fd7b54c4f6138d45d0f4dfd539b7cc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`TagResource` mutates the stored rule (`rule.tags = merged`) and records the client token, but never touches `rule.last_updated_timestamp`. So after a successful tag addition, `GetAutomationRule` and `ListAutomationRules` keep reporting the instant of the create (or of the last `UpdateAutomationRule`) as the rule's `lastUpdatedTimestamp`, even though the rule the service holds is not the rule that instant describes any more.

That contradicts the crate's own definition of the field: `StoredRule::last_updated_timestamp` is documented as "When this rule was last changed" — not "when UpdateAutomationRule last ran" — and every other path in this crate that writes a stored rule stamps it. `UpdateAutomationRule` stamps it on a caller update, and `UpdateEnrollmentConfiguration` stamps it for a SERVICE-driven change that is not an update call at all, where it explicitly reasons about which of the two rule-level bookkeeping fields a non-update write may touch: it leaves `ruleRevision` alone and stamps `last_updated_timestamp`. The tag path makes the opposite choice silently: it reasons in a comment about `ruleRevision` (deliberately not advanced, so a repeat still passes the revision check) and says nothing about the timestamp, which it simply does not write.

`UntagResource` carries the identical omission (`rule.tags.retain(...)` at `src/ops/untag_resource.rs:256` with no stamp), so the root cause is shared; `TagResource` is named here because it is the path that adds state to the rule.

## Evidence

The crate's definition of the field — `src/state.rs:928-931`:
```
    /// When this rule was last changed. Always a value: a rule that has never
    /// been updated was last changed when it was created, so a create stores its
    /// own creation instant here.
    pub last_updated_timestamp: ::aws_smithy_types::DateTime,
```
(and the modelled member, `smithy_query(query="shape",shape="AutomationRule")`: `"lastUpdatedTimestamp": {"target": "Timestamp", "doc": "<p>The timestamp when the automation rule was last updated.</p>"}`.)

The tag write, which changes the rule and stamps nothing — `src/ops/tag_resource.rs:267-282`:
```
        let Some(rule) = guard.rules.get_mut(&input.resource_arn) else {
            return Err(not_found(&input.resource_arn));
        };
        // The addition does not advance `ruleRevision`: a repeat of the same
        // request would then fail the revision check above, and the operation is
        // modelled `@idempotent`.
        rule.tags = merged;

        guard.record_token(
            client_token,
            Self::NAME,
            &request_fingerprint,
            Some(crate::state::TokenSubject::Rule(input.resource_arn.clone())),
        );

        Ok(TagResourceOutput {})
```
and its twin — `src/ops/untag_resource.rs:252-256`:
```
        // Remove exactly the keys named; a key the rule does not carry leaves
        // the rule as it is, so the same request may be repeated (the operation
        // is modelled `@idempotent`). For the same reason the removal does not
        // advance `ruleRevision`: a repeat would then fail the check above.
        rule.tags.retain(|tag| !input.tag_keys.contains(&tag.key));
```

The sibling write paths that DO stamp it — `src/ops/update_automation_rule.rs:413-415`:
```
        // "The revision number ... is incremented each time the rule is updated."
        rule.rule_revision += 1;
        rule.last_updated_timestamp = updated_at;
```
and the service-driven one, which reasons about exactly this distinction — `src/ops/update_enrollment_configuration.rs:271-289`:
```
        // "Inactive - Disables the Automation feature for your account and stops
        // all of your automation rules. If you opt in again later, all rules will
        // be inactive" — so opting out deactivates every rule this account owns.
        // `ruleRevision` is documented as incremented "each time the rule is
        // updated" through `UpdateAutomationRule`, the caller's
        // optimistic-concurrency token, so a service-driven stop leaves it alone.
...
                if let Some(rule) = guard.rules.get_mut(&arn) {
                    rule.status = "Inactive".to_string();
                    rule.last_updated_timestamp = stored_at;
                }
```

Test coverage: `lastUpdatedTimestamp` is asserted only around creates and updates — `tests/all/automation_rule_lifecycle.rs:748-751` ("case {case:?}: an update stamps lastUpdatedTimestamp") and `tests/all/rule_projection.rs:474-477`. A grep for `lastUpdatedTimestamp` across `tests/` returns no hit in `resource_tagging.rs` or `resource_tags.rs`, so no test reads the rule's update instant across a tag write and the stale value reported here is pinned by nothing.

## Steps to Reproduce

1. Mount the emulator and call UpdateEnrollmentConfiguration with {"status": "Active"}.
2. Call CreateAutomationRule with {"name": "r1", "ruleType": "AccountRule", "recommendedActionTypes": ["UpgradeEbsVolumeType"], "schedule": {"scheduleExpression": "30 12 * * ? *"}, "status": "Active"} and keep the ruleArn.
3. Call GetAutomationRule with that ruleArn and record lastUpdatedTimestamp (it equals createdTimestamp).
4. Wait a moment, then call TagResource with {"resourceArn": <ruleArn>, "ruleRevision": 1, "tags": [{"key": "environment", "value": "production"}]} — 200.
5. Call ListTagsForResource and GetAutomationRule: the tag IS on the rule, so the stored rule has changed — yet GetAutomationRule's lastUpdatedTimestamp is byte-identical to step 3, and so is the ListAutomationRules summary's.
6. Compare with the two sibling write paths: an UpdateAutomationRule on the same rule moves lastUpdatedTimestamp (tests/all/automation_rule_lifecycle.rs:748-751), and UpdateEnrollmentConfiguration {"status": "Inactive"} moves it for every rule it deactivates without being an update call at all (src/ops/update_enrollment_configuration.rs:286-288).
7. Repeat with UntagResource {"resourceArn": <ruleArn>, "ruleRevision": 1, "tagKeys": ["environment"]}: the tag is removed and lastUpdatedTimestamp again does not move.
