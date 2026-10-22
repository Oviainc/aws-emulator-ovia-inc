Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose.create-stream-buffering-hints-range-enforced firehose.create-stream-buffering-interval-below-minimum-rejected firehose.create-stream-buffering-interval-out-of-range-rejected firehose.create-stream-buffering-size-above-maximum-rejected firehose.create-stream-name-exceeds-length-limit-rejected firehose.put-record-batch-empty-records-list-rejected firehose.put-record-batch-exceeds-max-records firehose.put-record-batch-oversized-record-rejected firehose.put-record-batch-partial-failure-reported firehose.put-record-batch-response-order-matches-request-order firehose.put-record-data-exceeds-max-size firehose.put-record-oversized-payload-rejected firehose.splunk-retry-duration-out-of-range-rejected firehose.tag-value-exceeds-max-length-rejected firehose.create-stream-invalid-name-characters-rejected firehose.put-record-empty-payload-blocked firehose.file-extension-overrides-the-compression-suffix-without-changing-the-bytes firehose.create-duplicate-delivery-stream-rejected firehose.create-duplicate-name-resource-in-use firehose.describe-nonexistent-stream-not-found firehose.delete-nonexistent-stream-not-found firehose.delete-stream-transitions-to-deleting-then-gone firehose.delete-transitions-to-deleting firehose.put-record-nonexistent-stream-not-found firehose.delete-stream-during-creating-rejected firehose.create-stream-conflicting-destination-configs firehose.create-stream-conflicting-destinations-rejected
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose.create-stream-buffering-hints-range-enforced|run#9|AssertionError|assert msg1 == expected, f"expected message {expected!r}, got {msg1!r}"
Issue-Key: firehose.create-stream-buffering-interval-below-minimum-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.create-stream-buffering-interval-out-of-range-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.create-stream-buffering-size-above-maximum-rejected|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose|CreateDeliveryStream|ValidationException
Issue-Key: firehose.put-record-batch-empty-records-list-rejected|run#12|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: firehose.put-record-batch-exceeds-max-records|run#10|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: firehose.put-record-batch-oversized-record-rejected|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.put-record-batch-partial-failure-reported|run#16|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.put-record-batch-response-order-matches-request-order|run#20|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.put-record-data-exceeds-max-size|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.put-record-oversized-payload-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.splunk-retry-duration-out-of-range-rejected|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.tag-value-exceeds-max-length-rejected|run#17|AssertionError|assert err.get("Message") == expected_msg, (
Issue-Key: firehose.put-record-empty-payload-blocked|run#13|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.file-extension-overrides-the-compression-suffix-without-changing-the-bytes|run#18|AssertionError|assert message == _FILE_EXTENSION_PATTERN_MESSAGE, (
Issue-Key: firehose.create-duplicate-delivery-stream-rejected|run#12|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: firehose.create-duplicate-name-resource-in-use|run#6|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.describe-nonexistent-stream-not-found|run#6|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: firehose.delete-nonexistent-stream-not-found|run#13|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: firehose.delete-stream-transitions-to-deleting-then-gone|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: firehose.delete-transitions-to-deleting|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.put-record-nonexistent-stream-not-found|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.delete-stream-during-creating-rejected|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.create-stream-conflicting-destination-configs|run#7|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: firehose.create-stream-conflicting-destinations-rejected|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 360c4c1618594432a64f84ba94d0fa368ed8f619

# Firehose rejection messages are built by the wrong rules: values echoed, patterns anchored, account id dropped (6 causes)

> _Note: this issue was identified against the code base at commit `360c4c1618594432a64f84ba94d0fa368ed8f619`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeliveryStream

## Description
The HTTP status and Error.Code match AWS on these refusals; what diverges is the message, and in each case the rule that produces the message is known (it is built from the request and the account, or it is a fixed sentence), so the construction rule is the requirement. Six root causes, all in the firehose service (bootstrapped as service-com-amazonaws-firehose-firehose-20150804). The recorded strings below are examples of each rule, not literals to hard-code.

