Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7b44f3f32d1f58d662874eb8ca0f546e296cee00

# Bootstrap the fms (AWS Firewall Manager) emulator

_Filed: 2026-10-21T21:08:44Z_
_Operation: service-com-amazonaws-fms-awsfms-20180101 | Class: missing_

> _Note: this issue was identified against the code base at commit `7b44f3f32d1f58d662874eb8ca0f546e296cee00`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 42 operations in `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/` is an unmodified scaffold answering HTTP 501 `NotImplemented` with zero stored state, so no client workflow works at all. Implement a FIRST, SMALL slice: the Firewall Manager policy lifecycle, plus the admin-account association that gates it, backed by real persisted state in `ComAmazonawsFmsAwsfms20180101State`.

Wire protocol: awsJson1_1, `POST /` with `X-Amz-Target: AWSFMS_20180101.<OperationName>`; signing name `fms`.

Happy path to implement, in order:

1. **AssociateAdminAccount** — input `AdminAccount` (string, 1..1024, `^[0-9]+$`, required); output has no members. Must persist the admin account so GetAdminAccount can report it. If an admin account is already associated with a DIFFERENT value, fail with `InvalidOperationException`; re-associating the same account is idempotent.
2. **GetAdminAccount** — no input; output `AdminAccount` (string) and `RoleStatus` (AccountRoleStatus enum: READY | CREATING | PENDING_DELETION | DELETING | DELETED). Must return the account stored by step 1 and `RoleStatus = READY`; when nothing was ever associated, return `ResourceNotFoundException`.
3. **PutPolicy** — input `Policy` (Policy, required) + `TagList` (list<Tag>, 0..200); output `Policy` and `PolicyArn`. On CREATE (`Policy.PolicyId` absent): generate a 36-char id matching `^[a-z0-9A-Z-]{36}$` (a UUID works), generate a `PolicyUpdateToken`, build `PolicyArn` as `arn:aws:fms:<region>:<account>:policy/<PolicyId>`, store the full Policy (PolicyName, SecurityServicePolicyData{Type, ManagedServiceData, PolicyOption}, ResourceType, ResourceTypeList, ResourceTags, ExcludeResourceTags, RemediationEnabled, DeleteUnusedFMManagedResources, IncludeMap, ExcludeMap, ResourceSetIds, PolicyDescription, PolicyStatus) and the TagList, then echo back the stored Policy *including* the generated PolicyId/PolicyUpdateToken. On UPDATE (`PolicyId` present): the request's `PolicyUpdateToken` must equal the stored one — mismatch is `InvalidOperationException`, unknown PolicyId is `ResourceNotFoundException` — and a successful update must rotate `PolicyUpdateToken`. Default `PolicyStatus` to `ACTIVE`. No required member may be silently dropped: everything sent must be readable back by GetPolicy.
4. **GetPolicy** — input `PolicyId` (36 chars, required); output `Policy` + `PolicyArn`. Must return the stored policy verbatim (same PolicyId, PolicyName, SecurityServicePolicyData, current PolicyUpdateToken) and the same ARN PutPolicy returned. Unknown id → `ResourceNotFoundException`.
5. **ListPolicies** — input `NextToken` (1..4096), `MaxResults` (1..100); output `PolicyList` (list<PolicySummary>) + `NextToken`. Each summary must be projected from stored state: `PolicyArn`, `PolicyId`, `PolicyName`, `ResourceType`, `SecurityServiceType` (= stored `SecurityServicePolicyData.Type`), `RemediationEnabled`, `DeleteUnusedFMManagedResources`, `PolicyStatus`. Honour `MaxResults` as a page bound and return an opaque `NextToken` only when more policies remain; a token round-trips to the next page and omitting the token on the last page.
6. **DeletePolicy** — input `PolicyId` (required) + `DeleteAllPolicyResources` (boolean, default false); empty output. Must remove the policy (and its tags) from state so a subsequent GetPolicy returns `ResourceNotFoundException` and ListPolicies no longer lists it. Deleting an unknown id → `ResourceNotFoundException`.

NEVER STUBS: each of these six must compute its response from the shared engine state reached via `crate::state::service_state(&*self.engine)`. Hardcoded policies, echoing the input Policy without storing it, or generating a PolicyId that is never persisted are failures of this issue, not implementations of it. Errors must be the modeled awsJson1_1 shapes (`ResourceNotFoundException`, `InvalidInputException`, `InvalidOperationException`) at HTTP 400, not 501 and not 500.

