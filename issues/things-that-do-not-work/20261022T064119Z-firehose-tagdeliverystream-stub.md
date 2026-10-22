Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.concurrent-tag-and-untag-race-consistent-final-state firehose.e2e-tag-and-update-destination-reflected-then-teardown firehose.e2e-tag-encrypt-update-destination-consequence-flow firehose.list-tags-for-delivery-stream-limit firehose.list-tags-limit-exceeds-max firehose.list-tags-nonexistent-stream-not-found firehose.tag-and-list-tags-roundtrip firehose.tag-delivery-stream-disallowed-key-character-rejected firehose.tag-delivery-stream-invalid-argument-too-many-tags firehose.tag-delivery-stream-nonexistent-rejected firehose.tag-reserved-aws-prefix-rejected firehose.tag-stream-while-creating-is-accepted firehose.tag-value-empty-string-preserved firehose.tag-value-empty-string-preserved-round-trip firehose.tag-value-unicode-preserved firehose.tag-value-unicode-round-trip firehose.untag-delivery-stream-invalid-tag-key firehose.untag-removes-tags
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|TagDeliveryStream|NotImplemented
Issue-Key: firehose.tag-delivery-stream-disallowed-key-character-rejected|run#6|AssertionError|assert err["Code"] == "ValidationException", f"expected ValidationException, got {err!r}"
Issue-Key: firehose.tag-reserved-aws-prefix-rejected|run#4|AssertionError|assert err.get("Code") == "ValidationException", (
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Implement firehose TagDeliveryStream: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagDeliveryStream

## Description
`TagDeliveryStream` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-firehose-firehose-20150804/src/ops/tag_delivery_stream.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

18 recorded unit(s) fail on their `TagDeliveryStream` call, so each one unblocks only when `TagDeliveryStream` works: `firehose.concurrent-tag-and-untag-race-consistent-final-state`, `firehose.e2e-tag-and-update-destination-reflected-then-teardown`, `firehose.e2e-tag-encrypt-update-destination-consequence-flow`, `firehose.list-tags-for-delivery-stream-limit`, `firehose.list-tags-limit-exceeds-max`, `firehose.list-tags-nonexistent-stream-not-found`, `firehose.tag-and-list-tags-roundtrip`, `firehose.tag-delivery-stream-disallowed-key-character-rejected`, `firehose.tag-delivery-stream-invalid-argument-too-many-tags`, `firehose.tag-delivery-stream-nonexistent-rejected`, `firehose.tag-reserved-aws-prefix-rejected`, `firehose.tag-stream-while-creating-is-accepted`, `firehose.tag-value-empty-string-preserved`, `firehose.tag-value-empty-string-preserved-round-trip`, `firehose.tag-value-unicode-preserved`, `firehose.tag-value-unicode-round-trip`, `firehose.untag-delivery-stream-invalid-tag-key`, `firehose.untag-removes-tags`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-firehose-firehose-20150804/src/ops/tag_delivery_stream.rs`. Replaying the corpus, the emulator refused the `TagDeliveryStream` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the TagDeliveryStream operation: operation TagDeliveryStream is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the firehose emulator
Call TagDeliveryStream and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
