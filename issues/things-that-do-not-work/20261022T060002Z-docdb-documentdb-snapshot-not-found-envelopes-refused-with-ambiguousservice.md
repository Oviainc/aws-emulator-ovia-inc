Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-snapshot-not-found-envelope-variants
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-snapshot-not-found-envelope-variants
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|DescribeDBClusterSnapshots|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBClusterSnapshots|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusterSnapshots|4xx->5xx|status
Issue-Key: docdb|DeleteDBClusterSnapshot|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DeleteDBClusterSnapshot|4xx->5xx|$headers.content-type
Issue-Key: docdb|DeleteDBClusterSnapshot|4xx->5xx|status
Issue-Key: docdb|CopyDBClusterSnapshot|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|CopyDBClusterSnapshot|4xx->5xx|$headers.content-type
Issue-Key: docdb|CopyDBClusterSnapshot|4xx->5xx|status
Issue-Key: docdb|DescribeDBClusterSnapshotAttributes|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|DescribeDBClusterSnapshotAttributes|4xx->5xx|$headers.content-type
Issue-Key: docdb|DescribeDBClusterSnapshotAttributes|4xx->5xx|status
Issue-Key: docdb|ModifyDBClusterSnapshotAttribute|4xx->5xx|$.ErrorResponse
Issue-Key: docdb|ModifyDBClusterSnapshotAttribute|4xx->5xx|$headers.content-type
Issue-Key: docdb|ModifyDBClusterSnapshotAttribute|4xx->5xx|status
Issue-Commit: b65dcc161724406fef2f9858d8de1b033f95eeb7

# DocumentDB snapshot not-found envelopes refused with AmbiguousService 500 JSON

> _Note: this issue was identified against the code base at commit `b65dcc161724406fef2f9858d8de1b033f95eeb7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBClusterSnapshots

## Description
5 causes in this bundle; all 5 must be closed.

SHARED ROOT CAUSE (applies to every cause below)
AWS: DocumentDB is served on the RDS front door — its model declares awsService.endpointPrefix = 'rds' and sigv4Name = 'rds' (Smithy: com-amazonaws-docdb-amazonrdsv19, AmazonRDSv19 2014-10-31, protocol awsQuery, xmlNamespace http://rds.amazonaws.com/doc/2014-10-31/), as do RDS and Neptune. A missing cluster snapshot yields 404 text/xml <ErrorResponse><Error><Type>Sender</Type><Code>DBClusterSnapshotNotFoundFault</Code> (Smithy: DescribeDBClusterSnapshots error DBClusterSnapshotNotFoundFault, awsQueryError code DBClusterSnapshotNotFoundFault, httpError 404). 'AmbiguousService' is not an AWS error code.
Emulator: the router refuses before dispatch with HTTP 500, content-type application/x-amz-json-1.1, body {"__type":"AmbiguousService","message":"3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}; the collision is between service-com-amazonaws-rds-amazonrdsv19, service-com-amazonaws-docdb-amazonrdsv19 and service-com-amazonaws-neptune-amazonrdsv19. Two compounding faults: (a) nothing is served, (b) the refusal is JSON for an awsQuery client.
Why: snapshot restore/copy/share tooling cannot detect a missing snapshot — it sees an untyped 500 and, in botocore, a ResponseParserError rather than a ClientError carrying DBClusterSnapshotNotFoundFault.
Fix: the request router's service-resolution table AND its error serializer — resolve 'rds' deterministically to one handler for the shared Query surface (disambiguate on the Query body's Action/Version) and serialize router-level refusals for awsQuery services as text/xml <ErrorResponse>.
Related: open issue 20261022T035032Z-rds-every-rds-rds-request-is-refused-with-ambiguousservice records the same collision from the behavioral corpus with no wire fixture key, so it covers none of the steps below.

Cause 1 of 5: DescribeDBClusterSnapshots for a missing snapshot identifier (fixture docdb-snapshot-not-found-envelope-variants, step task#1). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterSnapshotNotFoundFault. Emulator: 500 application/x-amz-json-1.1 AmbiguousService. Why/Fix: shared root cause above.

Cause 2 of 5: DeleteDBClusterSnapshot for a missing snapshot (same fixture, step task#2). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterSnapshotNotFoundFault. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 3 of 5: CopyDBClusterSnapshot from a missing source snapshot (same fixture, step task#3). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterSnapshotNotFoundFault (recorded message variant mentions the source region; it is an example). Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 4 of 5: DescribeDBClusterSnapshotAttributes for a missing snapshot (same fixture, step task#4). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterSnapshotNotFoundFault. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Cause 5 of 5: ModifyDBClusterSnapshotAttribute for a missing snapshot (same fixture, step task#5). AWS: 404 text/xml ErrorResponse, Error/Code=DBClusterSnapshotNotFoundFault. Emulator: 500 AmbiguousService. Why/Fix: shared root cause above.

Traces: docdb-snapshot-not-found-envelope-variants#task#1 .. #task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1. Representative step docdb-snapshot-not-found-envelope-variants#task#1 (DescribeDBClusterSnapshots, missing snapshot):
  expected: status 404, content-type text/xml, {'ErrorResponse': {'Error': {'Type': 'Sender', 'Code': 'DBClusterSnapshotNotFoundFault', 'Message': 'DBClusterSnapshot not found: <id>'}}}
  actual:   status 500, content-type application/x-amz-json-1.1, $.ErrorResponse missing
Steps task#2..task#5 (DeleteDBClusterSnapshot, CopyDBClusterSnapshot, DescribeDBClusterSnapshotAttributes, ModifyDBClusterSnapshotAttribute) expect the same 404 + Error/Code=DBClusterSnapshotNotFoundFault and all returned 500 with content-type application/x-amz-json-1.1.
The body served is the router refusal, captured verbatim elsewhere in the same corpus: {'__type': 'AmbiguousService', 'message': "3 emulated services all claim requests for 'rds': rds, rds, rds; the request is rejected rather than served by an arbitrary one"}.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. docdb = boto3.client('docdb', endpoint_url=<emulator>, region_name='us-west-2')
3. docdb.describe_db_cluster_snapshots(DBClusterSnapshotIdentifier='no-such-snapshot') -> expected 404 text/xml ErrorResponse/Error/Code=DBClusterSnapshotNotFoundFault; actual 500, content-type application/x-amz-json-1.1, {"__type":"AmbiguousService",...}.
4. Repeat with delete_db_cluster_snapshot, copy_db_cluster_snapshot, describe_db_cluster_snapshot_attributes, modify_db_cluster_snapshot_attribute on the same missing snapshot — identical 500 refusal.
5. Replay the fixture steps listed under Traces to confirm.