Cause 1 of 6: Smithy constraint-violation messages echo the rejected value; Firehose's do not.
AWS: Smithy supplies the constraints that are being reported (e.g. CreateDeliveryStream.DeliveryStreamName @length 1..64; PutRecordBatch.Records @length 1..500; record Data @length max 1024000; BufferingHints sizeInMBs/intervalInSeconds ranges; Splunk RetryOptions.DurationInSeconds range; Tags value length). Recorded behavior: the message is "N validation error detected: Value at '<member path>' failed to satisfy constraint: Member must <constraint>" — the member path and the constraint clause are request/model-derived, and the offending VALUE is omitted.
Emulator: the same sentence but with the value interpolated after "Value", e.g. "Value '129' at '...sizeInMBs'", "Value '501 items' at 'records'", "Value '1100000 bytes' at 'records.2.member.data'", "Value 'vvvv...[257 bytes]' at 'tags.1.member.value'" (examples).
Why: the echoed value leaks request content (including record payload sizes and tag values) into an error surface AWS keeps value-free, and any client or test that matches the documented message shape fails.
Fix: the shared constraint-validation message formatter used by the firehose operation handlers must emit the value-free form.
Traces: firehose.create-stream-buffering-hints-range-enforced, firehose.create-stream-buffering-interval-below-minimum-rejected, firehose.create-stream-buffering-interval-out-of-range-rejected, firehose.create-stream-buffering-size-above-maximum-rejected, firehose.create-stream-name-exceeds-length-limit-rejected, firehose.put-record-batch-empty-records-list-rejected, firehose.put-record-batch-exceeds-max-records, firehose.put-record-batch-oversized-record-rejected, firehose.put-record-batch-partial-failure-reported, firehose.put-record-batch-response-order-matches-request-order, firehose.put-record-data-exceeds-max-size, firehose.put-record-oversized-payload-rejected, firehose.splunk-retry-duration-out-of-range-rejected, firehose.tag-value-exceeds-max-length-rejected.

