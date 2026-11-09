Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__s3.create-channel-on-provisioned-stream-rejected-until-on-demand iam__kinesis__s3.create-channel-output-key-template-variants-rejected iam__kinesis__s3.s3-channel-delivers-every-put-records-payload-once
Issue-Key: iam__kinesis__s3.create-channel-on-provisioned-stream-rejected-until-on-demand|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: iam__kinesis__s3.create-channel-output-key-template-variants-rejected|_create_channel_expect_error#2|AssertionError|raise AssertionError(
Issue-Key: iam__kinesis__s3.s3-channel-delivers-every-put-records-payload-once|_poll_until_delivered#21|Timeout|time.sleep(2)
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# iam__kinesis__s3: 3 net-new causes — the provisioned-mode CreateChannel refusal is misworded, a path-traversal OutputKeyTemplate is accepted, and an S3 channel delivers nothing

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChannel

## Description
Three distinct root causes in the Kinesis channel (S3 delivery) surface.

Cause 1 of 3: the refusal for a channel on a provisioned-mode stream uses emulator wording.
AWS: CreateChannel against a stream in Provisioned capacity mode is refused with the message `Request is invalid. Stream <stream name> under account <account id> is in Provisioned mode. Please enable On-demand mode to use this feature.` The fixed text is that three-sentence form; the variable parts are the stream NAME (not its ARN) and the account id, taken from the request and state. Smithy: kinesis#CreateChannel declares the service's validation/invalid-argument error for this refusal; the message template is a service fact — recorded behavior only, pinned by the recording.
Emulator: the message is `stream <stream arn> is in PROVISIONED mode; a channel is only supported for data streams with the ON_DEMAND capacity mode` — different wording, and it identifies the stream by ARN rather than by name.
Why: a client that matches on the refusal text (or surfaces it to an operator) does not recognise the condition, and the remedy AWS spells out ('enable On-demand mode') is absent.
Fix: the CreateChannel validation path's message for the capacity-mode precondition.
Traces: iam__kinesis__s3.create-channel-on-provisioned-stream-rejected-until-on-demand.

Cause 2 of 3: OutputKeyTemplate values AWS rejects are accepted.
AWS: CreateChannel validates OutputKeyTemplate and refuses malformed templates — among them a template containing a path-traversal segment such as `a/../b` — with a modelled validation error and no channel created. Smithy: kinesis#CreateChannel's S3 destination configuration carries OutputKeyTemplate and declares the service's validation error — recorded behavior only for the specific rejected forms, which the recording enumerates.
Emulator: CreateChannel with OutputKeyTemplate='a/../b' succeeds and returns a channel ARN.
Why: a client can create a channel whose delivered object keys escape the intended prefix, writing objects outside the agreed key space; the validation it relies on to keep delivery inside a prefix is absent.
Fix: OutputKeyTemplate validation in the CreateChannel handler's destination-configuration validation.
Traces: iam__kinesis__s3.create-channel-output-key-template-variants-rejected.

Cause 3 of 3: an S3-destination channel never delivers records.
AWS: once a channel on an on-demand stream is active, every payload written by PutRecords is delivered to the S3 destination exactly once, under keys rendered from the output key template, within the delivery window. Smithy: the delivery itself is service behaviour — recorded behavior only; the recording shows every PutRecords payload appearing once.
Emulator: the check does not settle and is killed at its 900 s cap: no delivered object ever appears.
Why: the channel reports as created while data is silently dropped — the worst failure mode for a delivery feature, since the producer sees successful PutRecords and the consumer sees nothing.
Fix: the channel delivery engine (stream-to-S3 writer) behind CreateChannel.
Traces: iam__kinesis__s3.s3-channel-delivers-every-put-records-payload-once.

## Evidence
Cause 1 — Expected message 'Request is invalid. Stream <name> under account <account> is in Provisioned mode. Please enable On-demand mode to use this feature.'; Actual 'stream arn:aws:kinesis:us-west-2:000000000000:stream/… is in PROVISIONED mode; a channel is only supported for data streams with the ON_DEMAND capacity mode'.
Cause 2 — Expected: CreateChannel with OutputKeyTemplate='a/../b' refused; Actual: AssertionError: CreateChannel with OutputKeyTemplate='a/../b' unexpectedly succeeded: 'arn:aws:kinesis:…:channel/…'.
Cause 3 — Expected: every PutRecords payload delivered once to S3; Actual: Error: Check timed out after 900.0s.

## Steps to Reproduce
Cause 1: create a Provisioned-mode stream and call CreateChannel on it; compare the message.
Cause 2: on an on-demand stream, call CreateChannel with OutputKeyTemplate='a/../b'; observe success.
Cause 3: create a channel with an S3 destination on an on-demand stream, PutRecords a batch, then list the destination prefix; observe no objects.
