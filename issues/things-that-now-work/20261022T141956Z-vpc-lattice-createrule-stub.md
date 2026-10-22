Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: vpc-lattice.duplicate-listener-rule-priority-rejected
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|CreateRule|NotImplemented
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# Implement vpc-lattice CreateRule: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateRule

## Description
`CreateRule` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_rule.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `CreateRule` call, so each one unblocks only when `CreateRule` works: `vpc-lattice.duplicate-listener-rule-priority-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/create_rule.rs`. Replaying the corpus, the emulator refused the `CreateRule` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateRule operation: operation CreateRule is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the vpc-lattice emulator
Call CreateRule and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
