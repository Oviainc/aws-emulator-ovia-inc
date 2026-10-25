Issue-Severity: LOW
Issue-Gap-Class: wrong-behavior
Issue-Commit: 4ffec8ed9c134267b14eefb2b325da9a0803e3ce

# Keyspace state is not partitioned by Region: keyspaces created in one Region are listed and resolved in every other

_Filed: 2026-10-24T00:49:58Z_
_Operation: ListKeyspaces | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `4ffec8ed9c134267b14eefb2b325da9a0803e3ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service store holds a single flat `keyspaces: BTreeMap<String, Keyspace>` with no Region dimension, while every ARN is minted from the requesting call's SigV4 credential-scope Region.

Expected (AWS): keyspaces and tables are Regional resources — "In an Amazon Web Services account, keyspace names must be unique within each Region" (CreateKeyspace). ListKeyspaces signed for Region B enumerates only Region B's keyspaces (plus B's service-managed system keyspaces, whose ARNs carry B's Region), GetKeyspace for a keyspace created only in Region A returns ResourceNotFoundException in Region B, and CreateKeyspace may reuse a name already taken in another Region (HTTP 200).

Actual (emulator): a keyspace created by a request signed for us-west-2 is returned by ListKeyspaces/GetKeyspace for a request signed for eu-central-1, carrying the us-west-2 ARN it was minted with; CreateKeyspace of that same name from eu-central-1 is refused with ConflictException 409 where AWS would create it; and the service-managed catalog is materialised once, on the FIRST request, so the `system*` keyspace/table ARNs are frozen to that first caller's Region and are reported with the wrong Region to every later caller from another Region. (The tag lookups compare the full ARN, so they already behave as if resources were Regional — the listing/reading paths do not, which is the inconsistency.)

## Evidence

Spec: CreateKeyspace documentation "In an Amazon Web Services account, keyspace names must be unique within each Region"; `ARN` / `keyspace_arn` are Region-qualified (`arn:aws:cassandra:<region>:<account>:/keyspace/<name>/`), and `request_region` derives the Region per request from the SigV4 credential scope.
Handler/state: services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs lines 521-538 (`keyspaces` keyed by name only, no Region key; `service_managed_catalog_materialised` a single global flag) and lines 942-998 (`ensure_service_managed_catalog(region)` returns early forever after the first call, so later Regions reuse the first Region's ARNs); src/ops/list_keyspaces.rs lines 217-246 and src/ops/get_keyspace.rs lines 176-202 read that unpartitioned map.
Tests: every test in tests/all signs for the single Region `us-west-2` (tests/all/keyspace_table_lifecycle.rs line 38), so no test exercises or asserts cross-Region isolation.

## Steps to Reproduce

1. Boot the emulator.
2. With credentials/scope for us-west-2: CreateKeyspace(keyspaceName='regional_ks').
3. With credentials/scope for eu-central-1: ListKeyspaces() — 'regional_ks' is listed, with an ARN whose Region is us-west-2; AWS lists nothing of the sort in eu-central-1.
4. With credentials/scope for eu-central-1: GetKeyspace(keyspaceName='regional_ks') — returns 200 with the us-west-2 ARN; AWS returns 404 ResourceNotFoundException.
5. With credentials/scope for eu-central-1: CreateKeyspace(keyspaceName='regional_ks') — returns 409 ConflictException; AWS returns 200 because the name is free in that Region.
6. Also observe that the system keyspace ARNs returned to the eu-central-1 caller carry us-west-2 (the Region of whichever request happened to materialise the catalog first).
