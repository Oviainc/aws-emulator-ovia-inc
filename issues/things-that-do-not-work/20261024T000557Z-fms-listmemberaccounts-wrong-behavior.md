Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# ListMemberAccounts invents the member list from policy scope maps instead of reporting the organization's accounts

_Filed: 2026-10-24T00:05:56Z_
_Operation: ListMemberAccounts | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

ListMemberAccounts must report 'the member accounts in the administrator's Amazon Web Services organization' — the account set Firewall Manager learns from Organizations, independent of any policy. The emulator instead synthesises the list from its own policy documents: `state::member_accounts()` returns the active administrator's account plus every AWSAccountId-shaped value found under the ACCOUNT key of any stored policy's IncludeMap OR ExcludeMap. Two observable divergences follow:

* An account that IS in the organization but is not named by any policy is never listed — on a fresh emulator with an administrator and no policies the operation reports exactly one account, where AWS reports every account in the org.
* An account named only in a policy's ExcludeMap — i.e. one explicitly taken OUT of that policy's scope, and possibly not an org member at all — is reported as a member account.

The repo hosts an Organizations emulator in the same engine (services/service-com-amazonaws-organizations-awsorganizationsv20161128) and cross-service reads of another service's store are an established pattern here, so the membership can be read from the organization rather than guessed from policy scope. Expected: the page is drawn from the organization's accounts. Actual: it is drawn from policy IncludeMap/ExcludeMap entries.

## Evidence

Model/docs: ListMemberAccounts returns 'a MemberAccounts object that lists the member accounts in the administrator's Amazon Web Services organization'; `CustomerPolicyScopeMap` entries are per-policy scope, and an ExcludeMap entry is by definition an account the policy does NOT apply to. Handler: `src/ops/list_member_accounts.rs` line 198 (`store.member_accounts()`); `src/state.rs` lines 531-553 build the set from `admin_account` plus both scope maps — the doc comment at lines 505-523 concedes 'Firewall Manager learns its organization's accounts from Organizations, which this emulator does not stand in for'. Cross-service precedent: `services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/list_s3_buckets.rs` reads another service's store through the shared engine. Test coverage: `tests/all/member_accounts.rs` asserts the current policy-derived derivation, not the organization-derived one.

## Steps to Reproduce

1. Call AssociateAdminAccount with account A; create accounts B and C in the Organizations emulator (or any org membership the engine holds).
2. POST ListMemberAccounts with {}. Observed: only A. Expected: the organization's accounts (A, B, C).
3. POST PutPolicy with a policy whose ExcludeMap is {"ACCOUNT":["999988887777"]} (an account in no organization), then POST ListMemberAccounts again. Observed: 999988887777 is now reported as a member account. Expected: it is not a member of the organization and must not appear.
