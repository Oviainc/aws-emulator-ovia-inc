Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 3f59b13755f238905b4b26aae43e5ec6021c53f3

# A delegated administrator's AdminScope governs PolicyStatus/ResourceSetStatus from the instant PutAdminAccount records it, although the crate's sibling reader (and the status constant's own doc) say the scope only takes effect at ONBOARDING_COMPLETE

_Filed: 2026-10-29T04:43:50Z_
_Operation: DeletePolicy | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `3f59b13755f238905b4b26aae43e5ec6021c53f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutAdminAccount` records a delegated administrator in `ONBOARDING` and lets the clock complete it (`settles_to = ORGANIZATION_STATUS_ONBOARDING_COMPLETE`, `settles_at = now + ADMIN_ONBOARDING_SPAN`, put_admin_account.rs:298-311). Three readers of that same `organization_admins` record then disagree about when the record's `admin_scope` is in force:

* `ComAmazonawsFmsAwsfms20180101State::managing_admin_accounts` applies `admin_scope_manages_account` ONLY to an administrator whose `status_at(now)` is `ORGANIZATION_STATUS_ONBOARDING_COMPLETE` (state.rs:1584-1587);
* `ComAmazonawsFmsAwsfms20180101State::policy_status` (state.rs:1733-1745) and `resource_set_status` (state.rs:1773-1782) take the same record with `organization_admins.get(::engine::emulator_account_id())` and apply `admin_scope_manages_policy_type` / `admin_scope_manages_region` with NO status check at all.

So in the `ADMIN_ONBOARDING_SPAN` window right after `PutAdminAccount` — while `GetAdminScope` and `ListAdminAccountsForOrganization` still report the account `ONBOARDING`, i.e. an account that cannot "perform actions on the resources defined in their AdminScope" yet (the sentence state.rs:173-174 quotes from `AdminAccountSummary$Status` for `ONBOARDING_COMPLETE`) — that not-yet-onboarded scope already decides every policy's and every resource set's status. A narrow scope therefore reports stored policies as `OUT_OF_ADMIN_SCOPE` and makes `DeletePolicy` and `PutPolicy` refuse them with the modelled `InvalidOperationException` through `policy_mutation_refusal` (delete_policy.rs:217-223), and makes `DeleteResourceSet`, `PutResourceSet`, `BatchAssociateResource` and `BatchDisassociateResource` refuse the resource set through `resource_set_mutation_refusal`, before the administrator the scope belongs to has been onboarded at all. Deleting a policy that the organisation's default administrator (whose scope is "full administrative scope", state.rs:2395-2402) still governs is thus refused for a span.

The two status derivations also never `settle` the record nor take the request's `now` — they read `admin_scope` out of whatever is stored — so there is no instant at which they consult the onboarding the record itself carries, unlike `managing_admin_accounts`, `organization_admin_at` and `admin_account_summaries`, which all take `now`.

## Evidence

The sibling reader of the SAME `organization_admins` record requires the onboarding to be complete before the scope counts — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:1581-1589:
```
        accounts.extend(
            self.organization_admins
                .values()
                .filter(|admin| {
                    admin.status_at(now) == ORGANIZATION_STATUS_ONBOARDING_COMPLETE
                        && admin_scope_manages_account(&admin.admin_scope, caller)
                })
                .map(|admin| admin.account_id.clone()),
        );
```

The policy status derivation applies the same record's scope with no status check — src/state.rs:1733-1745:
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
and the resource-set twin — src/state.rs:1773-1782:
```
    pub fn resource_set_status(&self, record: &ResourceSetRecord) -> &'static str {
        let Some(admin) = self.organization_admins.get(::engine::emulator_account_id()) else {
            return RESOURCE_SET_STATUS_ACTIVE;
        };
        if admin_scope_manages_region(&admin.admin_scope, &record.region) {
```