Cause 2 of 6: pattern constraints are reported with the model's anchors instead of the unanchored/alternation-normalized form.
AWS: the reported pattern is derived from the Smithy @pattern trait but rendered without the ^...$ anchors — the model's DeliveryStreamName pattern ^[a-zA-Z0-9_.-]+$ is reported as [a-zA-Z0-9_.-]+, and the FileExtension pattern ^$|\.[0-9a-z!\-_.*'()]+$ is reported in the grouped form ^(|\.[0-9a-z!\-_.*'()]+)$.
Emulator: the raw model pattern is pasted verbatim (^[a-zA-Z0-9_.-]+$, ^$|\.[0-9a-z!\-_.*'()]+$).
Why: clients that surface or log the pattern show a different regex than AWS does, and message-shape assertions fail even though the constraint being enforced is identical.
Fix: the same constraint-message formatter must normalize the @pattern rendering (strip anchors / use the grouped alternation form) before interpolating it.
Traces: firehose.create-stream-invalid-name-characters-rejected, firehose.put-record-empty-payload-blocked, firehose.file-extension-overrides-the-compression-suffix-without-changing-the-bytes.

Cause 3 of 6: the duplicate-name ResourceInUseException message is not built from the name AND the account id.
AWS: Smithy: CreateDeliveryStream models ResourceInUseException (client, HTTP 400). Recorded behavior: the message is built from the requested stream name and the caller's account id — "Firehose <name> under accountId <accountId> already exists".
Emulator: "Firehose stream <name> already exists" — the account id is dropped and the noun differs.
Why: a client or operator reading the error loses the account scoping AWS always states, which is exactly what disambiguates a cross-account name collision.
Fix: the CreateDeliveryStream handler's already-exists path must build the message from the stream name and the resolved account id.
Traces: firehose.create-duplicate-delivery-stream-rejected, firehose.create-duplicate-name-resource-in-use.

Cause 4 of 6: the ResourceNotFoundException message names the stream instead of the account.
AWS: Smithy: DescribeDeliveryStream, DeleteDeliveryStream and PutRecord all model ResourceNotFoundException (client, HTTP 400). Recorded behavior: the message is built from the caller's account id and does NOT echo the requested stream name — "Firehose under account <accountId> not found." for the control-plane reads/deletes and "Firehose not found under account <accountId>." for PutRecord.
Emulator: "Firehose stream <name> not found" — it echoes the requested name and omits the account id.
Why: the emulator leaks the probed name back to the caller on a not-found (an enumeration signal AWS deliberately withholds) and drops the account scoping; clients matching the documented not-found text fail.
Fix: the not-found path in the DescribeDeliveryStream / DeleteDeliveryStream / PutRecord handlers must build the message from the account id only, with the PutRecord wording variant.
Traces: firehose.describe-nonexistent-stream-not-found, firehose.delete-nonexistent-stream-not-found, firehose.delete-stream-transitions-to-deleting-then-gone, firehose.delete-transitions-to-deleting, firehose.put-record-nonexistent-stream-not-found.

Cause 5 of 6: the delete-while-CREATING ResourceInUseException message is not built from the name, account and state.
AWS: Smithy: DeleteDeliveryStream models ResourceInUseException (client, HTTP 400). Recorded behavior: the message is built from the stream name, the account id and the blocking state — "Firehose <name> under account <accountId> cannot be deleted in CREATING state".
Emulator: "Firehose stream <name> is in status CREATING; it cannot be deleted until it is ACTIVE" — no account id, and it asserts an ACTIVE-only precondition AWS does not state.
Why: the account scoping is lost and the emulator's wording implies a stricter rule than the service's, misleading clients that parse or surface it.
Fix: the DeleteDeliveryStream handler's state-conflict path must build the message from name, account id and current state.
Traces: firehose.delete-stream-during-creating-rejected.

Cause 6 of 6: the conflicting-destination-configuration refusal uses emulator-authored prose instead of the service's fixed sentence.
AWS: Smithy: CreateDeliveryStream takes S3DestinationConfiguration (deprecated) and ExtendedS3DestinationConfiguration as separate optional members with no modeled union, so the mutual exclusion is enforced in the service and reported with a fixed sentence. Recorded behavior: "Exactly one of S3DestinationConfiguration or ExtendedS3DestinationConfiguration must be specified."
Emulator: "only one destination configuration may be specified, but 2 were: S3DestinationConfiguration, ExtendedS3DestinationConfiguration" — a generated enumeration rather than the fixed sentence.
Why: the fixed sentence is the stable, documented signal clients key on for this mutual-exclusion failure; a generated list is not.
Fix: the CreateDeliveryStream handler's destination mutual-exclusion check must emit the service's fixed message.
Traces: firehose.create-stream-conflicting-destination-configs, firehose.create-stream-conflicting-destinations-rejected.

## Evidence
Cause 1 (examples) — firehose.create-stream-buffering-size-above-maximum-rejected: expected "1 validation error detected: Value at 'extendedS3DestinationConfiguration.bufferingHints.sizeInMBs' failed to satisfy constraint: Member must have value less than or equal to 128", got "...Value '500' at '...sizeInMBs'...". firehose.put-record-batch-exceeds-max-records: expected "Value at 'records' failed to satisfy constraint: Member must have length less than or equal to 500", got "Value '501 items' at 'records' ...". firehose.put-record-data-exceeds-max-size: expected "Value at 'record.data' ... less than or equal to 1024000", got "Value '1024001 bytes' at 'record.data' ...". firehose.tag-value-exceeds-max-length-rejected: expected "Value at 'tags.1.member.value' ... less than or equal to 256", got "Value 'vvvv...[257 bytes]' at 'tags.1.member.value' ...".

Cause 2 — firehose.create-stream-invalid-name-characters-rejected: expected pattern rendering "[a-zA-Z0-9_.-]+", got "^[a-zA-Z0-9_.-]+$" (same for firehose.put-record-empty-payload-blocked). firehose.file-extension-overrides-the-compression-suffix-without-changing-the-bytes: expected "^(|\\.[0-9a-z!\\-_.*'()]+)$", got "^$|\\.[0-9a-z!\\-_.*'()]+$".

Cause 3 — firehose.create-duplicate-delivery-stream-rejected: expected 'Firehose <name> under accountId 000000000000 already exists', got 'Firehose stream <name> already exists' (same in firehose.create-duplicate-name-resource-in-use).

Cause 4 — firehose.describe-nonexistent-stream-not-found / firehose.delete-nonexistent-stream-not-found / firehose.delete-transitions-to-deleting / firehose.delete-stream-transitions-to-deleting-then-gone: expected 'Firehose under account 000000000000 not found.', got 'Firehose stream <name> not found'. firehose.put-record-nonexistent-stream-not-found: expected 'Firehose not found under account 000000000000.', got 'Firehose stream <name> not found'.

Cause 5 — firehose.delete-stream-during-creating-rejected: expected 'Firehose <name> under account 000000000000 cannot be deleted in CREATING state', got 'Firehose stream <name> is in status CREATING; it cannot be deleted until it is ACTIVE'.

Cause 6 — firehose.create-stream-conflicting-destination-configs / firehose.create-stream-conflicting-destinations-rejected: expected 'Exactly one of S3DestinationConfiguration or ExtendedS3DestinationConfiguration must be specified.', got 'only one destination configuration may be specified, but 2 were: S3DestinationConfiguration, ExtendedS3DestinationConfiguration'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite.
2. Cause 1: call CreateDeliveryStream with ExtendedS3DestinationConfiguration.BufferingHints.SizeInMBs=500 (and separately IntervalInSeconds=-1 / 1000, a 65-char DeliveryStreamName, a 257-byte tag value, SplunkDestinationConfiguration.RetryOptions.DurationInSeconds=-1), and PutRecord/PutRecordBatch with an oversized Data member, 0 records and 501 records — read Error.Message and observe the rejected value is interpolated after "Value".
3. Cause 2: call CreateDeliveryStream/PutRecord with a DeliveryStreamName containing illegal characters, and CreateDeliveryStream with FileExtension='nodot' — observe the reported pattern carries the model's ^...$ anchors.
4. Cause 3: CreateDeliveryStream twice with the same name — read the ResourceInUseException message.
5. Cause 4: call DescribeDeliveryStream, DeleteDeliveryStream and PutRecord for a name that was never created — read the ResourceNotFoundException messages.
6. Cause 5: call DeleteDeliveryStream while the stream is still CREATING — read the ResourceInUseException message.
7. Cause 6: call CreateDeliveryStream supplying both S3DestinationConfiguration and ExtendedS3DestinationConfiguration — read the message.
