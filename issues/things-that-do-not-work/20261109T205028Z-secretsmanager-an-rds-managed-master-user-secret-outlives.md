Issue-Source: trace-sweep-divergence
Issue-Trace: rds-aurora-serverless-data-api-managed-secret-sql-round-trip
Issue-Step: teardown#17
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-aurora-serverless-data-api-managed-secret-sql-round-trip
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: secretsmanager|DescribeSecret|4xx->2xx|status
Issue-Related: 20261108T215449Z-secretsmanager-2-net-new-causes-a-stack-created.md
Issue-Commit: fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7

# secretsmanager: an RDS-managed master user secret outlives its deleted DB cluster

> _Note: this issue was identified against the code base at commit `fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecret

## Description
One cause. The replay reported no owning issue and no blocking stub for it.

Cause 1 of 1: after the owning DB cluster is deleted, DescribeSecret on its RDS-managed master user secret still answers 200 instead of refusing.
AWS: `DescribeSecret` takes `SecretId` (required, length 1..2048) and models `ResourceNotFoundException` (client error, HTTP 400) for a secret Secrets Manager cannot resolve. An RDS-managed master user secret is service-owned (`OwningService` = `rds`, name of the form `rds!cluster-…`) — its lifecycle is RDS's, not the caller's: when the owning DB cluster is deleted, RDS removes the managed secret, after which `DescribeSecret` on that secret's ARN or name is refused with `ResourceNotFoundException` at HTTP 400. The fixed message text for that error class is `Secrets Manager can't find the specified secret.`; the only variable part of the exchange is the per-run request id.
Emulator: long after the cluster delete completes, `DescribeSecret` keeps answering HTTP 200 with the full secret record (ARN, `rds!cluster-…` Name, `OwningService: rds`, the `aws:rds:primaryDBClusterArn` tag pointing at the now-deleted cluster, and an AWSCURRENT version). The replay polled for `ResourceNotFoundException` 19 times and hit its 180 s cap without the error ever appearing, so the secret is never removed — the delete of the cluster does not cascade to the secret it owns.
Why: a client cannot tell whether an RDS-managed secret is still live. Teardown verification that waits for the managed secret to disappear hangs until its timeout and then reports failure; a subsequent create that reuses the same cluster identifier collides with a stale `rds!cluster-…` secret that AWS would have reaped; and credentials for a destroyed cluster remain readable, which is exactly the opposite of the deletion guarantee callers rely on.
Fix: the RDS cluster-delete path must cascade to the managed master user secret it owns (remove the secret when the cluster deletion completes), and the Secrets Manager DescribeSecret handler must refuse a secret that is no longer present with `ResourceNotFoundException` at HTTP 400 and AWS's fixed message, rather than projecting a retained record. The cascade belongs in the DeleteDBCluster completion handling in rds state; the refusal belongs in the secretsmanager DescribeSecret handler's lookup gate. Related open issues 20261107T030257Z-rds-secretsmanager-2-net-new-causes-the-rds.md, 20261108T102113Z-rds-secretsmanager-the-rds-managed-master-user-secret.md and 20261108T215646Z-rds-secretsmanager-a-rolled-back-cloudformation-create-leaves.md concern other aspects of the managed secret (its name, its rotation reporting, a rolled-back CloudFormation delete) and carry no replay key for this step.
Traces: rds-aurora-serverless-data-api-managed-secret-sql-round-trip#teardown#17.

## Evidence
Replay of trace set simparity-wire-aws-v1 (98 fixtures, 993 steps, score 49.8%).

rds-aurora-serverless-data-api-managed-secret-sql-round-trip#teardown#17, secretsmanager DescribeSecret:
  status: expected 400, got 200
  $: poll: error code None not in ['ResourceNotFoundException'] after 19 attempt(s)
  $: poll capped at 180s — the emulator kept returning the live secret record, e.g. Name 'rds!cluster-…', OwningService 'rds', RotationEnabled false, Tags [{'Key': 'aws:rds:primaryDBClusterArn', 'Value': '<arn of the already-deleted cluster>'}], VersionIdsToStages {<version>: ['AWSCURRENT']}.

Expected: HTTP 400 with ResourceNotFoundException once the owning DB cluster's deletion completes. The ARN, secret-name suffix, version id, timestamps and request id in the observed record are per-run values; the HTTP status and the error code are the requirement.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Replay fixture rds-aurora-serverless-data-api-managed-secret-sql-round-trip up to its teardown: create an Aurora cluster with ManageMasterUserPassword, note DBCluster.MasterUserSecret.SecretArn, then DeleteDBCluster and wait for the cluster to be gone.
3. Step teardown#17: poll secretsmanager DescribeSecret with that SecretArn. The emulator answers HTTP 200 with the full secret record on every attempt for the whole 180 s budget; AWS answers HTTP 400 ResourceNotFoundException once RDS has reaped the managed secret.
