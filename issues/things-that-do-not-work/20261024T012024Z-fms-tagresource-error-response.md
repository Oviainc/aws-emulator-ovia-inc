Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 1f316ce023d643c000ad0b8fb7b5799232d706fd

# TagResource skips the Firewall Manager administrator gate and writes tags with no admin associated

_Filed: 2026-10-24T01:20:24Z_
_Operation: TagResource | Class: error-response_

> _Note: this issue was identified against the code base at commit `1f316ce023d643c000ad0b8fb7b5799232d706fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

TagResource is an organisation-scoped Firewall Manager call: it is answered only for a Firewall Manager administrator (or the organisation's management account). With no default-administrator association in the account/Region, the live service refuses it with a 400 whose Message is 'No default admin could be found for account <accountId> in Region <region>' — the refusal this emulator already reproduces for a dozen other operations through `state::require_administrator` / `state::no_administrator_refusal`, and decides BEFORE any identifier is looked up or any payload validated.

This handler never calls that gate. Against a freshly booted emulator with no AssociateAdminAccount call it instead (a) answers 400 ValidationException for a malformed ARN, (b) answers 400 InvalidInputException for a duplicate tag key, (c) answers 400 ResourceNotFoundException for a well-formed ARN naming nothing, and (d) answers 200 and PERSISTS tags onto a policy if one exists — all where AWS answers the one administrator refusal. The leak in (c)/(d) is the point of deciding authorization first: an unauthorised caller learns from the error code whether the named resource exists, and can mutate tag state AWS would never have let it touch.

Expected: authorization decided first, with the same 400 no-default-admin refusal every gated operation answers, and nothing written. Actual: no gate; the request's shape and the store decide the answer.

## Evidence

Behaviour grounding: FMS organisation-scoped calls are restricted to the Firewall Manager administrator; `src/state.rs` lines 640-695 document that rule (quoting ListMemberAccounts: "Either an Firewall Manager administrator or the organization's management account can make this request") and `no_administrator_refusal` reproduces the recorded live message 'No default admin could be found for account {} in Region {region}'. Handler: `src/ops/tag_resource.rs` handle (lines 166-244) locks the store and goes straight to ARN-format validation, duplicate-key validation and `policy_id_for_arn`, and never mentions `require_administrator`; `grep require_administrator src/ops` lists batch_associate_resource, delete_apps_list, delete_protocols_list, disassociate_third_party_firewall, get_notification_channel, get_protocols_list, get_resource_set, get_third_party_firewall_association_status, list_discovered_resources, list_member_accounts, put_notification_channel, put_protocols_list and put_resource_set — but not tag_resource. Test coverage: `tests/all/administrator_gated_operations.rs::gated_calls()` tables the gated operations and does not include TagResource; `tests/all/tagging_and_managing_admins.rs` drives TagResource without ever associating an administrator, so no test asserts the gate here. Distinct from the open TagResource wrong-behavior issue (ARN resolution limited to policies), which is about an authorised caller.

## Steps to Reproduce

1. Boot the emulator and do NOT call AssociateAdminAccount.
2. POST / with X-Amz-Target: AWSFMS_20180101.TagResource and {"ResourceArn":"arn:aws:fms:us-east-1:000000000000:policy/11111111-2222-3333-4444-555555555555","TagList":[{"Key":"k","Value":"v"}]}. Observed: 400 ResourceNotFoundException. Expected: 400 AccessDeniedException with Message 'No default admin could be found for account 000000000000 in Region us-east-1'.
3. Associate an administrator, PutPolicy a policy, then DisassociateAdminAccount and wait for the tear-down to complete; POST TagResource with the policy's ARN. Observed: 200 and the tags are persisted (readable once an administrator is re-associated). Expected: the 400 no-default-admin refusal with nothing written.
4. For contrast, POST GetResourceSet with any 22-character Identifier under the same precondition: the emulator already answers the 400 refusal, showing the inconsistency.
