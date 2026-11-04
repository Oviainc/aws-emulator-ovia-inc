Issue-Source: trace-sweep-divergence
Issue-Trace: docdb-instance-not-found-envelope-variants
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: docdb-instance-not-found-envelope-variants
Issue-Shape-Id: com.amazonaws.docdb#AmazonRDSv19
Issue-Key: docdb|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Code
Issue-Key: docdb|ApplyPendingMaintenanceAction|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: docdb|ApplyPendingMaintenanceAction|4xx->4xx|status
Issue-Commit: 30059ba7efbea1c9406793a7f2ccf8806d86986b

# DocDB ApplyPendingMaintenanceAction answers an unknown resource with 400 InvalidParameterValue instead of 404 ResourceNotFoundFault

> _Note: this issue was identified against the code base at commit `30059ba7efbea1c9406793a7f2ccf8806d86986b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ApplyPendingMaintenanceAction

## Description
Cause 1 of 1: ApplyPendingMaintenanceAction against a resource that does not exist is refused with the wrong status, the wrong error code and the wrong message.
  AWS: Smithy (services/service-com-amazonaws-docdb-amazonrdsv19/smithy-model/com-amazonaws-docdb-amazonrdsv19.json) binds ApplyPendingMaintenanceAction (input ApplyPendingMaintenanceActionMessage{ResourceIdentifier, ApplyAction, OptInType, all required}) to exactly three errors: InvalidDBClusterStateFault (400), InvalidDBInstanceStateFault (400, awsQueryError code InvalidDBInstanceState) and ResourceNotFoundFault (httpError 404, awsQueryError code ResourceNotFoundFault, doc "The specified resource ID was not found."). A ResourceIdentifier naming no existing cluster or instance is therefore the ResourceNotFoundFault case: HTTP 404, awsQuery ErrorResponse envelope with Error.Type=Sender and Error.Code=ResourceNotFoundFault. The recorded message is the fixed sentence "Resource not found: " followed by the one variable part, the resource identifier exactly as the request supplied it in ResourceIdentifier (the recording passed an ARN-form identifier and AWS echoed back the bare identifier it resolved from it, not a re-rendered ARN). InvalidParameterValue is not in this operation's modeled error set at all.
  Emulator: answers HTTP 400 with Error.Code=InvalidParameterValue and the emulator-authored message "Unknown resource: " followed by a re-synthesised ARN of the form arn:aws:rds:<region>:<account>:cluster:<identifier>. So the status, the error code and both the fixed text and the variable part of the message all diverge: the fixed text should be "Resource not found: " and the variable part should be the identifier from the request, not an ARN the emulator built from its own region/account state. (Note: the ARN's region and account digits are environment values and are not themselves the finding — the finding is that an ARN appears where the identifier belongs.)
  Why: every AWS SDK and the CLI classify not-found by error code and status. A client that catches ResourceNotFoundFault (or retries/branches on 404) around ApplyPendingMaintenanceAction sees neither: it gets a 400 InvalidParameterValue, which SDKs classify as a malformed-request fault, so the "resource is gone" branch never runs and the call surfaces as a client bug instead. Code-based waiters and idempotent "apply if present" flows break outright, and anything matching the message text fails too.
  Fix: the docdb ApplyPendingMaintenanceAction handler's resource-resolution path — on a ResourceIdentifier that resolves to nothing it must raise the modeled ResourceNotFoundFault (404 / Code ResourceNotFoundFault / Type Sender) with the "Resource not found: <ResourceIdentifier>" message, rather than falling through to the generic InvalidParameterValue refusal. The open issue "DocDB error responses use emulator-authored messages instead of AWS wording" (20261029T193448Z-docdb-error-responses-use-emulator-authored-messages-instead.md) carries sibling message-wording divergences on CreateDBCluster, CreateDBSubnetGroup and the parameter-group operations, but it does not carry this step's replay key and does not cover the status/code divergence here, which is a mis-selected error shape rather than a reworded one.
  Traces: docdb-instance-not-found-envelope-variants#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 99 steps, 83 passed, 16 mismatched, score 83.8%).

Fixture docdb-instance-not-found-envelope-variants, step task#5, operation ApplyPendingMaintenanceAction, verdict=mismatch, owned_by=[] (not covered by any open issue's replay key), stub_path empty.
  expected_status 404, actual_status 400.
  Detail, verbatim:
    "status: expected 404, got 400"
    "$.ErrorResponse.Error.Code: value_mismatch (expected 'ResourceNotFoundFault', got 'InvalidParameterValue')"
    "$.ErrorResponse.Error.Message: value_mismatch (expected 'Resource not found: sim2real-probe-docdb-nocluster-1b7fe834', got 'Unknown resource: arn:aws:rds:us-west-2:000000000000:cluster:sim2real-probe-docdb-nocluster-1b7fe834')"
  divergence_key: service=docdb|ApplyPendingMaintenanceAction|status=404->400|keys=$.ErrorResponse.Error.Code,$.ErrorResponse.Error.Message,status

Message anatomy (what is a requirement vs. a value): fixed text "Resource not found: "; variable part = the resource identifier taken from the request's ResourceIdentifier member. `sim2real-probe-docdb-nocluster-1b7fe834` is that run's generated identifier and is not part of the template. In the emulator's string, "Unknown resource: " is wrong fixed text and the whole `arn:aws:rds:...:cluster:...` rendering is wrong structure (an ARN synthesised from emulator region/account state where the request's identifier belongs).

Model reference: ApplyPendingMaintenanceAction errors = [InvalidDBClusterStateFault (client, 400), InvalidDBInstanceStateFault (client, 400, awsQueryError InvalidDBInstanceState/400), ResourceNotFoundFault (client, httpError 404, awsQueryError code ResourceNotFoundFault / httpResponseCode 404)]. ResourceNotFoundFault shape: structure, errorKind client, httpError 404, member `message` -> ExceptionMessage, doc "The specified resource ID was not found." InvalidParameterValue is not among the operation's modeled errors.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus), or replay fixture docdb-instance-not-found-envelope-variants alone and watch step task#5.
3. Equivalently by hand: POST the awsQuery action ApplyPendingMaintenanceAction to the docdb endpoint with ResourceIdentifier set to a cluster/instance identifier that was never created, ApplyAction and OptInType populated.
4. Observe HTTP 400 with ErrorResponse.Error.Code=InvalidParameterValue and message "Unknown resource: arn:aws:rds:<region>:<account>:cluster:<identifier>".
5. AWS answers HTTP 404 with ErrorResponse.Error.Type=Sender, Error.Code=ResourceNotFoundFault and message "Resource not found: <identifier>" (the identifier as supplied in ResourceIdentifier).
