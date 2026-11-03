Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-pip-traj-instance-private-ip-reverse-lookup
Issue-Step: task#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-pip-traj-instance-private-ip-reverse-lookup ec2-pip-traj-secondary-ip-request-mistakes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:UnassignPrivateIpAddresses
Issue-Commit: ce4f28f15b4841beefbad6c596b9d2158570753e

# Implement ec2 UnassignPrivateIpAddresses: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `ce4f28f15b4841beefbad6c596b9d2158570753e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UnassignPrivateIpAddresses

## Description
`UnassignPrivateIpAddresses` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/unassign_private_ip_addresses.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `UnassignPrivateIpAddresses` call, so each one unblocks only when `UnassignPrivateIpAddresses` works: `ec2-pip-traj-instance-private-ip-reverse-lookup#task#12`, `ec2-pip-traj-secondary-ip-request-mistakes#task#7`, `ec2-pip-traj-secondary-ip-request-mistakes#task#8`, `ec2-pip-traj-secondary-ip-request-mistakes#task#11`, `ec2-pip-traj-secondary-ip-request-mistakes#task#12`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/unassign_private_ip_addresses.rs`. Replaying the corpus, the emulator refused the `UnassignPrivateIpAddresses` call: status: expected 200, got 501
$.UnassignPrivateIpAddressesResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'return': 'true'}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call UnassignPrivateIpAddresses and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
