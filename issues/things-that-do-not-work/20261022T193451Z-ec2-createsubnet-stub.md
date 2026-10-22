Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__ec2__elasticache.e2e-vvd-create-vpc-valkey-dynamodb-lifecycle dynamodb__ec2__elasticache.vvd-agent-role-deletes-denied-while-owner-deletes-succeed ec2__ecs__elasticache.tgwc-service-subnets-spanning-ecs-and-cache-vpcs-rejected ec2__elasticache.tgwc-memcached-cluster-resolves-to-isolated-route-table-and-group ec2__elasticache.tgwc-memcached-transit-encryption-on-old-engine-rejected ec2__elasticache.vvd-valkey-engine-version-not-offered-rejected ec2__elasticache.vvd-valkey-replication-group-lives-in-the-vpc-and-pins-its-network elasticache.cache-cluster-duplicate-id elasticache.cache-cluster-lifecycle elasticache.cache-cluster-reboot-rejected-during-pending-modification elasticache.cache-subnet-group-duplicate-name-rejected elasticache.cache-subnet-group-lifecycle elasticache.concurrent-delete-cache-cluster-race elasticache.create-cache-cluster-missing-required-parameter-rejected elasticache.create-cache-cluster-unsupported-engine-version-rejected elasticache.create-snapshot-unsupported-node-type-rejected elasticache.delete-replication-group-snapshotting-state-blocked elasticache.delete-replication-group-wrong-state-rejected
Issue-Key: ec2|CreateSubnet|NotImplemented
Issue-Commit: 17f16131651e3a76635c083c093a8581359ec045

# Implement ec2 CreateSubnet: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `17f16131651e3a76635c083c093a8581359ec045`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSubnet

## Description
`CreateSubnet` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_subnet.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

18 recorded unit(s) fail on their `CreateSubnet` call, so each one unblocks only when `CreateSubnet` works: `dynamodb__ec2__elasticache.e2e-vvd-create-vpc-valkey-dynamodb-lifecycle`, `dynamodb__ec2__elasticache.vvd-agent-role-deletes-denied-while-owner-deletes-succeed`, `ec2__ecs__elasticache.tgwc-service-subnets-spanning-ecs-and-cache-vpcs-rejected`, `ec2__elasticache.tgwc-memcached-cluster-resolves-to-isolated-route-table-and-group`, `ec2__elasticache.tgwc-memcached-transit-encryption-on-old-engine-rejected`, `ec2__elasticache.vvd-valkey-engine-version-not-offered-rejected`, `ec2__elasticache.vvd-valkey-replication-group-lives-in-the-vpc-and-pins-its-network`, `elasticache.cache-cluster-duplicate-id`, `elasticache.cache-cluster-lifecycle`, `elasticache.cache-cluster-reboot-rejected-during-pending-modification`, `elasticache.cache-subnet-group-duplicate-name-rejected`, `elasticache.cache-subnet-group-lifecycle`, `elasticache.concurrent-delete-cache-cluster-race`, `elasticache.create-cache-cluster-missing-required-parameter-rejected`, `elasticache.create-cache-cluster-unsupported-engine-version-rejected`, `elasticache.create-snapshot-unsupported-node-type-rejected`, `elasticache.delete-replication-group-snapshotting-state-blocked`, `elasticache.delete-replication-group-wrong-state-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-ec2-amazonec2/src/ops/create_subnet.rs`. Replaying the corpus, the emulator refused the `CreateSubnet` call: File "contextlib.py", line 162, in __exit__
    self.gen.throw(value)
    ~~~~~~~~~~~~~~^^^^^^^
  File "check_dynamodb__ec2__elasticache_e2e_vvd_create_vpc_valkey_dynamodb_lifecycle.py", line 496, in phase
    raise _PhaseError(
        f"[phase {i}/{len(self.phases)} {name}] {type(exc).__name__}: {exc}") from exc
_simparity_dyn_check_dynamodb__ec2__elasticache_e2e_vvd_create_vpc_valkey_dynamodb_lifecycle_06432f4c._PhaseError: [phase 2/5 solve] ClientError: An error occurred (NotImplemented) when calling the CreateSubnet operation: operation CreateSubnet is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the ec2 emulator
Call CreateSubnet and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
