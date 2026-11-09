Issue-Source: trace-sweep-divergence
Issue-Trace: rds-refarch-aur-create-cluster-production-settings-refusals
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: rds-refarch-aur-create-cluster-production-settings-refusals rds-catalog-describe-input-validation rds-identifier-validation-and-lowercasing
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds|CreateDBCluster|4xx->2xx|$.CreateDBClusterResponse.CreateDBClusterResult.DBCluster.ReaderEndpoint
Issue-Key: rds|DescribeDBMajorEngineVersions|4xx->2xx|$.ErrorResponse
Issue-Key: rds|DescribeDBMajorEngineVersions|4xx->2xx|status
Issue-Key: rds|CreateGlobalCluster|4xx->2xx|$.CreateGlobalClusterResponse.CreateGlobalClusterResult.GlobalCluster.Endpoint
Issue-Key: rds|DescribeDBClusters|4xx->2xx|$.DescribeDBClustersResponse.DescribeDBClustersResult.DBClusters.DBCluster.ReaderEndpoint
Issue-Related: 20261029T214632Z-rds-accepts-nine-requests-aws-refuses-missing-request.md
Issue-Related: 20261102T104537Z-rds-cluster-parameter-group-and-catalog-reads-answer.md
Issue-Commit: fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7

# rds: invalid-input and not-found refusals are never raised — four requests AWS rejects answer HTTP 200

