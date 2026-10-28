Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.exceeding-shard-limit-blocked-by-server-side-enforcement
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis.exceeding-shard-limit-blocked-by-server-side-enforcement|run#5|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Commit: 4507d6d6dd116d6bbaed085269ed053454966346

# kinesis CreateStream enforces an emulator-internal 500-shard-per-stream ceiling instead of the account shard quota DescribeLimits reports, and invents the LimitExceededException message

> _Note: this issue was identified against the code base at commit `4507d6d6dd116d6bbaed085269ed053454966346`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStream

## Description
One net-new root cause (Cause 1 of 1) in the kinesis emulator, grounded in the failing behavioral check kinesis.exceeding-shard-limit-blocked-by-server-side-enforcement. No open issue covers it: the two bundled kinesis issues 20261027T011654Z-kinesis-9-net-new-root-causes-cloudformation-aws.md and 20261027T011654Z-kinesis-9-net-new-root-causes-dryrun-not.md, and 20261027T095220Z-kinesis-6-net-new-root-causes-startstreamencryption-accepts.md, describe other CreateStream/PutRecord/ListShards causes but none of them carries this check or the shard-quota refusal.

=== Cause 1 of 1: the shard ceiling CreateStream enforces is an emulator-internal per-stream constant, and its LimitExceededException message is invented ===
Failing check: kinesis.exceeding-shard-limit-blocked-by-server-side-enforcement.

AWS: Smithy models CreateStream's errors as InvalidArgumentException / LimitExceededException / ResourceInUseException / ValidationException, all HTTP 400 client errors (com.amazonaws.kinesis, 2013-12-02). The operation's own documentation names the condition: "You receive a LimitExceededException when making a CreateStream request when you try to do one of the following: Have more than five streams in the CREATING state at any point in time. Create more shards than are authorized for your account." The number authorized for the account is exactly the quota DescribeLimits publishes — DescribeLimitsOutput$ShardLimit and $OpenShardCount, both required integers (range 0..1000000) — so the ceiling CreateStream enforces and the ceiling DescribeLimits reports are one number, and a request for at most (ShardLimit - OpenShardCount) shards succeeds. Recorded behavior for the refusal (HTTP 400, code LimitExceededException): the message is
  "This request would exceed the shard limit for the account <accountId> in <region>. Current shard count for the account: <openShardCount>. Limit: <shardLimit>. Number of additional shards that would have resulted from this request: <requestedShardCount>. Refer to the AWS Service Limits page (http://docs.aws.amazon.com/general/latest/gr/aws_service_limits.html) for current limits and how to request higher limits."
