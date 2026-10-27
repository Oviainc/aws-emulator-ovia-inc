Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.deep-kinesis-resharding-update-shard-count-below-half-rejected kinesis.deep-kinesis-resharding-update-shard-count-beyond-double-rejected kinesis.increase-retention-period-beyond-max-rejected kinesis.merge-shards-on-ondemand-stream-fails kinesis.split-shard-invalid-hash-key-fails kinesis.update-stream-mode-concurrent-calls-conflict
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.deep-kinesis-resharding-update-shard-count-below-half-rejected|run#7|AssertionError|assert code == "InvalidArgumentException", (
Issue-Key: kinesis.deep-kinesis-resharding-update-shard-count-beyond-double-rejected|run#9|AssertionError|assert code == "InvalidArgumentException", (
Issue-Key: kinesis.increase-retention-period-beyond-max-rejected|run#6|AssertionError|assert code == "InvalidArgumentException", (
Issue-Key: kinesis.merge-shards-on-ondemand-stream-fails|run#7|AssertionError|assert error.get("Code") == "ValidationException", (
Issue-Key: kinesis.split-shard-invalid-hash-key-fails|run#8|AssertionError|assert code == "InvalidArgumentException", f"expected InvalidArgumentException, got {code}"
Issue-Key: kinesis.update-stream-mode-concurrent-calls-conflict|run#10|AssertionError|assert len(successes) == 1, f"expected exactly one winner, got {results}"
Issue-Commit: 026ab5e7ce30980fab75d38bce9af5c142700315

# kinesis: 5 net-new root causes — UpdateShardCount's half/double scaling bounds, IncreaseStreamRetentionPeriod's 8760-hour ceiling, MergeShards' on-demand refusal, SplitShard's NewStartingHashKey range check and UpdateStreamMode's concurrent-call conflict are all unreachable: each op answers NotImplemented

> _Note: this issue was identified against the code base at commit `026ab5e7ce30980fab75d38bce9af5c142700315`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateShardCount, IncreaseStreamRetentionPeriod, MergeShards, SplitShard, UpdateStreamMode

## Description
Five net-new root causes in the kinesis control plane. Every one of them is a validation/concurrency contract that the emulator never reaches, because the owning handler is still the scaffold that answers HTTP 501 with error code `NotImplemented` and message `operation <Op> is scaffolded but not implemented in this emulator`.

Related already-open issues, named here so they are not duplicated: `20261027T011654Z-kinesis-updateshardcount-stub.md`, `20261027T011654Z-kinesis-increasestreamretentionperiod-stub.md`, `20261027T011654Z-kinesis-mergeshards-stub.md`, `20261027T011654Z-kinesis-splitshard-stub.md`, `20261027T011654Z-kinesis-updatestreammode-stub.md`. Each of those issues carries its OWN explicit replay-key list (Issue-Trace-Ids), and NONE of them lists any of the six checks below — so if only those issues are closed, these six refusal/concurrency contracts stay unrun and ungraded. This issue exists to carry those replay keys and to state the AWS-side requirement each one grades, which the stub issues do not state.

