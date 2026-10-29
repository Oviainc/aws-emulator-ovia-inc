Issue-Source: trace-sweep-divergence
Issue-Trace: b1-endpoint-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: b1-endpoint-lifecycle
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms|CreateEndpoint|2xx->2xx|$.Endpoint.EndpointType
Issue-Key: dms|CreateEndpoint|2xx->2xx|$.Endpoint.EngineDisplayName
Issue-Key: dms|CreateEndpoint|2xx->2xx|$.Endpoint.IsReadOnly
Issue-Key: dms|CreateEndpoint|2xx->2xx|$.Endpoint.KmsKeyId
Issue-Key: dms|CreateEndpoint|2xx->2xx|$.Endpoint.MySQLSettings
Issue-Key: dms|DescribeEndpoints|2xx->2xx|$.Endpoints[*].EndpointType
Issue-Key: dms|DescribeEndpoints|2xx->2xx|$.Endpoints[*].EngineDisplayName
Issue-Key: dms|DescribeEndpoints|2xx->2xx|$.Endpoints[*].IsReadOnly
Issue-Key: dms|DescribeEndpoints|2xx->2xx|$.Endpoints[*].KmsKeyId
Issue-Key: dms|DescribeEndpoints|2xx->2xx|$.Endpoints[*].MySQLSettings
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# DMS Endpoint projection omits modeled Endpoint fields and renders EndpointType in the wrong case

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEndpoint

## Description
The emulator builds its DMS `Endpoint` structure from only a subset of the modeled members, and renders the EndpointType enum in the wrong case. The same projection is used by every operation that returns an Endpoint, so the defect shows up identically on the create path and the describe path. Two causes, one shared root: the Endpoint-to-wire projection.

=== Cause 1 of 2: CreateEndpoint's returned Endpoint is missing EngineDisplayName, IsReadOnly, KmsKeyId and the engine settings structure, and EndpointType is lowercased ===
Fixture b1-endpoint-lifecycle step task#1.

