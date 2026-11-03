Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-catalog-describe-certificates
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-catalog-describe-certificates docdb-catalog-describe-db-engine-versions docdb-catalog-describe-event-categories docdb-catalog-describe-orderable-db-instance-options docdb-query-envelope-shared-with-rds docdb-cluster-parameter-group-lifecycle
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|DescribeCertificates|2xx->2xx|$.DescribeCertificatesResponse.DescribeCertificatesResult.Certificates.Certificate[*]
Issue-Key: docdb|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[*]
Issue-Key: docdb|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap[*]
Issue-Key: docdb|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions.OrderableDBInstanceOption[*]
Issue-Key: docdb|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap.EventCategories.EventCategory[*]
Issue-Key: docdb|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.AllowedValues
Issue-Key: docdb|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.Description
Issue-Key: docdb|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.MinimumEngineVersion
Issue-Key: docdb|DescribeDBClusterParameters|2xx->2xx|$.DescribeDBClusterParametersResponse.DescribeDBClusterParametersResult.Parameters.Parameter.SupportedEngineModes
Issue-Commit: b357a994596866b4c3c5b5df5566beb9dd5436ae

# DocDB seeded catalog responses diverge from AWS reference data

> _Note: this issue was identified against the code base at commit `b357a994596866b4c3c5b5df5566beb9dd5436ae`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificates

## Description
This issue bundles SIX causes with one root fault: docdb's read-only catalog tables (CA certificates, engine versions, event categories, orderable instance options, default parameter metadata) are hand-seeded from stale or abbreviated data, so each catalog operation returns a different member set, a different ordering and different values than AWS. These are static reference data served by the emulator, not per-run values. A fixer must close ALL SIX causes below before submitting.

