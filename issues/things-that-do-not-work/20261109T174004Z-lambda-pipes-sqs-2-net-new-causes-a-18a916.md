Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__pipes__sqs.events-pipes-enrichment-failure-retains-source-record-and-withholds-target lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations
Issue-Key: lambda__pipes__sqs.events-pipes-enrichment-failure-retains-source-record-and-withholds-target|_drain_until#13|AssertionError|raise AssertionError(f"{description} did not arrive within {timeout_s}s; read {got!r}")
Issue-Key: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations|run#19|AssertionError|assert max_overlap >= max_concurrency, (
Issue-Commit: 6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4

# lambda__pipes__sqs: 2 net-new causes — a pipe with a failing enrichment never delivers the healthy record, and an SQS event source mapping never scales up to MaximumConcurrency

> _Note: this issue was identified against the code base at commit `6c5e636bebc82ccd4609eb5d54ba0275e4ea7ff4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Two distinct defects. Neither replay key is carried by an open issue; the open lambda__pipes__sqs issues (20261107T175408Z-...-0e28d4.md, 20261104T213346Z-..., 20261105T094842Z-...) cover different causes and their own checks are marked owned in this run.

Cause 1 of 2: a per-record enrichment failure stops the healthy records too.
AWS: when a pipe's enrichment fails for one record, that record is retained at the source (its SQS visibility timeout lapses and it is redelivered) and is withheld from the target, while records the enrichment handles successfully continue to be rendered and delivered to the target. The healthy record arrives well inside the recorded window.
Emulator: the healthy record never arrives — the target stays empty for the full 150 s wait ("the healthy record at the target did not arrive within 150s; read []"). The failing record's fate cannot be assessed either, because nothing is delivered at all.
Why: a client relying on per-record isolation sees one poisoned record take down the whole flow; good traffic is lost rather than delayed, and the error-propagation contract (retain the failure, deliver the rest) cannot be relied on.
Fix: the Pipes runtime's enrichment stage — per-record (not per-batch) failure accounting, retention of the failed record at the source, and continued hand-off of the successful records to the target stage.
Traces: lambda__pipes__sqs.events-pipes-enrichment-failure-retains-source-record-and-withholds-target.

Cause 2 of 2: an SQS event source mapping with MaximumConcurrency never scales beyond one concurrent invocation.
AWS: ScalingConfig.MaximumConcurrency is a cap, not a target: with a backlog of messages waiting, the SQS poller scales the number of simultaneous invocations UP toward the cap and holds there. With MaximumConcurrency=2 and 8 messages queued behind a slow handler, two invocations overlap in time.
Emulator: only 1 concurrent invocation is ever observed with 8 messages still waiting — the mapping polls strictly serially, so the cap is never approached.
Why: a client sizing throughput or testing concurrency-sensitive handler code (shared state, per-invocation limits, measured parallelism) observes behavior that cannot occur in AWS: the backlog drains at 1/N of the real rate, and a test asserting that the cap is reached fails while a test asserting serialization wrongly passes.
Fix: the Lambda event-source-mapping SQS poller's scaling logic — concurrent in-flight invocation batches up to ScalingConfig.MaximumConcurrency while a backlog exists.
Traces: lambda__pipes__sqs.sqs-esm-maximum-concurrency-limits-parallel-invocations.

## Evidence
Cause 1 — AssertionError: the healthy record at the target did not arrive within 150s; read [].

Cause 2 — AssertionError: observed only 1 concurrent invocations with a backlog of 8 messages waiting; a mapping that respects MaximumConcurrency=2 should still scale up to the cap, not merely stay under it.

## Steps to Reproduce
1. Create an SQS source, an SQS target, and a Lambda enrichment that raises for a marked record and echoes any other; CreatePipe (source -> enrichment -> target) DesiredState=RUNNING. Send a poisoned record and a healthy record; poll the target: the healthy record never arrives.
2. Create an SQS queue and a Lambda whose handler sleeps, then CreateEventSourceMapping with ScalingConfig.MaximumConcurrency=2; send ~10 messages and measure overlapping invocation windows — only one invocation is ever in flight.
