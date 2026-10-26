Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8

# PutPolicy edits a policy it reports as OUT_OF_ADMIN_SCOPE, which the model says cannot be edited

_Filed: 2026-10-26T01:53:38Z_
_Operation: PutPolicy | Class: validation_

> _Note: this issue was identified against the code base at commit `6a69e82c081230a77b3ef80ecf5b0e8bc7e37fe8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutPolicy`'s UPDATE branch (a request whose `Policy.PolicyId` names a stored policy) resolves the record, checks the `PolicyUpdateToken` and writes the new document. It never consults the policy's derived `CustomerPolicyStatus`, even though the very same handler computes it one block later to build the response. So a caller whose own delegated `AdminScope` (written by `PutAdminAccount`) covers neither the policy's `SecurityServicePolicyData.Type` nor the Region the policy was created in is told by `GetPolicy`/`ListPolicies`/`PutPolicy` that the policy is `OUT_OF_ADMIN_SCOPE` — i.e. that it may view but not edit it — and is then allowed to edit it anyway, rotating the update token and replacing the stored document.

The refusal belongs on the UPDATE branch only: the model's sentence is about editing an existing policy, and the crate's own test `policy_status_derivation::the_policy_status_follows_the_scope_of_the_administrator_that_governs_it` CREATES policies under a narrowed scope and pins that the create answers 200 with `OUT_OF_ADMIN_SCOPE`, so the CREATE branch must stay as it is. `InvalidOperationException` ("the operation wasn't possible") is declared by `PutPolicy`, so the refusal needs no unmodelled shape. With no delegated administrator configured for the caller `policy_status` already returns `ACTIVE`, so the ordinary single-administrator path is untouched.

## Evidence

Smithy model, `com.amazonaws.fms#Policy$PolicyStatus` (smithy_query query="doc", shape="com.amazonaws.fms#Policy", member="PolicyStatus"):
"`ACTIVE` - The administrator can manage and delete the policy." / "`OUT_OF_ADMIN_SCOPE` - The administrator can view the policy, but they can't edit or delete the policy. Existing policy protections stay in place. Any new resources that come into scope of the policy won't be protected."

The crate restates that rule as its own comment — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:239-243:
```
/// The `CustomerPolicyStatus` of a policy that is OUTSIDE the administrative
/// scope governing it: "the administrator can view the policy, but they can't
/// edit or delete the policy" (`Policy$PolicyStatus`). One of
/// [`crate::ops::types::CUSTOMER_POLICY_STATUS_VALUES`].
pub const POLICY_STATUS_OUT_OF_ADMIN_SCOPE: &str = "OUT_OF_ADMIN_SCOPE";
```

The derivation already exists in this crate — state.rs:1527-1539:
```
    pub fn policy_status(&self, record: &PolicyRecord) -> &'static str {
        let Some(admin) = self.organization_admins.get(::engine::emulator_account_id()) else {
            return POLICY_STATUS_ACTIVE;
        };
        let security_service_type = record.policy.security_service_policy_data.r#type.as_str();
        if admin_scope_manages_policy_type(&admin.admin_scope, security_service_type)
            && admin_scope_manages_region(&admin.admin_scope, &record.region)
        {
            POLICY_STATUS_ACTIVE
        } else {
            POLICY_STATUS_OUT_OF_ADMIN_SCOPE
        }
    }
```

The UPDATE branch writes without ever asking for it — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/put_policy.rs:539-560:
```
        let record = match requested_id {
            // UPDATE: the id names an existing policy version, and the token must
            // match that version.
            Some(policy_id) => {
                let Some(existing) = store.policies.get(&policy_id) else {
                    return Err(PutPolicyErrorModeled::ResourceNotFoundException(
                        crate::state::resource_not_found(),
                    )
                    .into());
                };
                if requested_token.as_deref() != Some(existing.update_token.as_str()) {
                    return Err(PutPolicyErrorModeled::InvalidOperationException(
```
and the same handler proves the status is available to it — put_policy.rs:614-621:
```
        // The derived status, read from the store the same way every other
        // reader of this policy reads it.
        let policy_status = store.policy_status(&record);
        let output = PutPolicyOutput {
            policy: Some(record.policy(policy_status)),
```

Test coverage: tests/all/policy_status_derivation.rs pins the DERIVED value for `PutPolicy`(create)/`GetPolicy`/`ListPolicies` (lines 294-470) but no test calls `PutPolicy` with an existing `PolicyId` for a policy the caller's scope does not cover, so nothing pins the current permissive edit.

## Steps to Reproduce

1. POST / with X-Amz-Target `AWSFMS_20180101.AssociateAdminAccount`, body {"AdminAccount": "111111111111"} (an account other than the emulator caller, so the caller is not the default administrator)
2. POST / with X-Amz-Target `AWSFMS_20180101.PutPolicy`, body {"Policy": {"PolicyName": "p", "SecurityServicePolicyData": {"Type": "WAFV2"}, "ResourceType": "AWS::CloudFront::Distribution", "ExcludeResourceTags": false, "RemediationEnabled": false}} signed for us-east-1 — note the returned `PolicyId` and `PolicyUpdateToken`
3. POST / with X-Amz-Target `AWSFMS_20180101.PutAdminAccount`, body {"AdminAccount": "<the emulator account id>", "AdminScope": {"AccountScope": {"AllAccountsEnabled": true}, "RegionScope": {"Regions": ["eu-central-1"]}, "PolicyTypeScope": {"AllPolicyTypesEnabled": true}}} — the caller's scope now covers no Region the policy lives in
4. POST / with X-Amz-Target `AWSFMS_20180101.GetPolicy`, body {"PolicyId": "<the id>"} — `Policy.PolicyStatus` is "OUT_OF_ADMIN_SCOPE"
5. POST / with X-Amz-Target `AWSFMS_20180101.PutPolicy`, body {"Policy": {"PolicyId": "<the id>", "PolicyUpdateToken": "<the current token>", "PolicyName": "p-edited", "SecurityServicePolicyData": {"Type": "WAFV2"}, "ResourceType": "AWS::CloudFront::Distribution", "ExcludeResourceTags": false, "RemediationEnabled": false}} — answers 200 and the edit is persisted, although the model says the administrator "can view the policy, but they can't edit or delete the policy"; expected the modelled `InvalidOperationException`
