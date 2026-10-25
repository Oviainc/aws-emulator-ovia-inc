Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# ListTagsForResource skips the Firewall Manager administrator gate and answers reads with no admin associated

_Filed: 2026-10-24T01:20:24Z_
_Operation: ListTagsForResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListTagsForResource is an organisation-scoped Firewall Manager call, answered only for a Firewall Manager administrator (or the organisation's management account). With no default-administrator association in the account/Region the live service refuses it with a 400 carrying 'No default admin could be found for account <accountId> in Region <region>', decided before the ARN is resolved — the same refusal this emulator already reproduces for a dozen other operations via `state::require_administrator`.

This handler never calls the gate: with no association it answers 400 ValidationException for a malformed ARN, 400 ResourceNotFoundException for a well-formed ARN naming nothing, and 200 with the resource's tag set when a policy with that ARN exists. That both leaks resource existence to an unauthorised caller and lets it read tag state AWS would not disclose.

Expected: authorization decided first, answering the one no-default-admin 400 whatever the request carries. Actual: no gate; the answer is decided from the ARN and the store.

## Evidence

Behaviour grounding: the administrator-only rule and the live refusal message are documented in `src/state.rs` lines 640-695 (`require_administrator`, `no_administrator_refusal`), and the gate is enforced by 13 sibling handlers. Handler: `src/ops/list_tags_for_resource.rs` handle (lines ~140-181) validates the ARN form and then reads `policy_id_for_arn` / `store.policies`, with no call to `require_administrator`. Test coverage: `tests/all/administrator_gated_operations.rs::gated_calls()` does not table ListTagsForResource, and `tests/all/tagging_and_managing_admins.rs` reads tags without associating an administrator — so no test asserts the gate. Distinct from the open ListTagsForResource wrong-behavior issue (only policy ARNs resolve), which concerns an authorised caller.

## Steps to Reproduce

1. Boot the emulator and do NOT call AssociateAdminAccount.
2. POST ListTagsForResource {"ResourceArn":"arn:aws:fms:us-east-1:000000000000:policy/11111111-2222-3333-4444-555555555555"}. Observed: 400 ResourceNotFoundException. Expected: 400 AccessDeniedException 'No default admin could be found for account 000000000000 in Region us-east-1'.
3. Associate an administrator, PutPolicy with a TagList, DisassociateAdminAccount and let the tear-down complete, then POST ListTagsForResource with the policy ARN. Observed: 200 with the TagList. Expected: the 400 no-default-admin refusal.
4. Contrast with GetNotificationChannel under the same precondition, which already answers the 400 refusal.
