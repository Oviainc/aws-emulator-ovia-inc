Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mgn.network-migration-mapping-and-analysis-without-execution-rejected
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn.network-migration-mapping-and-analysis-without-execution-rejected|run#19|AssertionError|assert real_analysis_code == "ConflictException", (
Issue-Commit: 374b6cf1741c892c0b442116db6d79f878d16651

# mgn StartNetworkMigrationAnalysis answers 501 NotImplemented instead of the modeled 202 jobID / ConflictException / ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `374b6cf1741c892c0b442116db6d79f878d16651`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartNetworkMigrationAnalysis

## Description
Cause 1 of 1: `StartNetworkMigrationAnalysis` is still an unimplemented scaffold stub, so every call to it is refused with HTTP 501 `NotImplemented` and none of its modeled outcomes (success, stage/job conflict, unknown execution) can be reached. Failing check: mgn.network-migration-mapping-and-analysis-without-execution-rejected.

AWS: Smithy model (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json) binds `StartNetworkMigrationAnalysis` to POST /network-migration/StartNetworkMigrationAnalysis with success status 202 and output member `jobID` (NetworkMigrationJobID, length 36..36, pattern ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$). Required input members are `networkMigrationDefinitionID` (length 21..21, pattern ^nmd-[0-9a-zA-Z]{17}$) and `networkMigrationExecutionID` (length 36..36, UUID pattern). The operation models the errors AccessDeniedException (403), ConflictException (409) — "The request could not be completed due to a conflict with the current state of the target resource" — ResourceNotFoundException (404) — "Resource not found exception" — ServiceQuotaExceededException (402), ThrottlingException (429) and ValidationException (400). The recording shows three distinct outcomes that the emulator must reproduce: (a) a well-formed, registered execution id whose definition has a mapping job currently in flight is refused with ConflictException, HTTP 409, message "A job cannot be started because another job for this execution is currently in progress." — that message is fixed text with no variable part; (b) a well-formed but never-staged execution id is refused with ResourceNotFoundException, HTTP 404, message "NetworkMigrationExecution with Id: '<networkMigrationExecutionID>' was not found", where the fixed text is "NetworkMigrationExecution with Id: '" ... "' was not found" and the only variable part is the `networkMigrationExecutionID` taken verbatim from the request (the uuid in any one run is just a sample value, not part of the template); (c) and no job record is created for a refused call — a rejected id must not afterwards appear in ListNetworkMigrationExecutions. The two refusals must be distinguishable: a known id in the wrong stage is a 409 conflict, an unknown id is a 404.

Emulator: the handler at services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/start_network_migration_analysis.rs returns `Err(crate::scaffolded::Scaffolded::not_implemented("StartNetworkMigrationAnalysis"))` unconditionally, producing HTTP 501 with error code `NotImplemented` and message "operation StartNetworkMigrationAnalysis is scaffolded but not implemented in this emulator". The replay reached this call after CreateNetworkMigrationDefinition, ListNetworkMigrationExecutions (one auto-staged execution at stage MAPPING) and StartNetworkMigrationMapping all succeeded, and the check failed with: "a real, known execution id in the wrong stage must be rejected with ConflictException (distinct from an unknown id's ResourceNotFoundException), got 'NotImplemented'".

Why: a client driving the network-migration pipeline cannot start an analysis job at all, and cannot tell a stage/job conflict from an unknown execution id. `NotImplemented` is not in the operation's modeled error set, so SDK error handling for ConflictException/ResourceNotFoundException never fires; a retry/branching client treats the 501 as a server fault rather than the conflict or not-found it actually is, and any test asserting the analysis stage of the pipeline is structurally unsatisfiable.

Fix: implement the operation's handler (the `handle` body in services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/start_network_migration_analysis.rs) against the service's network-migration state: resolve the definition and execution, refuse an unregistered execution id with ResourceNotFoundException, refuse an execution with an in-flight job (or one not yet past MAPPING) with ConflictException carrying the busy-job message, otherwise record a new analysis job and return its `jobID` with status 202; then delete the SCAFFOLDED marker line at the top of that file.

Traces: mgn.network-migration-mapping-and-analysis-without-execution-rejected (mismatch at check line 84). No existing open issue names this operation — the nearest neighbours, 20261101T185409Z-mgn-deletenetworkmigrationdefinition-stub.md and 20261104T044001Z-mgn-createreplicationconfigurationtemplate-stub.md, cover different stubs.

## Evidence
Expected (AWS / Smithy model + recording): POST /network-migration/StartNetworkMigrationAnalysis with a registered networkMigrationExecutionID whose definition has a mapping job in flight -> HTTP 409, Error.Code "ConflictException", Error.Message "A job cannot be started because another job for this execution is currently in progress." ; with a well-formed but never-staged execution id -> HTTP 404, Error.Code "ResourceNotFoundException", Error.Message "NetworkMigrationExecution with Id: '<networkMigrationExecutionID from the request>' was not found" ; otherwise -> HTTP 202 with a `jobID` matching ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$, and no execution record left behind for a refused id.

Actual (emulator): HTTP 501, Error.Code "NotImplemented", Error.Message "operation StartNetworkMigrationAnalysis is scaffolded but not implemented in this emulator" for every call. Source: services/service-com-amazonaws-mgn-applicationmigrationservice/src/ops/start_network_migration_analysis.rs line 170 returns `Scaffolded::not_implemented("StartNetworkMigrationAnalysis")`; line 4 still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker.

Replay assertion: AssertionError: a real, known execution id in the wrong stage must be rejected with ConflictException (distinct from an unknown id's ResourceNotFoundException), got 'NotImplemented'

## Steps to Reproduce
1. Point an mgn client at the emulator.
2. CreateNetworkMigrationDefinition with a sourceConfigurations entry (sourceEnvironment LOGICAL_MODEL + sourceS3Configuration), a targetS3Configuration and a targetNetwork; keep the returned networkMigrationDefinitionID.
3. ListNetworkMigrationExecutions(networkMigrationDefinitionID=<id>) and take the auto-staged execution's networkMigrationExecutionID (stage MAPPING).
4. StartNetworkMigrationMapping(networkMigrationDefinitionID=<id>, networkMigrationExecutionID=<exec>, securityGroupMappingStrategy="SKIP") -> succeeds with a jobID.
5. StartNetworkMigrationAnalysis(networkMigrationDefinitionID=<id>, networkMigrationExecutionID=<exec>) -> observe HTTP 501 NotImplemented instead of HTTP 409 ConflictException "A job cannot be started because another job for this execution is currently in progress."
6. StartNetworkMigrationAnalysis(networkMigrationDefinitionID=<id>, networkMigrationExecutionID=<a fresh uuid4>) -> observe HTTP 501 NotImplemented instead of HTTP 404 ResourceNotFoundException "NetworkMigrationExecution with Id: '<that uuid>' was not found".
