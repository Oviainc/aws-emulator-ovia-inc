Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: docdb-elastic.create-cluster-invalid-name-pattern-rejected docdb-elastic.create-cluster-invalid-shard-capacity-rejected docdb-elastic.create-cluster-shard-count-exceeds-max-rejected docdb-elastic.create-cluster-with-optional-config-persists
Issue-Shape-Id: com.amazonaws.docdbelastic#ChimeraDbLionfishServiceLambda
Issue-Key: docdb-elastic.create-cluster-invalid-name-pattern-rejected|run#5|AssertionError|assert message == f"Invalid cluster name - {bad_name}.", f"unexpected message: {message!r}"
Issue-Key: docdb-elastic.create-cluster-invalid-shard-capacity-rejected|run#6|AssertionError|assert message == "Invalid Input - '3'. ShardCapacity must be a power of 2 within [2, 64].", (
Issue-Key: docdb-elastic.create-cluster-shard-count-exceeds-max-rejected|run#5|AssertionError|assert message == "Invalid Input - '33'. ShardCount should be between 1 and 32.", (
Issue-Key: docdb-elastic|CreateCluster|ValidationException
Issue-Commit: 128b971c27980fd8fd6bfdfd4c5ec95349a617e4

# DocDB Elastic CreateCluster input validation uses generic framework wording for three fields and falsely rejects a lowercase preferredMaintenanceWindow

