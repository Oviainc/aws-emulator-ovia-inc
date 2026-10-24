Issue-Severity: HIGH
Issue-Gap-Class: error-response
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# ValidationException for an absent @required member omits the model-required reason (and fieldList)

_Filed: 2026-10-24T01:07:28Z_
_Operation: GetTokenBalance | Class: error-response_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation of this service models `ValidationException` as `{message (required), reason (REQUIRED, ValidationExceptionReason), fieldList}`. The emulator honours that shape ONLY for refusals its handlers make: `crate::state::validation` builds the full exception with `reason: "fieldValidationFailed"` and a `fieldList`.

There is one refusal path that bypasses it entirely: an ABSENT `@required` input member. That is detected while DESERIALIZING the request, so no handler ever runs. The framework turns it into `Refusal::Invalid([Violation{kind: MissingRequiredMember}])` and the service's `override_refusal` (src/service.rs:248-269) answers it with a `Refused { code, message }` — a struct that carries only an error code and a message string (vendor/smithy-server-runtime/src/pre_dispatch.rs:163-169). There is no seam through which `reason` or `fieldList` can be attached.

Expected (AWS): `POST /get-token-balance` with `ownerIdentifier` present and `tokenIdentifier` absent -> HTTP 400 `ValidationException` whose body carries `message` ("1 validation error detected: Value null at 'tokenIdentifier' failed to satisfy constraint: Member must not be null"), the model-required `reason` = `fieldValidationFailed`, and a `fieldList` entry naming the absent member — the same envelope every other 400 of this service carries.

Actual (emulator): HTTP 400 with the correct code and a correctly-worded `message`, but the body has NO `reason` member and no `fieldList`. An SDK client deserializing the modelled exception gets a null in a member the model marks `@required`, and a client that branches on `reason`/`fieldList` (which this service's other 400s all populate) sees nothing to branch on.

Same for every operation on the checklist: `GetTransaction`/`ListTransactionEvents` (absent `network`), `GetAssetContract` (absent `contractIdentifier`), `ListAssetContracts` (absent `contractFilter`), `ListTokenBalances` (absent `tokenFilter`), `ListTransactions` (absent `address`/`network`), `ListFilteredTransactionEvents` (absent `addressIdentifierFilter`). The fix belongs in one place — the same `skip_validation`-plus-handler-check pattern the service already uses for every CONSTRAINED member, extended to required-member presence so the refusal is raised through the modelled exception instead of the framework's two-field one.

## Evidence

Spec: smithy_query(query="shape", shape="ValidationException") -> `{message: required, reason: required (ValidationExceptionReason = unknownOperation|cannotParse|fieldValidationFailed|other), fieldList}`, httpError 400, declared by all 9 operations. `GetTokenBalanceInput$tokenIdentifier` and `$ownerIdentifier` are both `@required`.

Handler/code: src/state/validation.rs module doc, lines 52-54, states the gap outright: "The one refusal that cannot come through here is an ABSENT `@required` member: it is found while READING the request, so no input value ever exists for a handler to check, and the framework answers it directly." src/service.rs:262-268 returns `Refused { code, message }`; `Refused` (vendor/smithy-server-runtime/src/pre_dispatch.rs:163-169) has exactly two fields, so `reason` cannot be emitted on that path. src/service.rs:99-124 shows `MissingRequiredMember` is wired into the message renderer only.

Test coverage: tests/all/error_response_shape.rs asserts `reason` and `fieldList` on constraint failures only — every row in its table supplies all required members and violates a @pattern/@length/@range/enum. No test submits a request with a required member omitted, so nothing pins this path.

Not covered by the (fixed) issue 20261022T132335Z-managedblockchain-query-error-responses-are-not-aws-shaped: that one was closed by adding `reason`/`fieldList` to the handler-side refusals in src/state/validation.rs; the deserialization-time refusal still renders through src/service.rs's two-field `Refused`.

## Steps to Reproduce

1. POST /get-token-balance with body {"ownerIdentifier": {"address": "0xd8da6bf26964af9d7eed9e03e53415d37aa96045"}} — i.e. the @required tokenIdentifier omitted — against the emulator (region us-east-1, signing name managedblockchain-query).
2. Observe HTTP 400 with x-amzn-errortype ValidationException and a body containing only `message`; there is no `reason` member and no `fieldList`.
3. Contrast with POST /get-token-balance where tokenIdentifier is PRESENT but carries an out-of-pattern contractAddress (e.g. "under13"): that response does carry reason="fieldValidationFailed" and a fieldList entry, showing the two refusal paths disagree about the exception shape.
4. Repeat with any other operation's absent required member (e.g. POST /get-transaction with only {"transactionHash": "0x"+64 hex} and no `network`, or POST /list-token-balances with no `tokenFilter`) for the same two-field body.
