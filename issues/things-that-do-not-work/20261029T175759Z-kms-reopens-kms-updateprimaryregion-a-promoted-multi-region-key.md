Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms.update-primary-region-disabled-key-rejected
Issue-Shape-Id: com.amazonaws.kms#TrentService
Issue-Key: kms|DisableKey|KMSInvalidStateException
Issue-Commit: 2e707008848158ee4bc91f1419f4a05920706f3a

# REOPENS kms UpdatePrimaryRegion: a promoted multi-Region key is left in KeyState Updating forever, so DisableKey on it is refused KMSInvalidStateException '<arn> is updating.' (1 cause)

> _Note: this issue was identified against the code base at commit `2e707008848158ee4bc91f1419f4a05920706f3a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdatePrimaryRegion

## Description
REOPENS 20261027T005501Z-kms-concurrent-updateprimaryregion-racers-are-refused-validationexception-instead.md: its fix left the call refused. That issue asked for concurrent UpdatePrimaryRegion racers to be refused KMSInvalidStateException instead of ValidationException; the key is now marked 'updating' but never leaves that state, so an ordinary follow-up call on the promoted key is refused where it must succeed. (See also the fixed 20261024T102105Z-kms-updateprimaryregion-stub.md and the fixed 20261024T221823Z-kms-grant-authorization-grant-refusal-contracts-listretirablegrants-validation.md, whose Cause 8 covered the no-replica refusal message.)

ONE cause.

Cause 1 of 1 — after a successful UpdatePrimaryRegion the key set stays in KeyState Updating, so every state-gated operation on it is refused.
AWS: UpdatePrimaryRegion moves the key set through KeyState Updating transiently and then both keys settle back to Enabled; the promoted key is immediately usable for ordinary control-plane calls, and DisableKey on it succeeds (DescribeKey then reports KeyState Disabled). Smithy: UpdatePrimaryRegion models KMSInvalidStateException/DisabledException/UnsupportedOperationException/NotFoundException/InvalidArnException and returns Unit; DisableKey's own KMSInvalidStateException exists for keys genuinely in a non-permitting state, and KeyMetadata$KeyState is the only observable of that state.
Emulator: the promotion itself is reported (the check's polls saw the origin key become REPLICA and the replica become PRIMARY), but the subsequent DisableKey on the promoted key is refused KMSInvalidStateException, HTTP 400/409, with the message template '<key ARN> is updating.' — the Updating marker set by the promotion is never cleared, so the key is permanently unusable for state-gated operations. The error is raised by the emulator, not by the check: it aborts the check before its own assertions run.
Why: every multi-Region key promoted in the emulator becomes a dead key — DisableKey, ScheduleKeyDeletion and every other state-gated call on it are refused forever, so a client cannot rehearse a region failover (promote, then take the old primary out of service), and the suite's disabled-primary refusal contract (UpdatePrimaryRegion on a disabled primary -> DisabledException '<arn> is disabled.') can never be reached.
Fix: the UpdatePrimaryRegion handler's key state transition — clear the Updating state on both the promoted and demoted keys when the promotion completes (and make the transient marker time-bounded rather than terminal).
Traces: kms.update-primary-region-disabled-key-rejected.

## Evidence
kms.update-primary-region-disabled-key-rejected (verdict: error): "botocore.errorfactory.KMSInvalidStateException: An error occurred (KMSInvalidStateException) when calling the DisableKey operation: arn:aws:kms:us-east-1:<account>:key/mrk-<id> is updating." The exception is raised at the check's `replica_client.disable_key(KeyId=replica_arn)` line, which runs AFTER its positive control passed: UpdatePrimaryRegion(KeyId=<origin>, PrimaryRegion=<replica region>) returned, the origin key polled to MultiRegionKeyType=REPLICA and the replica polled to MultiRegionKeyType=PRIMARY. Expected: DisableKey succeeds and DescribeKey reports KeyState 'Disabled', after which UpdatePrimaryRegion on that disabled primary is refused DisabledException '<key ARN> is disabled.' The region, account and mrk-<id> shown are this run's values; the requirement is that no state-gated call is refused 'is updating' once the promotion has been reported.

## Steps to Reproduce
Replay kms.update-primary-region-disabled-key-rejected, or by hand: CreateKey(MultiRegion=True) in region A; ReplicateKey(ReplicaRegion=B); poll DescribeKey in B until KeyState=Enabled; UpdatePrimaryRegion(KeyId=<key>, PrimaryRegion=B) from A; poll until the B key reports MultiRegionKeyType=PRIMARY; then call DisableKey in region B on that key — observe KMSInvalidStateException '<arn> is updating.' instead of a successful disable. DescribeKey also keeps reporting KeyState 'Updating'.
