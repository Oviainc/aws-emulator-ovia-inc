Issue-Source: trace-sweep-divergence
Issue-Trace: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|DescribeDBEngineVersions|2xx->4xx|$.DescribeDBEngineVersionsResponse
Issue-Key: rds|DescribeDBEngineVersions|2xx->4xx|status
Issue-Commit: 304a5081e5ba402c52647393d53b96b506d32f91

# RDS DescribeDBEngineVersions refuses the sqlserver-dev-se engine with 400

> _Note: this issue was identified against the code base at commit `304a5081e5ba402c52647393d53b96b506d32f91`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBEngineVersions

## Description
DescribeDBEngineVersions answers an HTTP 400 refusal for an engine family the recording shows RDS serving normally, so the whole catalog page is lost instead of merely differing. 1 cause.

Cause 1 of 1: Engine 'sqlserver-dev-se'.
AWS: Smithy: DescribeDBEngineVersions (input DescribeDBEngineVersionsMessage, output DBEngineVersionMessage) declares NO errors at all — `errors: []` — so there is no modeled refusal for this operation's Engine/EngineVersion/DBParameterGroupFamily filters. The operation is a read-only catalog query: it answers 200 with a DBEngineVersions list of the DBEngineVersion rows matching the requested Engine, and an Engine the account cannot see simply yields a 200 with an empty DBEngineVersions list; the Marker/MaxRecords pagination members are the only thing that shortens the page. The recording confirms this for this engine: 200 with DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion rows for Engine 'sqlserver-dev-se' (DBParameterGroupFamily 'sqlserver-dev-se-17.0', MajorEngineVersion '17.00', MinorEngineVersion '4045.5.v1', EngineVersion '17.00.4045.5.v1', DBEngineVersionDescription 'SQL Server 2025 17.00.4045.5.v1', DefaultCharacterSet SQL_Latin1_General_CP1_CI_AS, and a ValidUpgradeTarget list of the 17.00.40xx targets). Those version strings are the recorded catalog's contents, not a per-run value — they are what the operation must publish for this engine.
Emulator: answers HTTP 400. The replay reports `status: expected 200, got 400` and `$.DescribeDBEngineVersionsResponse: missing`, i.e. the success envelope is absent entirely and an error envelope is returned in its place. Because the operation models no error, ANY 4xx status and ANY error code/message here is a defect: the requirement is status 200 and a DescribeDBEngineVersionsResponse envelope whose DBEngineVersions list carries the engine's rows (an empty list if the emulator's catalog genuinely has no rows for that engine), with no ErrorResponse at all.
Why: a client enumerating available engine versions — the AWS CLI/SDK `describe-db-engine-versions --engine <engine>` call, a Terraform/CDK provider resolving an engine version before CreateDBInstance, or a paginator walking Marker — gets a hard request failure instead of a result set. The SDK raises a modeled-error-free 400 that the client has no handler for, so engine discovery aborts rather than returning zero or more versions; code that then creates an instance of this engine family cannot validate its version at all. Hypothesis: the handler validates the Engine input against a hard-coded list of known engine families and refuses anything outside it, so newer/less common families (here 'sqlserver-dev-se') are rejected rather than returning an empty or populated list.
Fix: the DescribeDBEngineVersions handler's input handling and its engine-version catalog data — the Engine filter must select from the catalog instead of gating the request, so an unknown or unpopulated Engine returns 200 with an empty DBEngineVersions list rather than a 4xx.
Traces: rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se#task#4.

Related but NOT covering this: open issue 20261029T214632Z-rds-describedbengineversions-serves-a-stale-catalog-for-aurora.md owns the 200->200 content divergences of this operation for aurora-mysql, aurora-postgresql, custom-sqlserver-ee, db2-ae and db2-ce. It does not cover this step, which is a status-class failure (200 -> 400) on a different engine family, and the replay does not report this cluster as owned by it.

## Evidence
Replay cluster (trace set simparity-wire-aws-v1, verdict mismatch, affected_count 1):
  fixture rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se, step task#4
  divergence_key: service=rds|DescribeDBEngineVersions|status=200->400|keys=$.DescribeDBEngineVersionsResponse,status
  expected_status 200, actual_status 400
  detail: "status: expected 200, got 400" and "$.DescribeDBEngineVersionsResponse: missing" — expected envelope DescribeDBEngineVersionsResult.DBEngineVersions.DBEngineVersion[...] for Engine 'sqlserver-dev-se' (DBParameterGroupFamily 'sqlserver-dev-se-17.0', MajorEngineVersion '17.00', MinorEngineVersion '4045.5.v1', EngineVersion '17.00.4045.5.v1', DBEngineVersionDescription 'SQL Server 2025 17.00.4045.5.v1', SupportsBabelfish 'false', SupportsRDSProxy 'false', SupportsGlobalDatabases 'false', SupportsReadReplica 'false', SupportsCluster 'false', SupportsLimitlessDatabase 'false', DefaultCharacterSet SQL_Latin1_General_CP1_CI_AS, ValidUpgradeTarget targets 17.00.4055.5.v1 / 17.00.4065.4.v1 / 17.00.4075.5.v1), got no success envelope at all.
  stub_path: "" (not an unimplemented-operation block), owned_by: [] (no open issue carries this replay key).

Smithy evidence: DescribeDBEngineVersions -> input DescribeDBEngineVersionsMessage {Engine, EngineVersion, DBParameterGroupFamily, Filters, MaxRecords, Marker, DefaultOnly, ListSupportedCharacterSets, ListSupportedTimezones, IncludeAll}, output DBEngineVersionMessage {Marker, DBEngineVersions: DBEngineVersionList}, errors: [] — the operation models no refusal, so no 4xx is a legal response to any Engine value.

Expected vs actual, in one line: expected HTTP 200 + DescribeDBEngineVersionsResponse with the engine's DBEngineVersion rows; actual HTTP 400 with no DescribeDBEngineVersionsResponse.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Re-run fixture rds-catalog-describe-db-engine-versions-oracle-se2-cdb-sqlserver-dev-se, step task#4: it POSTs the RDS query-protocol action DescribeDBEngineVersions with Engine=sqlserver-dev-se.
4. Observe the response status: the emulator answers 400 with an error envelope; the recording answers 200 with DescribeDBEngineVersionsResponse.DescribeDBEngineVersionsResult.DBEngineVersions populated for that engine.
5. The fix is confirmed when that step returns status 200 and a DescribeDBEngineVersionsResponse envelope (DBEngineVersions list, empty or populated) with no ErrorResponse.
