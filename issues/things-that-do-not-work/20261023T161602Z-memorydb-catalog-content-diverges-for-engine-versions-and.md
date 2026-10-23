Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-parameter-group-lifecycle
Issue-Step: task#6
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-parameter-group-lifecycle memorydb-cluster-and-snapshot-not-found-envelopes
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="acllog-max-len"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-defrag-cycle-max"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-defrag-cycle-min"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-defrag-ignore-bytes"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-defrag-max-scan-fields"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-defrag-threshold-lower"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-defrag-threshold-upper"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-expire-effort"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-expire-effort"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-expire-effort"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="activedefrag"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="activedefrag"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="activedefrag"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="client-output-buffer-limit-normal-hard-limit"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="client-output-buffer-limit-normal-soft-limit"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="client-output-buffer-limit-normal-soft-seconds"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="client-output-buffer-limit-pubsub-hard-limit"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="client-output-buffer-limit-pubsub-soft-limit"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="client-output-buffer-limit-pubsub-soft-seconds"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hash-max-listpack-entries"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hash-max-listpack-entries"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hash-max-listpack-entries"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hash-max-listpack-entries"].Value
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hash-max-listpack-value"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hash-max-listpack-value"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hash-max-listpack-value"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hll-sparse-max-bytes"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hll-sparse-max-bytes"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hll-sparse-max-bytes"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hll-sparse-max-bytes"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="latency-tracking"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="latency-tracking"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="latency-tracking"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="latency-tracking"].Value
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-eviction"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-eviction"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-eviction"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-expire"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-expire"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-expire"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-server-del"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-server-del"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-server-del"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-user-del"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-user-del"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lazyfree-lazy-user-del"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lfu-decay-time"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lfu-log-factor"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="list-compress-depth"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="maxmemory-policy"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="maxmemory-policy"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="maxmemory-policy"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="maxmemory-samples"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="notify-keyspace-events"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="notify-keyspace-events"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="notify-keyspace-events"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-enabled"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-query-timeout-ms"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="set-max-intset-entries"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="set-max-intset-entries"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="set-max-intset-entries"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-max-len"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-max-len"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-max-len"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="stream-node-max-bytes"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="stream-node-max-entries"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="tcp-keepalive"]
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="timeout"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="timeout"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="timeout"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="tracking-table-max-keys"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="tracking-table-max-keys"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="tracking-table-max-keys"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="zset-max-listpack-entries"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="zset-max-listpack-entries"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="zset-max-listpack-entries"].Source
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="zset-max-listpack-value"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="zset-max-listpack-value"].MinimumEngineVersion
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="zset-max-listpack-value"].Source
Issue-Key: memorydb|DescribeEngineVersions|2xx->2xx|$.EngineVersions[*]
Issue-Commit: 205916506edbd7602d3640584e73a4ccd0a8f366

# MemoryDB catalog content diverges for engine versions and parameters

> _Note: this issue was identified against the code base at commit `205916506edbd7602d3640584e73a4ccd0a8f366`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameters

## Description
Two causes, one shared fault: the emulator's built-in MemoryDB catalogs do not match the recorded service catalogs, so successful responses carry different content than AWS returns. Both causes must be closed before submitting.

Cause 1 of 2: DescribeParameters for a parameter group returns an incomplete and differently-annotated parameter set. AWS: recorded behavior only (Smithy models Parameter{Name, Value, Description, DataType, AllowedValues, MinimumEngineVersion} with no values; the catalog itself is only observable from the recording). The recording shows the full engine parameter set including the active-defrag family (acllog-max-len, active-defrag-cycle-max/min, active-defrag-ignore-bytes, active-defrag-max-scan-fields, active-defrag-threshold-lower/upper), the client-output-buffer-limit-* family, lfu-decay-time, lfu-log-factor, list-compress-depth, maxmemory-samples, search-enabled, search-query-timeout-ms, stream-node-max-*, tcp-keepalive, each with AllowedValues, DataType, Description, MinimumEngineVersion and a Source member. Emulator: many of those parameters are absent entirely, the present ones omit Source, report MinimumEngineVersion '6.2' where AWS reports '7.2.6', and carry slightly different Description text and Value/AllowedValues for several entries. Why: a client that reads a parameter's AllowedValues/MinimumEngineVersion before UpdateParameterGroup, or that looks up a parameter by name, gets a wrong answer or a missing key for parameters AWS always returns. Fix: the memorydb parameter catalog (the static parameter definition table) and the DescribeParameters serializer's Source member.
Traces: memorydb-parameter-group-lifecycle#task#6 (also task#11).

Cause 2 of 2: DescribeEngineVersions returns a different engine-version catalog entry at the tail of the list. AWS (recorded behavior only): the final entry is valkey EngineVersion '7.2'/'7.3' with EnginePatchVersion '7.3.0' and ParameterGroupFamily 'memorydb_valkey7'. Emulator: valkey EngineVersion '8.0', EnginePatchVersion '8.0.0', ParameterGroupFamily 'memorydb_valkey8'. Why: a client that picks the newest EngineVersion from this list and passes it to CreateCluster, or that derives a ParameterGroupFamily from it, selects a family the rest of the emulator's catalog does not carry (the recorded default parameter groups cover memorydb_valkey7, not memorydb_valkey8), so the chosen value does not round-trip.
Traces: memorydb-cluster-and-snapshot-not-found-envelopes#task#1.

## Evidence
Cause 1: HTTP 200 on both sides, but $.Parameters[Name="acllog-max-len"], [Name="active-defrag-cycle-max"], [Name="active-defrag-cycle-min"], [Name="active-defrag-ignore-bytes"], [Name="active-defrag-max-scan-fields"], [Name="active-defrag-threshold-lower"], [Name="active-defrag-threshold-upper"], the client-output-buffer-limit-* entries, lfu-decay-time, lfu-log-factor, list-compress-depth, maxmemory-samples, search-enabled, search-query-timeout-ms, stream-node-max-bytes/entries and tcp-keepalive are all 'missing'; $.Parameters[Name="activedefrag"].Source missing (expected 'system'), .MinimumEngineVersion expected '7.2.6' got '6.2', .Description expected 'Enabled active memory defragmentation' got 'Enabled active memory defragmentation.'; the same Source/MinimumEngineVersion/Description divergence repeats across ~25 further parameters (memorydb-parameter-group-lifecycle task#6). Cause 2: $.EngineVersions[4] expected {'Engine': 'valkey', 'EnginePatchVersion': '7.3.0', 'EngineVersion': '7.3', 'ParameterGroupFamily': 'memorydb_valkey7'}, got {'Engine': 'valkey', 'EngineVersion': '8.0', 'EnginePatchVersion': '8.0.0', 'ParameterGroupFamily': 'memorydb_valkey8'} (memorydb-cluster-and-snapshot-not-found-envelopes task#1).

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1: (1) DescribeParameters for a parameter group as recorded in memorydb-parameter-group-lifecycle#task#6 and diff the returned Parameters list against the recording — whole parameter families are missing and the Source/MinimumEngineVersion/Description annotations differ; (2) DescribeEngineVersions as recorded in memorydb-cluster-and-snapshot-not-found-envelopes#task#1 and diff EngineVersions[4].
