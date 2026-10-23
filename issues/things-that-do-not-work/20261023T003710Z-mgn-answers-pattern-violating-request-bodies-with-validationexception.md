Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mgn.create-connector-invalid-ssm-instance-id-pattern-rejected mgn.network-migration-definition-validation-rejected
Issue-Shape-Id: com.amazonaws.mgn#ApplicationMigrationService
Issue-Key: mgn.create-connector-invalid-ssm-instance-id-pattern-rejected|run#9|AssertionError|assert bad_code == "BadRequestException", (
Issue-Key: mgn.network-migration-definition-validation-rejected|run#8|AssertionError|assert code == "BadRequestException", (
Issue-Commit: 0adaad19e8652dfd7295bc52b170efe472707b44

# mgn answers pattern-violating request bodies with ValidationException instead of BadRequestException (2 causes)

> _Note: this issue was identified against the code base at commit `0adaad19e8652dfd7295bc52b170efe472707b44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnector

## Description
Two net-new root causes in the mgn (ApplicationMigrationService) request-validation layer. Both are the same shape of defect — a member whose Smithy @pattern/@length is violated is refused with the wrong error code — but they are reached through two different operations and are listed separately so both must be closed.

Cause 1 of 2: CreateConnector with a malformed ssmInstanceID.
AWS: Smithy (com.amazonaws.mgn#CreateConnector) models ssmInstanceID -> SsmInstanceID, required, @length min 19 max 20, @pattern (^i-[0-9a-zA-Z]{17}$)|(^mi-[0-9a-zA-Z]{17}$); the operation's modeled errors are only UninitializedAccountException (400) and ValidationException (400) — there is NO BadRequestException shape in the model (smithy_query shape=BadRequestException -> 'not found'). Recorded behavior only: real MGN rejects a request body that violates these field patterns at its REST front end with HTTP 400 and error code BadRequestException, message 'Invalid request body' (message quoted as the example the recording carries; the requirement is the 400 status and the BadRequestException code). A structurally valid but nonexistent id instead clears field validation and is refused one step later by the account gate (UninitializedAccountException), which is what makes the two codes discriminating.
Emulator: the call is refused with error code ValidationException. Replay detail: "CreateConnector with a malformed ssmInstanceID must raise BadRequestException, got 'ValidationException'". No connector is created, so only the classification is wrong.
Why: a client that branches on the error code cannot tell a malformed-field rejection from MGN's other 400s. Code written against AWS catches BadRequestException to report 'bad input' and retries/initializes on ValidationException/UninitializedAccountException; against the emulator the malformed-input branch is never taken, and field-level rejections are indistinguishable from account-state rejections.
Fix: the mgn constraint-validation / error-serializer path that refuses modeled @pattern and @length violations before the operation handler runs (it currently emits the modeled ValidationException for every constraint failure). It must emit MGN's front-end BadRequestException (HTTP 400, x-amzn-errortype: BadRequestException) for a request body that fails member pattern/length validation, while leaving the handler-raised ValidationException/UninitializedAccountException paths alone.
Traces: mgn.create-connector-invalid-ssm-instance-id-pattern-rejected.

Cause 2 of 2: CreateNetworkMigrationDefinition with a malformed targetNetwork CIDR.
AWS: Smithy (com.amazonaws.mgn#CreateNetworkMigrationDefinition) requires targetNetwork -> TargetNetwork, whose inboundCidr/outboundCidr/inspectionCidr target Cidr with @length min 9 max 18 and @pattern ^((25[0-4]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\.){3}(25[0-5]|2[0-4][0-9]|1[0-9]{2}|[1-9]?[0-9])\/(1[6-9]|2[0-8])$ (dotted quad with /16../28). Modeled errors are ServiceQuotaExceededException (402) and ValidationException (400). Recorded behavior only: real MGN refuses inboundCidr='not-a-cidr' with HTTP 400 and error code BadRequestException, message 'Invalid request body' (message is the recorded example), and creates no definition.
Emulator: refused with error code ValidationException. Replay detail: "CreateNetworkMigrationDefinition with a malformed inboundCidr must raise BadRequestException, got 'ValidationException'". Note this rejection happens in the shared validation layer ahead of the handler, so it is NOT the same thing as the separately filed CreateNetworkMigrationDefinition 501 scaffold stub — the stub issue covers the well-formed/positive path, this cause covers the refusal code.
Why: same client impact as cause 1 — the documented field boundary of targetNetwork is reported under the wrong code, so a client's malformed-CIDR handling never runs and a validation failure looks like a generic service validation error.
Fix: same component as cause 1 (mgn constraint-validation/error-serializer for nested member pattern violations, here on a nested structure member rather than a top-level one — confirm nested shapes take the same path).
Traces: mgn.network-migration-definition-validation-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service mgn (0/20 passed).

[mismatch] mgn.create-connector-invalid-ssm-instance-id-pattern-rejected
  AssertionError: CreateConnector with a malformed ssmInstanceID must raise BadRequestException, got 'ValidationException'
  Expected (real AWS): HTTP 400, error code BadRequestException, message 'Invalid request body' (message = example), connector not created.
  Actual (emulator):   error code ValidationException.
  Request: mgn.create_connector(name=<unique>, ssmInstanceID='not-a-valid-ssm-instance-id')

[mismatch] mgn.network-migration-definition-validation-rejected
  AssertionError: CreateNetworkMigrationDefinition with a malformed inboundCidr must raise BadRequestException, got 'ValidationException'
  Expected (real AWS): HTTP 400, error code BadRequestException, message 'Invalid request body' (message = example), no definition created.
  Actual (emulator):   error code ValidationException.
  Request: mgn.create_network_migration_definition(..., targetNetwork={'topology':'ISOLATED_VPC','inboundCidr':'not-a-cidr','outboundCidr':'10.0.1.0/24','inspectionCidr':'10.0.2.0/24'})

Model support (services/service-com-amazonaws-mgn-applicationmigrationservice/smithy-model/com-amazonaws-mgn-applicationmigrationservice.json):
  SsmInstanceID: @length 19..20, @pattern (^i-[0-9a-zA-Z]{17}$)|(^mi-[0-9a-zA-Z]{17}$)
  Cidr (TargetNetwork.inboundCidr): @length 9..18, @pattern ^((25[0-4]|...)\.){3}(...)\/(1[6-9]|2[0-8])$
  CreateConnector errors: UninitializedAccountException(400), ValidationException(400)
  CreateNetworkMigrationDefinition errors: ServiceQuotaExceededException(402), ValidationException(400)
  BadRequestException: shape not present in the model — it is the REST front-end code the recordings show for a malformed request body.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo (branch mainline) and boot the emulator built from that checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1, or just the two checks below.
3. mgn.create-connector-invalid-ssm-instance-id-pattern-rejected: call CreateConnector with ssmInstanceID='not-a-valid-ssm-instance-id'. Observe the refusal carries error code ValidationException; real AWS answers HTTP 400 BadRequestException.
4. mgn.network-migration-definition-validation-rejected: call CreateNetworkMigrationDefinition with targetNetwork.inboundCidr='not-a-cidr'. Observe the refusal carries error code ValidationException; real AWS answers HTTP 400 BadRequestException.
5. Re-replay mgn.create-connector-invalid-ssm-instance-id-pattern-rejected and mgn.network-migration-definition-validation-rejected after the fix; BOTH must pass before closing.
