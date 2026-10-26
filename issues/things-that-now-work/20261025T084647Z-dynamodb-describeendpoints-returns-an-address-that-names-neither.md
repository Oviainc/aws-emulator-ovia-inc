Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb.describe-endpoints-returns-server-endpoints dynamodb.concurrent-create-backup-duplicate-name-accepted
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb.describe-endpoints-returns-server-endpoints|run#7|AssertionError|assert matching_region, f"no endpoint address referenced region {ctx.region}: {endpoints1}"
Issue-Key: dynamodb.concurrent-create-backup-duplicate-name-accepted|run#16|AssertionError|assert not failures, (
Issue-Commit: 8b92040b06acdac381093d8b397331a442bd94ad

# dynamodb: DescribeEndpoints returns an address that names neither the service nor the region, and CreateBackup's concurrency semantics are unreachable (2 causes)

> _Note: this issue was identified against the code base at commit `8b92040b06acdac381093d8b397331a442bd94ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeEndpoints

## Description
Cause 1 of 2: DescribeEndpoints does not report a regional DynamoDB service endpoint.
AWS: DescribeEndpoints is a no-input, server-side operation whose response is the required list member `Endpoints` of `Endpoint{Address, CachePeriodInMinutes}` (Smithy: DescribeEndpoints input {} / output Endpoints required list; no modeled errors; Endpoint.Address documented as the endpoint's address; CreateBackup and friends carry clientDiscoveredEndpoint required:false, i.e. this is the optional endpoint-discovery surface). The address AWS reports is the regional DynamoDB service endpoint — it names the service and the region the client called (`dynamodb.<region>.amazonaws.com` in the recording for us-west-2), and the set is stable across an immediate repeat call. The requirement is the FORM — the address identifies the dynamodb service and the caller's region — not any particular literal host.
Emulator: `Endpoints: [{'Address': '127.0.0.1:45283', 'CachePeriodInMinutes': 1440}]`. The address is the emulator's bare listener host:port; it contains neither 'dynamodb' nor the caller's region, so it carries none of the identity an endpoint-discovery response is supposed to convey. CachePeriodInMinutes (1440) and the repeat-call stability are correct.
Why: a client that uses endpoint discovery reads this list to learn which regional endpoint to address, and tooling/diagnostics echo it to show which region a call resolved to. An address that names no service and no region makes the response uninformative — a caller cannot confirm it reached the region it asked for, and any client that validates the discovered endpoint against the service/region it configured rejects it.
Fix: the DescribeEndpoints handler (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/describe_endpoints.rs) — derive the reported Address from the request's signing region and the dynamodb endpoint prefix rather than echoing the bound socket.
Traces: dynamodb.describe-endpoints-returns-server-endpoints.

Cause 2 of 2: CreateBackup cannot be called at all, so its concurrency semantics (no BackupName uniqueness, no serialization) are unimplemented.
AWS: Smithy CreateBackup takes required TableName (TableArn, @length 1..1024) and required BackupName (@length 3..255, pattern ^[a-zA-Z0-9_.-]+$) and returns BackupDetails; its modeled errors are BackupInUseException, ContinuousBackupsUnavailableException, LimitExceededException, TableInUseException, TableNotFoundException (all 400), InvalidEndpointException (421) and InternalServerError (500). Crucially, BackupName is NOT a uniqueness constraint and concurrent CreateBackup calls against one table are not serialized: the recording (confirmed on two independent real-AWS replays) fires 8 CreateBackup calls at one barrier for the SAME table with the SAME BackupName and all 8 succeed, each getting a distinct BackupArn, each independently retrievable through DescribeBackup reporting that same shared BackupName. None of the modeled errors above is returned for that race.
Emulator: all 8 racers are refused with error code `NotImplemented` ('operation CreateBackup is scaffolded but not implemented in this emulator'), so no backup is created and nothing about the race can be observed.
Why: a client taking periodic backups under a fixed naming scheme (one name per table per schedule tick, possibly fired concurrently by retries or by several workers) gets every call refused; and once the operation is implemented, the natural implementation — treating BackupName as a per-table unique key, or taking a table-level lock — would wrongly turn the AWS-legal race into BackupInUseException/LimitExceededException and silently drop backups a caller believes it took. NotImplemented is not a code DynamoDB ever returns, so the refusal is also unclassifiable by a client's error handling.
Fix: the CreateBackup handler and the backup state it owns (services/service-com-amazonaws-dynamodb-dynamodb-20120810/src/ops/create_backup.rs) — each call must mint its own BackupArn and register an independent backup record keyed on the ARN, with BackupName stored as a non-unique attribute and no serialization across concurrent calls on one table. The bare stub is covered by the auto-filed CreateBackup scaffold-stub issue (reported in this run as blocking 6 other checks); what is recorded HERE and not in that issue is the concurrency/uniqueness requirement its implementation must satisfy, which is why this check's replay key is listed separately.
Traces: dynamodb.concurrent-create-backup-duplicate-name-accepted.

## Evidence
Cause 1: AssertionError: no endpoint address referenced region us-west-2: [{'Address': '127.0.0.1:45283', 'CachePeriodInMinutes': 1440}] (check_dynamodb_describe_endpoints_returns_server_endpoints.py line 28). AWS recording: an Endpoints entry whose Address names the dynamodb service and the caller's region (dynamodb.us-west-2.amazonaws.com for us-west-2), CachePeriodInMinutes=1440, stable across an immediate repeat call.
Cause 2: AssertionError: expected all 8 concurrent same-named CreateBackup calls on the same table to succeed independently (real DynamoDB does not enforce BackupName uniqueness or serialize concurrent backup creates at this concurrency), but 8 were rejected: ['NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented', 'NotImplemented'] (check_dynamodb_concurrent_create_backup_duplicate_name_accepted.py line 78). AWS recording: 8 successes, 8 distinct BackupArns, each DescribeBackup reporting the shared BackupName.

## Steps to Reproduce
Cause 1:
1. dynamodb.describe_endpoints() against the emulator with region us-west-2.
2. Observe Endpoints[0].Address = '127.0.0.1:<port>' — it contains neither 'dynamodb' nor 'us-west-2', where AWS reports the regional dynamodb endpoint for the caller's region.
3. Call describe_endpoints() again and confirm the set is stable (it is) — only the address form diverges.
Cause 2:
1. dynamodb.create_table(BillingMode='PAY_PER_REQUEST'), wait for TableStatus=ACTIVE and for describe_continuous_backups ContinuousBackupsStatus=ENABLED.
2. Fire 8 concurrent create_backup(TableName=<table>, BackupName=<one shared name>) calls released from a single threading.Barrier.
3. Observe all 8 refused with error code NotImplemented, where AWS returns 8 successes with 8 distinct BackupArns, each describe_backup reporting the shared BackupName.
4. Or replay the two check ids in trace_ids.
