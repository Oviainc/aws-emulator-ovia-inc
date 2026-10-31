Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: chime-sdk-identity.concurrent-register-endpoint-same-token-dedup chime-sdk-identity.deregister-endpoint-removes chime-sdk-identity.register-endpoint-idempotent-token chime-sdk-identity.register-endpoint-invalid-pinpoint-arn-inactive chime-sdk-identity.update-app-instance-user-endpoint-allow-messages-persists
Issue-Shape-Id: com.amazonaws.chimesdkidentity#ChimeIdentityService
Issue-Key: chime-sdk-identity.concurrent-register-endpoint-same-token-dedup|run#16|AssertionError|assert len(ok_endpoint_ids) == 1, f"idempotency token dedup broke under contention: {results}"
Issue-Key: chime-sdk-identity|RegisterAppInstanceUserEndpoint|NotImplemented
Issue-Commit: fc350f637d730e9c7a46ff0b276c9700aec9957e

# Implement chime-sdk-identity RegisterAppInstanceUserEndpoint: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `fc350f637d730e9c7a46ff0b276c9700aec9957e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterAppInstanceUserEndpoint

## Description
`RegisterAppInstanceUserEndpoint` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/register_app_instance_user_endpoint.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `RegisterAppInstanceUserEndpoint` call, so each one unblocks only when `RegisterAppInstanceUserEndpoint` works: `chime-sdk-identity.concurrent-register-endpoint-same-token-dedup`, `chime-sdk-identity.deregister-endpoint-removes`, `chime-sdk-identity.register-endpoint-idempotent-token`, `chime-sdk-identity.register-endpoint-invalid-pinpoint-arn-inactive`, `chime-sdk-identity.update-app-instance-user-endpoint-allow-messages-persists`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-chimesdkidentity-chimeidentityservice/src/ops/register_app_instance_user_endpoint.rs`. Replaying the corpus, the emulator refused the `RegisterAppInstanceUserEndpoint` call: File "run.py", line 189, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ^^^^^^^
  File "check_chime_sdk_identity_concurrent_register_endpoint_same_token_dedup.py", line 73, in run
    assert len(ok_endpoint_ids) == 1, f"idempotency token dedup broke under contention: {results}"
           ^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: idempotency token dedup broke under contention: [('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator'), ('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator'), ('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator'), ('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator'), ('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator'), ('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator'), ('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator'), ('error', 'NotImplemented', 'operation RegisterAppInstanceUserEndpoint is scaffolded but not implemented in this emulator')]

## Steps to Reproduce
Boot the chime-sdk-identity emulator
Call RegisterAppInstanceUserEndpoint and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
