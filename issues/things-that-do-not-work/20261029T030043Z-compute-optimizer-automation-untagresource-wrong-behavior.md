Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a6f1838150b2f559ad354b34ee3fa3c889d17ebe

# UntagResource removes tags from the stored rule but never stamps lastUpdatedTimestamp, unlike the crate's other revision-preserving rule mutation

_Filed: 2026-10-29T03:00:43Z_
_Operation: UntagResource | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a6f1838150b2f559ad354b34ee3fa3c889d17ebe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UntagResource` mutates the stored rule in place (`rule.tags.retain(...)`) and leaves `rule.last_updated_timestamp` untouched, so after a successful removal `GetAutomationRule` and `ListAutomationRules` report the shortened tag list together with a `lastUpdatedTimestamp` that predates the removal.

The crate has already settled how a rule mutation that deliberately does NOT advance `ruleRevision` treats the timestamp: `UpdateEnrollmentConfiguration`'s opt-out path rewrites `rule.status`, states that it leaves the revision alone, and stamps `rule.last_updated_timestamp = stored_at`; `UpdateAutomationRule` stamps it as well. The two tagging handlers are the only writers of a stored rule that skip the stamp, and the only comment they carry on the subject concerns `ruleRevision`, not the timestamp. (The same defect exists in `TagResource`, filed separately — both handlers need the stamp.)

## Evidence

The write that skips the stamp — `src/ops/untag_resource.rs:249-256`:
```
        let Some(rule) = guard.rules.get_mut(&input.resource_arn) else {
            return Err(not_found(&input.resource_arn));
        };
        // Remove exactly the keys named; a key the rule does not carry leaves
        // the rule as it is, so the same request may be repeated (the operation
        // is modelled `@idempotent`). For the same reason the removal does not
        // advance `ruleRevision`: a repeat would then fail the check above.
        rule.tags.retain(|tag| !input.tag_keys.contains(&tag.key));
```
(`grep -n last_updated_timestamp src/ops/untag_resource.rs` → no match)

The sibling revision-preserving mutation that DOES stamp it — `src/ops/update_enrollment_configuration.rs:274-288`:
```
        // `ruleRevision` is documented as incremented "each time the rule is
        // updated" through `UpdateAutomationRule`, the caller's
        // optimistic-concurrency token, so a service-driven stop leaves it alone.
        if input.status == "Inactive" {
            ...
                if let Some(rule) = guard.rules.get_mut(&arn) {
                    rule.status = "Inactive".to_string();
                    rule.last_updated_timestamp = stored_at;
                }
```

The meaning of the member the readers answer with, `src/ops/get_automation_rule.rs:66-67`:
```
//! * `lastUpdatedTimestamp` (Timestamp) — The timestamp when the automation rule was last
//!   updated.
```

Test coverage: `tests/all/resource_tagging.rs` asserts the revision rule only (line 234: "Tagging is not a rule revision, so the same request stays valid."); no test asserts `lastUpdatedTimestamp` around a tag removal, so nothing pins the current behaviour with a stated reason.

## Steps to Reproduce

1. Call `UpdateEnrollmentConfiguration` with `{"status":"Active"}`.
2. Call `CreateAutomationRule` with a minimal valid rule carrying `tags:[{"key":"owner","value":"payments"}]`; keep `ruleArn` and `ruleRevision`.
3. Call `GetAutomationRule` and record `lastUpdatedTimestamp` (T0).
4. Call `UntagResource` with `{"resourceArn": <ruleArn>, "ruleRevision": <ruleRevision>, "tagKeys":["owner"]}` — answered 200.
5. Call `GetAutomationRule` again: `tags` is now empty, yet `lastUpdatedTimestamp` is still T0.
6. Contrast inside the crate: `UpdateEnrollmentConfiguration` with `{"status":"Inactive"}` changes `rule.status` without bumping `ruleRevision` and DOES move `lastUpdatedTimestamp`.
