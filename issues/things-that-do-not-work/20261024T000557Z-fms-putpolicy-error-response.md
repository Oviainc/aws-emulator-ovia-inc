Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# The policy operations skip the Firewall Manager administrator gate: PutPolicy/GetPolicy/DeletePolicy/ListPolicies succeed with no admin associated

_Filed: 2026-10-24T00:05:56Z_
_Operation: PutPolicy | Class: error-response_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Firewall Manager's policy operations are organisation-scoped and can only be called by the account that holds the Firewall Manager default-administrator association. With no association in the account/Region, the real service refuses them with a 400 whose Message is 'No default admin could be found for account <accountId> in Region <region>' (the exact behaviour this emulator already reproduces for eleven other operations through `crate::state::require_administrator` / `no_administrator_refusal`).

The four implemented policy operations never call that gate, so against a freshly booted emulator with no AssociateAdminAccount call:

* Cause 1 — PutPolicy answers 200 and PERSISTS a policy (minting PolicyId, PolicyUpdateToken and the policy ARN) where AWS refuses the write outright. This is the worst of the four: state that AWS would never have created becomes visible to every later read.
* Cause 2 — ListPolicies answers 200 with an (empty, or seeded) PolicyList instead of the refusal, so a client cannot distinguish 'no policies' from 'you are not the FMS administrator'.
* Cause 3 — GetPolicy answers 400 ResourceNotFoundException for any id instead of the administrator refusal, i.e. the wrong error code for the wrong reason.
* Cause 4 — DeletePolicy likewise answers ResourceNotFoundException (or 200 and DELETES a policy, if one was planted by the ungated PutPolicy) instead of refusing.

Expected: each of the four decides authorization FIRST — before the identifier is looked up, the payload validated, or anything written — and with no active administrator association answers the same 400 refusal the eleven gated operations already answer. Actual: the gate is absent from all four handlers.

## Evidence

Spec/behaviour grounding: FMS policy management is restricted to the Firewall Manager administrator account; the repo's own recorded trace `fms-no-default-admin-access-denied` (cited in the fixed issue 20261022T150923Z-fms-error-responses-carry-missing-or-non-conforming) records AWS answering GetResourceSet with 400 and Message 'No default admin could be found for account 000000000000 in Region us-west-2' when no administrator is configured. Handlers: `src/ops/put_policy.rs` (handle at line 197 — locks the store and goes straight to the create/update branch), `src/ops/get_policy.rs` (line 144), `src/ops/delete_policy.rs` (line 177), `src/ops/list_policies.rs` (line 169); none of them mentions `require_administrator`, while `grep require_administrator src/ops` shows it in batch_associate_resource, get_resource_set, get_protocols_list, get_notification_channel, put_notification_channel, delete_apps_list, delete_protocols_list, list_member_accounts, list_discovered_resources, get_third_party_firewall_association_status, disassociate_third_party_firewall and put_resource_set. Test coverage: `tests/all/administrator_gated_operations.rs::gated_calls()` tables eleven operations and none of the four policy operations, and `tests/all/policy_lifecycle.rs` drives PutPolicy without ever associating an administrator — so no test asserts the gate for these four.

## Steps to Reproduce

1. Boot the emulator and do NOT call AssociateAdminAccount.
2. POST / with X-Amz-Target: AWSFMS_20180101.PutPolicy and a minimal valid Policy ({"PolicyName":"p","SecurityServicePolicyData":{"Type":"WAFV2"},"ResourceType":"AWS::ElasticLoadBalancingV2::LoadBalancer","ExcludeResourceTags":false,"RemediationEnabled":false}). Observed: 200 with a minted PolicyId/PolicyArn. Expected: 400 with the no-default-admin refusal and nothing stored.
3. POST ListPolicies with {}. Observed: 200 with PolicyList containing the policy just written. Expected: 400 no-default-admin refusal.
4. POST GetPolicy and DeletePolicy with that PolicyId. Observed: 200 / 200. Expected: 400 no-default-admin refusal for both.
5. For contrast, POST GetResourceSet with any 22-character Identifier under the same precondition: the emulator already answers the 400 refusal, showing the inconsistency.