Cause 1 of 6: DescribeCertificates serves a stale CA set with no Thumbprint.
AWS: Smithy DescribeCertificates returns CertificateMessage{Certificates, Marker} of Certificate{CertificateIdentifier, CertificateType, Thumbprint, ValidFrom, ValidTill, CertificateArn}. The current RDS/DocDB CA list AWS returns begins rds-ca-ecc384-g1, rds-ca-rsa4096-g1, rds-ca-rsa2048-g1 (the 2021 generation), each with a Thumbprint and with the real issuance/expiry instants (e.g. ValidFrom/ValidTill at the certificate's actual second-resolution time, not midnight).
Emulator: the list is led by the retired rds-ca-2019 (ValidTill 2024, i.e. already expired), the 2021 CAs follow in a different order, no Certificate carries a Thumbprint member at all, and the ValidFrom/ValidTill values are midnight-rounded placeholders.
Why: a client that chooses a CA for cluster certificate rotation picks an expired one or cannot pin by thumbprint.
Fix: the docdb DescribeCertificates handler's seeded certificate table must carry AWS's current CA set, in AWS's order, each with Thumbprint and the real validity instants.

Cause 2 of 6: DescribeDBEngineVersions serves an abbreviated engine-version catalog.
AWS: DescribeDBEngineVersions returns DBEngineVersionMessage{Marker, DBEngineVersions} of DBEngineVersion, whose members include DBEngineVersionDescription, DBParameterGroupFamily, SupportedCACertificateIdentifiers, SupportsCertificateRotationWithoutRestart and ValidUpgradeTarget ("A list of engine versions that this database engine version can be upgraded to"). For docdb, AWS emits one entry per published engine version starting at 3.6.0 (family docdb3.6), each with the full flag set the recording shows (SupportsBabelfish, SupportsRDSProxy, SupportsGlobalDatabases, SupportsParallelQuery, SupportsReadReplica, SupportsCluster, SupportsLimitlessDatabase, SupportsDataGroups, MajorEngineVersion/MinorEngineVersion) and a populated ValidUpgradeTarget list of UpgradeTarget entries.
Emulator: the DBEngineVersions list differs element-for-element from index 0 onward - different ordering and a materially smaller member set per entry.
Why: a client that resolves an upgrade path or a supported CA identifier from this catalog reads an empty or wrong answer and cannot plan an upgrade.
Fix: the seeded docdb engine-version table behind DescribeDBEngineVersions must enumerate the published versions in AWS's order with the full DBEngineVersion member set, including ValidUpgradeTarget and SupportedCACertificateIdentifiers.

Cause 3 of 6: DescribeEventCategories (unfiltered) returns the wrong source types in the wrong order.
AWS: DescribeEventCategories returns EventCategoriesMessage{EventCategoriesMapList} of EventCategoriesMap{SourceType, EventCategories}. AWS's unfiltered map list covers every source type the service documents - including db-snapshot first, and entries for custom-engine-version, db-proxy, blue-green-deployment and the other documented types - each with its own documented category list.
Emulator: the map list is in a different order (db-instance at index 0 where AWS has db-snapshot) and its membership differs at indices 0,1,2,4,5,6,7,8,9,10 - whole source types present in AWS are absent and others appear in their place.
Why: a client enumerating valid (source type, category) pairs to build an event subscription validates against a list that does not match what the service will later accept.
Fix: the seeded event-category table must be keyed by AWS's full source-type list, in AWS's order, with AWS's category list per type.

Cause 4 of 6: DescribeOrderableDBInstanceOptions returns the wrong per-option member set and wrong capability values.
AWS: DescribeOrderableDBInstanceOptions returns OrderableDBInstanceOptionsMessage{OrderableDBInstanceOptions, Marker}; the modeled OrderableDBInstanceOption carries Engine, EngineVersion, DBInstanceClass, LicenseModel, AvailabilityZones, Vpc and StorageType, and the recorded wire envelope additionally carries the full capability flag set (SupportsClusters, SupportsGlobalDatabases, SupportsHttpEndpoint, SupportsOptimizedReads/Writes, SupportsKerberosAuthentication, SupportedNetworkTypes, RequiresCustomProcessorFeatures, SupportsDedicatedLogVolume, AvailableProcessorFeatures, SupportsAdditionalStorageVolumes, OutpostCapable, ...). For a docdb option AWS reports every AZ the Region offers and MultiAZCapable/ReadReplicaCapable/SupportsEnhancedMonitoring as false for docdb instance classes.
Emulator: every element of the list diverges - the AvailabilityZones list is short by the Region's fourth zone, MultiAZCapable and ReadReplicaCapable and SupportsEnhancedMonitoring are reported true where AWS reports false, SupportsClusters/SupportsGlobalDatabases and the rest of the flag set are missing, and an unmodeled SupportsStorageAutoscaling is emitted.
Why: a client that filters instance classes by capability (clusters, global databases, AZ availability) selects an unsupportable configuration.
Fix: the seeded orderable-options table must project the full flag set with docdb's real capability values and derive AvailabilityZones from the Region's zone list.

Cause 5 of 6: DescribeEventCategories(SourceType=db-cluster) omits documented categories.
AWS: for SourceType 'db-cluster' AWS returns the db-cluster category list including 'migration' and 'serverless' alongside failover, failure, notification, creation, deletion, maintenance, configuration change and global-failover.
Emulator: the returned EventCategory list is missing 'migration' at index 1 and 'serverless' at index 4 (the emulator's list substitutes 'creation' and 'failure' at those positions), i.e. the filtered list is a shorter, differently ordered set.
Why: a CreateEventSubscription built from this list silently cannot subscribe to migration or serverless cluster events.
Fix: same seeded event-category table as Cause 3 - the db-cluster entry must carry AWS's full category list in AWS's order.

Cause 6 of 6: DescribeDBClusterParameters returns wrong metadata for the default 'tls' parameter.
AWS: DescribeDBClusterParameters returns DBClusterParameterGroupDetails{Parameters, Marker} of Parameter{ParameterName, ParameterValue, Description, Source, ApplyType, DataType, AllowedValues, IsModifiable, MinimumEngineVersion, ApplyMethod}. For the docdb 'tls' parameter AWS reports Description 'TLS Settings', AllowedValues in the order 'disabled,enabled,fips-140-3,tls1.2+,tls1.3+', MinimumEngineVersion equal to the earliest engine version of the group's parameter-group family that supports the parameter, and a SupportedEngineModes element with member 'provisioned' (recorded behavior only - SupportedEngineModes is not in the on-disk docdb Parameter shape).
Emulator: Description is 'Config to enable/disable TLS', AllowedValues is ordered 'enabled,disabled,fips-140-3,tls1.2+,tls1.3+', MinimumEngineVersion is the fixed '3.6.0' regardless of the requested family, and SupportedEngineModes is absent.
Why: a client that renders the allowed-value list or gates on MinimumEngineVersion shows the wrong order and the wrong minimum version; the AllowedValues string is also what the modify-path error message quotes (see the separate error-wording issue), so the two disagree together.
Fix: the seeded docdb default cluster-parameter table must carry AWS's Description, AllowedValues ordering and SupportedEngineModes, and derive MinimumEngineVersion from the group's DBParameterGroupFamily.

Traces: docdb-catalog-describe-certificates#task#1 (Cause 1), docdb-catalog-describe-db-engine-versions#task#1 (Cause 2), docdb-catalog-describe-event-categories#task#1 (Cause 3), docdb-catalog-describe-orderable-db-instance-options#task#1 (Cause 4), docdb-query-envelope-shared-with-rds#task#1 (Cause 5), docdb-cluster-parameter-group-lifecycle#task#6 (Cause 6).

## Evidence
Cause 1 - fixture docdb-catalog-describe-certificates step task#1, DescribeCertificates, 200/200: '$.DescribeCertificatesResponse.DescribeCertificatesResult.Certificates.Certificate[0]: expected {CertificateIdentifier: rds-ca-ecc384-g1, Thumbprint: <thumbprint>, ValidFrom/ValidTill at second resolution, CertificateType: CA}, got {CertificateIdentifier: rds-ca-2019, ValidTill 2024-08-22T00:00:00Z, no Thumbprint}'; Certificate[1] and [2] likewise diverge (order + missing Thumbprint + midnight-rounded dates).
Cause 2 - fixture docdb-catalog-describe-db-engine-versions step task#1, DescribeDBEngineVersions, 200/200: DBEngineVersion[0..7] all reported missing; expected entry 0 is engine docdb 3.6.0, family docdb3.6, with SupportsCertificateRotationWithoutRestart, SupportedCACertificateIdentifiers{member: rds-ca-rsa4096-g1, rds-ca-rsa2048-g1} and a ValidUpgradeTarget list of UpgradeTarget entries.
Cause 3 - fixture docdb-catalog-describe-event-categories step task#1, DescribeEventCategories, 200/200: EventCategoriesMap[0] expected SourceType db-snapshot, got db-instance; [2] expected db-instance, got db-parameter-group; [5] expected db-cluster, got db-cluster-snapshot; [6] expected custom-engine-version, got db-proxy; indices 1,4,7,8,9,10 likewise.
Cause 4 - fixture docdb-catalog-describe-orderable-db-instance-options step task#1, DescribeOrderableDBInstanceOptions, 200/200: all 66 OrderableDBInstanceOption entries diverge; entry 0 expected AvailabilityZones of four zones and MultiAZCapable false / ReadReplicaCapable false / SupportsEnhancedMonitoring false plus SupportsClusters, SupportsGlobalDatabases, SupportsHttpEndpoint, SupportedNetworkTypes{member: IPV4, DUAL}, AvailableProcessorFeatures, RequiresCustomProcessorFeatures, SupportsDedicatedLogVolume, SupportsOptimizedReads/Writes, SupportsAdditionalStorageVolumes, SupportsKerberosAuthentication; got three zones, MultiAZCapable true, ReadReplicaCapable true, SupportsEnhancedMonitoring true and an unmodeled SupportsStorageAutoscaling.
Cause 5 - fixture docdb-query-envelope-shared-with-rds step task#1, DescribeEventCategories, 200/200: 'EventCategoriesMap.EventCategories.EventCategory[1]: expected migration, got creation' and '[4]: expected serverless, got failure'.
Cause 6 - fixture docdb-cluster-parameter-group-lifecycle step task#6, DescribeDBClusterParameters, 200/200: 'Parameter.AllowedValues: expected disabled,enabled,fips-140-3,tls1.2+,tls1.3+, got enabled,disabled,fips-140-3,tls1.2+,tls1.3+'; 'Parameter.MinimumEngineVersion: expected 5.0.0, got 3.6.0'; 'Parameter.Description: expected TLS Settings, got Config to enable/disable TLS'; 'Parameter.SupportedEngineModes: missing (expected {member: provisioned})'.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus.
2. Cause 1: fixture docdb-catalog-describe-certificates step task#1 - Action=DescribeCertificates. Compare the Certificates list order, the presence of Thumbprint and the ValidFrom/ValidTill resolution against AWS's current CA set.
3. Cause 2: fixture docdb-catalog-describe-db-engine-versions step task#1 - Action=DescribeDBEngineVersions (Engine=docdb). Compare the DBEngineVersions ordering and per-entry member set, especially ValidUpgradeTarget and SupportedCACertificateIdentifiers.
4. Cause 3: fixture docdb-catalog-describe-event-categories step task#1 - Action=DescribeEventCategories with no SourceType. Compare the EventCategoriesMapList source types and their order.
5. Cause 4: fixture docdb-catalog-describe-orderable-db-instance-options step task#1 - Action=DescribeOrderableDBInstanceOptions(Engine=docdb). Compare AvailabilityZones and the capability flags of each option.
6. Cause 5: fixture docdb-query-envelope-shared-with-rds step task#1 - Action=DescribeEventCategories&SourceType=db-cluster. 'migration' and 'serverless' are absent from the returned EventCategory list.
7. Cause 6: fixture docdb-cluster-parameter-group-lifecycle step task#6 - create a cluster parameter group in the docdb5.0 family and Action=DescribeDBClusterParameters on it; inspect the 'tls' Parameter's Description, AllowedValues order, MinimumEngineVersion and the absent SupportedEngineModes.
