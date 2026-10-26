Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: efbe6fec395222c954ec73ab018b3346b3354542

# ResourceSetStatus is a hardcoded ACTIVE, never derived from the administrator's Region scope the way PolicyStatus is

_Filed: 2026-10-25T15:37:02Z_
_Operation: GetResourceSet | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `efbe6fec395222c954ec73ab018b3346b3354542`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every reader of a resource set (GetResourceSet, PutResourceSet, ListResourceSets) reports `ResourceSetStatus` through `ResourceSetRecord::resource_set()`, which assigns the constant `RESOURCE_SET_STATUS_ACTIVE` unconditionally. The model documents that member as the in/out-of-admin-Region-scope indicator, and this very crate already derives exactly that distinction for the analogous `Policy$PolicyStatus` in `ComAmazonawsFmsAwsfms20180101State::policy_status`, using the `admin_scope_manages_region` helper that lives beside it. So a caller whose own delegated `AdminScope` (written by PutAdminAccount) covers no Region, or covers only other Regions, is still told every resource set is `ACTIVE` — i.e. that it may edit and delete it — while a policy created in the same Region by the same caller is correctly reported `OUT_OF_ADMIN_SCOPE`. The value is a canned constant rather than a value computed from state, and the function's own doc comment claims the opposite of what the code does.

## Evidence

Smithy model, `com.amazonaws.fms#ResourceSet$ResourceSetStatus` (smithy_query query="shape"): "Indicates whether the resource set is in or out of an admin's Region scope." / "ACTIVE - The administrator can manage and delete the resource set." / "OUT_OF_ADMIN_SCOPE - The administrator can view the resource set, but they can't edit or delete the resource set."

The reader hardcodes it, under a comment that says it comes from the record — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:391-402:
```
    /// The stored resource set as `PutResourceSet` / `GetResourceSet` report it:
    /// the caller's own members, with the service-owned `Id`, `UpdateToken`,
    /// `LastUpdateTime` and `ResourceSetStatus` filled in from the record, so a
    /// read never reports a stale token or a status the caller invented.
    pub fn resource_set(&self) -> ResourceSet {
        ...
        resource_set.resource_set_status = Some(RESOURCE_SET_STATUS_ACTIVE.to_string());
```
and the constant itself asserts the premise rather than checking it — state.rs:245-248:
```
/// The default `ResourceSetStatus` of a resource set an administrator created:
/// it is in the administrator's Region scope, so the administrator can manage
/// and delete it. One of [`crate::ops::types::RESOURCE_SET_STATUS_VALUES`].
pub const RESOURCE_SET_STATUS_ACTIVE: &str = "ACTIVE";
```

The sibling derivation this crate already performs for the identically-documented policy member — state.rs:1457-1468:
```
    pub fn policy_status(&self, record: &PolicyRecord) -> &'static str {
        let Some(admin) = self.organization_admins.get(::engine::emulator_account_id()) else {
            return POLICY_STATUS_ACTIVE;
        };
        let security_service_type = record.policy.security_service_policy_data.r#type.as_str();
        if admin_scope_manages_policy_type(&admin.admin_scope, security_service_type)
            && admin_scope_manages_region(&admin.admin_scope, &record.region)
        { POLICY_STATUS_ACTIVE } else { POLICY_STATUS_OUT_OF_ADMIN_SCOPE }
```

The handler that returns it — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_resource_set.rs:172-175:
```
        Ok(GetResourceSetOutput {
            resource_set: record.resource_set(),
            resource_set_arn: record.arn.clone(),
        })
```

Test coverage: tests/all/administrator_gated_operations.rs:596-602 only asserts the reported status is a member of the model's catalog (`statuses.contains(&reported_status)`), and tests/all/resource_set_listing.rs:154 copies `stored["ResourceSetStatus"]` into the expected summary — neither pins ACTIVE with a stated reason, while tests/all/policy_status_derivation.rs:1-26 pins the full derivation for the policy twin.

## Steps to Reproduce

1. POST / with X-Amz-Target AWSFMS_20180101.AssociateAdminAccount and body {"AdminAccount": "111111111111"} (an account other than the caller, so the caller is not the default administrator)
2. POST / with X-Amz-Target AWSFMS_20180101.PutAdminAccount and body {"AdminAccount": "<the emulator account id>", "AdminScope": {"RegionScope": {"Regions": ["eu-central-1"]}, "AccountScope": {"AllAccountsEnabled": true}, "PolicyTypeScope": {"AllPolicyTypesEnabled": true}}} — the caller's scope now covers only eu-central-1
3. Sign a PutResourceSet request for us-east-1 with body {"ResourceSet": {"Name": "scoped-set", "ResourceTypeList": ["AWS::EC2::VPC"]}} — 200
4. POST / with X-Amz-Target AWSFMS_20180101.GetResourceSet and body {"Identifier": "<the minted Id>"} signed for us-east-1 — ResourceSetStatus is "ACTIVE" although the caller's AdminScope covers no Region the set lives in; a PutPolicy/GetPolicy of a policy created in the same Region reports "OUT_OF_ADMIN_SCOPE" for the same caller
