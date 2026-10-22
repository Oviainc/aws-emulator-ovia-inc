Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-certificates
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-certificates rds-certificate-management rds-catalog-describe-event-categories rds-event-subscription-lifecycle rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge rds-catalog-describe-orderable-db-instance-options-db-r5d-8xlarge-db-x2iedn-xlarge rds-create-db-instance-catalog-triple rds-global-cluster-mutation-envelopes
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeCertificates|2xx->2xx|$.DescribeCertificatesResponse.DescribeCertificatesResult.Certificates.Certificate[*]
Issue-Key: rds|DescribeCertificates|2xx->2xx|$.DescribeCertificatesResponse.DescribeCertificatesResult.DefaultCertificateForNewLaunches
Issue-Key: rds|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap[*]
Issue-Key: rds|DescribeEventCategories|2xx->2xx|$.DescribeEventCategoriesResponse.DescribeEventCategoriesResult.EventCategoriesMapList.EventCategoriesMap.EventCategories.EventCategory[*]
Issue-Key: rds|DescribeOrderableDBInstanceOptions|2xx->2xx|$.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.MinorEngineVersion
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportedFeatureNames
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsCluster
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsDataGroups
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsRDSProxy
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.DBEngineDescription
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.DBEngineVersionDescription
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.ExportableLogTypes.member
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.ServerlessV2FeaturesSupport.MaxCapacity
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.ServerlessV2FeaturesSupport.MinCapacity
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportedFeatureNames.member[*]
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsChangeDataCaptureStreams
Issue-Key: rds|DescribeDBEngineVersions|2xx->2xx|$.DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion.SupportsEnhancedBinlog
Issue-Commit: 41eca1b5094053f7eb75d5bb9c5bde3b8db45df6

# RDS static reference catalogs for certificates event categories and orderable options are invented

