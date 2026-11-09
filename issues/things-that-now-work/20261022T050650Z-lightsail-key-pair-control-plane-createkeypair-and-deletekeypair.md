Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lightsail.keypair-delete-fingerprint-mismatch-rejected lightsail.concurrent-keypair-creation-duplicate-name-race
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail.keypair-delete-fingerprint-mismatch-rejected|run#5|AssertionError|assert code == "NotFoundException", f"unexpected code deleting nonexistent key pair: {code!r}"
Issue-Key: lightsail.concurrent-keypair-creation-duplicate-name-race|run#9|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner among {racer_count} racers, got {len(successes)}: {results}"
Issue-Commit: 7545d0b8b2e4fb4a41c801fcbb48b523234ffec8

# Lightsail key-pair control plane: CreateKeyPair and DeleteKeyPair answer 501 NotImplemented, so name-uniqueness and not-found rejection are never enforced (2 causes)

> _Note: this issue was identified against the code base at commit `7545d0b8b2e4fb4a41c801fcbb48b523234ffec8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteKeyPair

## Description
Two net-new root causes in the Lightsail key-pair control plane. Both surface as the emulator answering HTTP 501 `NotImplemented` where AWS answers a modeled result or a modeled client error, so no key-pair state is ever created, read back, or refused. Related already-filed work: a scaffold-stub issue naming `CreateKeyPair` (services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_key_pair.rs) is filed for the two checks it *blocked outright*; the cause below is the additional, separately-observable behavioural consequence seen from a check that caught the 501 and asserted on it, and `DeleteKeyPair` is not covered by any existing issue.

Cause 1 of 2: DeleteKeyPair for a key-pair name that was never created must be refused with NotFoundException, but the operation is an unimplemented scaffold stub.
AWS: Smithy model (com.amazonaws.lightsail#DeleteKeyPair) — input `keyPairName` (ResourceName, required, pattern ^\w[\w\-]*\w$) and optional `expectedFingerprint`; output `operation` (Operation); modeled errors include `NotFoundException` (HTTP 404) — "Lightsail throws this exception when it cannot find a resource". Deleting a name that does not exist is therefore a 404 `NotFoundException`, not a server-side failure. Recorded behavior additionally shows the documented permissiveness boundary that `expectedFingerprint` is validated only for the account's *default* key pair, so a mismatched fingerprint on a regular key pair still deletes successfully (HTTP 200 with an `operation`), and that a re-delete of an already-deleted name is the same 404 `NotFoundException`. The observed message text for the 404 (example: `The KeyPair does not exist: <keyPairName>`) is built from the request's `keyPairName`, so the rule — name the missing key pair — is the requirement, not the exact recorded string.
Emulator: every DeleteKeyPair call answers HTTP 501 with error code `NotImplemented` ("operation DeleteKeyPair is scaffolded but not implemented in this emulator"). services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_key_pair.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` returns `Scaffolded::not_implemented("DeleteKeyPair")`.
Why: a client cannot delete a key pair at all, and cannot distinguish "this key pair does not exist" (a 404 client error it is expected to handle, e.g. for idempotent teardown) from a server fault. Because the code is `NotImplemented` and the status 501, botocore raises a generic ClientError rather than the modeled `NotFoundException`, so `except NotFoundException` cleanup paths break, and every test or tool that relies on delete-then-confirm-gone never completes. It also leaves the fingerprint permissiveness boundary unexercised, so a client that wrongly assumes `expectedFingerprint` is a strict precondition is never corrected.
Fix: implement the DeleteKeyPair handler (services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_key_pair.rs) against the service key-pair state: look the name up, return `NotFoundException` (404) when absent, otherwise remove it and return the `operation` record; enforce `expectedFingerprint` only for the default key pair. Remove the SCAFFOLDED marker when done.
Traces: lightsail.keypair-delete-fingerprint-mismatch-rejected (mismatch: `unexpected code deleting nonexistent key pair: 'NotImplemented'`, expected `NotFoundException`).

