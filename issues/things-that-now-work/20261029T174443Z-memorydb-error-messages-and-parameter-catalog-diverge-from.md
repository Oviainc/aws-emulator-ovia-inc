Issue-Source: trace-sweep-divergence
Issue-Trace: memorydb-catalog-describe-parameter-groups
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: memorydb-catalog-describe-parameter-groups memorydb-cluster-and-multiregion-request-validation memorydb-events-updates-and-offerings-validation memorydb-parameter-group-lifecycle memorydb-parameter-update-and-reset-rules
Issue-Shape-Id: com.amazonaws.memorydb#AmazonMemoryDB
Issue-Key: memorydb|DescribeParameterGroups|2xx->2xx|$.ParameterGroups[Name="default.memorydb-redis7.search"].Description
Issue-Key: memorydb|DescribeParameterGroups|2xx->2xx|$.ParameterGroups[Name="default.memorydb-valkey7.search"].Description
Issue-Key: memorydb|UpdateCluster|4xx->4xx|$.message
Issue-Key: memorydb|CreateCluster|4xx->4xx|$.message
Issue-Key: memorydb|DescribeEvents|4xx->4xx|$.message
Issue-Key: memorydb|DescribeServiceUpdates|4xx->4xx|$.message
Issue-Key: memorydb|DescribeParameterGroups|4xx->4xx|$.message
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="acllog-max-len"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="active-expire-effort"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="hll-sparse-max-bytes"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="latency-tracking"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lfu-decay-time"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lfu-log-factor"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="lfu-log-factor"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="list-compress-depth"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="maxmemory-samples"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-enabled"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-enabled"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-enabled"].Value
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-query-timeout-ms"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="search-query-timeout-ms"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].AllowedValues
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-log-slower-than"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="slowlog-max-len"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="tcp-keepalive"].Description
Issue-Key: memorydb|DescribeParameters|2xx->2xx|$.Parameters[Name="tracking-table-max-keys"].Description
Issue-Key: memorydb|DeleteParameterGroup|4xx->4xx|$.message
Issue-Commit: c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c

# MemoryDB error messages and parameter catalog diverge from AWS