> _Note: this issue was identified against the code base at commit `41eca1b5094053f7eb75d5bb9c5bde3b8db45df6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeCertificates

## Description
The emulator's read-only RDS reference catalogs - CA certificates, event-category mappings, orderable DB instance options, and the per-version DBEngineVersion envelope - are locally invented tables rather than the AWS catalogs. Each operation answers 200, so the fault is entirely in the content and shape of the reference data. Seven causes, one per diverging cluster.

Cause 1 of 7: DescribeCertificates returns the wrong certificate set and omits Thumbprint.
AWS: Smithy: DescribeCertificates -> Certificates: CertificateList of Certificate {CertificateIdentifier, CertificateType, Thumbprint, ValidFrom, ValidTill, CertificateArn, CustomerOverride, CustomerOverrideValidTill}; error CertificateNotFoundFault (404). Recorded behavior only for the catalog contents: AWS lists rds-ca-ecc384-g1, rds-ca-rsa4096-g1 and rds-ca-rsa2048-g1, each with a Thumbprint, and the ARN has an EMPTY account field (arn:aws:rds:us-west-2::cert:<id>) because CA certificates are service-owned, not account-owned.
Emulator: serves rds-ca-2019 first, omits Thumbprint on every entry, and stamps the caller's account into the ARN (arn:aws:rds:us-west-2:000000000000:cert:rds-ca-2019). Expected entries [0]..[3] are all reported missing.
Why: a client that picks a CACertificateIdentifier for CreateDBInstance/ModifyDBInstance from this list can select an identifier AWS does not offer, and a client pinning a certificate by Thumbprint has no value to read. The account-qualified ARN is also not a valid RDS certificate ARN.
Fix: the DescribeCertificates reference catalog and its Certificate serializer (ARN construction, Thumbprint member).
Traces: rds-catalog-describe-certificates#task#1.

Cause 2 of 7: DescribeCertificates does not reflect a certificate override and still omits Thumbprint.
AWS: recorded behavior only - after the override step the recording shows rds-ca-2019 with CustomerOverride 'true' and a CustomerOverrideValidTill member present, alongside rds-ca-rsa4096-g1 and rds-ca-rsa2048-g1 with Thumbprints.
Emulator: returns CustomerOverride 'false' for rds-ca-2019, no CustomerOverrideValidTill, no Thumbprint, and a different entry ordering; DefaultCertificateForNewLaunches also diverges.
Why: a client that sets a default CA and then reads it back cannot observe the override it just made.
Fix: the certificate-override state the DescribeCertificates handler reads, plus the same serializer gap as cause 1.
Traces: rds-certificate-management#task#3.

Cause 3 of 7: DescribeEventCategories returns the wrong source types and category sets.
AWS: Smithy: DescribeEventCategories -> EventCategoriesMapList of EventCategoriesMap {SourceType, EventCategories}. Recorded behavior only for the contents: AWS's db-snapshot map is [notification, creation, deletion, restoration], db-cluster-snapshot is [backup, notification], and db-instance carries 16 categories including backtrack and security.
Emulator: serves a differently ordered list with different per-source-type category sets (e.g. db-cluster-snapshot reduced to a single 'backup', db-instance missing backtrack/security). Expected maps [0],[1],[2],[5],[6],[7],[8],[9],[10] are all reported missing.
Why: a client building a CreateEventSubscription EventCategories argument from this catalog will send categories AWS rejects for that SourceType, and will not see categories AWS supports.
Fix: the DescribeEventCategories reference catalog.
Traces: rds-catalog-describe-event-categories#task#1.

Cause 4 of 7: DescribeEventCategories for a single SourceType returns the categories in the wrong order and with the wrong members.
AWS: recorded behavior only - for the filtered db-instance map AWS returns the categories beginning backup, deletion, availability, low storage, restoration, configuration change, failover, failure, notification, read replica, ... .
Emulator: returns an alphabetised set beginning availability, backup, configuration change, ... so EventCategory[0],[1],[2],[4],[5],[6],[7],[9],[10],[11],... all diverge.
Why: EventCategories is an ordered wire list; a client that compares it positionally, or that round-trips it into CreateEventSubscription, sees a different list from AWS.
Fix: the same DescribeEventCategories reference catalog, including the order in which categories are emitted.
Traces: rds-event-subscription-lifecycle#task#1.

Cause 5 of 7: DescribeOrderableDBInstanceOptions does not serve an OrderableDBInstanceOption list.
AWS: Smithy: DescribeOrderableDBInstanceOptions -> OrderableDBInstanceOptions: OrderableDBInstanceOptionsList of OrderableDBInstanceOption {Engine, EngineVersion, DBInstanceClass, LicenseModel, AvailabilityZones, StorageType, MinStorageSize, MaxStorageSize, SupportsIops, SupportsStorageEncryption, SupportsPerformanceInsights, SupportedNetworkTypes, MultiAZCapable, ReadReplicaCapable, Vpc, ...}. Recorded behavior only for the contents: AWS returns one option per (engine, version, class, licence, storage type) combination, e.g. mysql 8.4.9 on db.c6gd.12xlarge with StorageType gp2 and gp3 variants.
Emulator: $.DescribeOrderableDBInstanceOptionsResponse.DescribeOrderableDBInstanceOptionsResult.OrderableDBInstanceOptions is a type_mismatch - no OrderableDBInstanceOption list is served for these filters. This cluster covers 269 steps.
Why: a client that validates a DBInstanceClass/StorageType/AZ combination before CreateDBInstance cannot read the list at all, and a client iterating OrderableDBInstanceOption gets a structurally different document.
Fix: the DescribeOrderableDBInstanceOptions catalog and the OrderableDBInstanceOptions list serializer.
Traces: rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1 (cluster of 269 affected steps).

Cause 6 of 7: the single-version DescribeDBEngineVersions envelope omits members AWS always emits.
AWS: recorded behavior only for these members - MinorEngineVersion, SupportsRDSProxy, SupportsCluster and SupportsDataGroups are present on every DBEngineVersion AWS returns but are NOT in this checkout's vendored Smithy DBEngineVersion shape, so the model says nothing about them; the recording shows them populated. SupportedFeatureNames IS modeled (FeatureNameList) and the recording shows it empty for this engine/version.
Emulator: MinorEngineVersion, SupportsRDSProxy, SupportsDataGroups and SupportsCluster are absent (got None) and SupportedFeatureNames is a non-empty {'member': 's3Import'} where the recording has it empty.
Why: an SDK deserialising DBEngineVersion gets null for MinorEngineVersion and for the three capability booleans, so capability gating (RDS Proxy support, cluster support) silently reads false/absent, and a feature-name check sees s3Import advertised where AWS advertises nothing.
Fix: the DBEngineVersion serializer and the per-version catalog row it reads.
Traces: rds-create-db-instance-catalog-triple#task#6.

Cause 7 of 7: the same single-version envelope also misreports ServerlessV2 limits, log types and feature names for Aurora PostgreSQL.
AWS: Smithy: DBEngineVersion.ServerlessV2FeaturesSupport {MinCapacity, MaxCapacity}, ExportableLogTypes: LogTypeList, SupportedFeatureNames: FeatureNameList. Recorded behavior only for the values: MinCapacity 0.0, ExportableLogTypes [iam-db-auth-error, instance, postgresql], SupportedFeatureNames beginning Bedrock.
Emulator: ServerlessV2FeaturesSupport.MinCapacity 0.5 and MaxCapacity 256, ExportableLogTypes collapsed to the single scalar 'postgresql' instead of a three-member list, SupportedFeatureNames[0] 's3Import' instead of 'Bedrock', and MinorEngineVersion / SupportsRDSProxy / SupportsCluster / SupportsDataGroups / SupportsChangeDataCaptureStreams / SupportsEnhancedBinlog all absent.
Why: a client sizing an Aurora Serverless v2 cluster reads a minimum capacity of 0.5 where AWS reports 0.0, and a client enabling log exports sees one exportable log type instead of three - the list-vs-scalar difference also changes the parsed type.
Fix: the DBEngineVersion serializer (list emission for ExportableLogTypes) and the per-version catalog row.
Traces: rds-global-cluster-mutation-envelopes#task#1.

## Evidence
Every cluster replays expected_status 200 / actual_status 200. Cause 1: expected Certificate[0] {'CertificateIdentifier': 'rds-ca-ecc384-g1', 'Thumbprint': '2ee3dcc06e50192559b13929e73484354f23387d', 'CertificateArn': 'arn:aws:rds:us-west-2::cert:rds-ca-ecc384-g1'}, got {'CertificateIdentifier': 'rds-ca-2019', 'CertificateArn': 'arn:aws:rds:us-west-2:000000000000:cert:rds-ca-2019'} with no Thumbprint. Cause 2: expected rds-ca-2019 CustomerOverride 'true' + CustomerOverrideValidTill, got CustomerOverride 'false'. Cause 3: expected EventCategoriesMap[0] {'SourceType': 'db-snapshot', ...[notification, creation, deletion, restoration]}, got {'SourceType': 'db-instance', ...}. Cause 4: EventCategory[0] expected 'backup', got 'availability'. Cause 5: OrderableDBInstanceOptions type_mismatch, affected_count 269. Cause 6: SupportsRDSProxy expected 'true', got None; SupportedFeatureNames expected '' , got {'member': 's3Import'}. Cause 7: ServerlessV2FeaturesSupport.MinCapacity expected '0.0', got '0.5'; ExportableLogTypes.member expected ['iam-db-auth-error','instance','postgresql'], got 'postgresql'.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay rds-catalog-describe-certificates#task#1 (Action=DescribeCertificates) and diff Certificates.Certificate[*] - note the missing Thumbprint and the account-qualified ARN.
3. Replay rds-catalog-describe-event-categories#task#1 and rds-event-subscription-lifecycle#task#1 and diff EventCategoriesMapList.
4. Replay rds-catalog-describe-orderable-db-instance-options-db-c6gd-12xlarge-db-r5d-4xlarge#task#1 and observe OrderableDBInstanceOptions is not a list of OrderableDBInstanceOption.
5. Replay rds-create-db-instance-catalog-triple#task#6 and rds-global-cluster-mutation-envelopes#task#1 and diff the single DBEngineVersion envelope.
