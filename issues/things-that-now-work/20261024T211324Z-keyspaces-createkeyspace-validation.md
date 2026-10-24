Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e0e9a90161aff7ec754055c046e25b8f716c6f2d

# replicationSpecification.regionList accepts strings that are not AWS Regions

_Filed: 2026-10-24T21:13:24Z_
_Operation: CreateKeyspace | Class: validation_

> _Note: this issue was identified against the code base at commit `e0e9a90161aff7ec754055c046e25b8f716c6f2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::resolve_replication` checks only the list's size cap and that it contains the request's own Region; every entry is otherwise stored verbatim. `region` is modelled as a plain string with `length 2..25` and no pattern, so the boundary validator cannot refuse a non-Region either.

Expected: a `regionList` entry that is not an AWS Region (e.g. `"not-a-region"`, `"zz-fake-9"`) is refused with 400 `ValidationException`, so a MULTI_REGION keyspace can only ever be replicated in real Regions.

Actual: `CreateKeyspace {"keyspaceName":"ks","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-east-1","not-a-region"]}}` answers 200, and `GetKeyspace` / `ListKeyspaces` then report `replicationRegions` containing the bogus Region plus a `replicationGroupStatuses` entry for it; `CreateTable`/`UpdateTable` will also accept `replicaSpecifications` naming it. The same gap applies to `UpdateKeyspace`, which resolves its `regionList` through the same helper.

## Evidence

Spec: `ReplicationSpecification$regionList` — "the Amazon Web Services Regions where the keyspace is replicated in" (target `region`, length 2..25, NO pattern/enum — smithy_query shape=ReplicationSpecification), and `ReplicationGroupStatus$region` — "The name of the Region that was added to the keyspace"; a value that names no Region cannot be replicated to, which is the "invalid or malformed request" `ValidationException` covers. Handler: `src/state.rs` `resolve_replication` lines ~1972-2015 — only MAX_REPLICATION_REGIONS and the current-Region membership are checked; `src/ops/create_keyspace.rs` lines ~176-182 and `src/ops/update_keyspace.rs` lines ~212-214 are its only callers. Tests: every regionList row in `tests/all/*.rs` uses real Region names (`us-east-1`, `eu-west-1`, `ap-south-1`); no test asserts a refusal for a non-Region. Precedent for enforcing this in-repo: `is_aws_region` in datazone/resiliencehub, `is_known_region` in license-manager-linux-subscriptions.

## Steps to Reproduce

1. CreateKeyspace {"keyspaceName":"ks_bogus","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-east-1","not-a-region"]}} — emulator answers 200; AWS answers 400 ValidationException
2. GetKeyspace {"keyspaceName":"ks_bogus"} — replicationRegions contains "not-a-region" and replicationGroupStatuses carries an entry for it
3. Same with UpdateKeyspace on an existing keyspace: {"keyspaceName":"ks1","replicationSpecification":{"replicationStrategy":"MULTI_REGION","regionList":["us-east-1","zz-fake-9"]}} is accepted
