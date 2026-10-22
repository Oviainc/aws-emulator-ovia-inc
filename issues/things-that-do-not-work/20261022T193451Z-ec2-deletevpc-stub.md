Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber
Issue-Key: ec2|DeleteVpc|NotImplemented
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement ec2 DeleteVpc: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVpc

## Description
`DeleteVpc` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_vpc.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteVpc` call, so each one unblocks only when `DeleteVpc` works: `elasticache__sns.cache-cluster-event-notification-reaches-the-sns-subscriber`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_vpc.rs`. Replaying the corpus, the emulator refused the `DeleteVpc` call: File "check_elasticache__sns_cache_cluster_event_notification_reaches_the_sns_subscriber.py", line 609, in run
    ctx.run_cleanups()
    ~~~~~~~~~~~~~~~~^^
  File "context.py", line 489, in run_cleanups
    raise RuntimeError(f"{len(failures)} teardown(s) failed: {named}") from failures[0]
RuntimeError: 1 teardown(s) failed: ClientError('An error occurred (NotImplemented) when calling the DeleteVpc operation: operation DeleteVpc is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteVpc and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
