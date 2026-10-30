Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Step: task#30
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-ipam-consolidated-delete-and-deprovision-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteIpamPrefixListResolverTarget
Issue-Commit: 57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf

# Implement ec2 DeleteIpamPrefixListResolverTarget: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `57b303ffac73a2f2fe8b11084ee3a9c28d52eaaf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteIpamPrefixListResolverTarget

## Description
`DeleteIpamPrefixListResolverTarget` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_prefix_list_resolver_target.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DeleteIpamPrefixListResolverTarget` call, so each one unblocks only when `DeleteIpamPrefixListResolverTarget` works: `ec2-ipam-consolidated-delete-and-deprovision-lifecycle#task#30`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_ipam_prefix_list_resolver_target.rs`. Replaying the corpus, the emulator refused the `DeleteIpamPrefixListResolverTarget` call: status: expected 200, got 501
$.DeleteIpamPrefixListResolverTargetResponse: missing (expected {'requestId': '<shaped-not-asserted>', 'ipamPrefixListResolverTarget': {'desiredVersion': '1', 'ipamPrefixListResolverId': 'ipam-plr-0000000000000002c', 'ipamPrefixListResolverTargetArn': 'arn:aws:ec2::000000000000:ipam-prefix-list-resolver-target/ipam-plrt-0000000000000002d', 'ipamPrefixListResolverTargetId': 'ipam-plrt-0000000000000002d', 'ownerId': '000000000000', 'prefixListId': 'pl-00000000000000024', 'prefixListRegion': 'us-west-2', 'state': 'delete-in-progress', 'trackLatestVersion': 'true'}}, got None)

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteIpamPrefixListResolverTarget and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
