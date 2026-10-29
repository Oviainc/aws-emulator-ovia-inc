Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sts.get-delegated-access-token-returns-usable-credentials
Issue-Shape-Id: com.amazonaws.sts#AWSSecurityTokenServiceV20110615
Issue-Key: sts.get-delegated-access-token-returns-usable-credentials|run#10|AssertionError|assert message_absent == (
Issue-Commit: 6dde6b5b59a4735e67d87b555115fcc549deec6e

# STS GetDelegatedAccessToken: required-member ValidationError message renders a value for the @sensitive TradeInToken member and the XML <Message> over-escapes apostrophes (2 causes)

> _Note: this issue was identified against the code base at commit `6dde6b5b59a4735e67d87b555115fcc549deec6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDelegatedAccessToken

## Description
This issue bundles TWO distinct net-new root causes in the STS service (service token `sts`), both observed on one failing check. A fixer must close BOTH before submitting; each cause names its own failing check ids.

Context, not a duplicate: the operation itself now answers (the scaffold issue 20261028T104240Z-sts-getdelegatedaccesstoken-stub.md is fixed, and cause 4 of the open issue 20261024T065715Z-sts-sessions-carry-no-policy-scoping-packedpolicysize-oversized.md described the old NotImplemented refusal). Neither covers what is below: the operation is reached, it correctly refuses with code `ValidationError`, and the divergence is purely in the refusal's MESSAGE BYTES. The suite did not mark this check as owned — the open issue carries a different replay key for it (`run#6`, the garbage-token code assertion, which now PASSES) while this failure is the later message assertion.

Cause 1 of 2: for a required member that is ABSENT and modeled @sensitive, the validation message renders a value segment (`Value null at ...`) where AWS renders none (`Value at ...`).
AWS: Smithy `com.amazonaws.sts#GetDelegatedAccessToken` input `GetDelegatedAccessTokenRequest$TradeInToken` -> `tradeInTokenType`, `@required` and `@sensitive`; the service is `awsQuery` (version 2011-06-15), so a request-shape rejection is an XML `ErrorResponse` with `<Code>ValidationError</Code>` at HTTP 400. For this operation AWS answers, verbatim template:
  `1 validation error detected: Value at '<memberPath>' failed to satisfy constraint: Member must not be null`
Fixed text: `1 validation error detected: Value at '` + `' failed to satisfy constraint: Member must not be null`. The single variable part is `<memberPath>` — the member's wire path taken from the request, here `tradeInToken`. There is NO value segment: the member is @sensitive, so AWS does not echo (or name) the offending value. Contrast the non-sensitive members of this same service, where AWS DOES echo the value in the same message family — e.g. `1 validation error detected: Value '<the value as sent>' at 'policy' failed to satisfy constraint: Member must have length less than or equal to 2048`. Hypothesis (neither the model nor the recording states which of the two rules the formatter should key on): the suppression could equally be keyed on "the member was absent" rather than on @sensitive; AWS's `Value null at '<path>'` form is used for an explicitly-null value of a non-sensitive member. Either rule reproduces the recorded message for this operation.
Emulator: the message is `1 validation error detected: Value null at 'tradeInToken' failed to satisfy constraint: Member must not be null` — the literal token `null` is injected as the value segment for a member AWS redacts entirely. The code (`ValidationError`) and status are correct.
Why: a client or SDK-side tool that parses the validation message to point a user at the offending input is shown a phantom value (`null`) for a member whose value AWS deliberately never discloses, and any test or log assertion written against the AWS wording fails. It also means the emulator would leak a @sensitive member's value into an error body for the non-absent cases of the same code path.
Fix: the shared request-validation message formatter in the framework's validate stage (the `deserialize -> validate -> handle -> serialize` walk that runs against `GET_DELEGATED_ACCESS_TOKEN_INPUT_SCHEMA`), not the operation handler — `handle` is never entered for this request. Omit the value segment for a member the model marks @sensitive (and/or for an absent member) instead of rendering `null`.
Traces: sts.get-delegated-access-token-returns-usable-credentials.