Everything in that string is fixed text, including the sentence order, the punctuation and the whole "Refer to the AWS Service Limits page (…)" clause. The variable parts are five values taken from the request and from service state: <accountId> (the caller's account), <region> (the region the request was signed for), <openShardCount> and <shardLimit> (the same two numbers DescribeLimits answers with at that moment), and <requestedShardCount> (the ShardCount the refused request carried).

Emulator: the error CODE is right, but both the limit and the message are the emulator's own. The CreateStream handler compares the requested ShardCount against an internal per-stream constant (crate::state::MAX_SHARDS_PER_STREAM, 500) and refuses with the invented message
  "a stream of <n> shards exceeds the <cap> shards one stream of this emulator may hold"
(variable parts: the requested shard count and the emulator's own constant). Three divergences follow from that one cause: (1) the message shares no fixed text with AWS's — it names no account, no region, no current shard count and no account limit, and announces the emulator as the authority; (2) the ceiling is per-STREAM and unrelated to the account quota the emulator itself reports: in the replay DescribeLimits answered ShardLimit 1000 / OpenShardCount 0, so a well-formed CreateStream for any ShardCount in 501..1000 is refused although the account the emulator describes has the capacity, and the two surfaces contradict each other; (3) the neighbouring ceilings in the same handler (per-region streams/shards and emulator-wide streams/shards) refuse with the same style of invented, emulator-naming wording, so no CreateStream refusal on this path is AWS-shaped.

Why: capacity planning against Kinesis is exactly "read DescribeLimits, subtract OpenShardCount from ShardLimit, create a stream that fits". A client that does this gets a 400 LimitExceededException from the emulator for a request the emulator's own DescribeLimits said would fit, and the refusal text tells it neither what the limit was nor how many shards it is already using, so it cannot compute a retry size. Conversely, operators and SDK tests that match on AWS's message (it is the string Service Quotas tooling and runbooks key on) see an emulator-specific sentence instead. Note the check never reached its later assertions (the ResourceNotFoundException wording of the leftover stream, and the within-limits positive control), so those are ungraded here; the not-found wording itself is already covered as Cause 2 of 20261027T011654Z-kinesis-9-net-new-root-causes-dryrun-not.md.

Fix: the shard-ceiling branch of the CreateStream handler's validation, and the account-limit source it reads. The check must be made against the same account+region shard quota the DescribeLimits handler projects (ShardLimit vs OpenShardCount, counting the shards the account already holds plus the requested ShardCount), not against a per-stream constant, and the refusal must build AWS's LimitExceededException message from the caller's account id, the signed region, the current open shard count, that limit and the requested shard count. If the emulator must keep an internal memory ceiling, it belongs in the number DescribeLimits reports as ShardLimit so the two surfaces agree; the per-region and emulator-wide ceilings in the same handler should refuse through the same message builder.

## Evidence
From the replayed behavioral suite (simparity-behavioral-ga-prod-v1), 65/228 passed. Check kinesis.exceeding-shard-limit-blocked-by-server-side-enforcement, verdict mismatch.

Setup as the check ran it: DescribeLimits answered ShardLimit 1000, OpenShardCount 0, so the check asked for remaining+5 = 1005 shards via CreateStream(StreamName=<unique>, ShardCount=1005).

The error code matched (LimitExceededException). The message did not:
  AssertionError: expected message 'This request would exceed the shard limit for the account 000000000000 in us-west-2. Current shard count for the account: 0. Limit: 1000. Number of additional shards that would have resulted from this request: 1005. Refer to the AWS Service Limits page (http://docs.aws.amazon.com/general/latest/gr/aws_service_limits.html) for current limits and how to request higher limits.', got 'a stream of 1005 shards exceeds the 500 shards one stream of this emulator may hold'

(The account id 000000000000, region us-west-2, the counts 0/1000 and the 1005 are this run's values filling the template's variable slots; the requirement is the template, not those numbers.)

Source of the divergence in the checkout: services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/create_stream.rs lines 335-342 (`if shard_count > crate::state::MAX_SHARDS_PER_STREAM { … limit_exceeded(format!("a stream of {shard_count} shards exceeds the {} shards one stream of this emulator may hold", …)) }`), with the sibling invented ceilings at lines 398-445 (per-region and emulator-wide) and the error constructor at lines 239-244.

## Steps to Reproduce
1. kinesis.describe_limits() → note ShardLimit (L) and OpenShardCount (O); the emulator answers L=1000, O=0 on a fresh account.
2. kinesis.create_stream(StreamName='<unique>', ShardCount=L-O+5) → HTTP 400 LimitExceededException, but with message 'a stream of <L-O+5> shards exceeds the 500 shards one stream of this emulator may hold' instead of AWS's 'This request would exceed the shard limit for the account <account> in <region>. Current shard count for the account: <O>. Limit: <L>. Number of additional shards that would have resulted from this request: <L-O+5>. Refer to the AWS Service Limits page (http://docs.aws.amazon.com/general/latest/gr/aws_service_limits.html) for current limits and how to request higher limits.'
3. The limit itself: kinesis.create_stream(StreamName='<unique2>', ShardCount=600, StreamModeDetails={'StreamMode':'PROVISIONED'}) → refused LimitExceededException even though step 1 reported 1000 free shards; real AWS creates the stream (and only refuses once the account's own shard quota would be exceeded).
4. kinesis.describe_stream_summary(StreamName='<unique>') → ResourceNotFoundException, confirming the refused create left nothing behind (that part of the check is correct).