=== Cause 1 of 5: UpdateShardCount does not enforce the half/double scaling bounds ===
AWS: Smithy UpdateShardCount — input TargetShardCount (@required, PositiveIntegerObject, range min 1) and ScalingType (@required); modeled errors include InvalidArgumentException (client, HTTP 400). The operation's own documentation states the default limits: "Scale up to more than double your current shard count for a stream" and "Scale down below half your current shard count for a stream" are both disallowed. A target outside [ceil(current/2), 2*current] is refused with 400 InvalidArgumentException; the bound is evaluated against the stream's LIVE open shard count, and the refusal must leave the shard map completely untouched (no shard closed, no child minted). Recorded message template, fixed text exactly: "UpdateShardCount cannot scale down below half your current open shard count. Current open shard count: <N> Target shard count: <T>" for the scale-down bound and "UpdateShardCount cannot scale up over double your current open shard count. Current open shard count: <N> Target shard count: <T>" for the scale-up bound. Fixed text = everything but <N> and <T>; <N> is the stream's current open shard count read from emulator state and <T> is the request's TargetShardCount (the 4/1 and 1/3 values in the recordings are only one run's fill-ins). Exactly half and exactly double must SUCCEED on the same stream and echo CurrentShardCount/TargetShardCount.
Emulator: both calls are refused with error code 'NotImplemented' (HTTP 501). Replay: "AssertionError: UpdateShardCount 4 -> 1 (below half) returned 'NotImplemented', expected InvalidArgumentException" and "AssertionError: UpdateShardCount 1 -> 3 (over double) returned 'NotImplemented', expected InvalidArgumentException".
Why: a client that resizes a provisioned stream cannot distinguish "this target violates the 2x/0.5x rolling bound" from "this API does not exist". Autoscaling code that catches InvalidArgumentException to clamp its target and retry sees an unmodeled 501 instead and aborts; code that trusts a 2xx sees none. An implementation that only checks `target <= 2*current` will also accept 4 -> 1 and silently collapse a 4-shard stream to one shard, so the scale-DOWN bound needs its own check against the live open shard count.
Fix: the UpdateShardCount handler and its input validation, in the kinesis service crate's ops module for that operation; the bound must be computed from the stream's open-shard set in service state, and the refusal path must not mutate that state.
Traces: kinesis.deep-kinesis-resharding-update-shard-count-below-half-rejected, kinesis.deep-kinesis-resharding-update-shard-count-beyond-double-rejected.

=== Cause 2 of 5: IncreaseStreamRetentionPeriod does not enforce the 8760-hour ceiling ===
AWS: Smithy IncreaseStreamRetentionPeriod — input RetentionPeriodHours (@required, integer); modeled errors include InvalidArgumentException (client, HTTP 400). The operation's documentation states "The maximum value of a stream's retention period is 8760 hours (365 days)". A request above that ceiling is refused with 400 InvalidArgumentException and message, fixed text exactly: "Maximum allowed retention period is 8760 hours. Requested retention period (<H> hours) is too long." — fixed text is everything but <H>, which is the request's RetentionPeriodHours (8761 is one run's fill-in). The refusal must leave the stream's RetentionPeriodHours as it was (24 on a freshly created stream), and exactly 8760 must be accepted and become visible in DescribeStreamSummary.
Emulator: the call is refused with error code 'NotImplemented'. Replay: "AssertionError: expected InvalidArgumentException for RetentionPeriodHours=8761, got NotImplemented".
Why: the documented ceiling is unverifiable and the whole retention contract is unreachable — a client cannot set a legal retention period at all, and one probing the boundary gets an unmodeled 501 rather than the modeled client error it must retry/clamp on.
Fix: the IncreaseStreamRetentionPeriod handler and its input validation in the kinesis service crate's ops module, plus the stream-state field it writes (RetentionPeriodHours, surfaced by DescribeStreamSummary).
Traces: kinesis.increase-retention-period-beyond-max-rejected.

=== Cause 3 of 5: MergeShards does not refuse an ON_DEMAND stream with ValidationException ===
AWS: Smithy MergeShards — input ShardToMerge and AdjacentShardToMerge (both @required ShardId); modeled errors include ValidationException and InvalidArgumentException (client, HTTP 400). Manual resharding is a provisioned-mode-only operation, so MergeShards on a stream in on-demand mode is refused with 400 ValidationException and message, fixed text exactly: "Request is invalid. Stream <StreamName> under account <AccountId> is in On-Demand mode." — fixed text is everything but <StreamName> (from the request) and <AccountId> (the caller's account from emulator state). The rejection must leave the shard set byte-identical.
Emulator: error code 'NotImplemented' instead. Replay: "AssertionError: expected ValidationException, got NotImplemented".
Why: the mode-guard is the only thing stopping a caller from hand-resharding an on-demand stream. Against the emulator a caller cannot tell a mode violation (ValidationException — fix your request) from a transport/serverside fault (501), so mode-aware client logic is untestable and an eventual permissive implementation could mutate an on-demand stream's shard map, which AWS never does.
Fix: the MergeShards handler in the kinesis service crate's ops module — a stream-mode guard read from the stream's StreamModeDetails in service state, ahead of any shard mutation.
Traces: kinesis.merge-shards-on-ondemand-stream-fails.

=== Cause 4 of 5: SplitShard does not validate NewStartingHashKey against the parent's hash-key range ===
AWS: Smithy SplitShard — input ShardToSplit (@required) and NewStartingHashKey (@required HashKey, pattern ^(0|([1-9]\\d{0,38}))$); modeled errors include InvalidArgumentException (client, HTTP 400). A NewStartingHashKey that is not strictly inside the parent's range — specifically not both greater than one plus the parent's StartingHashKey and less than the parent's EndingHashKey, i.e. one that would mint an empty child — is refused with 400 InvalidArgumentException and message, fixed text exactly: "NewStartingHashKey <K> used in SplitShard() on shard <ShardId> in stream <StreamName> under account <AccountId> is not both greater than one plus the shard's StartingHashKey <LO> and less than the shard's EndingHashKey <HI>." — fixed text is everything but <K> (request), <ShardId>/<StreamName> (request) and <LO>/<HI>/<AccountId> (emulator state). The refused call must leave the parent open (no EndingSequenceNumber) and mint no children; a split of the same parent at its midpoint must then succeed and produce children covering [LO, mid-1] and [mid, HI].
Emulator: error code 'NotImplemented' instead. Replay: "AssertionError: expected InvalidArgumentException, got NotImplemented".
Why: a client cannot distinguish a bad hash key from a missing API, so it cannot correct and retry; and because the parameter is unvalidated there is nothing preventing a later implementation from closing a parent shard into an empty child, which permanently corrupts the shard lineage AWS guarantees.
Fix: the SplitShard handler and its input validation in the kinesis service crate's ops module — compare NewStartingHashKey against the parent shard's HashKeyRange held in service state before closing the parent or minting children.
Traces: kinesis.split-shard-invalid-hash-key-fails.

=== Cause 5 of 5: concurrent UpdateStreamMode calls produce no winner and no ResourceInUseException ===
AWS: Smithy UpdateStreamMode — input StreamARN (@required) and StreamModeDetails (@required); modeled errors include ResourceInUseException and ValidationException (client, HTTP 400). Two overlapping UpdateStreamMode calls on the same stream race: exactly ONE is accepted (200, stream goes UPDATING then settles on the new mode) and the other is refused with 400 ResourceInUseException because the stream is not in ACTIVE state while the first update is in flight. The stream must end up in the requested mode (ON_DEMAND here) as reported by DescribeStreamSummary.
Emulator: both concurrent calls are refused identically. Replay: "AssertionError: expected exactly one winner, got [('err', 'NotImplemented'), ('err', 'NotImplemented')]".
Why: there is no winner and no loser, so the stream never changes mode and a client gets neither the 200 it must wait on nor the ResourceInUseException it must back off and retry on. Any client that serialises capacity-mode changes by catching ResourceInUseException sees an unmodeled 501 and gives up.
Fix: the UpdateStreamMode handler in the kinesis service crate's ops module — it must take the stream record in service state under a single-writer guard, admit one caller (transitioning StreamStatus to UPDATING and then ACTIVE with the new StreamModeDetails) and refuse any overlapping caller with ResourceInUseException while the stream is not ACTIVE.
Traces: kinesis.update-stream-mode-concurrent-calls-conflict.

## Evidence
Behavioral replay, suite simparity-behavioral-ga-prod-v1 (kinesis 21/199 passed). Each failing check below reached the emulator and was answered with error code 'NotImplemented' (HTTP 501, message "operation <Op> is scaffolded but not implemented in this emulator") where AWS answers a modeled 400 client error or a 200.

1) kinesis.deep-kinesis-resharding-update-shard-count-below-half-rejected [mismatch]
   expected: 400 InvalidArgumentException, Message "UpdateShardCount cannot scale down below half your current open shard count. Current open shard count: <N> Target shard count: <T>" (recorded run: N=4, T=1), shard map unchanged, then 4 -> 2 accepted.
   actual:   AssertionError: UpdateShardCount 4 -> 1 (below half) returned 'NotImplemented', expected InvalidArgumentException

2) kinesis.deep-kinesis-resharding-update-shard-count-beyond-double-rejected [mismatch]
   expected: 400 InvalidArgumentException, Message "UpdateShardCount cannot scale up over double your current open shard count. Current open shard count: <N> Target shard count: <T>" (recorded run: N=1, T=3), nothing mutated, then 1 -> 2 accepted.
   actual:   AssertionError: UpdateShardCount 1 -> 3 (over double) returned 'NotImplemented', expected InvalidArgumentException