## State Model

In `src/state.rs`, on `ComAmazonawsFmsAwsfms20180101State`:

- `admin_account: Option<AdminAccountRecord>` where `AdminAccountRecord { account_id: String, role_status: AccountRoleStatus /* READY */, associated_at: SystemTime }` — written by AssociateAdminAccount, read by GetAdminAccount, cleared by DisassociateAdminAccount later.
- `policies: BTreeMap<String /* PolicyId */, PolicyRecord>` (BTreeMap so ListPolicies has a stable, deterministic order for paging) where `PolicyRecord { policy_id: String, policy_arn: String, update_token: String, policy: Policy /* the full modeled struct as stored */, tags: Vec<Tag>, created_at: SystemTime, updated_at: SystemTime }`.
- `tags_by_arn: HashMap<String /* ResourceArn */, Vec<Tag>>` — populated from `PutPolicy.TagList` now so the later TagResource/UntagResource/ListTagsForResource slice has a home; keyed by the ARN PutPolicy minted.

Everything else (apps lists, protocols lists, resource sets, compliance/violation reporting, notification channel, third-party firewall, admin scope, member accounts, tagging ops) stays 501 for now and is tracked by separate per-operation gap issues.

## Evidence

`grep -rl '^//! SCAFFOLDED, NOT IMPLEMENTED:' services/service-com-amazonaws-fms-awsfms-20180101/src/ops/` matches all 42 operation files, including `put_policy.rs`, `get_policy.rs`, `list_policies.rs`, `delete_policy.rs`, `associate_admin_account.rs`, `get_admin_account.rs` — each returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` (HTTP 501) and touches no state. Field names, requiredness, string lengths/patterns, MaxResults range 1..100 and the declared error sets are taken from `services/service-com-amazonaws-fms-awsfms-20180101/OPERATIONS.md` (sections AssociateAdminAccount, GetAdminAccount, PutPolicy, GetPolicy, ListPolicies, DeletePolicy; awsJson1_1 / `AWSFMS_20180101.<Op>` target from its Wire protocol section). The `Policy` / `PolicySummary` / `SecurityServicePolicyData` member lists and the PolicyUpdateToken rule ("the PolicyUpdateToken in the request must match the PolicyUpdateToken of the current policy version") come from the Smithy model shapes `com.amazonaws.fms#Policy`, `#PolicySummary`, `#SecurityServicePolicyData`. ListPolicies is `@paginated(inputToken=NextToken, outputToken=NextToken, items=PolicyList)` per OPERATIONS.md. No tests exercise any of these operations today.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI `fms` client at it.
2. `aws fms associate-admin-account --admin-account 123456789012` → expect HTTP 200 with an empty body (today: 501 NotImplemented).
3. `aws fms get-admin-account` → expect `AdminAccount=123456789012`, `RoleStatus=READY`, computed from the previous call's state.
4. `aws fms put-policy --policy '{"PolicyName":"demo","SecurityServicePolicyData":{"Type":"WAFV2","ManagedServiceData":"{\"type\":\"WAFV2\"}"},"ResourceType":"AWS::ElasticLoadBalancingV2::LoadBalancer","ExcludeResourceTags":false,"RemediationEnabled":false}'` → expect 200 with a generated 36-char `Policy.PolicyId`, a `Policy.PolicyUpdateToken`, and a `PolicyArn` of the form `arn:aws:fms:<region>:<account>:policy/<PolicyId>`.
5. `aws fms get-policy --policy-id <PolicyId>` → expect the exact policy just stored (PolicyName `demo`, SecurityServicePolicyData.Type `WAFV2`, same PolicyUpdateToken, same PolicyArn).
6. `aws fms list-policies --max-results 1` → expect exactly one PolicySummary whose PolicyId/PolicyName/SecurityServiceType match the stored policy; after a second put-policy, expect a `NextToken` that round-trips to the second page and no token on the final page.
7. `aws fms put-policy` with the same PolicyId but a stale `PolicyUpdateToken` → expect `InvalidOperationException` (HTTP 400); with the current token → expect 200 and a rotated token.
8. `aws fms delete-policy --policy-id <PolicyId>` → expect 200, then `aws fms get-policy --policy-id <PolicyId>` → expect `ResourceNotFoundException` (HTTP 400) and `list-policies` no longer includes it.
9. `aws fms get-policy --policy-id 00000000-0000-0000-0000-000000000000` on a fresh server → expect `ResourceNotFoundException`, not 501.