Cause 2 of 2: CreateKeyPair never enforces key-pair name uniqueness, because the operation is an unimplemented scaffold stub — concurrent creates of one name all fail instead of exactly one winning.
AWS: Smithy model (com.amazonaws.lightsail#CreateKeyPair) — input `keyPairName` (ResourceName, required, pattern ^\w[\w\-]*\w$) plus `tags`; output `keyPair`, `publicKeyBase64`, `privateKeyBase64`, `operation`; modeled errors include `InvalidInputException` (HTTP 400) — "Lightsail throws this exception when user input does not conform to the validation rules of an input field" — and `NotFoundException` (404), `OperationFailureException` (400). Recorded behavior: key-pair names are unique per account/region, so when N requests race for the same `keyPairName`, exactly one returns HTTP 200 with `keyPair.name` equal to the requested name and the losers are refused with 400 `InvalidInputException`; the resulting single key pair is then readable via GetKeyPair, and a create for a genuinely different name always succeeds (so the rejection is about name collision, not about concurrency).
Emulator: all racing CreateKeyPair calls answer HTTP 501 with error code `NotImplemented`; the replay observed 0 winners out of 6 racers, every result being `('error', 'NotImplemented')`. services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_key_pair.rs is still a scaffold stub.
Why: no key pair can be created, so no downstream instance/key workflow can run; and because the failure code is `NotImplemented` (501) rather than `InvalidInputException` (400), a client that treats a duplicate-name collision as a retriable/ignorable client error instead sees an unmodeled server error — and a client that relies on at-most-one-winner semantics to elect a single owner of a name gets no winner at all.
Fix: implement the CreateKeyPair handler (services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_key_pair.rs) with an atomic insert into the shared key-pair state (the uniqueness check and insert must happen under one lock so concurrent requests cannot both win), returning `InvalidInputException` (400) for a name that already exists and the generated key material plus `operation` for the winner.
Traces: lightsail.concurrent-keypair-creation-duplicate-name-race (mismatch: `expected exactly 1 winner among 6 racers, got 0`).

## Evidence
Suite: simparity-behavioral-aws-v1, lightsail 0/25 passed.

Cause 1 — lightsail.keypair-delete-fingerprint-mismatch-rejected [mismatch]:
  Expected: DeleteKeyPair(keyPairName=<never-created name>) -> HTTP 404, Error.Code = "NotFoundException", message naming the missing key pair (example: "The KeyPair does not exist: sim2real-ls-kp-ghost-...").
  Actual:   HTTP 501, Error.Code = "NotImplemented", message "operation DeleteKeyPair is scaffolded but not implemented in this emulator".
  Assertion: `AssertionError: unexpected code deleting nonexistent key pair: 'NotImplemented'`
  Also unreachable as a result: DeleteKeyPair with a mismatched expectedFingerprint on a non-default key pair must still return HTTP 200, and a re-delete of the deleted name must return 404 NotFoundException.

Cause 2 — lightsail.concurrent-keypair-creation-duplicate-name-race [mismatch]:
  Expected: 6 concurrent CreateKeyPair(keyPairName=<same name>) -> exactly 1 success (HTTP 200, keyPair.name == requested name) and 5 refusals with HTTP 400, Error.Code = "InvalidInputException"; GetKeyPair then returns that single key pair; CreateKeyPair for a different name succeeds.
  Actual:   all 6 -> HTTP 501, Error.Code = "NotImplemented".
  Assertion: `AssertionError: expected exactly 1 winner among 6 racers, got 0: [('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented'), ('error', 'NotImplemented')]`

Source evidence: services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/delete_key_pair.rs line 4 (`//! SCAFFOLDED, NOT IMPLEMENTED`) and line 185 (`Err(crate::scaffolded::Scaffolded::not_implemented("DeleteKeyPair"))`); services/service-com-amazonaws-lightsail-lightsail-20161128/src/ops/create_key_pair.rs is the same scaffold shape.

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at it in the emulator's region.

Cause 1 (DeleteKeyPair not-found rejection):
1. ls.delete_key_pair(keyPairName="sim2real-ls-kp-ghost-1")  # name never created
2. Expect botocore to raise ls.exceptions.NotFoundException (HTTP 404, Error.Code "NotFoundException") naming that key pair. Observed: ClientError with HTTP 501 and Error.Code "NotImplemented".
3. (Once create/delete work) ls.create_key_pair(keyPairName=N); ls.delete_key_pair(keyPairName=N, expectedFingerprint="00:11:...:ff") must still succeed (200); ls.get_key_pair(keyPairName=N) and a second ls.delete_key_pair(keyPairName=N) must both be 404 NotFoundException.

Cause 2 (CreateKeyPair name-uniqueness race):
1. Pick one name N; from a ThreadPoolExecutor with 6 workers, call ls.create_key_pair(keyPairName=N, tags=[{"key":"sim2real","value":"..."}]) concurrently from all 6.
2. Expect exactly 1 call to return 200 with keyPair.name == N, and the other 5 to raise ClientError with HTTP 400 / Error.Code "InvalidInputException". Observed: all 6 raise HTTP 501 / "NotImplemented" and there is no winner.
3. ls.get_key_pair(keyPairName=N) should then return the single key pair; ls.create_key_pair(keyPairName=<different name>) should succeed (positive control).

Replay the two checks: lightsail.keypair-delete-fingerprint-mismatch-rejected, lightsail.concurrent-keypair-creation-duplicate-name-race.
