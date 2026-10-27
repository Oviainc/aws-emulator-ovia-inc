Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.concurrent-update-primary-region-serialized
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms.concurrent-update-primary-region-serialized|run#14|AssertionError|assert all(code == "KMSInvalidStateException" for code in failures), failures
Issue-Commit: d558c4fc1331790598d7d8dd6ea4bd673681392c

# KMS: concurrent UpdatePrimaryRegion racers are refused ValidationException instead of KMSInvalidStateException (1 cause)

> _Note: this issue was identified against the code base at commit `d558c4fc1331790598d7d8dd6ea4bd673681392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePrimaryRegion

## Description
REOPENS 20261024T102105Z-kms-updateprimaryregion-stub.md: its fix left the call refused.

Cause 1 of 1 — UpdatePrimaryRegion no longer answers NotImplemented, but when several UpdatePrimaryRegion calls are issued concurrently against the same multi-Region key set (all naming the same target replica Region), the losing racers are refused with the wrong error class.

AWS: UpdatePrimaryRegion serializes concurrent promotions of one multi-Region key set. Exactly one caller wins; a caller that arrives while the key set is transiently busy moving the primary is refused with KMSInvalidStateException. Smithy: UpdatePrimaryRegion's modeled errors are DisabledException, InvalidArnException, KMSInternalException, KMSInvalidStateException (httpError 409, awsQueryError code KMSInvalidStateException), NotFoundException and UnsupportedOperationException — ValidationException is NOT among them, so it cannot be the refusal for a key that is merely busy. KMSInvalidStateException is precisely the modeled fault for "the request was rejected because the state of the specified resource is not valid for this request". The key set must then settle in exactly ONE deterministic configuration — the target Region PRIMARY, the origin Region its REPLICA — seen consistently from both Region endpoints, with both views listing the same single replica set (no split brain).

Emulator: one racer succeeds (so the operation itself works), but the two losing racers are refused with error code 'ValidationException'. Replay evidence: `AssertionError: ['ValidationException', 'ValidationException']` from the assertion `all(code == "KMSInvalidStateException" for code in failures)`. The check stops at that assertion, so the subsequent demotion/promotion and no-split-brain legs are left ungraded; they must also hold once the code is corrected.

Why: a client that races or retries UpdatePrimaryRegion branches on KMSInvalidStateException to mean "the key set is busy, retry" — AWS SDK/client retry and backoff logic keys on that transient-state fault. ValidationException is a permanent input fault that no client retries, so an application that retries correctly against AWS will abandon the promotion against the emulator, and conversely an operator debugging a failed promotion is told the request was malformed when it was merely serialized behind another caller.

Fix: the UpdatePrimaryRegion handler's concurrency/state gate in services/service-com-amazonaws-kms-trentservice — the serialization guard must classify a busy multi-Region key set as KMSInvalidStateException (HTTP 409) rather than falling through to generic request validation. Message requirement: the fixed text is AWS's KMSInvalidStateException wording for a key that is not in a valid state for the request, with the key ARN taken from the request/state as its only variable part; do not emit a ValidationException message.

Traces: kms.concurrent-update-primary-region-serialized.

## Evidence
kms.concurrent-update-primary-region-serialized [mismatch]:
  File "check_kms_concurrent_update_primary_region_serialized.py", line 69, in run
    assert all(code == "KMSInvalidStateException" for code in failures), failures
  AssertionError: ['ValidationException', 'ValidationException']

Setup: CreateKey(MultiRegion=True) in region A, ReplicateKey into region B, poll the replica to KeyState=Enabled, then fire 3 concurrent update_primary_region(KeyId=<key>, PrimaryRegion=<region B>) calls from a ThreadPoolExecutor(max_workers=3).
Expected (AWS): 1 success + 2 refusals, each refusal Error.Code == "KMSInvalidStateException", HTTP 409.
Actual (emulator): 1 success + 2 refusals, each refusal Error.Code == "ValidationException".
Model: Smithy UpdatePrimaryRegion errors = {DisabledException 409, InvalidArnException 400, KMSInternalException 500, KMSInvalidStateException 409, NotFoundException 404, UnsupportedOperationException 400}; ValidationException is not modeled on this operation.
Still ungraded behind the stopped assertion: DescribeKey from both endpoints must converge on MultiRegionKeyType REPLICA at the origin and PRIMARY at the target, PrimaryKey.Region == target, and both views' ReplicaKeys sets equal to {origin region}.

## Steps to Reproduce
1. Replay the behavioral suite and re-run check kms.concurrent-update-primary-region-serialized.
2. Or by hand against the emulator: kms.create_key(MultiRegion=True) in region A; kms.replicate_key(KeyId=<id>, ReplicaRegion=<region B>); poll describe_key at the region-B endpoint until KeyState == "Enabled".
3. Issue three concurrent kms.update_primary_region(KeyId=<id>, PrimaryRegion=<region B>) calls from three threads.
4. Inspect the Error.Code of every call that did not succeed — observe "ValidationException" where AWS returns "KMSInvalidStateException" (HTTP 409).
5. Then confirm the follow-on legs: describe_key at both endpoints must agree on a single final configuration (target PRIMARY, origin REPLICA, identical ReplicaKeys sets).
