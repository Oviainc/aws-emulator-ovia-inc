Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-conflict-route-table
Issue-Step: teardown#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-conflict-route-table ec2-gateway-vpc-endpoint-lifecycle ec2-lsrno-traj-cross-region-s3-endpoint-refused ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3 ec2-lsrno-traj-route-lookup-implicit-main-association ec2-nfwr-traj-find-tables-routing-to-an-endpoint ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes ec2-pst-traj-detached-igw-leaves-blackhole-default-route ec2-pst-traj-implicit-main-association-is-invisible-to-subnet-filter ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter ec2-pst-traj-route-filters-match-per-table-not-per-route ec2-pst-traj-wrong-kind-and-short-network-ids-are-not-found ec2-route-table-association-swap ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap ec2-subnet-route-table-lifecycle ec2-t12-post-delete-not-found-probes
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|stub:DeleteRouteTable
Issue-Commit: b64c3d45927e2833db57dde0eaeae2a33a5b497f

# Implement ec2 DeleteRouteTable: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b64c3d45927e2833db57dde0eaeae2a33a5b497f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteRouteTable

## Description
`DeleteRouteTable` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_route_table.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

19 recorded unit(s) fail on their `DeleteRouteTable` call, so each one unblocks only when `DeleteRouteTable` works: `ec2-conflict-route-table#teardown#9`, `ec2-gateway-vpc-endpoint-lifecycle#teardown#13`, `ec2-lsrno-traj-cross-region-s3-endpoint-refused#teardown#11`, `ec2-lsrno-traj-isolated-subnet-routes-only-same-region-s3#teardown#13`, `ec2-lsrno-traj-route-lookup-implicit-main-association#teardown#11`, `ec2-nfwr-traj-find-tables-routing-to-an-endpoint#teardown#9`, `ec2-nfwr-traj-find-tables-routing-to-an-endpoint#teardown#10`, `ec2-nfwr-traj-route-target-to-firewall-endpoint-mistakes#teardown#13`, `ec2-pst-traj-detached-igw-leaves-blackhole-default-route#teardown#20`, `ec2-pst-traj-implicit-main-association-is-invisible-to-subnet-filter#teardown#15`, `ec2-pst-traj-instance-joined-to-public-subnet-by-subnet-filter#teardown#18`, `ec2-pst-traj-route-filters-match-per-table-not-per-route#teardown#15`, `ec2-pst-traj-route-filters-match-per-table-not-per-route#teardown#16`, `ec2-pst-traj-wrong-kind-and-short-network-ids-are-not-found#teardown#13`, `ec2-route-table-association-swap#teardown#11`, `ec2-route-table-association-swap#teardown#12`, `ec2-sshp-traj-keyless-public-instance-eic-push-and-eip-swap#teardown#33`, `ec2-subnet-route-table-lifecycle#teardown#18`, `ec2-t12-post-delete-not-found-probes#task#11`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_route_table.rs`. Replaying the corpus, the emulator refused the `DeleteRouteTable` call: status: expected 200, got 501

## Steps to Reproduce
Boot the ec2 emulator
Call DeleteRouteTable and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