> _Note: this issue was identified against the code base at commit `fce22d92ce0f3b4c883797a9d5b97c4bc60b16e7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBCluster

## Description
Four divergence clusters share one root cause: the rds front door performs no request validation and no existence gate on these paths, so requests AWS refuses with a modeled error are accepted and answered HTTP 200 with a success envelope. All four are on the `rds` service and the replay reported no owning issue and no blocking stub for any of them.

Cause 1 of 4: CreateDBCluster accepts a reserved MasterUsername.
AWS: `CreateDBCluster` takes `MasterUsername` (String) and models `InvalidParameterValue` among the awsQuery error codes the service returns for a bad parameter value (HTTP 400, Error/Type `Sender`). AWS refuses a master username that is a word reserved by the engine. Message: the fixed text is `MasterUsername <name> cannot be used as it is a reserved word used by the engine`, where `<name>` is the `MasterUsername` value taken verbatim from the request (the recording shows the engine-reserved name it sent).
Emulator: the create is accepted and returns HTTP 200 with a `CreateDBClusterResponse` carrying a full DBCluster envelope (including a synthesized ReaderEndpoint); no `ErrorResponse` is produced at all.
Why: a client that relies on the refusal to catch a misconfigured master user silently provisions a cluster whose master user AWS would never have allowed, and the negative test in any deployment-guard suite passes when it must fail.
Fix: CreateDBCluster input validation in the rds service crate — reject engine-reserved master usernames before any state is created, serializing the awsQuery `InvalidParameterValue` Sender error at HTTP 400.
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#8.

Cause 2 of 4: DescribeDBClusters returns a cluster that does not exist.
AWS: `DescribeDBClusters` models exactly one error, `DBClusterNotFoundFault` (awsQuery code `DBClusterNotFoundFault`, HTTP 404), raised when the requested `DBClusterIdentifier` names no cluster. Message: the fixed text is `DBCluster <identifier> not found.` — the trailing period included — with `<identifier>` the `DBClusterIdentifier` from the request.
Emulator: the describe answers HTTP 200 with a DBCluster envelope for an identifier whose create was refused (or whose cluster never existed), including a synthesized ReaderEndpoint; no `ErrorResponse` is emitted.
Why: existence checks invert. A caller that probes for a cluster to decide whether to create it, or that asserts a failed create left nothing behind, concludes the cluster exists. Combined with cause 1 this makes the whole refusal-then-verify pattern unusable.
Fix: the DescribeDBClusters handler's existence gate — when `DBClusterIdentifier` resolves to no cluster in rds state, raise `DBClusterNotFoundFault` at HTTP 404 instead of projecting a fabricated record. Related open issues 20261106T224204Z-rds-not-found-and-state-guard-refusal-messages.md and 20261108T233943Z-rds-cluster-and-subnet-group-refusals-carry-the.md concern refusal wording/classification on other paths and carry no replay key for this step.
Traces: rds-refarch-aur-create-cluster-production-settings-refusals#task#11.

Cause 3 of 4: CreateGlobalCluster accepts a malformed GlobalClusterIdentifier.
AWS: Smithy constrains `CreateGlobalClusterMessage.GlobalClusterIdentifier` with `@length min 1 max 255` and `@pattern ^[A-Za-z][0-9A-Za-z-:._]*$`, and the service refuses a violating identifier with `InvalidParameterValue` (HTTP 400, Error/Type `Sender`). Message: the fixed text is `The parameter GlobalClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.` — entirely fixed except for the member name `GlobalClusterIdentifier`, which is the input member that failed.
Emulator: the create succeeds with HTTP 200, a `CreateGlobalClusterResponse` envelope and a synthesized global Endpoint; no `ErrorResponse` is produced.
Why: identifiers AWS rejects are admitted into state, so a client can create resources whose names cannot exist in AWS and whose later DNS-derived endpoints are invalid; identifier-validation tests pass against the emulator and fail against AWS.
Fix: CreateGlobalCluster input validation — enforce the modeled `@pattern`/`@length` on `GlobalClusterIdentifier` and the additional identifier rules the message states (no trailing hyphen, no consecutive hyphens) before minting the global cluster.
Traces: rds-identifier-validation-and-lowercasing#task#7.

Cause 4 of 4: DescribeDBMajorEngineVersions accepts an unknown engine name.
AWS: `DescribeDBMajorEngineVersions` takes `Engine` (`@length min 1 max 50`). The shipped model declares no operation-specific errors, so this rests on the service-common awsQuery error set: AWS refuses an unrecognised engine with `InvalidParameterValue` at HTTP 400, Error/Type `Sender`. Message: the fixed text is `unrecognized engine name: <engine>`, with `<engine>` the `Engine` value from the request.
Emulator: the call answers HTTP 200 with a `DescribeDBMajorEngineVersionsResponse` and no `ErrorResponse` element.
Why: a client that validates a user-supplied engine name by calling the catalog gets a success for an engine that does not exist, so bad engine names flow into later create calls.
Fix: DescribeDBMajorEngineVersions input validation in the rds catalog handler — gate `Engine` against the known engine set and raise `InvalidParameterValue` at HTTP 400.
Traces: rds-catalog-describe-input-validation#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 (98 fixtures, 993 steps, score 49.8%).

Cause 1 — rds-refarch-aur-create-cluster-production-settings-refusals#task#8, CreateDBCluster: status expected 400, got 200.
  $.ErrorResponse: missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'MasterUsername <reserved-name> cannot be used as it is a reserved word used by the engine'}, 'RequestId': <per-run>}; got None.
  $...DBCluster.ReaderEndpoint: host_not_own (expected None — a refusal carries no DBCluster at all).

Cause 2 — rds-refarch-aur-create-cluster-production-settings-refusals#task#11, DescribeDBClusters: status expected 404, got 200.
  $.ErrorResponse: missing — expected {'Error': {'Type': 'Sender', 'Code': 'DBClusterNotFoundFault', 'Message': 'DBCluster <identifier> not found.'}, 'RequestId': <per-run>}; got None.
  $...DBCluster.ReaderEndpoint: host_not_own (expected None).

Cause 3 — rds-identifier-validation-and-lowercasing#task#7, CreateGlobalCluster: status expected 400, got 200.
  $.ErrorResponse: missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'The parameter GlobalClusterIdentifier is not a valid identifier. Identifiers must begin with a letter; must contain only ASCII letters, digits, and hyphens; and must not end with a hyphen or contain two consecutive hyphens.'}, 'RequestId': <per-run>}; got None.
  $...GlobalCluster.Endpoint: host_not_own (expected None).

Cause 4 — rds-catalog-describe-input-validation#task#1, DescribeDBMajorEngineVersions: status expected 400, got 200.
  $.ErrorResponse: missing — expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue', 'Message': 'unrecognized engine name: <engine>'}, 'RequestId': <per-run>}; got None.

In each case the only per-run value inside the expected envelope is RequestId; the status, error Code, Type and the fixed message text are the requirement.

## Steps to Reproduce
1. git_clone() then replay_wire_corpus() against the built emulator.
2. Cause 1: fixture rds-refarch-aur-create-cluster-production-settings-refusals step task#8 — CreateDBCluster with MasterUsername set to an engine-reserved word. Emulator answers 200 with a DBCluster; AWS answers 400 InvalidParameterValue.
3. Cause 2: same fixture step task#11 — DescribeDBClusters for the DBClusterIdentifier whose create was refused. Emulator answers 200 with a DBCluster; AWS answers 404 DBClusterNotFoundFault.
4. Cause 3: fixture rds-identifier-validation-and-lowercasing step task#7 — CreateGlobalCluster with a GlobalClusterIdentifier that violates the modeled pattern (leading hyphen / consecutive hyphens). Emulator answers 200; AWS answers 400 InvalidParameterValue.
5. Cause 4: fixture rds-catalog-describe-input-validation step task#1 — DescribeDBMajorEngineVersions with Engine set to a name no RDS engine uses. Emulator answers 200; AWS answers 400 InvalidParameterValue.
