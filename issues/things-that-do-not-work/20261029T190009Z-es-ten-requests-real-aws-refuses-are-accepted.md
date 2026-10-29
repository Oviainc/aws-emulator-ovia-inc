Issue-Source: trace-sweep-divergence
Issue-Trace: es-domain-creation-access-policy-not-json-rejected
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-domain-creation-access-policy-not-json-rejected esdef-create-domain-gp2-server-defaults esval-ebs-gp2-iops-not-accepted esval-range-snapshot-start-hour-25 esval-topology-standby-single-data-node esval-topology-three-az-two-data-nodes eserr-create-domain-ultrawarm-as-data-node-invalid-type esval-ebs-gp3-iops-below-floor esval-ebs-gp3-throughput-below-floor esval-ebs-gp3-volume-size-above-instance-cap esval-ebs-gp3-volume-size-below-instance-floor eserr-describe-instance-type-limits-t2-micro-invalid-type esdef-list-instance-type-details-max-results-ignored es-cross-cluster-connection-errors es-package-management-errors opensearch-start-migration-invalid-source-error opensearch-application-default-setting-lifecycle opensearch-application-error-paths opensearch-put-default-application-setting-nonexistent-arn opensearch-describe-insight-details-not-found
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|CreateDomain|4xx->2xx|$.message
Issue-Key: es|CreateDomain|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|CreateDomain|4xx->2xx|status
Issue-Key: es|DescribeInstanceTypeLimits|4xx->2xx|$.message
Issue-Key: es|DescribeInstanceTypeLimits|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|DescribeInstanceTypeLimits|4xx->2xx|status
Issue-Key: es|ListInstanceTypeDetails|4xx->2xx|$.message
Issue-Key: es|ListInstanceTypeDetails|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|ListInstanceTypeDetails|4xx->2xx|status
Issue-Key: es|CreateOutboundConnection|4xx->2xx|$.message
Issue-Key: es|CreateOutboundConnection|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|CreateOutboundConnection|4xx->2xx|status
Issue-Key: es|CreatePackage|4xx->2xx|$.message
Issue-Key: es|CreatePackage|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|CreatePackage|4xx->2xx|status
Issue-Key: es|StartMigration|4xx->2xx|$.message
Issue-Key: es|StartMigration|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|StartMigration|4xx->2xx|status
Issue-Key: es|PutDefaultApplicationSetting|4xx->2xx|$.message
Issue-Key: es|PutDefaultApplicationSetting|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|PutDefaultApplicationSetting|4xx->2xx|status
Issue-Key: es|DescribeInsightDetails|4xx->2xx|$.message
Issue-Key: es|DescribeInsightDetails|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: es|DescribeInsightDetails|4xx->2xx|status
Issue-Related: 20261029T190009Z-es-constraint-violation-refusal-messages-are-emulator-composed.md
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: ten requests real AWS refuses are accepted with 200 or the wrong refusal

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
10 net-new root causes: semantic preconditions that the emulator never evaluates, so a request real AWS refuses is answered 200 (or, in one case, refused with the wrong numbers).

Cause 1 of 10 - CreateDomain does not validate AccessPolicies as JSON. AWS: a CreateDomain whose AccessPolicies is not parseable JSON is refused 400 with the fixed message "This policy contains invalid Json" (no variable part). Emulator: 200, the domain is created. This cluster also covers the esval CreateDomain probes (gp2 Iops supplied for a gp2 volume, SnapshotOptions start hour 25, MultiAZWithStandby with a single data node, three-AZ zone awareness with two data nodes) which AWS likewise refuses and the emulator accepts with 200. Trace: es-domain-creation-access-policy-not-json-rejected#task#1 (affected also esdef-create-domain-gp2-server-defaults#task#1, esval-ebs-gp2-iops-not-accepted#task#1, esval-range-snapshot-start-hour-25#task#1, esval-topology-standby-single-data-node#task#1, esval-topology-three-az-two-data-nodes#task#1).

Cause 2 of 10 - CreateDomain does not reject an UltraWarm instance type used as the data-node type. AWS: 409 with "Invalid instance type: <the instanceType from the request>". Emulator: 200. The same cluster covers gp3 Iops below the floor, gp3 Throughput below the floor and gp3 VolumeSize above the instance cap, all refused by AWS and accepted here. Trace: eserr-create-domain-ultrawarm-as-data-node-invalid-type#task#2.

Cause 3 of 10 - the per-instance EBS volume-size cap table is wrong. AWS refuses with "Volume size must be between <floor> and <cap> for <instanceType> instance type and version <version>" where the cap for t3.small.search is 100; the emulator refuses with the same template but a cap of 512. Status 409 and the template are already right - only the cap value, which must come from the instance/volume limit table, is wrong. Trace: esval-ebs-gp3-volume-size-below-instance-floor#task#1.

