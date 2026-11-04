Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-route-table-association-swap
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-route-table-association-swap
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:ReplaceRouteTableAssociation
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# Implement ec2 ReplaceRouteTableAssociation: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReplaceRouteTableAssociation

## Description
`ReplaceRouteTableAssociation` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/replace_route_table_association.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `ReplaceRouteTableAssociation` call, so each one unblocks only when `ReplaceRouteTableAssociation` works: `ec2-route-table-association-swap#task#6`, `ec2-route-table-association-swap#task#8`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/replace_route_table_association.rs`. Replaying the corpus, the emulator refused the `ReplaceRouteTableAssociation` call: status: expected 200, got 501
$.ReplaceRouteTableAssociationResponse: missing (expected {'requestId': '021bed3c-41dd-468a-8b40-a6c77725bf34', 'newAssociationId': '{{new_association_id}}', 'associationState': {'state': 'associated'}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call ReplaceRouteTableAssociation and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
