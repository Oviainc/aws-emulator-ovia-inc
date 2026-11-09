Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: network-security-manager.delete-rule-blocked-while-associated-with-policy network-security-manager.delete-rule-blocked-while-associated-with-template network-security-manager.duplicate-rule-names-coexist-as-distinct-resources network-security-manager.policy-snapshot-priority-tracks-live-policy-while-configuration-freezes network-security-manager.rule-snapshot-preserves-version-after-live-rule-is-updated network-security-manager.update-rule-rejects-all-prior-update-tokens-after-new-version
Issue-Shape-Id: com.amazonaws.networksecuritymanager#PiccoloCustomerAPIService
Issue-Key: network-security-manager|CreateRule|NotImplemented
Issue-Commit: e38416fc0970ac2ae0c65df6bda0d7aff4be2337

# Implement network-security-manager CreateRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e38416fc0970ac2ae0c65df6bda0d7aff4be2337`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
`CreateRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice/src/ops/create_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateRule` call, so each one unblocks only when `CreateRule` works: `network-security-manager.delete-rule-blocked-while-associated-with-policy`, `network-security-manager.delete-rule-blocked-while-associated-with-template`, `network-security-manager.duplicate-rule-names-coexist-as-distinct-resources`, `network-security-manager.policy-snapshot-priority-tracks-live-policy-while-configuration-freezes`, `network-security-manager.rule-snapshot-preserves-version-after-live-rule-is-updated`, `network-security-manager.update-rule-rejects-all-prior-update-tokens-after-new-version`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-networksecuritymanager-piccolocustomerapiservice/src/ops/create_rule.rs`. Replaying the corpus, the emulator refused the `CreateRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRule operation: operation CreateRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the network-security-manager emulator
Call CreateRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
