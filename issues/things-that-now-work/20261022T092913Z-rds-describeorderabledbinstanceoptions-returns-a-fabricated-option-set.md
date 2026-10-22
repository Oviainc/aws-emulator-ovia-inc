Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge rds-catalog-describe-orderable-db-instance-options-db-r5d-8xlarge-db-x2iedn-xlarge rds-create-db-instance-catalog-triple
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions
Issue-Key: rds|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions.OrderableDBInstanceOption
Issue-Commit: 0b8884c4ab978d23b4fe746f795a1277a2a36df3

# RDS DescribeOrderableDBInstanceOptions returns a fabricated option set

> _Note: this issue was identified against the code base at commit `0b8884c4ab978d23b4fe746f795a1277a2a36df3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeOrderableDBInstanceOptions

## Description
Two causes in the orderable-options catalog.

Cause 1 of 2 — DescribeOrderableDBInstanceOptions filtered by Engine/DBInstanceClass. AWS: OrderableDBInstanceOptions is a list with one entry per (engine version, class, license model, storage type) combination, each carrying AvailabilityZones, MultiAZCapable, ReadReplicaCapable, SupportsStorageEncryption, SupportsIops, SupportsStorageThroughput/MinStorageThroughputPerDbInstance/MaxStorageThroughputPerDbInstance, MinStorageSize/MaxStorageSize, MaxIopsPerGib, SupportedNetworkTypes, SupportsClusters, SupportedActivityStreamModes, AvailableProcessorFeatures, RequiresCustomProcessorFeatures, OutpostCapable and so on (Smithy: DescribeOrderableDBInstanceOptions -> OrderableDBInstanceOptionsMessage{OrderableDBInstanceOptions, Marker}; no modeled errors, so the 200 body is the whole contract). Emulator: the element has the wrong type/shape — the per-class/storage-type combinations recorded for db.c6gd.12xlarge and db.r5d.4xlarge are not served. Why: clients size instances and pick StorageType/Iops from this catalog; a fabricated set makes those choices invalid and, because CreateDBInstance validates DBInstanceClass against it, it is also why an invalid class is not rejected. Fix: the orderable-options catalog data plus the DescribeOrderableDBInstanceOptions filter/serializer. Traces: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1 (269 steps in this cluster).

Cause 2 of 2 — the same operation narrowed to Engine=mysql, EngineVersion=8.0.39, DBInstanceClass=db.t4g.micro. AWS returns one entry with StorageType 'gp2' for that combination; the emulator returns a single entry with StorageType 'gp3' and a set of invented capability members (SupportsIops, SupportsStorageThroughput, SupportedEngineModes, MinStorageSize 20, ...) instead of the recorded combination, and collapses the list to a single element. Fix: as cause 1 — one OrderableDBInstanceOption per storage type, values taken from the catalog rather than defaulted. Traces: rds-create-db-instance-catalog-triple#task#7.

## Evidence
Cause 1: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1 — affected_count 269; $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions type_mismatch (expected the OrderableDBInstanceOption list recorded for db.c6gd.12xlarge, e.g. StorageType gp2 MinStorageSize 5 MaxStorageSize 65536, and the gp3 entry with MinStorageThroughputPerDbInstance 125 / MaxIopsPerGib 500.0).
Cause 2: rds-create-db-instance-catalog-triple#task#7 — expected [{Engine mysql, EngineVersion 8.0.39, DBInstanceClass db.t4g.micro, LicenseModel general-public-license, Vpc true, StorageType gp2}], got a single object with StorageType 'gp3' plus additional invented members.

## Steps to Reproduce
1) Replay the wire corpus (replay_wire_corpus).
2) Fixture rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge task#1..task#25: call DescribeOrderableDBInstanceOptions with each recorded Engine/DBInstanceClass filter and diff OrderableDBInstanceOptions against the recording.
3) Fixture rds-create-db-instance-catalog-triple task#7: DescribeOrderableDBInstanceOptions for mysql 8.0.39 / db.t4g.micro and compare StorageType and the member set.
