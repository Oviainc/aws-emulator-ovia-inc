Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 889b7a0ea20193cdb3c9577846ef2b90647eb8e8

# A GetProtectionStatus NextToken carries only the PolicyId, so replaying it with a different MemberAccountId or time window silently drops attacks from the page

_Filed: 2026-10-29T08:03:07Z_
_Operation: GetProtectionStatus | Class: pagination_

> _Note: this issue was identified against the code base at commit `889b7a0ea20193cdb3c9577846ef2b90647eb8e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`GetProtectionStatus` pages the attack inventory for one policy, but the page it serves is narrowed by three MORE request members than the `PolicyId`: `MemberAccountId` selects the account whose resources were attacked, and `StartTime` / `EndTime` select the queried period (the handler applies all three as filters on the same iterator it later takes the `NextToken` from).

The token, however, is only checked against the POLICY: it is minted as the next matching attack's raw store key (`encode_key_token(&attack.key())`) and validated with `decode_key_token(token).filter(|key| key.starts_with(&prefix))`, where `prefix` is `shield_attack_key_prefix(&input.policy_id)`. So a token minted under one `(MemberAccountId, StartTime, EndTime)` selection is accepted verbatim under any other selection of the same policy and used straight as the lower bound of `store.shield_attacks.range(lower..)`.

An attack of the new selection whose key sorts below that foreign boundary is therefore skipped: the `Data` document is short — or an empty JSON array — and because the walk then runs to the end of the policy's range, no `NextToken` comes back, so the caller cannot tell a truncated page from a complete one. Attack keys are `<PolicyId>#<zero-padded start instant>#<AttackId>`, so account A's earlier attacks sort below account B's later ones and resuming B's boundary under `MemberAccountId: A` drops them.

The handler's own comment claims only the policy matters ("or one minted for ANOTHER policy's page, names no boundary in this policy's attacks"), but the page it hands back is not "this policy's attacks" — it is this policy's attacks in one account over one period.

## Evidence

The rule this crate wrote down for exactly this situation — services/service-com-amazonaws-fms-awsfms-20180101/src/state.rs:3174-3185:
```
/// Encode a page boundary of a listing whose page set is SELECTED by a request
/// member — the accounts one policy applies to, the members of one resource set,
/// the lists of one namespace — as an opaque `PaginationToken` carrying the
/// selecting value beside the boundary.
///
/// A boundary of such a page names nothing in another selection: the key may be
/// absent there, or sort after items that selection does hold, so resuming from
/// it would silently drop them.
```

The token check `GetProtectionStatus` performs — src/ops/get_protection_status.rs:229-252:
```
        // A token this service did not mint, or one minted for ANOTHER policy's
        // page, names no boundary in this policy's attacks: every key of this
        // policy's range carries the policy's prefix.
        let prefix = crate::state::shield_attack_key_prefix(&input.policy_id);
        let start = match input.next_token.as_deref() {
            Some(token) => match crate::state::decode_key_token(token)
                .filter(|key| key.starts_with(&prefix))
```

The three further selections the SAME iterator applies, which the token does not record — src/ops/get_protection_status.rs:259-278:
```
        let lower = start.unwrap_or_else(|| prefix.clone());
        let mut remaining = store
            .shield_attacks
            .range(lower..)
            .take_while(|(key, _)| key.starts_with(&prefix))
            .map(|(_, attack)| attack)
            // `MemberAccountId` names "the Amazon Web Services account that is
            // in scope of the policy that you want to get the details for", so
            // it selects the attacks on THAT account's resources ...
            .filter(|attack| {
                input
                    .member_account_id
                    .as_deref()
                    .is_none_or(|account| attack.account_id == account)
            })
            // `StartTime` / `EndTime` are "the start / end of the time period to
            // query for the attacks", so an attack is reported when it overlaps
            // that period; an absent bound leaves that side unbounded.
            .filter(|attack| attack.overlaps(input.start_time, input.end_time));
```
and the boundary it mints from that filtered iterator — src/ops/get_protection_status.rs:295-300:
```
        // A token only when an attack is left AFTER this page, so the last page
        // carries none. It is the next attack's own key, which is where the
        // following page resumes.
        let next_token = remaining
            .next()
            .map(|attack| crate::state::encode_key_token(&attack.key()));
```

The key order that makes the drop silent — src/state.rs:994-1009:
```
/// The start instant is zero-padded Unix time, so the lexicographic order of the
/// keys within one policy IS chronological order — which makes a page boundary a
/// key, the way every other paginated read of this service resumes.
...
    format!(
        "{policy_id}#{:020}.{:09}#{attack_id}",
```

The sibling that scopes the other `PolicyId`-narrowed listing to its selecting member — src/ops/list_compliance_status.rs:204-208:
```
            Some(token) => {
                match crate::state::decode_scoped_page_token(&input.policy_id, token)
                    .filter(|account| crate::state::is_account_id(account))
```

Test coverage today: tests/all/narrowed_page_token_scoping.rs:3-21 states the rule ("a token minted under one selection is refused under another rather than resumed from as an arbitrary key") and names its table as "`ListComplianceStatus` ... `ListResourceSetResources` ... `ListAppsLists` and `ListProtocolsLists`" — `GetProtectionStatus` is not a row. tests/all/protection_status.rs seeds `store.shield_attacks` and walks pages, but never hands a page-1 token back with a different `MemberAccountId` or time window.

## Steps to Reproduce

1. Boot the emulator, call `AssociateAdminAccount` with the emulator's own account, then `PutPolicy` for a `SHIELD_ADVANCED` policy; note the minted `PolicyId`.
2. Seed `store.shield_attacks` (the inventory no operation writes) with four attacks under that `PolicyId`: two for account `111111111111` started early, two for account `222222222222` started later — so account 111's keys sort below account 222's.
3. Call `GetProtectionStatus` with `{"PolicyId": "<id>", "MemberAccountId": "222222222222", "MaxResults": 1}` → 200, `Data` carries account 222's first attack and a `NextToken` naming its second.
4. Call `GetProtectionStatus` with `{"PolicyId": "<id>", "MemberAccountId": "111111111111", "MaxResults": 100, "NextToken": "<that token>"}`.
5. Expected (the rule state.rs:3179-3185 states and list_compliance_status.rs:204-208 applies): the token names no boundary of account 111's page, so it is refused with the modelled `InvalidInputException`.
6. Actual: 200 with `Data` = `[]` and no `NextToken` — both of account 111's attacks are silently dropped and the empty document is indistinguishable from "this account was never attacked". The same holds when the replayed request changes `StartTime` / `EndTime` instead of the account.
