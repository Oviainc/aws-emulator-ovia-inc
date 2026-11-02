Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: payment-cryptography.concurrent-create-alias-same-name-exactly-one-wins
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography.concurrent-create-alias-same-name-exactly-one-wins|run#15|AssertionError|assert all(m == expected for m in messages), (
Issue-Commit: d6c346b47b96edf6fdc824863e97819a2f75dfc1

# payment-cryptography CreateAlias: concurrent duplicate-name losers get the emulator's own ConflictException sentence instead of AWS's "Alias <AliasName> already exists."

> _Note: this issue was identified against the code base at commit `d6c346b47b96edf6fdc824863e97819a2f75dfc1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAlias

## Description
One root cause, found by behavioral replay of the concurrent CreateAlias race.

Cause 1 of 1 — every racer that loses a concurrent CreateAlias on one AliasName is refused with the crate's hand-authored sentence rather than AWS's duplicate-alias message.
Failing check: payment-cryptography.concurrent-create-alias-same-name-exactly-one-wins.

AWS: Smithy — CreateAlias (com.amazonaws.paymentcryptography#CreateAlias) is @idempotent, takes AliasName (required, @length 7..256, @pattern ^alias/[a-zA-Z0-9/_-]+$) and optional KeyArn, and models ConflictException at HTTP 409 ("This request can cause an inconsistent state for the resource ... trying to create a resource that already exists"); the operation's own documentation states "The alias must be unique in the account and Amazon Web Services Region". Recorded behavior: when several CreateAlias calls race for one AliasName with DIFFERENT KeyArns, at least one succeeds and every other call is refused with Code=ConflictException and Message `Alias <AliasName> already exists.` — the fixed text is `Alias `, then ` already exists.` with the trailing period; the single variable part is the AliasName exactly as supplied in the request. A generated name from any one run (for example `alias/sim2real-pc-race-9f55f61e1651`) only fills that slot and is not part of the template.

Emulator: the error code and HTTP status are correct (ConflictException/409) and the winner/convergence semantics hold — the alias converges to a winning racer's key and re-reads are stable — but every loser's Message is `the alias <AliasName> already exists and is bound to another key`: lower-case lead-in, different wording, no trailing period, and an extra clause AWS never emits. The sentence is built in the handler's rebind branch (services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/src/ops/create_alias.rs, the `existing.as_deref() != Some(key_arn)` branch inside `handle`).

Why: a client relying on AWS's wording cannot classify the refusal. SDK users, log scrapers and golden-file tests match the refusal sentence to distinguish "name already taken" (retry with another name, or re-read the binding) from an unrelated 409; against this emulator they see an unrecognised string and surface an unknown failure. In the concurrent path this is the ONLY signal a losing racer has about why it lost, so the divergence is observable on every contended create, not just the serial duplicate.

Fix: the CreateAlias handler's ConflictException construction — emit AWS's fixed sentence with the request's AliasName interpolated, for the concurrent/contended path as well as the serial one. No state or serializer change is needed.

Already-filed issue named for completeness, not relied upon: 20261031T180548Z-payment-cryptography-control-plane-duplicate-alias-and-already.md files the same wording defect on the SERIAL duplicate-name path (its Cause 1, check payment-cryptography.create-alias-duplicate-name-conflict-rejected). It does not carry this check's replay key, so this concurrent check is filed here rather than assumed covered; a fixer closing both should re-replay both check ids.

Traces: check payment-cryptography.concurrent-create-alias-same-name-exactly-one-wins; handler at services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/src/ops/create_alias.rs lines 223-232.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 — payment-cryptography 7/21 passed (14 failed; 13 of the failures are already filed as unimplemented-stub clusters or owned by an open issue).

[mismatch] payment-cryptography.concurrent-create-alias-same-name-exactly-one-wins:
  AssertionError: expected every loser's message to be 'Alias alias/sim2real-pc-race-9f55f61e1651 already exists.', got ['the alias alias/sim2real-pc-race-9f55f61e1651 already exists and is bound to another key', 'the alias alias/sim2real-pc-race-9f55f61e1651 already exists and is bound to another key', 'the alias alias/sim2real-pc-race-9f55f61e1651 already exists and is bound to another key', 'the alias alias/sim2real-pc-race-9f55f61e1651 already exists and is bound to another key']

The check's earlier assertions passed: at least one racer won, all 5 racers resolved, and every loser's Code was ConflictException (correct). Only the Message diverges.

Expected message template: fixed text 'Alias ' + <AliasName from the request> + fixed text ' already exists.' (trailing period). Emulator template: 'the alias ' + <AliasName> + ' already exists and is bound to another key'. The alias name shown above is the check's per-run generated name and is not part of the requirement.

Smithy cited: CreateAlias — @idempotent; input AliasName (required, length 7..256, pattern ^alias/[a-zA-Z0-9/_-]+$), KeyArn (length 70..150, pattern ^arn:aws:payment-cryptography:...:key/...$); errors include ConflictException (httpError 409) and ResourceNotFoundException (404).

## Steps to Reproduce
1. CreateKey five times with KeyAttributes={KeyUsage: TR31_D0_SYMMETRIC_DATA_ENCRYPTION_KEY, KeyClass: SYMMETRIC_KEY, KeyAlgorithm: AES_128, KeyModesOfUse: {Encrypt, Decrypt, Wrap, Unwrap all true}}, Exportable=true. Keep the five KeyArns.
2. Pick one unique alias name 'alias/<unique>'.
3. From 5 threads concurrently call CreateAlias(AliasName='alias/<unique>', KeyArn=<a distinct one of the five ARNs per thread>).
4. Collect the responses. Observed: one 200 and four ConflictException refusals whose Message is 'the alias alias/<unique> already exists and is bound to another key'. Expected: the same ConflictException code/409 status but Message 'Alias alias/<unique> already exists.'
5. GetAlias(AliasName='alias/<unique>') — the binding does point at a winning racer's key and is stable across re-reads, so only the refusal message needs changing.
