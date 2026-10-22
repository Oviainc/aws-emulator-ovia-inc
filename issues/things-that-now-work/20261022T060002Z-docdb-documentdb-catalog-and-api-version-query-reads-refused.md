Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-catalog-describe-certificates
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-catalog-describe-certificates docdb-catalog-describe-db-engine-versions docdb-catalog-describe-event-categories docdb-instance-not-found-envelope-variants docdb-query-envelope-shared-with-rds docdb-snapshot-not-found-envelope-variants docdb-catalog-describe-orderable-db-instance-options
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|DescribeCertificates|2xx->5xx|$.DescribeCertificatesResponse
Issue-Key: docdb|DescribeCertificates|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeCertificates|2xx->5xx|status
Issue-Key: docdb|DescribeDBEngineVersions|2xx->5xx|$.DescribeDBEngineVersionsResponse
Issue-Key: docdb|DescribeDBEngineVersions|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBEngineVersions|2xx->5xx|status
Issue-Key: docdb|DescribeEventCategories|2xx->5xx|$.DescribeEventCategoriesResponse
Issue-Key: docdb|DescribeEventCategories|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeEventCategories|2xx->5xx|status
Issue-Key: docdb|DescribeOrderableDBInstanceOptions|2xx->5xx|$.DescribeOrderableDBInstanceOptionsResponse
Issue-Key: docdb|DescribeOrderableDBInstanceOptions|2xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeOrderableDBInstanceOptions|2xx->5xx|status
Issue-Key: docdb|DescribeDBClusters|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBClusters|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusters|4xx->5xx|status
Issue-Key: docdb|DescribeDBEngineVersions|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBEngineVersions|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBEngineVersions|4xx->5xx|status
Issue-Key: docdb|DescribeEventCategories|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeEventCategories|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeEventCategories|4xx->5xx|status
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# DocumentDB catalog and API-version Query reads refused with AmbiguousService 500 JSON

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
7 causes in this bundle; all 7 must be closed.