> _Note: this issue was identified against the code base at commit `128b971c27980fd8fd6bfdfd4c5ec95349a617e4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
CreateCluster's request validation is the crate's generic Smithy-style constraint validator rather than the service's own. Three refusals carry a "1 validation error detected: Value at '<member>' failed to satisfy constraint: ..." sentence AWS never emits for these fields, and one well-formed request AWS accepts is refused outright. Four causes, all in CreateCluster's validation path. Note: cause 2 is the same defect the open issue 20261029T172712Z-docdb-elastic-createcluster-shardcapacity-refusal-uses-generic-constraint-wording-plus.md reports from the WIRE corpus; it does not carry this behavioral check's replay key, so the check is recorded here.

Cause 1 of 4: clusterName pattern refusal uses generic framework wording.
AWS: 400 ValidationException whose message is the service's own sentence "Invalid cluster name - <clusterName>." — fixed text "Invalid cluster name - " followed by the rejected name taken verbatim from the request and a trailing period. Smithy: CreateClusterInput.clusterName is a plain required String with NO @length and NO @pattern in the model, so any member-constraint sentence about it is emulator-authored; the service-specific wording is recorded behavior. A compliant name must still be accepted and echoed lower-cased.
Emulator: 400 with code ValidationException (correct) but message "1 validation error detected: Value at 'clusterName' failed to satisfy constraint: Member must have length between 1 and 63, inclusive, contain only letters, numbers and hyphens, start with a letter, not end with a hyphen and not contain two consecutive hyphens".
Why: a client that surfaces, logs or matches on the refusal text sees a sentence AWS never serves for this field and loses the rejected name AWS echoes back in it.
Fix: CreateCluster's own validation (services/.../src/ops/create_cluster.rs) must raise the clusterName refusal with the service-specific message instead of falling through to the crate's generic member-constraint validator.
Traces: docdb-elastic.create-cluster-invalid-name-pattern-rejected.

Cause 2 of 4: shardCapacity domain refusal uses generic framework wording.
AWS: 400 ValidationException with message "Invalid Input - '<shardCapacity>'. ShardCapacity must be a power of 2 within [2, 64]." — fixed text "Invalid Input - '" ... "'. ShardCapacity must be a power of 2 within [2, 64].", the only variable part being the rejected shardCapacity from the request. Smithy: CreateClusterInput.shardCapacity is a required Integer with no modeled enumeration (documented allowed values 2, 4, 8, 16, 32, 64), so the allowed-values sentence is emulator-authored.
Emulator: 400 with message "1 validation error detected: Value at 'shardCapacity' failed to satisfy constraint: Member must be one of the allowed values [2, 4, 8, 16, 32, 64]".
Why: as cause 1 — the "power of 2 within [2, 64]" rule AWS states is lost and the text never matches.
Fix: CreateCluster's shardCapacity domain check in the operation's validation.
Traces: docdb-elastic.create-cluster-invalid-shard-capacity-rejected.

Cause 3 of 4: shardCount range refusal uses generic framework wording.
AWS: 400 ValidationException with message "Invalid Input - '<shardCount>'. ShardCount should be between 1 and 32." — fixed text "Invalid Input - '" ... "'. ShardCount should be between 1 and 32.", variable part the rejected shardCount from the request. Smithy: CreateClusterInput.shardCount is a required Integer with no modeled @range, so the range sentence is emulator-authored. An in-range shardCount must still be accepted and reach ACTIVE.
Emulator: 400 with message "1 validation error detected: Value at 'shardCount' failed to satisfy constraint: Member must be between 1 and 32, inclusive".
Why: as cause 1.
Fix: CreateCluster's shardCount range check in the operation's validation.
Traces: docdb-elastic.create-cluster-shard-count-exceeds-max-rejected.

Cause 4 of 4: a lowercase day-of-week in preferredMaintenanceWindow is refused, and the optional creation-time configuration is therefore never persisted.
AWS: preferredMaintenanceWindow is accepted case-insensitively in its day-of-week abbreviation — a value of the form ddd:hh24:mi-ddd:hh24:mi with lowercase days is VALID, and once the cluster is ACTIVE GetCluster echoes it with the abbreviation normalized to title case (requested "<day>:hh:mm-<day>:hh:mm" in lower case comes back title-cased). Smithy: CreateClusterInput.preferredMaintenanceWindow is an optional plain String with no @pattern, so the case restriction is emulator-authored; the case-insensitive accept and the title-case normalization on echo are recorded behavior. The same request's other optional members must also persist verbatim: shardInstanceCount, backupRetentionPeriod and preferredBackupWindow are echoed as requested, not as service defaults, and `shards` carries one entry per SHARD (shardCount), not one per replica instance.
Emulator: the whole CreateCluster call is refused with 400 ValidationException "1 validation error detected: Value at 'preferredMaintenanceWindow' failed to satisfy constraint: Member must be of the form ddd:hh24:mi-ddd:hh24:mi, with ddd one of Mon, Tue, Wed, Thu, Fri, Sat, Sun" — a case-sensitive pattern check — so no cluster is created and the persistence of the optional configuration is untestable.
Why: a client using the lowercase form AWS accepts (and the form every AWS CLI example tolerates) cannot create a cluster at all; this is a false rejection, the most damaging class, and it also hides whether the optional creation-time configuration round-trips.
Fix: CreateCluster's preferredMaintenanceWindow validation must compare the day abbreviation case-insensitively and the handler/state must store the normalized title-cased value for echo, alongside the other optional members.
Traces: docdb-elastic.create-cluster-with-optional-config-persists.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service docdb-elastic.

Cause 1 — [mismatch] docdb-elastic.create-cluster-invalid-name-pattern-rejected: code ValidationException matched; AssertionError: unexpected message: "1 validation error detected: Value at 'clusterName' failed to satisfy constraint: Member must have length between 1 and 63, inclusive, contain only letters, numbers and hyphens, start with a letter, not end with a hyphen and not contain two consecutive hyphens" — expected "Invalid cluster name - <bad_name>." (fixed text "Invalid cluster name - " + the rejected name from the request + ".").

Cause 2 — [mismatch] docdb-elastic.create-cluster-invalid-shard-capacity-rejected: AssertionError: unexpected message: "1 validation error detected: Value at 'shardCapacity' failed to satisfy constraint: Member must be one of the allowed values [2, 4, 8, 16, 32, 64]" — expected "Invalid Input - '3'. ShardCapacity must be a power of 2 within [2, 64]." where 3 is the per-request rejected value filling the template, not part of it.

Cause 3 — [mismatch] docdb-elastic.create-cluster-shard-count-exceeds-max-rejected: AssertionError: unexpected message: "1 validation error detected: Value at 'shardCount' failed to satisfy constraint: Member must be between 1 and 32, inclusive" — expected "Invalid Input - '33'. ShardCount should be between 1 and 32." where 33 is the per-request rejected value.

Cause 4 — [error] docdb-elastic.create-cluster-with-optional-config-persists: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the CreateCluster operation: 1 validation error detected: Value at 'preferredMaintenanceWindow' failed to satisfy constraint: Member must be of the form ddd:hh24:mi-ddd:hh24:mi, with ddd one of Mon, Tue, Wed, Thu, Fri, Sat, Sun. The request was CreateCluster with authType=PLAIN_TEXT, shardCapacity=2, shardCount=1, shardInstanceCount=2, backupRetentionPeriod=3, preferredBackupWindow="04:00-04:30", preferredMaintenanceWindow="mon:05:00-mon:05:30" — a value AWS accepts, echoing "Mon:05:00-Mon:05:30" once ACTIVE. Because creation was refused, the check's later assertions (shardInstanceCount==2, backupRetentionPeriod==3, preferredBackupWindow=="04:00-04:30", len(shards)==1) were never reached.

In all four causes the HTTP status (400) and error code (ValidationException) are correct where a refusal is correct; the divergence is the message wording, plus the false rejection in cause 4.

## Steps to Reproduce
1. Boot the emulator and target the DocDB Elastic (docdb-elastic) endpoint.
2. POST /cluster (CreateCluster) with an otherwise valid request and a clusterName that violates the documented name rule (e.g. one starting with a digit) -> observe the generic "1 validation error detected: Value at 'clusterName' ..." message instead of "Invalid cluster name - <name>.".
3. Repeat with shardCapacity=3 -> generic allowed-values message instead of "Invalid Input - '3'. ShardCapacity must be a power of 2 within [2, 64].".
4. Repeat with shardCount=33 -> generic range message instead of "Invalid Input - '33'. ShardCount should be between 1 and 32.".
5. POST /cluster with valid required members plus shardInstanceCount=2, backupRetentionPeriod=3, preferredBackupWindow="04:00-04:30", preferredMaintenanceWindow="mon:05:00-mon:05:30" -> observe 400 ValidationException on preferredMaintenanceWindow instead of a 200; AWS accepts it, and GetCluster on the ACTIVE cluster echoes "Mon:05:00-Mon:05:30" plus the other requested optional values with one `shards` entry per shard.
6. Or re-replay behavioral checks docdb-elastic.create-cluster-invalid-name-pattern-rejected, docdb-elastic.create-cluster-invalid-shard-capacity-rejected, docdb-elastic.create-cluster-shard-count-exceeds-max-rejected, docdb-elastic.create-cluster-with-optional-config-persists.