Cause 2 of 2: the awsQuery XML error body over-escapes the apostrophe inside `<Message>` as `&apos;`.
AWS: recorded behavior only (the model fixes the error envelope, not its character escaping). In the recorded `ErrorResponse` the member path inside `<Message>` is delimited by LITERAL apostrophes — `... Value at 'tradeInToken' failed ...` — i.e. XML text content escapes only the characters XML requires (`&`, `<`), and `'` is emitted raw.
Emulator: the raw response body contains `... Value null at &apos;tradeInToken&apos; failed ...`. This is visible only to a client that reads the raw XML (the check extracts `<Message>` with a regex over the response text); botocore's XML parser unescapes it, which is why the same divergence is invisible on the SDK-parsed messages of this service. It therefore affects EVERY sts error message containing an apostrophe, not just this operation.
Why: a client that matches, logs or diffs the raw error body — a proxy, a recorded-fixture comparison, a non-botocore SDK or any tooling doing a byte comparison of the error envelope — sees a message that does not equal the one AWS sends, and member paths come out as `&apos;tradeInToken&apos;`.
Fix: the awsQuery/XML error serializer's text escaping for the `<Message>` element (escape only `&` and `<`; emit `'` literally).
Traces: sts.get-delegated-access-token-returns-usable-credentials.

## Evidence
Replay of suite simparity-behavioral-ga-prod-v1, check `sts.get-delegated-access-token-returns-usable-credentials` (verdict: mismatch). The check first proves the healthy path — GetCallerIdentity returns an Account, and `get_delegated_access_token(TradeInToken=<never-issued token>)` is correctly refused `ValidationError` / 'Invalid trade-in token' (both assertions PASS). It then hand-signs a SigV4 POST of `Action=GetDelegatedAccessToken&Version=2011-06-15` with NO TradeInToken (bypassing botocore's required-parameter check) and reads `<Code>`/`<Message>` straight out of the response text.

  File "check_sts_get_delegated_access_token_returns_usable_credentials.py", line 42, in run
    assert message_absent == (
           ^^^^^^^^^^^^^^^^^^^
  AssertionError: 1 validation error detected: Value null at &apos;tradeInToken&apos; failed to satisfy constraint: Member must not be null

Expected (AWS), from the check's own assertion:
  "1 validation error detected: Value at 'tradeInToken' failed to satisfy constraint: Member must not be null"
Actual (emulator), raw from `<Message>`:
  "1 validation error detected: Value null at &apos;tradeInToken&apos; failed to satisfy constraint: Member must not be null"

Two independent deltas in that one string: (1) the extra value segment `null ` after `Value` — cause 1; (2) `&apos;` where AWS sends `'` — cause 2. `code_absent` compared equal to `ValidationError`, so the error code and the 400 class are already right.

Model citations: smithy_query(query="operation", operation="GetDelegatedAccessToken") on services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/com-amazonaws-sts-awssecuritytokenservicev20110615.json reports input `{"TradeInToken": {"target": "tradeInTokenType", "required": true, "sensitive": true}}`; service_info reports protocols [awsQuery], xmlNamespace https://sts.amazonaws.com/doc/2011-06-15/. The handler at services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/ops/get_delegated_access_token.rs is implemented (it declares INPUT_SCHEMA and is driven by the framework's validate stage), confirming the diverging message is produced before `handle` is entered. Contrast evidence that non-sensitive members DO carry a value segment in this same service, from other checks in the same replay: "Value '<...>' at 'tags.1.member.value' failed to satisfy constraint: Member must have length less than or equal to 256" and "Value '<...>' at 'policy' failed to satisfy constraint: Member must have length less than or equal to 2048".

## Steps to Reproduce
1. Boot the emulator and replay check `sts.get-delegated-access-token-returns-usable-credentials`.
2. Manual repro (both causes in one request): hand-sign a SigV4 POST to the STS endpoint with Content-Type application/x-www-form-urlencoded and body `Action=GetDelegatedAccessToken&Version=2011-06-15` — i.e. omit the required TradeInToken, which botocore will not let you do through the client.
3. Read the RAW response body (do not let an XML parser unescape it). Observe HTTP 400 with `<Code>ValidationError</Code>` (correct) and `<Message>1 validation error detected: Value null at &apos;tradeInToken&apos; failed to satisfy constraint: Member must not be null</Message>`.
4. AWS answers `<Message>1 validation error detected: Value at 'tradeInToken' failed to satisfy constraint: Member must not be null</Message>` — no value segment for the @sensitive member, and literal apostrophes.
5. Cause 2 in isolation: provoke any sts ValidationError whose message contains a member path (e.g. GetFederationToken with an invalid Name) and compare the raw body's `<Message>` bytes — every apostrophe arrives as `&apos;`.