SHARED ROOT CAUSE (applies to every cause below)
AWS: DocumentDB is reached on the RDS front door. Its own model declares awsService.endpointPrefix = 'rds' and sigv4Name = 'rds' (Smithy: com-amazonaws-docdb-amazonrdsv19, service AmazonRDSv19, version 2014-10-31, protocol awsQuery, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), exactly as RDS and Neptune do. AWS dispatches such a request to the Query API and answers either 200 with a text/xml <...Response> envelope or a modeled awsQuery error as text/xml <ErrorResponse><Error><Type>Sender</Type><Code>..</Code>. 'AmbiguousService' is not an AWS error code and appears in no AWS model.
Emulator: the router refuses the request before any operation runs, answering HTTP 500 with content-type application/x-amz-json-1.1 and body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}. The three colliding registrations are service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19, all of which are bootstrapped. Two distinct faults compound: (a) no DocumentDB operation is served at all, (b) the refusal body is JSON although the resolved service's protocol is awsQuery, so it is not even parseable as an error.
Why: every DocumentDB call fails. A botocore client gets ResponseParserError ('invalid XML received') instead of a typed ClientError, so the caller never sees a status-appropriate error code; success paths and modeled not-found/validation paths are both unreachable.
Fix: the request router's service-resolution table AND the router's error serializer — 'rds' must resolve deterministically to one handler serving the RDS/DocDB/Neptune Query surface (disambiguate on the Query body's Action/Version rather than refusing), and any router-level refusal for an awsQuery service must be serialized as text/xml <ErrorResponse>, not JSON __type.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice reports the same router collision from the BEHAVIORAL corpus (DescribeDBEngineVersions) and carries no wire fixture key, so it does not cover the wire steps below. Note its 'Fix' claim that DocDB/Neptune have their own endpoint prefixes is contradicted by the models: all three declare endpointPrefix 'rds'.

Cause 1 of 7: DescribeCertificates catalog read (fixture docdb-catalog-describe-certificates, step task#1). AWS: 200 text/xml <DescribeCertificatesResponse> with DescribeCertificatesResult/Certificates/Certificate entries (Smithy: DescribeCertificates -> CertificateMessage; only modeled error CertificateNotFoundFault, awsQueryError code CertificateNotFound, httpError 404). Emulator: 500, content-type application/x-amz-json-1.1, no DescribeCertificatesResponse element. Why/Fix: shared root cause above.

Cause 2 of 7: DescribeDBEngineVersions catalog read (fixture docdb-catalog-describe-db-engine-versions, step task#1). AWS: 200 text/xml <DescribeDBEngineVersionsResponse> with DBEngineVersions/DBEngineVersion entries for engine docdb. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 3 of 7: DescribeEventCategories catalog read (fixture docdb-catalog-describe-event-categories, step task#1; cluster also covers docdb-instance-not-found-envelope-variants#task#9, docdb-query-envelope-shared-with-rds#task#1, docdb-snapshot-not-found-envelope-variants#task#6). AWS: 200 text/xml <DescribeEventCategoriesResponse> with EventCategoriesMapList. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 4 of 7: DescribeOrderableDBInstanceOptions catalog read (fixture docdb-catalog-describe-orderable-db-instance-options, step task#1). AWS: 200 text/xml <DescribeOrderableDBInstanceOptionsResponse> with OrderableDBInstanceOption entries. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 5 of 7: unknown API Version in the Query envelope (fixture docdb-query-envelope-shared-with-rds, step task#2). AWS: 400 text/xml <ErrorResponse> with Error/Type=Sender and Error/Code=InvalidAction (recorded message example: 'Could not find operation DescribeDBClusters for version 2099-01-01'); the message rule is built from the request's Action and Version, the exact text is an example. Emulator: 500 application/x-amz-json-1.1 AmbiguousService — the Query envelope is never inspected. Why/Fix: shared root cause above; the Action/Version pair the router must use to disambiguate is the same field that produces this error.

Cause 6 of 7: DescribeDBEngineVersions with no resolvable default engine version (fixture docdb-query-envelope-shared-with-rds, step task#4). AWS: 400 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=InvalidParameterCombination (recorded message example: "RDS can't find a default engine version."). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 7 of 7: DescribeEventCategories with an invalid SourceType (fixture docdb-query-envelope-shared-with-rds, step task#6). AWS: 400 text/xml <ErrorResponse> Error/Type=Sender, Error/Code=InvalidParameterValue, message built from the request naming the rejected event source type and the valid types (recorded text is an example). Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Traces: docdb-catalog-describe-certificates#task#1, docdb-catalog-describe-db-engine-versions#task#1, docdb-catalog-describe-event-categories#task#1, docdb-catalog-describe-orderable-db-instance-options#task#1, docdb-query-envelope-shared-with-rds#task#2, docdb-query-envelope-shared-with-rds#task#4, docdb-query-envelope-shared-with-rds#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up; /__admin/health 200). Representative step docdb-catalog-describe-certificates#task#1:
  expected: status 200, content-type text/xml, body $.DescribeCertificatesResponse.DescribeCertificatesResult.Certificates.Certificate[...]
  actual:   status 500, content-type application/x-amz-json-1.1, $.DescribeCertificatesResponse missing
The body on every one of these steps is the router refusal, captured verbatim on the polling steps of the same corpus:
  {"__type": "AmbiguousService", "message": "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}
Same expected-vs-actual shape for the other six causes: expected 200 text/xml <DescribeDBEngineVersionsResponse> / <DescribeEventCategoriesResponse> / <DescribeOrderableDBInstanceOptionsResponse>, and expected 400 text/xml <ErrorResponse> with Error/Code InvalidAction (task#2), InvalidParameterCombination (task#4) and InvalidParameterValue (task#6); actual in all cases 500 + application/x-amz-json-1.1 AmbiguousService.

## Steps to Reproduce
1. Boot the emulator from this checkout (all three of rds/docdb/neptune are bootstrapped).
2. docdb = boto3.client('docdb', endpoint_url=<emulator>, region_name='us-west-2')
3. docdb.describe_certificates() -> expected HTTP 200 text/xml DescribeCertificatesResponse; actual HTTP 500, content-type application/x-amz-json-1.1, body {"__type":"AmbiguousService",...} (botocore raises ResponseParserError).
4. Repeat with describe_db_engine_versions(), describe_event_categories(), describe_orderable_db_instance_options() — identical refusal.
5. POST a Query body with Version=2099-01-01&Action=DescribeDBClusters -> expected 400 text/xml ErrorResponse/Error/Code=InvalidAction; actual the same 500 JSON refusal.
6. Replay the fixture steps listed under Traces to confirm.
