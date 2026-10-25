Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# PutPolicy stores and echoes the caller's PolicyStatus instead of deriving the service-owned admin-scope status

_Filed: 2026-10-24T00:05:56Z_
_Operation: PutPolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Policy$PolicyStatus` (CustomerPolicyStatus: ACTIVE | OUT_OF_ADMIN_SCOPE) is a SERVICE-derived field: it 'indicates whether the policy is in or out of an admin's policy or Region scope', i.e. it is computed from the administrator's AdminScope, not chosen by the caller. The handler only defaults it when absent — a request that sends PolicyStatus=OUT_OF_ADMIN_SCOPE has that value stored and echoed back by PutPolicy, GetPolicy and the ListPolicies summary. On the real service a policy created by the administrator inside its own scope comes back ACTIVE whatever the request said, and OUT_OF_ADMIN_SCOPE is reachable only by changing the admin scope. The same crate already does the right thing for the analogous derived member: PutResourceSet clears `resource_set_status` and overlays the record's own status for every reader.

Expected: the caller's PolicyStatus is ignored (stripped like PolicyId/PolicyUpdateToken) and the stored/reported value is derived from the administrator scope. Actual: the input value is persisted as-is, so a client can plant a status the service would never report.

## Evidence

Model (smithy_query shape com.amazonaws.fms#Policy, member PolicyStatus): 'Indicates whether the policy is in or out of an admin's policy or Region scope … ACTIVE - the administrator can manage and delete the policy; OUT_OF_ADMIN_SCOPE - the administrator can view the policy, but they can't edit or delete the policy' — a scope-derived condition, not customer input. Handler: `src/ops/put_policy.rs` lines 222-227 take out only `policy_id` and `policy_update_token` and set `policy_status` solely when `is_none()`; contrast `src/ops/put_resource_set.rs` lines 205-206, which clear `last_update_time` and `resource_set_status` because the record owns them. Test coverage: `tests/all/policy_lifecycle.rs` lines 192-207 and 330-338 currently ASSERT the echo (expected status = the request's PolicyStatus when present), so the divergent behaviour is pinned by a test that would have to change with the fix.

## Steps to Reproduce

1. Call AssociateAdminAccount.
2. POST PutPolicy with a valid policy document that includes "PolicyStatus":"OUT_OF_ADMIN_SCOPE".
3. Observed: 200 and the response (and a later GetPolicy / ListPolicies summary) reports PolicyStatus=OUT_OF_ADMIN_SCOPE. Expected: the service-derived status for a policy the administrator created in scope — ACTIVE.
