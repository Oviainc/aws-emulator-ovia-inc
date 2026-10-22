Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-instance-not-found-envelope-variants
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-instance-not-found-envelope-variants
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|DescribeDBInstances|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBInstances|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBInstances|4xx->5xx|status
Issue-Key: docdb|DeleteDBInstance|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DeleteDBInstance|4xx->5xx|$headers.content-type
Issue-Key: docdb|DeleteDBInstance|4xx->5xx|status
Issue-Key: docdb|RebootDBInstance|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|RebootDBInstance|4xx->5xx|$headers.content-type
Issue-Key: docdb|RebootDBInstance|4xx->5xx|status
Issue-Key: docdb|CreateDBInstance|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|CreateDBInstance|4xx->5xx|$headers.content-type
Issue-Key: docdb|CreateDBInstance|4xx->5xx|status
Issue-Key: docdb|ApplyPendingMaintenanceAction|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|ApplyPendingMaintenanceAction|4xx->5xx|$headers.content-type
Issue-Key: docdb|ApplyPendingMaintenanceAction|4xx->5xx|status
Issue-Key: docdb|DescribeGlobalClusters|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeGlobalClusters|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeGlobalClusters|4xx->5xx|status
Issue-Key: docdb|DescribeEventSubscriptions|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeEventSubscriptions|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeEventSubscriptions|4xx->5xx|status
Issue-Key: docdb|DescribeCertificates|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeCertificates|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeCertificates|4xx->5xx|status
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# DocumentDB instance and global-cluster not-found paths refused with AmbiguousService 500

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBInstances

## Description
8 causes in this bundle; all 8 must be closed.

SHARED ROOT CAUSE (applies to every cause below)
AWS: DocumentDB is served on the RDS front door — its model declares awsService.endpointPrefix = 'rds' and sigv4Name = 'rds' (Smithy: com-amazonaws-docdb-amazonrdsv19, AmazonRDSv19 2014-10-31, protocol awsQuery, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), as do RDS and Neptune. A missing resource yields a 404 text/xml <ErrorResponse><Error><Type>Sender</Type><Code>..</Code> with the resource-specific awsQuery code. 'AmbiguousService' is not an AWS error code.
Emulator: the router refuses before dispatch with HTTP 500, content-type application/x-amz-json-1.1, body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}; the collision is between service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19. Two compounding faults: (a) nothing is served, (b) the refusal is JSON for an awsQuery client.
Why: every not-found discriminator of the DocumentDB instance/global-cluster/subscription/certificate surface collapses into one untyped 500; a botocore client raises ResponseParserError rather than a ClientError, so code that distinguishes DBInstanceNotFound from GlobalClusterNotFoundFault or CertificateNotFound cannot work at all.
Fix: the request router's service-resolution table AND its error serializer — resolve 'rds' deterministically to one handler for the shared Query surface (disambiguate on the Query body's Action/Version) and serialize router-level refusals for awsQuery services as text/xml <ErrorResponse>.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice records the same collision from the behavioral corpus with no wire fixture key, so it covers none of the steps below.

Cause 1 of 8: DescribeDBInstances for a missing instance (fixture docdb-instance-not-found-envelope-variants, step task#1). AWS: 404 text/xml ErrorResponse, Error/Type=Sender, Error/Code=DBInstanceNotFound (awsQuery code for DBInstanceNotFoundFault, httpError 404). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 2 of 8: DeleteDBInstance for a missing instance (same fixture, step task#2). AWS: 404 text/xml ErrorResponse, Error/Code=DBInstanceNotFound. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 3 of 8: RebootDBInstance for a missing instance (same fixture, step task#3). AWS: 404 text/xml ErrorResponse, Error/Code=DBInstanceNotFound. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 4 of 8: CreateDBInstance naming a missing cluster (same fixture, step task#4). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterNotFoundFault (Smithy: DBClusterNotFoundFault, awsQueryError code DBClusterNotFoundFault, httpError 404) — the failure is attributed to the cluster, not the instance. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 5 of 8: ApplyPendingMaintenanceAction on a missing resource ARN (same fixture, step task#5). AWS: 404 text/xml ErrorResponse, Error/Code=ResourceNotFoundFault. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 6 of 8: DescribeGlobalClusters for a missing global cluster (same fixture, step task#6). AWS: 404 text/xml ErrorResponse, Error/Code=GlobalClusterNotFoundFault (awsQueryError code GlobalClusterNotFoundFault, httpError 404). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 7 of 8: DescribeEventSubscriptions for a missing subscription (same fixture, step task#7). AWS: 404 text/xml ErrorResponse, Error/Code=SubscriptionNotFound. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 8 of 8: DescribeCertificates for a missing certificate identifier (same fixture, step task#8). AWS: 404 text/xml ErrorResponse, Error/Code=CertificateNotFound (Smithy: DescribeCertificates error CertificateNotFoundFault, awsQueryError code CertificateNotFound, httpError 404) — note the same operation must answer 200 for an unfiltered call. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Traces: docdb-instance-not-found-envelope-variants#task#1 .. #task#8.

## Evidence
Replay of trace set simparity-wire-aws-v1. Representative step docdb-instance-not-found-envelope-variants#task#1 (DescribeDBInstances, missing instance):
  expected: status 404, content-type text/xml, {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'DBInstanceNotFound', 'Message': 'DBInstance <id> not found.'}}}
  actual:   status 500, content-type application/x-amz-json-1.1, $.ErrorResponse missing
The remaining seven steps expect 404 text/xml with Error/Code DBInstanceNotFound (task#2, task#3), DBClusterNotFoundFault (task#4), ResourceNotFoundFault (task#5), GlobalClusterNotFoundFault (task#6), SubscriptionNotFound (task#7) and CertificateNotFound (task#8); all got status 500 with content-type application/x-amz-json-1.1 and no ErrorResponse element.
The body served is the router refusal, captured verbatim elsewhere in the same corpus: {'__type': 'AmbiguousService', 'message': "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. docdb = boto3.client('docdb', endpoint_url=<emulator>, region_name='us-west-2')
3. docdb.describe_db_instances(DBInstanceIdentifier='no-such-instance') -> expected 404 text/xml ErrorResponse/Error/Code=DBInstanceNotFound; actual 500, content-type application/x-amz-json-1.1, {"__type":"AmbiguousService",...}.
4. Repeat with delete_db_instance, reboot_db_instance (expect DBInstanceNotFound), create_db_instance(DBClusterIdentifier='no-such-cluster') (expect DBClusterNotFoundFault), apply_pending_maintenance_action (expect ResourceNotFoundFault), describe_global_clusters (expect GlobalClusterNotFoundFault), describe_event_subscriptions (expect SubscriptionNotFound), describe_certificates(CertificateIdentifier='no-such-cert') (expect CertificateNotFound) — every one returns the same 500 JSON refusal.
5. Replay the fixture steps listed under Traces to confirm.