3) kinesis.increase-retention-period-beyond-max-rejected [mismatch]
   expected: 400 InvalidArgumentException, Message "Maximum allowed retention period is 8760 hours. Requested retention period (<H> hours) is too long." (recorded run: H=8761), retention still 24, then 8760 accepted and visible in DescribeStreamSummary.
   actual:   AssertionError: expected InvalidArgumentException for RetentionPeriodHours=8761, got NotImplemented

4) kinesis.merge-shards-on-ondemand-stream-fails [mismatch]
   expected: 400 ValidationException, Message "Request is invalid. Stream <StreamName> under account <AccountId> is in On-Demand mode.", shard set unchanged.
   actual:   AssertionError: expected ValidationException, got NotImplemented

5) kinesis.split-shard-invalid-hash-key-fails [mismatch]
   expected: 400 InvalidArgumentException, Message "NewStartingHashKey <K> used in SplitShard() on shard <ShardId> in stream <StreamName> under account <AccountId> is not both greater than one plus the shard's StartingHashKey <LO> and less than the shard's EndingHashKey <HI>.", parent still open, then the midpoint split succeeds with children [LO, mid-1] and [mid, HI].
   actual:   AssertionError: expected InvalidArgumentException, got NotImplemented

6) kinesis.update-stream-mode-concurrent-calls-conflict [mismatch]
   expected: of two overlapping UpdateStreamMode(ON_DEMAND) calls, exactly one 200 and exactly one 400 ResourceInUseException; stream settles ACTIVE/ON_DEMAND.
   actual:   AssertionError: expected exactly one winner, got [('err', 'NotImplemented'), ('err', 'NotImplemented')]

