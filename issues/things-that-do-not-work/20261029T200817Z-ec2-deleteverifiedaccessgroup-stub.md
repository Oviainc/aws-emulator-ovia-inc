Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-verified-access-delete-lifecycle
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-verified-access-delete-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteVerifiedAccessGroup
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# Implement ec2 DeleteVerifiedAccessGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVerifiedAccessGroup

## Description
`DeleteVerifiedAccessGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_verified_access_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteVerifiedAccessGroup` call, so each one unblocks only when `DeleteVerifiedAccessGroup` works: `ec2-verified-access-delete-lifecycle#task#10`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_verified_access_group.rs`. Replaying the corpus, the emulator refused the `DeleteVerifiedAccessGroup` call: status: expected 200, got 501
$.DeleteVerifiedAccessGroupResponse: missing (expected {'requestId': 'ada510d2-5ff6-4854-9e80-70244a7c73e3', 'verifiedAccessGroup': {'creationTime': '2026-10-29T19:59:32.248189Z', 'deletionTime': '2026-09-02T00:15:00.000Z', 'description': '', 'lastUpdatedTime': '2026-10-29T19:59:32.248189Z', 'owner': '000000000000', 'sseSpecification': {'customerManagedKeyEnabled': 'false'}, 'verifiedAccessGroupArn': 'arn:aws:ec2:us-west-2:000000000000:verified-access-group/vagr-0000000000000002f', 'verifiedAccessGroupId': 'vagr-0000000000000002f', 'verifiedAccessInstanceId': 'vai-0000000000000002d'}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteVerifiedAccessGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
