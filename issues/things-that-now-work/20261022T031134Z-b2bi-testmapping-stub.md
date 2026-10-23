Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: b2bi.test-mapping-jsonata-identity-roundtrip b2bi.test-mapping-jsonata-transform-roundtrip
Issue-Shape-Id: com.amazonaws.b2bi#B2BI
Issue-Key: b2bi|TestMapping|NotImplemented
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# Implement b2bi TestMapping: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TestMapping

## Description
`TestMapping` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-b2bi-b2bi/src/ops/test_mapping.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `TestMapping` call, so each one unblocks only when `TestMapping` works: `b2bi.test-mapping-jsonata-identity-roundtrip`, `b2bi.test-mapping-jsonata-transform-roundtrip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-b2bi-b2bi/src/ops/test_mapping.rs`. Replaying the corpus, the emulator refused the `TestMapping` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TestMapping operation: operation TestMapping is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the b2bi emulator
Call TestMapping and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal

## Resolution

`TestMapping` is implemented in `services/service-com-amazonaws-b2bi-b2bi/src/ops/test_mapping.rs`:
the caller's `mappingTemplate` is evaluated over `inputFileContent` through the `jsonata-rs`
interpreter (pinned `=0.3.4`), and `mappedFileContent` is the serialized result, so the answer
is computed from the request's own document rather than echoed or synthesized. The scaffold
marker is gone, and the handler persists nothing — the operation owns no resource, so a request
changes nothing a later caller sees.

The string-building constructs an EDI mapping template needs are EVALUATED: concatenation
(`&`), `$string`, `$join` and `$replace` with a string pattern — which an earlier iteration of
this fix answered with a 501 — because the engine asks its ARENA for the buffer each of them
writes into, so `bumpalo`'s allocation limit declines an over-ceiling layout before the system
is asked for the memory, and the refusal unwinds into the modelled 400. That is measured, not
assumed: a 100 KB member aliased a thousand times is refused with the 64 MiB arena ceiling named
in the message (the engine asks the arena for the whole 100 000 000-byte buffer), and the same
shape at a hundred aliases is refused by the 5 000 000-byte answer ceiling, charged on the value
before it is serialized. The tests assert WHICH bound answered, so an engine upgrade that moved
such a buffer onto the global heap fails the build.

Three constructs are answered with the 501 that names them, because each is ONE interpreter step
with no bound this emulator can place ahead of it: a regular-expression literal (`regress`
backtracks with no step budget) and `$distinct` (a quadratic native pass), whose single step
cannot be interrupted — the interpreter checks its time box BETWEEN steps and a `std::thread`
cannot be cancelled, so such a step would hold one of the handler's evaluation threads and a
core for the life of the process — and `$pad`, whose result is a global-heap `String` sized by a
number in the template rather than by any value, so no ceiling here precedes it. Everything
else is evaluated, under the arena's allocation limit, the interpreter's time and depth boxes, a
runtime deadline that frees the caller, the answer-size charge, and four evaluation permits with
a live-thread cap; every bound answers with a modelled error (400 while evaluating, 429 for the
slot), never with a wrong mapping and never by ending the process.

`services/service-com-amazonaws-b2bi-b2bi/tests/all/test_mapping.rs` drives all of this over the
wire: a table of JSONata constructs with the value each must compute, the identity round trip
over randomly generated documents, the string-building constructs in the positions a dispatch
decision could get wrong beside the refusal of the three above, the aliasing sweep that names
the bound each row meets, the fuzzed template and document bytes, and each bound the handler
publishes.

### Residual risk

`replay_issue_units()` still refuses to run in the fixing environment ("refusing to replay 2
checks on an injected emulator endpoint"), so neither recorded unit was replayed against this
implementation; the suite above is the evidence.

A mapping that matches a pattern, de-duplicates a list or pads a fixed-width field is
unanswerable here — the real service evaluates all three — and that is the largest thing a suite
written against this emulator cannot conclude about AWS. The handler's module docs carry the
bound each one lacks.
