Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: verifiedpermissions.concurrent-create-policy-store-alias-race verifiedpermissions.create-policy-store-alias-conflict-while-pending-deletion-rejected verifiedpermissions.delete-policy-store-alias-soft-delete-transitions-to-pending-deletion verifiedpermissions.policy-store-alias-resolves-in-is-authorized
Issue-Shape-Id: com.amazonaws.verifiedpermissions#VerifiedPermissions
Issue-Key: verifiedpermissions.concurrent-create-policy-store-alias-race|run#11|AssertionError|assert len(successes) == 1, f"expected exactly 1 winner, got successes={successes} failures={failures}"
Issue-Key: verifiedpermissions|CreatePolicyStoreAlias|NotImplemented
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Implement verifiedpermissions CreatePolicyStoreAlias: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePolicyStoreAlias

## Description
`CreatePolicyStoreAlias` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_store_alias.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `CreatePolicyStoreAlias` call, so each one unblocks only when `CreatePolicyStoreAlias` works: `verifiedpermissions.concurrent-create-policy-store-alias-race`, `verifiedpermissions.create-policy-store-alias-conflict-while-pending-deletion-rejected`, `verifiedpermissions.delete-policy-store-alias-soft-delete-transitions-to-pending-deletion`, `verifiedpermissions.policy-store-alias-resolves-in-is-authorized`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-verifiedpermissions-verifiedpermissions/src/ops/create_policy_store_alias.rs`. Replaying the corpus, the emulator refused the `CreatePolicyStoreAlias` call: File "run.py", line 175, in <lambda>
    finished, exc, frame = _on_daemon_thread(lambda: fn(ctx), f"check:{name}", timeout_s)
                                                     ~~^^^^^
  File "check_verifiedpermissions_concurrent_create_policy_store_alias_race.py", line 49, in run
    assert len(successes) == 1, f"expected exactly 1 winner, got successes={successes} failures={failures}"
           ^^^^^^^^^^^^^^^^^^^
AssertionError: expected exactly 1 winner, got successes=[] failures=[('error', 'PSJPfe8fgQnwl8FAu3TKk6j5', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator')), ('error', 'PSGxN6GmdShnY39saywtCOVx', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator')), ('error', 'PS9U2CCCsiTSu74BBHdXEuyj', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator')), ('error', 'PS3ovRt88cTvFDArGuSN8dA6', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator')), ('error', 'PS7C2XJVZzaJO2pb9PuOYzhI', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator')), ('error', 'PS0a1I34cjPNXB0C2HzsEeX0', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator')), ('error', 'PSLL0D1fmshQ47TgQk9AjK9C', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator')), ('error', 'PS2KupsQW66OZ08E7hdmxLYT', ('NotImplemented', 'operation CreatePolicyStoreAlias is scaffolded but not implemented in this emulator'))]

## Steps to Reproduce
Boot the verifiedpermissions emulator
Call CreatePolicyStoreAlias and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
