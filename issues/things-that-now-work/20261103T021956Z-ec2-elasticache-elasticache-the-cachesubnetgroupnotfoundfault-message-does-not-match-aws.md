Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected ec2__elasticache.vvd-cli-cache-subnet-group-matches-sdk
Issue-Key: ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected|run#9|AssertionError|assert got == want, f"a dangling subnet group raised {got!r}, expected {want!r}"
Issue-Key: ec2__elasticache.vvd-cli-cache-subnet-group-matches-sdk|_cli#7|AssertionError|assert proc.returncode == 0, f"aws {' '.join(args)} exited {proc.returncode}: {proc.stderr[-400:]}"
Issue-Commit: 6a6bdf627b2774134f5b74f649503c2b12968e19

# ElastiCache: the CacheSubnetGroupNotFoundFault message does not match AWS's template, and the CLI path fails where the SDK path succeeds (2 root causes)

> _Note: this issue was identified against the code base at commit `6a6bdf627b2774134f5b74f649503c2b12968e19`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReplicationGroup

## Description
Two net-new root causes replayed under the `ec2__elasticache` suite heading.

Cause 1 of 2: the unknown-cache-subnet-group refusal message diverges from AWS's template.
AWS: recorded behavior only. CreateReplicationGroup naming a cache subnet group that does not exist is refused CacheSubnetGroupNotFoundFault with `Cache subnet group '<name>' does not exist.` - fixed text `Cache subnet group '` / `' does not exist.`, with the group NAME quoted and taken from the request.
Emulator: returns the same fault class with `Cache subnet group <name> not found.` - the name is unquoted and the predicate is `not found.` instead of `does not exist.`
Why: the fault class alone does not say which group is missing when a request names several resources; clients and operators parse the quoted name out of the message. An unquoted name in a differently-worded sentence defeats both byte-exact assertions and name extraction.
Fix: the subnet-group resolution message template in the CreateReplicationGroup handler (and any other operation that resolves a cache subnet group by name).
Traces: ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected

Cause 2 of 2: `aws elasticache create-cache-subnet-group` exits non-zero for a request the SDK path accepts.
AWS: recorded behavior only. The CLI is a thin wrapper over the same API: `aws elasticache create-cache-subnet-group --cache-subnet-group-name <n> --cache-subnet-group-description <d> --subnet-ids <s1> <s2> --output json` must exit 0 and print the same CacheSubnetGroup document the SDK call returns for the identical input.
Emulator: the CLI invocation exits 254 (a botocore client-error exit) on the same subnet ids, so the CLI and SDK surfaces disagree.
Why: operators and scripts drive this API from the CLI; a non-zero exit aborts a deployment script at a step the SDK reports as fine, and the two surfaces disagreeing means neither can be trusted as the reference. Note that the already-open issue 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md records CreateCacheSubnetGroup refusing valid subnets with `Some input subnets in :[...] are invalid.`; this CLI check is NOT on that issue and is listed here so the CLI parity is re-replayed when that refusal is fixed - if the same refusal is the cause, closing it must make this check pass too.
Fix: the CreateCacheSubnetGroup handler's subnet resolution (and the error-to-exit-code path it drives).
Traces: ec2__elasticache.vvd-cli-cache-subnet-group-matches-sdk

## Evidence
Replayed failing checks under `### ec2__elasticache` (neither marked ALREADY FILED / OWNED by):

1. ec2__elasticache.vvd-replication-group-unknown-subnet-group-rejected -> AssertionError: a dangling subnet group raised ('CacheSubnetGroupNotFoundFault', 'Cache subnet group sim2real-vvd-nosg-a12d573534e8 not found.'), expected ('CacheSubnetGroupNotFoundFault', "Cache subnet group 'sim2real-vvd-nosg-a12d573534e8' does not exist.")
2. ec2__elasticache.vvd-cli-cache-subnet-group-matches-sdk -> AssertionError: aws elasticache create-cache-subnet-group --cache-subnet-group-name <name> --cache-subnet-group-description storage app subnet group --subnet-ids subnet-0000000000000002b subnet-0000000000000002d --output json exited 254

Context: the many OWNED-by failures in the same group all report `InvalidParameterValue ... Some input subnets in :[...] are invalid.` from CreateCacheSubnetGroup (issue 20261023T155010Z-elasticache-control-plane-is-unusable-end-to-end.md), which is the likely shared cause of the CLI exit above.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite; both checks appear under `### ec2__elasticache`, unmarked.
2. Standalone repros:
   - elasticache.create_replication_group(ReplicationGroupId=..., CacheSubnetGroupName='does-not-exist', ...): the message must read `Cache subnet group 'does-not-exist' does not exist.` with the name quoted.
   - Create a VPC with two subnets in different AZs, then run `aws elasticache create-cache-subnet-group --cache-subnet-group-name <n> --cache-subnet-group-description <d> --subnet-ids <s1> <s2> --output json`: exit code must be 0 and the printed document must match the boto3 create_cache_subnet_group response for the same input.
