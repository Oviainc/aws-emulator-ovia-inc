Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.stop-stream-encryption-on-unencrypted-stream-rejected
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.stop-stream-encryption-on-unencrypted-stream-rejected|run#9|AssertionError|assert code == "InvalidArgumentException", (
Issue-Commit: 11a7d97ef748050dca45f9e746efe80150732e78

# kinesis StopStreamEncryption answers 501 NotImplemented, so the "Cannot stop encrypting an unencrypted stream." InvalidArgumentException refusal (and disabling encryption at all) never happens

> _Note: this issue was identified against the code base at commit `11a7d97ef748050dca45f9e746efe80150732e78`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopStreamEncryption

## Description
Cause 1 of 1: StopStreamEncryption is an unimplemented scaffold stub, so every call to it — valid or invalid — is refused with the scaffold's 501 `NotImplemented` and none of the operation's modeled behaviour exists.

AWS: Smithy (services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json, operation com.amazonaws.kinesis#StopStreamEncryption) models StopStreamEncryption with required `EncryptionType` (enum KMS|NONE) and `KeyId` (len 1..2048), optional `StreamName`/`StreamARN`, and the modeled errors AccessDeniedException, InvalidArgumentException, LimitExceededException, ResourceInUseException and ResourceNotFoundException, all HTTP 400. InvalidArgumentException is documented as "A specified parameter exceeds its restrictions, is not supported, or can't be used. For more information, see the returned message." Recorded behavior: calling StopStreamEncryption on a stream whose EncryptionType is NONE is REFUSED with HTTP 400, error code `InvalidArgumentException` and the message `Cannot stop encrypting an unencrypted stream.` — that message is entirely fixed text for this error class (it carries no request- or state-derived value), and the refusal leaves the stream untouched: DescribeStreamSummary still reports EncryptionType `NONE` and StreamStatus `ACTIVE` (a rejected request must not start an update). On a genuinely KMS-encrypted stream the same call succeeds (200, empty body) and the stream asynchronously returns to EncryptionType `NONE`.

Emulator: the operation's handler is the generated scaffold — services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/stop_stream_encryption.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its `handle` body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("StopStreamEncryption"))`. The replayed check therefore received error code `NotImplemented` (HTTP 501) where AWS answers 400 `InvalidArgumentException`: `AssertionError: expected InvalidArgumentException stopping encryption on an unencrypted stream, got NotImplemented`. Because the call never reaches any state, neither the unencrypted-stream refusal, its message, the "no state change" guarantee, nor the success path that returns an encrypted stream to EncryptionType NONE is emulated.

Why: a client that disables SSE on a Kinesis stream sees a 501 with a code outside the operation's modeled error set. Retry/branching logic keyed on InvalidArgumentException (the documented signal that encryption is already off) never fires, 501 is not retried or handled by AWS SDK error mappers for this operation, and no application can ever turn stream encryption off against the emulator — the stream stays reported as it was.

Fix: implement the handler in services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/stop_stream_encryption.rs (resolve the stream by StreamName/StreamARN, refuse an unencrypted stream with the modeled InvalidArgumentException and AWS's fixed message, otherwise clear the stream's encryption in the service state so DescribeStreamSummary reports EncryptionType NONE and StreamStatus returns to ACTIVE) and delete the SCAFFOLDED marker line. Note: this check's later positive control also calls StartStreamEncryption, which is a separate already-filed stub (20261027T011654Z-kinesis-startstreamencryption-stub.md); the divergence graded and reported here is the StopStreamEncryption refusal itself, which the open StartStreamEncryption issue does not cover.

Traces: kinesis.stop-stream-encryption-on-unencrypted-stream-rejected.

## Evidence
Expected (real AWS): StopStreamEncryption(StreamName=<unencrypted stream>, EncryptionType="KMS", KeyId=<a KMS key id>) -> HTTP 400, Error.Code `InvalidArgumentException`, Error.Message `Cannot stop encrypting an unencrypted stream.` (fixed text, no variable parts); DescribeStreamSummary afterwards still shows EncryptionType=`NONE`, StreamStatus=`ACTIVE`. On a KMS-encrypted stream the same call returns HTTP 200 and the stream converges to EncryptionType=`NONE`.

Actual (emulator): HTTP 501, Error.Code `NotImplemented` for every StopStreamEncryption call. Replay detail: `File "check_kinesis_stop_stream_encryption_on_unencrypted_stream_rejected.py", line 104, in run / assert code == "InvalidArgumentException" / AssertionError: expected InvalidArgumentException stopping encryption on an unencrypted stream, got NotImplemented`. Source confirmation: services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/stop_stream_encryption.rs line 4 (`SCAFFOLDED, NOT IMPLEMENTED`) and line 206 (`Err(crate::scaffolded::Scaffolded::not_implemented("StopStreamEncryption"))`).

## Steps to Reproduce
1. CreateStream(StreamName=<name>, ShardCount=1) and wait for StreamStatus ACTIVE; confirm DescribeStreamSummary reports EncryptionType=NONE.
2. Create any KMS key and call StopStreamEncryption(StreamName=<name>, EncryptionType="KMS", KeyId=<key id>).
3. Observe HTTP 501 / Error.Code NotImplemented instead of HTTP 400 / InvalidArgumentException with message "Cannot stop encrypting an unencrypted stream.".
4. Re-replay behavioral check kinesis.stop-stream-encryption-on-unencrypted-stream-rejected to confirm the fix (it also asserts EncryptionType stays NONE and StreamStatus stays ACTIVE after the refusal, and that StopStreamEncryption succeeds on a genuinely encrypted stream).