Model facts used (services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json): UpdateShardCount input TargetShardCount @required range min 1 + ScalingType @required, errors AccessDenied/InvalidArgument/LimitExceeded/ResourceInUse/ResourceNotFound/Validation; its doc lists the "more than double" and "below half" default limits. IncreaseStreamRetentionPeriod input RetentionPeriodHours @required, errors include InvalidArgumentException; its doc states the 8760-hour maximum. MergeShards input ShardToMerge/AdjacentShardToMerge @required, errors include ValidationException. SplitShard input ShardToSplit/NewStartingHashKey @required (HashKey pattern ^(0|([1-9]\\d{0,38}))$), errors include InvalidArgumentException. UpdateStreamMode input StreamARN/StreamModeDetails @required, errors include ResourceInUseException. All modeled errors are client-kind with HTTP 400 (defaulted).

## Steps to Reproduce
1. Boot the kinesis emulator from this checkout.
2. Cause 1: CreateStream(ShardCount=4, PROVISIONED), wait ACTIVE, then UpdateShardCount(TargetShardCount=1, ScalingType=UNIFORM_SCALING) — observe error code NotImplemented (501) instead of 400 InvalidArgumentException "UpdateShardCount cannot scale down below half your current open shard count. Current open shard count: 4 Target shard count: 1"; also confirm UpdateShardCount(TargetShardCount=2) is accepted. On a 1-shard stream, UpdateShardCount(TargetShardCount=3) must be refused with the "...cannot scale up over double..." message while TargetShardCount=2 is accepted.
3. Cause 2: CreateStream(ShardCount=1), confirm DescribeStreamSummary reports RetentionPeriodHours 24, then IncreaseStreamRetentionPeriod(RetentionPeriodHours=8761) — observe NotImplemented instead of 400 InvalidArgumentException "Maximum allowed retention period is 8760 hours. Requested retention period (8761 hours) is too long."; then IncreaseStreamRetentionPeriod(8760) must succeed and be visible.
4. Cause 3: CreateStream(StreamModeDetails={StreamMode: ON_DEMAND}), wait ACTIVE, ListShards, then MergeShards on the first two shard ids — observe NotImplemented instead of 400 ValidationException "Request is invalid. Stream <name> under account <account> is in On-Demand mode." and verify the shard ids are unchanged.
5. Cause 4: CreateStream(ShardCount=1), ListShards, then SplitShard(NewStartingHashKey = the shard's own StartingHashKey) — observe NotImplemented instead of 400 InvalidArgumentException with the NewStartingHashKey message; verify the parent is still open, then split at the midpoint and verify the two children's hash ranges.
6. Cause 5: CreateStream(PROVISIONED, ShardCount=1), wait ACTIVE, then issue two UpdateStreamMode(StreamARN, {StreamMode: ON_DEMAND}) calls concurrently — observe both refused NotImplemented instead of exactly one 200 and one 400 ResourceInUseException, and the stream never settling on ON_DEMAND.
7. Re-replay the six check ids in trace_ids to confirm the fix.
