Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: 2334cad17af0cf812822af0fde730e954dc43dac

# Authorization-gate seam still documents DisassociateUser as a 501 scaffold

_Filed: 2026-10-27T15:39:24Z_
_Operation: DisassociateUser | Class: maintainability_

> _Note: this issue was identified against the code base at commit `2334cad17af0cf812822af0fde730e954dc43dac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::service`'s `override_operation` is the one place the instance-authorization rule is restated outside `crate::state`, and its copy of the rule has drifted: it says only `AssociateUser` is gated because `DisassociateUser` "answers the scaffold's 501 for every request, so there is nothing yet for a gate in front of it to be consistent with; its own issue owns it". `DisassociateUser` is fully implemented — it resolves an association and answers a 200 `InstanceUserSummary` — and it carries no `SCAFFOLDED, NOT IMPLEMENTED` header, so the stated justification no longer describes the code it points at. A maintainer reading this seam is told an implemented operation is unwritten and that a (now-fixed) issue owns it.

No response and no test changes: the only correction is to the drifted restatement of the gating rule in `service.rs` (the rule itself lives once in `state.rs::authorizes_instance`, which `associate_user.rs` and `list_user_associations.rs` both call). This is purely documentation drift — `DisassociateUser` resolves the association by ARN or triple and fails closed with the same `UnauthorizedException` when nothing resolves, so no observable behaviour depends on the stale sentence.

## Evidence

The drifted copy — `src/service.rs:155-157`:
```
// Only `AssociateUser` is gated. `DisassociateUser` answers the scaffold's 501 for
// every request, so there is nothing yet for a gate in front of it to be
// consistent with; its own issue owns it.
```
The code it describes — `src/ops/disassociate_user.rs:155-209`, an implemented handler with no scaffold marker, ending in:
```
        Ok(DisassociateUserOutput {
            instance_user_summary: crate::state::instance_user_summary(&record),
        })
```
The single home of the rule the comment restates — `src/state.rs:1015-1022`:
```
/// Whether the caller may be shown to own `instance_id`: true exactly when the
/// account's EC2 fleet holds it.
///
/// The gate every operation that acts on an instance runs BEFORE it looks anything
/// else up, so a refusal discloses nothing about this service's own resources.
pub fn authorizes_instance(&self, instance_id: &str) -> bool {
```
Existing tests that cover the behaviour the fix must leave untouched: `tests/all/user_disassociation.rs` (`a_disassociation_is_observable_in_disassociating_until_the_teardown_completes`, `re_associating_a_user_still_being_disassociated_conflicts`), `tests/all/resource_resolution_refusals.rs:315-345` (`an_arn_this_service_holds_no_resource_for_fails_closed`, DisassociateUser rows) and `:273-309` (`the_instance_gate_fails_closed_before_any_other_lookup`, AssociateUser rows).

## Steps to Reproduce

1. Open `services/service-com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions/src/service.rs` and read the comment at lines 155-157 inside `override_operation`.
2. Open `src/ops/disassociate_user.rs` and grep for `SCAFFOLDED, NOT IMPLEMENTED` / `not_implemented(` — neither is present; the handler persists the disassociation and returns 200 at lines 197-209.
3. Conclude the seam's justification for not gating DisassociateUser is stale: the operation is implemented, not a 501 scaffold.
