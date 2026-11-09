Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__fis.deploy-fis-experiment-template-invalid-action-id-rejected-rollback cloudformation__fis.deploy-fis-target-account-configuration-round-trip-and-delete
Issue-Key: cloudformation__fis.deploy-fis-experiment-template-invalid-action-id-rejected-rollback|run#8|AssertionError|assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
Issue-Key: cloudformation__fis|CreateStack|ValidationError
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# cloudformation__fis: 2 net-new causes — the CREATE_FAILED handler reason for a refused AWS::FIS::ExperimentTemplate drops the SDK exception suffix, and AWS::FIS::TargetAccountConfiguration is an unrecognized resource type

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two distinct root causes in how the emulator's CloudFormation handles AWS::FIS::* resources. Both were found by replaying the behavioral suite's `cloudformation__fis` deploy checks.

Cause 1 of 2: the resource-status reason CloudFormation records for a FIS resource handler failure omits the AWS SDK exception suffix.
  AWS: when an AWS::FIS::ExperimentTemplate declares an action whose `actionId` is not in the FIS action catalog, the FIS CreateExperimentTemplate call the resource handler makes is refused with ResourceNotFoundException (Smithy: CreateExperimentTemplate errors include ResourceNotFoundException, httpError 404 — services/service-com-amazonaws-fis-faultinjectionsimulator/smithy-model/com-amazonaws-fis-faultinjectionsimulator.json). CloudFormation then fails the resource and records a ResourceStatusReason of the fixed form `Resource handler returned message: "<sdk exception message>" (RequestToken: <uuid>, HandlerErrorCode: <code>)`, and the `<sdk exception message>` is the full AWS SDK exception string: the service's own message followed by the SDK-appended suffix `(Service: <ServiceName>, Status Code: <http status>, Request ID: <request id>) (SDK Attempt Count: <n>)`. For this refusal the fixed text is `Invalid actionId in action '<action name>'.` plus that suffix, where `<action name>` is the template's action key, `<ServiceName>` is `Fis`, `<http status>` is `404` (the status of the modeled ResourceNotFoundException), and the request id and attempt count are per-call values; HandlerErrorCode is `NotFound`.
  Emulator: the recorded reason parses to message `Invalid actionId in action 'wait'.` and HandlerErrorCode `NotFound` — the service message and handler error code are right, but the whole ` (Service: Fis, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)` suffix is absent. The stack does roll back, so only the reason text diverges.
  Why: a client (or operator, or CDK/IaC tooling) that reads ResourceStatusReason to recover the failing service, the HTTP status and the request id gets none of them — the status code and request id needed to correlate the failure with the FIS call are simply missing, and any parser expecting the SDK exception form fails to match.
  Fix: the CloudFormation resource-handler error-to-ResourceStatusReason rendering (the component that wraps a downstream service refusal into `Resource handler returned message: ...`) must render the downstream error as a full SDK exception string, appending the service name, the refusal's HTTP status and the request id, instead of only the service's message. Note the open issue 20261109T141116Z-cloudformation-scheduler-cloudformation-create-failed-reason-for-a-rejected-aws.md reports the same suffix loss for AWS::Scheduler::Schedule; it does not cover the FIS handler, so this cause is filed separately and may share a fix.
  Traces: cloudformation__fis.deploy-fis-experiment-template-invalid-action-id-rejected-rollback.

Cause 2 of 2: AWS::FIS::TargetAccountConfiguration is not a registered CloudFormation resource type.
  AWS: AWS::FIS::TargetAccountConfiguration is a published CloudFormation resource type backed by the FIS target-account-configuration API (Smithy: CreateTargetAccountConfiguration POST /experimentTemplates/{experimentTemplateId}/targetAccountConfigurations/{accountId}, GetTargetAccountConfiguration, DeleteTargetAccountConfiguration — all modeled in the FIS service). CreateStack accepts a template declaring it, provisions the configuration in FIS, and DeleteStack removes it; a round trip through Get/List on FIS sees the stack-created configuration.
  Emulator: CreateStack is refused outright with HTTP 400, error code `ValidationError` and message `Template format error: Unrecognized resource types: [AWS::FIS::TargetAccountConfiguration]` — the fixed text being `Template format error: Unrecognized resource types: [<types>]` with `<types>` taken from the submitted template. No stack is created, so nothing of the multi-account FIS workflow can be deployed.
  Why: any client deploying a multi-account FIS experiment by template cannot create a stack at all; the whole AWS::FIS::TargetAccountConfiguration surface is unreachable through CloudFormation even though the FIS API that backs it is implemented.
  Fix: register AWS::FIS::TargetAccountConfiguration in CloudFormation's resource-type registry and implement its create/read/delete handler against the FIS target-account-configuration API (the CloudFormation resource-type registry and its FIS resource handlers).
  Traces: cloudformation__fis.deploy-fis-target-account-configuration-round-trip-and-delete.

## Evidence
Cause 1 — check cloudformation__fis.deploy-fis-experiment-template-invalid-action-id-rejected-rollback [mismatch]:
  assert got == _REJECTED, f"handler reason {got!r}, expected {_REJECTED!r}"
  AssertionError: handler reason ("Invalid actionId in action 'wait'.", 'NotFound'),
    expected ("Invalid actionId in action 'wait'. (Service: Fis, Status Code: 404, Request ID: <id>) (SDK Attempt Count: <n>)", 'NotFound')
  The check parses ResourceStatusReason with
    ^Resource handler returned message: "(?P<message>.*)" \(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\)$
  Expected <message>: the FIS message plus the SDK suffix `(Service: Fis, Status Code: 404, Request ID: <request id>) (SDK Attempt Count: <n>)`.
  Actual <message>: the FIS message only. HandlerErrorCode matched ('NotFound'). The request id and attempt count are per-run values and are normalized by the harness; the divergence is the absence of the suffix, not its values.

Cause 2 — check cloudformation__fis.deploy-fis-target-account-configuration-round-trip-and-delete [error]:
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation:
    Template format error: Unrecognized resource types: [AWS::FIS::TargetAccountConfiguration]
  Expected: CreateStack accepted, stack reaching CREATE_COMPLETE, the target account configuration visible through the FIS API, and DeleteStack reaching DELETE_COMPLETE.

## Steps to Reproduce
Cause 1:
1. CreateStack with a template containing an AWS::FIS::ExperimentTemplate whose actions map holds one action (e.g. key 'wait') with an actionId that is not in the FIS action catalog, plus a valid roleArn and stopConditions.
2. Poll DescribeStacks until a terminal status; the stack rolls back as expected.
3. Read the DELETE/CREATE_FAILED event's ResourceStatusReason for that logical resource and parse it as `Resource handler returned message: "<message>" (RequestToken: ..., HandlerErrorCode: ...)`.
4. Observe <message> is `Invalid actionId in action 'wait'.` with no `(Service: Fis, Status Code: 404, Request ID: ...) (SDK Attempt Count: ...)` suffix.

Cause 2:
1. CreateStack with a template declaring an AWS::FIS::ExperimentTemplate and an AWS::FIS::TargetAccountConfiguration for it (accountId + roleArn).
2. Observe CreateStack is refused immediately with ValidationError / `Template format error: Unrecognized resource types: [AWS::FIS::TargetAccountConfiguration]`; no stack is created, so the intended FIS round trip and DeleteStack never run.