AWS: CreateEndpoint returns HTTP 200 with an `Endpoint` structure (Smithy: CreateEndpoint -> CreateEndpointResponse.Endpoint, shape com.amazonaws.databasemigrationservice#Endpoint). That structure models, among others, `EngineDisplayName` ("The expanded name for the engine name. For example, if the EngineName parameter is \"aurora\", this value would be \"Amazon Aurora MySQL\""), `IsReadOnly` (BooleanOptional, "Indicates whether the endpoint is read-only"), `KmsKeyId` ("An KMS key identifier that is used to encrypt the connection parameters for the endpoint. If you don't specify a value for the KmsKeyId parameter, then DMS uses your default encryption key"), and one per-engine settings structure selected by EngineName -- for EngineName "mysql" that is `MySQLSettings` (shape MySQLSettings). AWS populates all of them on a create: EngineDisplayName is the human-readable expansion of the requested EngineName, IsReadOnly is false for an endpoint the caller created directly, KmsKeyId is the account/region default DMS KMS key when the request omitted one, and the engine settings structure echoes the connection parameters that were supplied at top level or in the settings member (for MySQL: ServerName, Port, DatabaseName, Username, and AuthenticationMethod defaulting to password). `EndpointType` is returned as the uppercase form of the requested value: a request with EndpointType "source" comes back as "SOURCE" (recorded behavior for the response rendering -- the model's ReplicationEndpointTypeValue enum lists the lowercase request values "source"/"target", and says nothing about response casing).

Emulator: HTTP 200, but $.Endpoint.EndpointType is "source" where AWS returns "SOURCE"; $.Endpoint.EngineDisplayName, $.Endpoint.IsReadOnly, $.Endpoint.KmsKeyId and $.Endpoint.MySQLSettings are all absent from the response body entirely (replay reports each as `missing`, got None).

Why: a client reading Endpoint.EndpointType and comparing it to the value AWS returns takes the wrong branch, or fails a case-sensitive equality check against "SOURCE". A client that reads back the engine settings it just configured (the common "create then verify/round-trip the connection parameters" pattern) gets nothing, so it cannot tell a correctly configured endpoint from an empty one; absent KmsKeyId makes the endpoint look unencrypted, and absent IsReadOnly makes a tri-state (true/false/unset) read as unset, so code guarding modify/delete on `IsReadOnly is False` never runs.

Fix: the CreateEndpoint handler's response projection / serializer -- services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/create_endpoint.rs together with whatever shared Endpoint-to-wire helper it and describe_endpoints.rs both use (and the EndpointRecord fields in src/state.rs backing them). Derive EngineDisplayName from EngineName, default IsReadOnly to false and KmsKeyId to the default DMS key, emit the per-engine settings structure matching EngineName from the stored settings, and render the EndpointType enum in the response in AWS's uppercase form.

Traces: b1-endpoint-lifecycle#task#1

=== Cause 2 of 2: DescribeEndpoints returns the same under-populated Endpoint ===
Fixture b1-endpoint-lifecycle step task#2.

AWS: DescribeEndpoints returns HTTP 200 with `Endpoints`, a list of the same `Endpoint` shape (Smithy: DescribeEndpoints -> DescribeEndpointsResponse.Endpoints, target EndpointList of Endpoint), so each element carries the identical member set CreateEndpoint returned for that endpoint: EndpointType uppercase, EngineDisplayName, IsReadOnly, KmsKeyId and the engine settings structure for its EngineName.

Emulator: HTTP 200, but $.Endpoints[0].EndpointType is "source" instead of "SOURCE", and $.Endpoints[0].EngineDisplayName, $.Endpoints[0].IsReadOnly, $.Endpoints[0].KmsKeyId and $.Endpoints[0].MySQLSettings are all missing -- the same five divergences as on the create path, confirming one shared projection rather than two independent bugs.

Why: describe is how a client discovers endpoints it did not create in-process (inventory listings, drift checks, task wiring). Those clients see endpoints with no engine settings and no encryption key, and a lowercase EndpointType that fails the same comparisons as above.

Fix: the same shared Endpoint projection, reached from services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/describe_endpoints.rs -- fixing the helper must fix both paths; both operations must be re-replayed.

Traces: b1-endpoint-lifecycle#task#2

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 -- fixture b1-endpoint-lifecycle, step task#1, CreateEndpoint, expected HTTP 200, actual HTTP 200:
  $.Endpoint.EndpointType: value_mismatch (expected 'SOURCE', got 'source')
  $.Endpoint.EngineDisplayName: missing (expected 'MySQL', got None)
  $.Endpoint.IsReadOnly: missing (expected False, got None)
  $.Endpoint.KmsKeyId: missing (expected '<shaped-not-asserted>', got None)
  $.Endpoint.MySQLSettings: missing (expected {'AuthenticationMethod': 'password', 'DatabaseName': 'hr', 'Port': 3306, 'ServerName': 'db.sim2real-probe.invalid', 'Username': 'sim2real_probe'}, got None)
  (The recorded settings values above are this run's request parameters filling the template; the requirement is that each echoes the corresponding request value, and that AuthenticationMethod defaults to password. KmsKeyId is asserted only as shaped/present, not as a literal key id.)

Cause 2 -- fixture b1-endpoint-lifecycle, step task#2, DescribeEndpoints, expected HTTP 200, actual HTTP 200:
  $.Endpoints[0].EndpointType: value_mismatch (expected 'SOURCE', got 'source')
  $.Endpoints[0].EngineDisplayName: missing (expected 'MySQL', got None)
  $.Endpoints[0].IsReadOnly: missing (expected False, got None)
  $.Endpoints[0].KmsKeyId: missing (expected '<shaped-not-asserted>', got None)
  $.Endpoints[0].MySQLSettings: missing (expected the same echoed MySQL connection settings, got None)

Smithy evidence (services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/smithy-model/com-amazonaws-databasemigrationservice-amazondmsv20160101.json): Endpoint models EngineDisplayName, IsReadOnly (BooleanOptional), KmsKeyId and MySQLSettings; ReplicationEndpointTypeValue is an enum with values source/target; both CreateEndpointResponse.Endpoint and DescribeEndpointsResponse.Endpoints target the same Endpoint shape.

No open issue covers this: the only open DMS issue (20261023T134102Z-...-createendpoint-skips-serviceaccessrolearn-validation-and-createreplicationsubnetgroup-is.md) is a behavioral finding about ServiceAccessRoleArn validation and the CreateReplicationSubnetGroup 501 stub, and says nothing about the Endpoint response projection.

## Steps to Reproduce
1. Replay fixture b1-endpoint-lifecycle against the emulator.
2. Cause 1 (step task#1): CreateEndpoint with EndpointIdentifier=<id>, EndpointType='source', EngineName='mysql' and MySQL connection parameters (ServerName, Port 3306, DatabaseName, Username, Password), no KmsKeyId. Expect HTTP 200 with Endpoint.EndpointType == 'SOURCE', Endpoint.EngineDisplayName == 'MySQL', Endpoint.IsReadOnly == false, Endpoint.KmsKeyId present (default DMS key), and Endpoint.MySQLSettings echoing ServerName/Port/DatabaseName/Username with AuthenticationMethod 'password'. The emulator returns EndpointType 'source' and omits the other four members.
3. Cause 2 (step task#2): DescribeEndpoints filtered on that endpoint id. Expect HTTP 200 with Endpoints[0] carrying the identical member set. The emulator returns the same under-populated Endpoint with lowercase EndpointType.
4. Both steps must be re-replayed to confirm a fix, since they share one projection.