Cause 4 of 10 - DescribeInstanceTypeLimits accepts an instance type the service does not offer. AWS: 409 "Invalid instance type: <the instanceType from the request>". Emulator: 200 with a limits document. Trace: eserr-describe-instance-type-limits-t2-micro-invalid-type#task#2.

Cause 5 of 10 - ListInstanceTypeDetails ignores the retrieve-AZ precondition. AWS: asking for AZ information without the retrieve-AZ flag enabled is refused 400 with "Retrieve AZ flag need to be enabled to get Instance Type AZ information.". Emulator: 200. Trace: esdef-list-instance-type-details-max-results-ignored#task#3.

Cause 6 of 10 - CreateOutboundConnection does not check the local domain's owner against the caller. AWS: 400 ValidationException "OwnerId for local domain should be the same as caller account id.". Emulator: 200, the connection is created. Trace: es-cross-cluster-connection-errors#task#3.

Cause 7 of 10 - CreatePackage does not check that the package's S3 object is reachable. AWS: 400 ValidationException "Package file not accessible. Please validate that you have access to the S3 object and S3 bucket is present in the same region.". Emulator: 200. Trace: es-package-management-errors#task#1.

Cause 8 of 10 - StartMigration does not validate migrationOptions. AWS: 400 with the constraint report for migrationOptions.workspace.type (enum value set [ALL, OBSERVABILITY, ESSENTIALS, SECURITY_ANALYTICS, SEARCH]) and migrationOptions.source.datasourceArn (must be an aoss collection ARN). Emulator: 200, the migration is started. Trace: opensearch-start-migration-invalid-source-error#task#2.

Cause 9 of 10 - PutDefaultApplicationSetting does not check that the application exists. AWS: 404 with "Application not found for ARN: <the applicationArn from the request>". Emulator: 200, the setting is stored against a nonexistent application. Trace: opensearch-application-default-setting-lifecycle#task#3.

Cause 10 of 10 - DescribeInsightDetails does not validate insightId. AWS: 400 ValidationException with the standard constraint report for insightId against \\p{XDigit}{8}-\\p{XDigit}{4}-\\p{XDigit}{4}-\\p{XDigit}{4}-\\p{XDigit}{12} (Smithy: GUID @pattern, @length 36..36). Emulator: 200. Trace: opensearch-describe-insight-details-not-found#task#2.

Why: every one of these lets a client build state the real service would have rejected - a domain with an unparseable access policy or an impossible topology, an outbound connection to a domain the caller does not own, a package whose artifact does not exist, a migration from a non-existent source, a default-application pointer to nothing. Code validated against the emulator fails on first contact with AWS, and the failure surfaces far from the call that should have been refused.

Fix: the input-validation stage of each named operation's handler (CreateDomain, DescribeInstanceTypeLimits, ListInstanceTypeDetails, CreateOutboundConnection, CreatePackage, StartMigration, PutDefaultApplicationSetting, DescribeInsightDetails) and, for cause 3, the instance/EBS limit table the CreateDomain validator reads.

## Evidence
Cause 1: expected 400 + $.message 'This policy contains invalid Json'; actual 200 with no message and no x-amzn-errortype.
Cause 2: expected 409 + 'Invalid instance type: ultrawarm1.medium.elasticsearch'; actual 200.
Cause 3: expected 'Volume size must be between 10 and 100 for t3.small.search instance type and version OpenSearch_2.11'; actual '... between 10 and 512 ...' (status 409 matches).
Cause 4: expected 409 + 'Invalid instance type: t2.micro.search'; actual 200.
Cause 5: expected 400 + 'Retrieve AZ flag need to be enabled to get Instance Type AZ information.'; actual 200.
Cause 6: expected 400 + ValidationException + 'OwnerId for local domain should be the same as caller account id.'; actual 200.
Cause 7: expected 400 + ValidationException + 'Package file not accessible. Please validate that you have access to the S3 object and S3 bucket is present in the same region.'; actual 200.
Cause 8: expected 400 + the 2-clause constraint report; actual 200.
Cause 9: expected 404 + 'Application not found for ARN: <arn>'; actual 200.
Cause 10: expected 400 + ValidationException + the insightId constraint report; actual 200.

## Steps to Reproduce
1. Replay es-domain-creation-access-policy-not-json-rejected step task#1: CreateDomain with a non-JSON AccessPolicies; observe 200 instead of 400 'This policy contains invalid Json'.
2. Replay eserr-create-domain-ultrawarm-as-data-node-invalid-type step task#2 and eserr-describe-instance-type-limits-t2-micro-invalid-type step task#2 for the invalid-instance-type gates.
3. Replay es-cross-cluster-connection-errors step task#3, es-package-management-errors step task#1, opensearch-start-migration-invalid-source-error step task#2, opensearch-application-default-setting-lifecycle step task#3 and opensearch-describe-insight-details-not-found step task#2 for the remaining gates.
