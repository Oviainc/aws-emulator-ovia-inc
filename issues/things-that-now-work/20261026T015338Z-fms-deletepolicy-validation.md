Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# DeletePolicy deletes a policy reported as OUT_OF_ADMIN_SCOPE, which the model says cannot be deleted

_Filed: 2026-10-26T01:53:38Z_
_Operation: DeletePolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeletePolicy` removes whatever policy the `PolicyId` names, with no reference to the policy's derived `CustomerPolicyStatus`. The model documents that an `OUT_OF_ADMIN_SCOPE` policy can be viewed but not deleted, and this crate already derives that status (`ComAmazonawsFmsAwsfms20180101State::policy_status`) and reports it from `GetPolicy`, `PutPolicy` and the `ListPolicies` summary. So a caller holding a delegated `AdminScope` (written by `PutAdminAccount`) that covers neither the policy's type nor its Region is told the policy is out of its scope and can then delete it — together with the policy's violations and recorded attacks when `DeleteAllPolicyResources` is set.

This is not the administrator gate that a previous fix deliberately removed from `DeletePolicy`: with no delegated administrator configured for the caller, `policy_status` returns `ACTIVE` (state.rs:1528-1530), so the no-association path stays ungated exactly as that fix requires. The refusal is only for the narrowed-scope case, and `InvalidOperationException` ("the operation failed because there was nothing to do or the operation wasn't possible") is one of `DeletePolicy`'s own modelled errors, which is what the handler's doc demands of any refusal it answers.

## Evidence

Smithy model, `com.amazonaws.fms#Policy$PolicyStatus` (smithy_query query="doc", shape="com.amazonaws.fms#Policy", member="PolicyStatus"):
"`OUT_OF_ADMIN_SCOPE` - The administrator can view the policy, but they can't edit or delete the policy. Existing policy protections stay in place."

The crate's own comment on the constant — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:239-243:
```
/// The `CustomerPolicyStatus` of a policy that is OUTSIDE the administrative
/// scope governing it: "the administrator can view the policy, but they can't
/// edit or delete the policy" (`Policy$PolicyStatus`). One of
/// [`crate::ops::types::CUSTOMER_POLICY_STATUS_VALUES`].
pub const POLICY_STATUS_OUT_OF_ADMIN_SCOPE: &str = "OUT_OF_ADMIN_SCOPE";
```

The handler deletes without consulting it — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/delete_policy.rs:204-219:
```
        let mut store = state.lock();
        // `PolicyId` is an opaque lookup key: the store decides the answer, and
        // an id no stored policy carries is the modelled not-found, so nothing
        // is removed. The request's cleanup selector decides how far the
        // removal reaches — the policy alone, or the policy and everything this
        // store holds on its behalf.
        if store
            .delete_policy(&input.policy_id, input.delete_all_policy_resources)
            .is_none()
        {
            return Err(DeletePolicyErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            )
            .into());
        }
        Ok(DeletePolicyOutput)
```
while the status it would need is a method on the store it already holds — state.rs:1527-1539 (`pub fn policy_status(&self, record: &PolicyRecord) -> &'static str`), used by `GetPolicy` at ops/get_policy.rs:180 (`policy: Some(record.policy(store.policy_status(record)))`).

The handler's own doc accepts a modelled refusal — delete_policy.rs:14-17:
```
//! administrator association. This operation does NOT take
//! [`crate::state::require_administrator`]: the only refusals it can answer are
//! the ones its model declares, and an authorization code is declared for no
//! operation of this service.
```
and `InvalidOperationException` IS declared for `DeletePolicy` (OPERATIONS.md:67).

Test coverage: tests/all/policy_lifecycle.rs and tests/all/policy_resource_cleanup.rs exercise `DeletePolicy` only with the caller holding no narrowed scope, so no test pins deletion of an out-of-scope policy.

## Steps to Reproduce

1. POST / with X-Amz-Target `AWSFMS_20180101.AssociateAdminAccount`, body {"AdminAccount": "111111111111"} (not the emulator caller account)
2. POST / with X-Amz-Target `AWSFMS_20180101.PutPolicy`, body {"Policy": {"PolicyName": "p", "SecurityServicePolicyData": {"Type": "WAFV2"}, "ResourceType": "AWS::CloudFront::Distribution", "ExcludeResourceTags": false, "RemediationEnabled": false}} signed for us-east-1 — note the returned `PolicyId`
3. POST / with X-Amz-Target `AWSFMS_20180101.PutAdminAccount`, body {"AdminAccount": "<the emulator account id>", "AdminScope": {"AccountScope": {"AllAccountsEnabled": true}, "RegionScope": {"Regions": ["eu-central-1"]}, "PolicyTypeScope": {"AllPolicyTypesEnabled": true}}}
4. POST / with X-Amz-Target `AWSFMS_20180101.GetPolicy`, body {"PolicyId": "<the id>"} — `Policy.PolicyStatus` is "OUT_OF_ADMIN_SCOPE"
5. POST / with X-Amz-Target `AWSFMS_20180101.DeletePolicy`, body {"PolicyId": "<the id>", "DeleteAllPolicyResources": true} — answers 200 and the policy is gone, although the model says the administrator "can view the policy, but they can't edit or delete the policy"; expected the modelled `InvalidOperationException`
