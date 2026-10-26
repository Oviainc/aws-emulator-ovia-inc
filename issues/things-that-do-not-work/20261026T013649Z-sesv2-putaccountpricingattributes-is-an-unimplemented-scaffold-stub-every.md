Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.put-account-pricing-attributes-conflict-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.put-account-pricing-attributes-conflict-rejected|run#15|AssertionError|assert not unexpected, f"unexpected error code(s) from racers: {unexpected}"
Issue-Commit: cf795f95b2012478973f10e6d859373fd85504ab

# sesv2 PutAccountPricingAttributes is an unimplemented scaffold stub: every call is refused 501 NotImplemented, so the account pricing plan can never be set and a concurrent-write loser is never ConflictException

> _Note: this issue was identified against the code base at commit `cf795f95b2012478973f10e6d859373fd85504ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAccountPricingAttributes

## Description
One net-new root cause in sesv2.

Cause 1 of 1: `PutAccountPricingAttributes` is never served — the handler is still the generated scaffold stub, so every call (including both racers of a concurrent-write check) is refused with a 501 `NotImplemented` instead of applying the requested pricing plan or refusing a losing concurrent writer with the modeled `ConflictException`. Failing check: sesv2.put-account-pricing-attributes-conflict-rejected.

AWS: Smithy (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json): `PutAccountPricingAttributes` — HTTP `PUT /v2/email/account/pricing-attributes`, success status 200 with an empty `PutAccountPricingAttributesResponse` body; input `Plan` is a REQUIRED `PricingPlan` enum whose values are exactly `NONE | ESSENTIALS | PRO | ENTERPRISE`; modeled errors are `BadRequestException` (HTTP 400, "The input you provided is invalid."), `ConflictException` (HTTP 409, documented "If there is already an ongoing account details update under review.") and `TooManyRequestsException` (HTTP 429). The accepted plan is observable through `GetAccount`, whose output carries `PricingAttributes { CurrentPlan, NextPlan }` (Smithy: `PricingAttributes.NextPlan` — "The pricing plan that will become active at the start of the next monthly cycle, if a scheduled change has been requested. This field is empty when no scheduled change is pending."). Recorded behavior only, for the parts the model does not state: a plan change to a HIGHER tier than the account's current plan takes effect immediately and appears on `PricingAttributes.CurrentPlan`; a change to a lower tier (including `NONE`) leaves `CurrentPlan` untouched and stages the new plan on `PricingAttributes.NextPlan` for the next billing cycle. For two concurrent calls on the same account, AWS lands at least one winner (200) and refuses any loser with error code `ConflictException` specifically — never a different or generic error code; the recording notes AWS does not guarantee the conflict fires on every race (some races land both writers).

Emulator: both concurrent calls are refused. The replay reports error code `NotImplemented` with message `operation PutAccountPricingAttributes is scaffolded but not implemented in this emulator` (the emulator's scaffold refusal is HTTP 501). In that message, no part is a request- or state-derived value: it is emulator-invented fixed text naming the operation, and AWS has no such error code for this operation at all. The handler file services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_account_pricing_attributes.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker and returns `Err(crate::scaffolded::Scaffolded::not_implemented("PutAccountPricingAttributes"))` from `handle`.

Why: a client that sets its SES pricing plan cannot do so at all — it gets a 501 with a non-AWS error code that no SDK retry/handling path models, instead of a 200 and a plan it can read back through `GetAccount.PricingAttributes`. Code that distinguishes an immediate upgrade (`CurrentPlan`) from a deferred downgrade (`NextPlan`), and code that treats a concurrent-write loser's `ConflictException` as retryable, both break: the loser is indistinguishable from the winner because both are refused with the same non-modeled code, and neither `CurrentPlan` nor `NextPlan` ever changes.

Fix: implement the operation's handler, services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_account_pricing_attributes.rs (`PutAccountPricingAttributes::handle`) — apply the plan to the account-singleton SES state the `GetAccount` handler reads (upgrade → `CurrentPlan`, lower tier → staged on `NextPlan`), return the empty 200 response, refuse a concurrent in-flight account update with the modeled `ConflictException` (409), and delete the SCAFFOLDED marker line so the stub inventory stops counting it.

Traces: sesv2.put-account-pricing-attributes-conflict-rejected (mismatch; `AssertionError: unexpected error code(s) from racers: [('error', 'NotImplemented'), ('error', 'NotImplemented')]`). Not covered by any open issue: the SESv2 stub issues filed so far name other operations (CreateConfigurationSet, CreateTenant, PutSuppressedDestination, ListSuppressedDestinations, …), and this check is not among those owned by 20261025T214624Z-sesv2-7-net-new-behavioral-divergences-listemailidentities-unreachable.md.

## Evidence
Expected (real AWS, per the Smithy model and the recording): PUT /v2/email/account/pricing-attributes with a valid `Plan` returns HTTP 200 with an empty body; at least one of two concurrent calls succeeds and any loser is refused HTTP 409 with error code `ConflictException`; afterwards GetAccount reports the winning upgrade on `PricingAttributes.CurrentPlan`, or a winning downgrade staged on `PricingAttributes.NextPlan` with `CurrentPlan` unchanged, and `NextPlan` absent when no downgrade won.

Actual (emulator): BOTH concurrent calls are refused with HTTP 501, error code `NotImplemented`, message "operation PutAccountPricingAttributes is scaffolded but not implemented in this emulator". The check fails at check_sesv2_put_account_pricing_attributes_conflict_rejected.py:74 — `AssertionError: unexpected error code(s) from racers: [('error', 'NotImplemented'), ('error', 'NotImplemented')]`. No winner exists, so `PricingAttributes` is never updated either. Source: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/put_account_pricing_attributes.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED:`) and line 153 (`Err(crate::scaffolded::Scaffolded::not_implemented("PutAccountPricingAttributes"))`).

## Steps to Reproduce
1. Boot the emulator and point an sesv2 client at it.
2. Call GetAccount and note `PricingAttributes.CurrentPlan` (defaults to NONE).
3. Call PutAccountPricingAttributes with any plan the account is not on, e.g. Plan="ESSENTIALS" (PUT /v2/email/account/pricing-attributes). Observe HTTP 501 / `NotImplemented` instead of 200.
4. Fire two such calls concurrently with different plans (e.g. ESSENTIALS and PRO) released by a barrier: AWS lands at least one 200 and refuses any loser with 409 `ConflictException`; the emulator refuses both with `NotImplemented`.
5. Call GetAccount again: `PricingAttributes.CurrentPlan` / `NextPlan` are unchanged.
Replay: sesv2.put-account-pricing-attributes-conflict-rejected.