> _Note: this issue was identified against the code base at commit `c5b0e480c5d08ce7c34e019fd56b1aa2c0c8407c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeParameterGroups

## Description
Replay of the memorydb wire corpus produced 8 divergence clusters (79 mismatched steps). They share one fault family: the emulator serves its own wording and its own parameter catalog instead of MemoryDB's. Every cluster returns the AWS status (400->400 or 200->200); what diverges is the error message text, the modelled validation order, or the catalog values inside a 200 body. 8 CAUSES, all of which must be closed before this issue is resolved.

Cause 1 of 8: DescribeParameterGroups - default parameter-group Description for search-capable families. Fixture memorydb-catalog-describe-parameter-groups step task#1.
AWS: recorded behavior only (Smithy: ParameterGroup.Description is documented merely as "A description of the parameter group"). For a service-supplied default group of a search-capable family, AWS words the description "Customized default parameter group for <family> with search-enabled yes", where <family> is the engine family token with hyphens replaced by underscores and the ".search" qualifier dropped (e.g. family memorydb_redis7, memorydb_valkey7). The sentence is fixed text; only the family token comes from the group being described.
Emulator: returns the generic default-group template, "Default parameter group for memorydb_redis7.search" / "Default parameter group for memorydb_valkey7.search" - wrong sentence form, and it leaves the ".search" qualifier inside the family token.
Why: a client that reads the catalog to tell a search-enabled default group from a plain one, or that diffs parameter-group descriptions for drift detection, sees text AWS never emits and reports spurious drift.
Fix: the memorydb default parameter-group catalog seeding in the service's state/catalog layer (the built-in default groups), not the DescribeParameterGroups serializer.
Traces: memorydb-catalog-describe-parameter-groups#task#1.

Cause 2 of 8: UpdateCluster - ClusterNotFound message interpolates the cluster name. Fixture memorydb-cluster-and-multiregion-request-validation step task#2.
AWS: Smithy: UpdateCluster declares ClusterNotFoundFault (awsQueryError code ClusterNotFound); the recorded response is HTTP 400 with $.message exactly "Cluster not found." - the whole message is fixed text, with NO variable part: AWS does not name the missing cluster.
Emulator: HTTP 400 with "Cluster <ClusterName> not found.", interpolating the requested name (the recorded run shows a per-run probe name, which is a value from the request, not part of any template).
Why: clients and tests that match MemoryDB's documented not-found message fail to recognise the error, and the emulator leaks a request value into a message AWS keeps constant.
Fix: the UpdateCluster handler's not-found error construction.
Traces: memorydb-cluster-and-multiregion-request-validation#task#2.

Cause 3 of 8: CreateCluster - NumShards lower-bound message. Fixture memorydb-cluster-and-multiregion-request-validation step task#10 (cluster also covers task#11, task#12).
AWS: Smithy: CreateCluster.NumShards is IntegerOptional and the operation declares InvalidParameterValueException (code InvalidParameterValue, 400). Recorded response: HTTP 400 with $.message exactly "Number of shards cannot be less than 1." - entirely fixed text.
Emulator: HTTP 400 with "NumShards must be at least 1." - emulator-invented wording naming the wire member rather than AWS's sentence.
Why: a client branching on the validation message (or an assertion in a deployment test) never matches, so a shard-count misconfiguration is reported differently from production.
Fix: CreateCluster input validation in the operation's validation layer.
Traces: memorydb-cluster-and-multiregion-request-validation#task#10.

Cause 4 of 8: DescribeEvents - invalid SourceType yields the generic constraint message. Fixture memorydb-events-updates-and-offerings-validation step task#2.
AWS: Smithy: DescribeEvents.SourceType targets enum SourceType with values [node, parameter-group, subnet-group, cluster, user, acl]; the operation declares InvalidParameterValueException (400). Recorded response: HTTP 400 with $.message exactly "Invalid event source type. Valid types are node,cluster,parameter-group,subnet-group,user,acl" - all fixed text, including the valid-type list in that order, comma-separated with no spaces. The rejected value is NOT echoed.
Emulator: HTTP 400 with the framework's generic constraint template, "Value '<value>' at 'sourceType' failed to satisfy constraint: Member must satisfy enum value set: [node, parameter-group, subnet-group, cluster, user, acl]" - wrong sentence, wrong list order/format, and it echoes the request value AWS omits.
Why: a client that validates its own enum handling against MemoryDB's message, or surfaces it to a user, gets a message shaped like generic smithy validation rather than MemoryDB's documented one.
Fix: DescribeEvents input validation / enum rejection mapping for this operation, replacing the generic enum-constraint message with MemoryDB's.
Traces: memorydb-events-updates-and-offerings-validation#task#2.

Cause 5 of 8: DescribeServiceUpdates - invalid Status member yields the generic constraint message. Fixture memorydb-events-updates-and-offerings-validation step task#10 (cluster also covers task#11).
AWS: Smithy: DescribeServiceUpdates.Status is ServiceUpdateStatusList (0..4) of enum ServiceUpdateStatus [available, in-progress, complete, scheduled]; the operation declares InvalidParameterValueException (400). Recorded response: HTTP 400 with $.message "The input service update status <value> is not valid." - the fixed text is "The input service update status " ... " is not valid." and <value> is the rejected status string taken verbatim from the request (the recording's "bogus" is just that run's input).
Emulator: HTTP 400 with "Value '<value>' at 'status.1.member' failed to satisfy constraint: Member must satisfy enum value set: [available, in-progress, complete, scheduled]" - the generic framework template plus an index-qualified member path AWS never emits.
Why: clients cannot parse out the offending status the way MemoryDB presents it, and message assertions against production fail.
Fix: DescribeServiceUpdates input validation / enum rejection mapping.
Traces: memorydb-events-updates-and-offerings-validation#task#10.

Cause 6 of 8: ParameterGroupNotFound message interpolates the group name. Fixture memorydb-parameter-group-lifecycle step task#5 (cluster also covers memorydb-parameter-group-lifecycle#teardown#15 and memorydb-parameter-update-and-reset-rules#task#17).
AWS: Smithy: DescribeParameterGroups (and the other parameter-group operations) declare ParameterGroupNotFoundFault (code ParameterGroupNotFound). Recorded response: HTTP 400 with $.message exactly "Parameter Group not found." - fixed text, two words separated by a space, with NO variable part and no group name.
Emulator: HTTP 400 with "ParameterGroup not found: <ParameterGroupName>" - unspaced subject, colon form, and it appends the requested group name (a request value) that AWS never includes.
Why: the message is a requirement of the error class; clients matching MemoryDB's not-found text fail, and the divergence is shared across every parameter-group lookup path, so it misreports the whole lifecycle.
Fix: the shared parameter-group lookup error in the memorydb state layer that all parameter-group operations raise.
Traces: memorydb-parameter-group-lifecycle#task#5.

Cause 7 of 8: DescribeParameters - the engine parameter catalog's Description, AllowedValues and Value diverge wholesale. Fixture memorydb-parameter-group-lifecycle step task#6 (cluster also covers memorydb-parameter-group-lifecycle#task#11).
AWS: recorded behavior only for the per-parameter text (Smithy models Parameter.Description as "A description of the parameter", Parameter.AllowedValues as "The valid range of values for the parameter"); the catalog content itself is service-supplied and identical for every account, so it is a requirement, not a per-run value. Status 200. The recording requires, among others: acllog-max-len Description "The maximum length of the ACL Log"; active-expire-effort "The amount of effort that redis uses to expire items in the active expiration job"; hll-sparse-max-bytes AllowedValues "1-16000"; latency-tracking "Track the per command latencies and enable exporting the percentile distribution via the INFO latencystats command and cumulative latency distributions (histograms) via the LATENCY command."; lfu-decay-time "The amount of time in minutes to decrement the key counter for LFU eviction policyd"; lfu-log-factor AllowedValues "1-" and Description "The log factor for incrementing key counter for LFU eviction policy"; list-compress-depth "Number of quicklist ziplist nodes from each side of the list to exclude from compression. The head and tail of the list are always uncompressed for fast push/pop operations"; maxmemory-samples AllowedValues "1-"; search-enabled AllowedValues "yes,no" (plus its Value and Description); and further divergences on search-query-timeout-ms, slowlog-log-slower-than, slowlog-max-len, tcp-keepalive, tracking-table-max-keys.
Emulator: 200 with its own paraphrased descriptions and different ranges - e.g. "The maximum number of entries in the ACL log", "The amount of effort Redis uses to expire items in the active expiration job", hll-sparse-max-bytes AllowedValues "0-", lfu-log-factor AllowedValues "0-", maxmemory-samples AllowedValues "1-64", and search-enabled AllowedValues "true,false" with a boolean-style Value.
Why: AllowedValues is what clients and consoles validate against before calling UpdateParameterGroup, so search-enabled advertised as "true,false" makes the emulator reject the "yes"/"no" values AWS actually accepts (and accept values AWS rejects); the widened/narrowed numeric ranges mis-validate lfu-log-factor, hll-sparse-max-bytes and maxmemory-samples the same way, and the paraphrased descriptions break catalog diffing.
Fix: the memorydb engine parameter catalog table that DescribeParameters reads (the catalog data, not the serializer).
Traces: memorydb-parameter-group-lifecycle#task#6.

Cause 8 of 8: DeleteParameterGroup - identifier validation is skipped, so the wrong error class is returned. Fixture memorydb-parameter-update-and-reset-rules step task#18.
AWS: Smithy: DeleteParameterGroup.ParameterGroupName is required and the operation declares InvalidParameterValueException (code InvalidParameterValue, 400) alongside InvalidParameterGroupStateFault. AWS validates the identifier's syntax BEFORE resolving the group or applying the default-group protection, and returns HTTP 400 with $.message "The parameter ParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens." Everything is fixed text except the member name, "ParameterGroupName", which names the request member at fault.
Emulator: HTTP 400 with "Cannot delete a default parameter group." - it never performs the identifier check and falls through to the default-group guard, so the response describes a different fault class entirely.
Why: a client sending a syntactically invalid name is told the group is a protected default, so it retries with a different group instead of fixing the malformed identifier; validation-ordering tests against AWS also invert.
Fix: DeleteParameterGroup input validation - add the identifier-syntax check on ParameterGroupName and run it ahead of the default-group protection check.
Traces: memorydb-parameter-update-and-reset-rules#task#18.

## Evidence
replay_wire_corpus (trace set simparity-wire-aws-v1): 233 steps, 154 passed, 79 mismatched, score 66.1%, emulator_up=true, bootstrap_signal=false. 8 divergence clusters, all service=memorydb, none carrying stub_path or owned_by.

1) DescribeParameterGroups 200->200, $.ParameterGroups[Name="default.memorydb-redis7.search"].Description expected 'Customized default parameter group for memorydb_redis7 with search-enabled yes', got 'Default parameter group for memorydb_redis7.search' (same for default.memorydb-valkey7.search / memorydb_valkey7).
2) UpdateCluster 400->400, $.message expected 'Cluster not found.', got 'Cluster <name> not found.'
3) CreateCluster 400->400, $.message expected 'Number of shards cannot be less than 1.', got 'NumShards must be at least 1.' (3 steps).
4) DescribeEvents 400->400, $.message expected 'Invalid event source type. Valid types are node,cluster,parameter-group,subnet-group,user,acl', got "Value '<v>' at 'sourceType' failed to satisfy constraint: Member must satisfy enum value set: [node, parameter-group, subnet-group, cluster, user, acl]".
5) DescribeServiceUpdates 400->400, $.message expected 'The input service update status <v> is not valid.', got "Value '<v>' at 'status.1.member' failed to satisfy constraint: Member must satisfy enum value set: [available, in-progress, complete, scheduled]" (2 steps).
6) DescribeParameterGroups 400->400, $.message expected 'Parameter Group not found.', got 'ParameterGroup not found: <name>' (3 steps across 2 fixtures).
7) DescribeParameters 200->200, 19 diverging JSON paths: Description mismatches on acllog-max-len, active-expire-effort, latency-tracking, lfu-decay-time, lfu-log-factor, list-compress-depth, search-enabled, search-query-timeout-ms, slowlog-log-slower-than, slowlog-max-len, tcp-keepalive, tracking-table-max-keys; AllowedValues mismatches hll-sparse-max-bytes '1-16000'->'0-', lfu-log-factor '1-'->'0-', maxmemory-samples '1-'->'1-64', search-enabled 'yes,no'->'true,false', search-query-timeout-ms, slowlog-log-slower-than; plus search-enabled Value (2 steps).
8) DeleteParameterGroup 400->400, $.message expected 'The parameter ParameterGroupName is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.', got 'Cannot delete a default parameter group.'

Smithy evidence cited: SourceType enum = [node, parameter-group, subnet-group, cluster, user, acl]; ServiceUpdateStatus enum = [available, in-progress, complete, scheduled]; CreateCluster/DescribeEvents/DescribeServiceUpdates/DeleteParameterGroup all declare InvalidParameterValueException (InvalidParameterValue); UpdateCluster declares ClusterNotFoundFault; the parameter-group ops declare ParameterGroupNotFoundFault; Parameter members Description/AllowedValues/Value and ParameterGroup.Description carry no wire traits constraining the text, so the recorded catalog text is the requirement.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus); all 8 clusters below reproduce deterministically.
3. Cause 1: replay memorydb-catalog-describe-parameter-groups step task#1 - DescribeParameterGroups and inspect the Description of the default.*.search groups.
4. Cause 2: replay memorydb-cluster-and-multiregion-request-validation step task#2 - UpdateCluster on a non-existent cluster; read $.message.
5. Cause 3: replay memorydb-cluster-and-multiregion-request-validation step task#10 - CreateCluster with NumShards below 1; read $.message.
6. Cause 4: replay memorydb-events-updates-and-offerings-validation step task#2 - DescribeEvents with an unmodelled SourceType; read $.message.
7. Cause 5: replay memorydb-events-updates-and-offerings-validation step task#10 - DescribeServiceUpdates with an unmodelled Status entry; read $.message.
8. Cause 6: replay memorydb-parameter-group-lifecycle step task#5 - DescribeParameterGroups for a group that does not exist; read $.message.
9. Cause 7: replay memorydb-parameter-group-lifecycle step task#6 - DescribeParameters on a default group; compare each Parameter's Description/AllowedValues/Value against the fixture.
10. Cause 8: replay memorydb-parameter-update-and-reset-rules step task#18 - DeleteParameterGroup with a syntactically invalid ParameterGroupName; read $.message.

## Resolution note

Causes 1-6 and 8 are closed, and cause 7 is closed for 12 of its 19 recorded paths: the
catalog now carries the service's own text and value sets for acllog-max-len,
active-expire-effort, hll-sparse-max-bytes, latency-tracking, lfu-decay-time,
lfu-log-factor, list-compress-depth, maxmemory-samples, tcp-keepalive and search-enabled
(yes,no plus its Value), and what a parameter publishes is what an update accepts.

STILL DIVERGING, 7 paths of cause 7, all inside the 200 body of
memorydb-parameter-group-lifecycle#task#6: `Description` of search-enabled,
search-query-timeout-ms, slowlog-log-slower-than, slowlog-max-len and
tracking-table-max-keys, and `AllowedValues` of search-query-timeout-ms and
slowlog-log-slower-than. This report quotes the required text for every other path but
not for these, and a replay withholds a recorded 200-body value, so the catalog's exact
wording and bounds for them could not be established from inside the fix; each is left at
the engine-grounded value documented beside it in `state::ENGINE_PARAMETERS` rather than
at another paraphrase, and the suite deliberately pins none of those five descriptions.
Anyone holding the recorded listing for a default group can close them by correcting those
seven fields in that one table.
