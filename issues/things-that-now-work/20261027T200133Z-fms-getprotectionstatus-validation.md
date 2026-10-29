Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 4e290f8d6b034b90409323db1641d26f5c9d1b54

# GetProtectionStatus accepts a MemberAccountId the policy does not apply to, answering 200 with an empty Data document instead of the not-found its sibling member-account readers give

_Filed: 2026-10-27T20:01:33Z_
_Operation: GetProtectionStatus | Class: validation_

> _Note: this issue was identified against the code base at commit `4e290f8d6b034b90409323db1641d26f5c9d1b54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Three operations of this service take a member account together with a `PolicyId`: `GetComplianceDetail`, `GetViolationDetails` and `GetProtectionStatus`. The first two resolve the account against the policy's scope (`store.policy_scope_accounts`) and answer the modelled `ResourceNotFoundException` when the policy does not apply to it, with the stated reason that an account outside the policy's scope "was never evaluated against this policy". `GetProtectionStatus` skips that resolution entirely: the account is used only as a filter over the attack inventory, so a `MemberAccountId` the policy does not apply to — or one that is no account of the organisation at all — is answered 200 with `Data` = `"[]"`.

The result is indistinguishable from "that account is in the policy's scope and no attack was recorded against it", which is the one thing a caller is asking this operation to tell apart. `GetProtectionStatus` declares `ResourceNotFoundException`, so the refusal its siblings give needs no unmodelled shape.

## Evidence

The model says the member account is one the policy covers — `smithy_query(query="doc",shape="com.amazonaws.fms#GetProtectionStatusRequest",member="MemberAccountId")`:

    "The Amazon Web Services account that is in scope of the policy that you want to get the details for."

and `GetProtectionStatus` declares the error the siblings use (`services/service-com-amazonaws-fms-awsfms-20180101/OPERATIONS.md:133`): "**Errors:** `InternalErrorException (HTTP 400)`, `InvalidInputException (HTTP 400)`, `ResourceNotFoundException (HTTP 400)`".

Sibling 1 — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_compliance_detail.rs:197-213`:

    // "The Amazon Web Services account that owns the resources that you want
    // to get the details for" is a MEMBER account of the policy: ... An
    // account outside that scope was never evaluated against this policy, so
    // there is no compliance detail of it to report, and the pair is the same
    // modelled not-found as an unknown policy ...
    if !store.policy_scope_accounts(&policy.policy).contains(&input.member_account) {
        return Err(GetComplianceDetailErrorModeled::ResourceNotFoundException(
            crate::state::resource_not_found(),
        ).into());
    }

Sibling 2 — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_violation_details.rs:197-212`: the same check, with "the same scope the compliance surfaces report one entry per. An account outside that scope was never evaluated against this policy, so it holds no violation of it to detail, and the request is the same modelled not-found as an unknown policy."

This handler — `services/service-com-amazonaws-fms-awsfms-20180101/src/ops/get_protection_status.rs:265-274` — only filters, never resolves:

    // `MemberAccountId` names "the Amazon Web Services account that is in
    // scope of the policy that you want to get the details for", so it
    // selects the attacks on THAT account's resources; absent, every
    // account in the policy's scope is reported.
    .filter(|attack| {
        input.member_account_id.as_deref()
            .is_none_or(|account| attack.account_id == account)
    })

and the empty page is still a success — `get_protection_status.rs:287-294` builds `Data` as an empty JSON array.

Not pinned by a test: every `MemberAccountId` row in `tests/all/protection_status.rs:384-390` uses `"111111111111"`, which the seeded policy's `IncludeMap` names (`tests/all/protection_status.rs:210`: `"IncludeMap": { "ACCOUNT": ["111111111111", "222222222222", "333333333333"] }`), so no test covers an out-of-scope account.

## Steps to Reproduce

1. AssociateAdminAccount with the calling account so the administrator gate passes.
2. PutPolicy with a SHIELD_ADVANCED document whose IncludeMap is { "ACCOUNT": ["111111111111"] }; note the minted PolicyId.
3. Call GetComplianceDetail with that PolicyId and MemberAccount "999999999999" — answers 400 ResourceNotFoundException ("The referenced item does not exist."), because the policy does not apply to that account.
4. Call GetProtectionStatus with the same PolicyId and MemberAccountId "999999999999" — answers 200 with Data "[]", AdminAccountId and ServiceType filled in, i.e. a successful report about an account the policy was never evaluated against.
