Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 9ba218dfac78ec8aaaf32d0b8457457c6bd04b91

# GetProtectionStatus with no MemberAccountId reports attacks of accounts the policy does not apply to, contradicting its own comment and its own not-found rule for a named out-of-scope account

_Filed: 2026-10-29T17:35:14Z_
_Operation: GetProtectionStatus | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `9ba218dfac78ec8aaaf32d0b8457457c6bd04b91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler resolves a NAMED `MemberAccountId` against the policy's own scope and answers the modelled `ResourceNotFoundException` for an account the policy does not apply to, on the stated ground that "the policy holds no attack report of it". But the attack selection that follows applies NO policy-scope filter at all when the member account is absent: `is_none_or(..)` lets every attack recorded under the `PolicyId` through, whatever account it names. The comment sitting directly above that filter states the opposite rule — "absent, every account in the policy's scope is reported".

So the two paths of one handler disagree about the same (policy, account) pair: `GetProtectionStatus {PolicyId, MemberAccountId: X}` answers 400 `ResourceNotFoundException` for an out-of-scope X, while `GetProtectionStatus {PolicyId}` serves that same X's attack inside the `Data` document — and the caller is then told the account it just read an attack for does not exist. The shared resolution the refusal goes through states the invariant the unfiltered page breaks: "an account outside that scope was never evaluated against this policy and no surface of the policy holds anything about it — which is the modelled not-found, not an empty report".

The attack inventory is populated through the store (no handler writes `shield_attacks`, by design: "the inventory holds exactly what something put in it"), so the divergence shows up as soon as the inventory holds an attack for an account the policy's `IncludeMap`/`ExcludeMap` scope leaves out — e.g. after a policy's scope is narrowed by a later `PutPolicy`.

## Evidence

The filter and the comment that states the rule it does not implement — services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_protection_status.rs:289-297:
```
            // The member account the request names is resolved above, so what is
            // left here is the selection: it picks the attacks on THAT account's
            // resources; absent, every account in the policy's scope is reported.
            .filter(|attack| {
                input
                    .member_account_id
                    .as_deref()
                    .is_none_or(|account| attack.account_id == account)
            })
```
The same handler's refusal for a NAMED out-of-scope account — src/ops/get_protection_status.rs:232-251:
```
        // `MemberAccountId` names "the Amazon Web Services account that is in
        // scope of the policy that you want to get the details for", so it is a
        // MEMBER account of the policy and is RESOLVED against the policy's own
        // scope ... An account the policy does not apply to was never
        // evaluated against this policy, so the policy holds no attack report of
        // it and the pair is the same modelled not-found as an unknown policy —
        // not an empty page ...
        if input
            .member_account_id
            .as_deref()
            .is_some_and(|account| !store.policy_applies_to_account(&policy.policy, account))
        {
            return Err(GetProtectionStatusErrorModeled::ResourceNotFoundException(
                crate::state::resource_not_found(),
            )
            .into());
        }
```
The shared resolution's own statement of the invariant — src/state.rs:1860-1870:
```
    /// Whether the stored `policy` applies to the member account `account`.
    ///
    /// The ONE resolution every operation that reports about a (policy, member
    /// account) pair uses: Firewall Manager evaluates a policy only against the
    /// accounts in the policy's own scope ([`Self::policy_scope_accounts`]), so
    /// an account outside that scope was never evaluated against this policy and
    /// no surface of the policy holds anything about it — which is the modelled
    /// not-found, not an empty report.
    pub fn policy_applies_to_account(&self, policy: &Policy, account: &str) -> bool {
        self.policy_scope_accounts(policy).contains(account)
    }
```
Test coverage today does NOT pin the divergence: tests/all/protection_status.rs seeds attacks only for accounts that are all inside the policy's `IncludeMap` (`SCOPED_ACCOUNTS`, protection_status.rs:213-214 = ["111111111111", "222222222222", "333333333333", "444444444444"], and every row of `attacks()` names one of the first three), and tests/all/member_account_policy_scope.rs drives the absent-member-account request with an EMPTY inventory ("let attacks = with_store(&engine, |store| store.shield_attacks.len()); assert_eq!(attacks, 0, ...)", member_account_policy_scope.rs:249-266). So filtering the page by the policy's scope changes no existing assertion.

## Steps to Reproduce

1. Boot the emulator and call `AssociateAdminAccount` with the emulator account → 200.
2. Call `PutPolicy` with a `SHIELD_ADVANCED` policy whose `IncludeMap` is `{"ACCOUNT": ["111111111111"]}` (ResourceType `AWS::EC2::EIP`, `ExcludeResourceTags`/`RemediationEnabled` false) → 200; note the minted `PolicyId`.
3. Seed one `ShieldAttackRecord` under that `PolicyId` with `account_id: "222222222222"` through the store seam, the way tests/all/protection_status.rs seeds its attack table (no handler writes `shield_attacks`).
4. Call `GetProtectionStatus` with `{"PolicyId": <id>, "MemberAccountId": "222222222222"}` → 400 `ResourceNotFoundException` ("The referenced item does not exist."), because the policy does not apply to that account.
5. Call `GetProtectionStatus` with `{"PolicyId": <id>}` (no `MemberAccountId`).
6. Expected, per the comment above the selection ("absent, every account in the policy's scope is reported") and per `policy_applies_to_account`'s "no surface of the policy holds anything about it": the `Data` array is empty, since the only recorded attack belongs to an account outside the policy's scope.
7. Actual: `Data` carries that attack's entry with `"AccountId": "222222222222"` — an account the same handler reports as not found when it is named.
