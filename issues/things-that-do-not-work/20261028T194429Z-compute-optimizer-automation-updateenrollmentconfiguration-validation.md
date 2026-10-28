Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d

# Accept an opt-in less than 24 hours after an opt-out, which the status member documents as refused

_Filed: 2026-10-28T19:44:29Z_
_Operation: UpdateEnrollmentConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `1fce8a5a4e99700c9d3c5ad3e1ef05753020fc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateEnrollmentConfigurationRequest$status` documents TWO caller-facing rules in one paragraph: the `Pending`/`Failed` values may not be sent, and "You must wait at least 24 hours after opting out to opt in again." The handler implements the first clause (it quotes it in a comment and refuses the two server-only statuses with `InvalidParameterValueException`) and ignores the second: it writes `status` into the enrollment store unconditionally, consulting neither the previously stored status nor the stored `last_updated_timestamp`. A caller can therefore set `status=Inactive` and immediately set `status=Active` again in the same second and get HTTP 200 with `status: Active`, where the modelled refusal for a request this service cannot honour (`InvalidParameterValueException`, HTTP 400, "One or more parameter values are not valid.") is what the documented rule calls for. The data needed to judge the rule is already persisted — `EnrollmentConfig` keeps `status` and `last_updated_timestamp`, and the handler stamps the latter on every write — so the gate has nothing to look up that the store does not already hold. Nothing in the crate (no constant, comment, helper or test) acknowledges the cooldown.

## Evidence

Smithy model, `smithy_query(query="doc",shape="UpdateEnrollmentConfigurationRequest",member="status")`:

  "Inactive - Disables the Automation feature for your account and stops all of your automation rules. If you opt in again later, all rules will be inactive, and you must enable the rules you want to run. You must wait at least 24 hours after opting out to opt in again."

The handler reads that same paragraph as normative — but only its other clause — at services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/update_enrollment_configuration.rs:186-203:

  "// `status` documents which members of the enum a CALLER may send: \"The
   // Pending and Failed options cannot be used to update the enrollment
   // status of an account. They are returned in the response". Both are
   // legal members of `EnrollmentStatus`, so the protocol boundary admits
   // them and the refusal belongs here."
  `if !crate::state::SETTABLE_ENROLLMENT_STATUSES.contains(&input.status.as_str()) { ... InvalidParameterValueException ... }`

and then applies the new status with no further precondition, update_enrollment_configuration.rs:251-259:

  `let stored_at = crate::state::now();`
  `let enrollment = guard.enrollments.entry(account_id.clone()).or_default();`
  `enrollment.status = input.status.clone();`
  `enrollment.status_reason = None;`
  `enrollment.last_updated_timestamp = Some(stored_at);`

The facts the rule needs are stored, state.rs:826-831:

  `pub struct EnrollmentConfig { pub status: String, pub status_reason: Option<String>, pub organization_rule_mode: Option<String>, pub last_updated_timestamp: Option<::aws_smithy_types::DateTime> }`

Nothing in the crate mentions the cooldown: `grep -n "24 hour|wait at least"` over the crate matches only the generated doc header (src/ops/update_enrollment_configuration.rs:58) and the unrelated `clientToken` sentence in src/ops/associate_accounts.rs:70.

Test coverage: no test asserts the cooldown. The one place the sequence occurs shows the immediate re-opt-in is accepted — tests/all/recommended_action_inventory.rs:354-364:

  "// Opting out and back in: the account's inventory is the same set, in the
   // same order, with no second copy of anything.
   for (case, status) in [(\"opt-out\", \"Inactive\"), (\"opt-in-again\", \"Active\")] { call_ok(&router, \"UpdateEnrollmentConfiguration\", &json!({\"status\": status}), case).await; }"

That assertion is about inventory idempotency (`"re-enrolling must not duplicate or rewrite an action"`), not about the cooldown, so it pins the gap rather than justifying it.

## Steps to Reproduce

1. POST / with `X-Amz-Target: ComputeOptimizerAutomationService.UpdateEnrollmentConfiguration` and body `{"status":"Active"}` — 200, status Active.
2. Immediately POST the same target with `{"status":"Inactive"}` — 200, status Inactive, `lastUpdatedTimestamp` stamped now.
3. Immediately POST the same target with `{"status":"Active"}` again (far less than 24 hours after the opt-out).
4. Observe HTTP 200 with `status: "Active"`: the opt-in is applied although `status` documents "You must wait at least 24 hours after opting out to opt in again", so the modelled `InvalidParameterValueException` (HTTP 400) is never raised and `GetEnrollmentConfiguration` reports the account enrolled again.