The rule both sides are measured against, as this crate itself states it from the model — src/state.rs:167-177:
```
/// `OrganizationStatus` of an administrator account the service is onboarding
/// as a Firewall Manager administrator ("the account is onboarding to Firewall
/// Manager as an administrator", `AdminAccountSummary$Status`). One of
/// [`crate::ops::types::ORGANIZATION_STATUS_VALUES`].
pub const ORGANIZATION_STATUS_ONBOARDING: &str = "ONBOARDING";

/// `OrganizationStatus` of an administrator account that is onboarded and "can
/// perform actions on the resources defined in their AdminScope"
/// (`AdminAccountSummary$Status`). One of
/// [`crate::ops::types::ORGANIZATION_STATUS_VALUES`].
pub const ORGANIZATION_STATUS_ONBOARDING_COMPLETE: &str = "ONBOARDING_COMPLETE";
```
The Smithy sentence those docs quote, `smithy_query(query="shape", shape="AdminAccountSummary")` on `Status`: "`ONBOARDING` - The account is onboarding to Firewall Manager as an administrator." / "`ONBOARDING_COMPLETE` - Firewall Manager The account is onboarded to Firewall Manager as an administrator, and can perform actions on the resources defined in their AdminScope."

The window is non-zero and stored on the record — src/state.rs:189-200:
```
/// How long an administrator account `PutAdminAccount` onboarded stays
/// `ONBOARDING` before the clock makes `ONBOARDING_COMPLETE` due.
...
pub const ADMIN_ONBOARDING_SPAN: Duration = Duration::from_millis(500);
```
and src/ops/put_admin_account.rs:298-311 creates the record in `ORGANIZATION_STATUS_ONBOARDING` with `settles_at: Some(now + crate::state::ADMIN_ONBOARDING_SPAN)`.

The refusal this drives on the operation named here — src/ops/delete_policy.rs:217-223:
```
        if let Some(refusal) = store
            .policies
            .get(&input.policy_id)
            .and_then(|existing| store.policy_mutation_refusal(existing))
        {
            return Err(DeletePolicyErrorModeled::InvalidOperationException(refusal).into());
        }
```

Test coverage today: tests/all/policy_status_derivation.rs (`put_admin_scope`, lines 168-181) onboards the calling account and reads `GetPolicy`/`ListPolicies` immediately, and tests/all/policy_mutation_admin_scope.rs and tests/all/resource_set_mutation_admin_scope.rs drive the same scope table through `PutPolicy`/`DeletePolicy` and the resource-set mutators. None of those suites mentions the administrator's `OrganizationStatus` at all — their stated rule is only "the derived value follows the administrative scope that governs the policy" (policy_status_derivation.rs:20-23) — and tests/all/get_admin_scope_lifecycle_status.rs:148-161 is the only suite that exercises the `ONBOARDING` vs `ONBOARDING_COMPLETE` distinction, through `GetAdminScope` alone. So nothing pins the scope of a still-`ONBOARDING` administrator being in force.

## Steps to Reproduce

1. Boot the emulator and call `AssociateAdminAccount` with `{"AdminAccount": "111111111111"}` (an account other than the emulator's own, so the caller can hold a narrowed scope — `PutAdminAccount` refuses to scope the default administrator) → 200.
2. Call `PutPolicy` with a minimal `WAFV2` policy (`PolicyName`, `SecurityServicePolicyData.Type = WAFV2`, `ResourceType = AWS::ElasticLoadBalancingV2::LoadBalancer`, `ExcludeResourceTags = false`, `RemediationEnabled = false`) signed for us-east-1 → 200; note the minted `PolicyId`. `GetPolicy` reports `PolicyStatus: ACTIVE`.
3. Call `PutAdminAccount` with `{"AdminAccount": "<the emulator account id>", "AdminScope": {"AccountScope": {"AllAccountsEnabled": true}, "PolicyTypeScope": {"AllPolicyTypesEnabled": true}, "RegionScope": {"Regions": ["eu-west-1"]}}}` → 200. The record is created `ONBOARDING` with `settles_at = now + 500 ms`.
4. Within that 500 ms, call `GetAdminScope` for the same account → `Status: ONBOARDING`, i.e. an account that per `AdminAccountSummary$Status` "can perform actions on the resources defined in their AdminScope" only once the status is `ONBOARDING_COMPLETE`.
5. In the same window, call `GetPolicy` for the policy from step 2 and then `DeletePolicy` for it.
6. Expected (the rule state.rs:1584-1587 applies to the identical record): the still-`ONBOARDING` administrator's scope does not govern yet, so the policy is reported `ACTIVE` and the delete is performed.
7. Actual: `policy_status` consults the `ONBOARDING` record's `admin_scope`, reports `OUT_OF_ADMIN_SCOPE`, and `DeletePolicy` answers 400 `InvalidOperationException` from `policy_mutation_refusal`. The same window makes `PutPolicy`, `PutResourceSet`, `DeleteResourceSet`, `BatchAssociateResource` and `BatchDisassociateResource` refuse too.
