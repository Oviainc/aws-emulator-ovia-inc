Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivs-realtime.import-public-key-invalid-pem-rejected
Issue-Shape-Id: com.amazonaws.ivsrealtime#AmazonInteractiveVideoServiceRealTime
Issue-Key: ivs-realtime.import-public-key-invalid-pem-rejected|run#6|AssertionError|assert message == expected_message, message
Issue-Commit: cd08eede305788d0e1af3dc59e94a11366e984d0

# ImportPublicKey @pattern refusal quotes the raw modeled pattern instead of AWS's anchored ^.*…​.*$ rendering

> _Note: this issue was identified against the code base at commit `cd08eede305788d0e1af3dc59e94a11366e984d0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportPublicKey

## Description
One root cause in the ivs-realtime emulator's request-boundary @pattern violation message. A fixer must close this one cause before submitting.

Cause 1 of 1 — the ValidationException raised when a member violates its modeled @pattern quotes the pattern in its bare modeled form, where AWS quotes the effective, anchored regex it actually applies (the modeled pattern wrapped in `^.*` … `.*$`).

AWS: Smithy (services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/smithy-model): ImportPublicKeyRequest$publicKeyMaterial targets PublicKeyMaterial, @required, @pattern "-----BEGIN PUBLIC KEY-----\\r?\\n([a-zA-Z0-9+/=\\r\\n]+)\\r?\\n-----END PUBLIC KEY-----(\\r?\\n)?"; the operation models ValidationException (HTTP 400). Smithy @pattern is an unanchored (partial) match, and the recording shows AWS reporting it that way: material that does not match is refused with HTTP 400, Code "ValidationException", Message "ImportPublicKey.publicKeyMaterial does not match the required pattern ^.*-----BEGIN PUBLIC KEY-----\\r?\\n([a-zA-Z0-9+/=\\r\\n]+)\\r?\\n-----END PUBLIC KEY-----(\\r?\\n)?.*$". Message template: the fixed text is "<Operation>.<member> does not match the required pattern " followed by the effective regex; the variable parts are the operation name and member name from the request and the member's modeled pattern, and the regex is ALWAYS rendered anchored — a literal `^.*` prefix and a literal `.*$` suffix around the modeled pattern. No value from the request is echoed.

Emulator: the code and status are right (400 / ValidationException), but the Message is "ImportPublicKey.publicKeyMaterial does not match the required pattern -----BEGIN PUBLIC KEY-----\\r?\\n([a-zA-Z0-9+/=\\r\\n]+)\\r?\\n-----END PUBLIC KEY-----(\\r?\\n)?" — the modeled pattern pasted verbatim, with the `^.*` prefix and `.*$` suffix that express the unanchored semantics both missing. (The handler's own non-PEM clause, "publicKeyMaterial … could not decode as a PEM file", is never reached for this input because the boundary pattern check refuses first, so this is the boundary message, not the handler's.)

Why: a client or operator that matches, logs or diffs the refusal text sees a sentence AWS never sends. Worse, the quoted regex is the only machine-usable part of the answer: a caller that lifts it out of the message to pre-validate input locally gets an ANCHORED-looking pattern and will reject material AWS accepts (any PEM carrying surrounding whitespace, headers or trailing content, which the real unanchored `^.*…​.*$` form admits). The defect is in the shared message builder, so it is not specific to ImportPublicKey: every @pattern member on this service reports the same way.

Fix: the request-boundary constraint-violation message builder for @pattern (the shared smithy server-runtime validation layer that formats "<Operation>.<member> does not match the required pattern <regex>" before the operation's `handle` runs) must render the modeled pattern in the anchored form AWS reports, not the raw trait value. This is not a change to the per-operation handler: services/service-com-amazonaws-ivsrealtime-amazoninteractivevideoservicerealtime/src/ops/import_public_key.rs only supplies the deeper PEM/ECDSA check (lines 190-202, 336-346) and is not on this path.

Traces: ivs-realtime.import-public-key-invalid-pem-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, ivs-realtime 46/69 passed.

Cause 1: [mismatch] ivs-realtime.import-public-key-invalid-pem-rejected — check_ivs_realtime_import_public_key_invalid_pem_rejected.py line 86, `assert message == expected_message, message`:
  AssertionError: ImportPublicKey.publicKeyMaterial does not match the required pattern -----BEGIN PUBLIC KEY-----\r?\n([a-zA-Z0-9+/=\r\n]+)\r?\n-----END PUBLIC KEY-----(\r?\n)?
Expected Message (check lines 1025-1028): "ImportPublicKey.publicKeyMaterial does not match the required pattern ^.*-----BEGIN PUBLIC KEY-----\r?\n([a-zA-Z0-9+/=\r\n]+)\r?\n-----END PUBLIC KEY-----(\r?\n)?.*$".
The preceding assertion on the error Code (`assert code == "ValidationException"`) PASSED, so the status/code classification is correct and only the message's rendering of the regex diverges: the actual text is missing the leading "^.*" and the trailing ".*$".
Model check (smithy_query operation=ImportPublicKey): publicKeyMaterial pattern = "-----BEGIN PUBLIC KEY-----\\r?\\n([a-zA-Z0-9+/=\\r\\n]+)\\r?\\n-----END PUBLIC KEY-----(\\r?\\n)?", errors include ValidationException (httpError 400) — i.e. the emulator is echoing the trait value as stored, unwrapped.
Not covered by any open issue: 20261029T194619Z-ivs-realtime-refusals-use-the-wrong-error-codes.md collects seven refusal-wording causes for TagResource, CreateParticipantToken, CreateIngestConfiguration, CreateEncoderConfiguration and GetStageSession, but none for ImportPublicKey and none about the boundary @pattern renderer; 20261024T004812Z-ivs-realtime-deletepublickey-stub.md covers only the unimplemented DeletePublicKey this check uses for cleanup.

## Steps to Reproduce
1. ListPublicKeys and record the names present.
2. ImportPublicKey(name="<any name matching ^[a-zA-Z0-9-_]*$>", publicKeyMaterial="not-a-pem-key").
   Expected: HTTP 400, Code "ValidationException", Message "ImportPublicKey.publicKeyMaterial does not match the required pattern ^.*-----BEGIN PUBLIC KEY-----\r?\n([a-zA-Z0-9+/=\r\n]+)\r?\n-----END PUBLIC KEY-----(\r?\n)?.*$" (the modeled pattern wrapped in ^.* and .*$).
   Actual: the same status and code, but Message "ImportPublicKey.publicKeyMaterial does not match the required pattern -----BEGIN PUBLIC KEY-----\r?\n([a-zA-Z0-9+/=\r\n]+)\r?\n-----END PUBLIC KEY-----(\r?\n)?" — the bare trait value.
3. ListPublicKeys again: the set must be unchanged (the refused import creates nothing).
4. Positive control: generate a valid ECDSA P-384 (secp384r1) public key in SubjectPublicKeyInfo PEM form and ImportPublicKey with it — it must be accepted and the returned publicKey.name must equal the requested name.
5. Generalisation (same cause, any @pattern member): ImportPublicKey(name="bad name!", publicKeyMaterial=<valid PEM>) — the name refusal must likewise quote PublicKeyName's pattern anchored as "^.*^[a-zA-Z0-9-_]*$.*$"-style effective form the renderer produces for that trait, not the bare trait value.
