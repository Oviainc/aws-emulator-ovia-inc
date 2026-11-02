Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ecs.invalid-parameter-task-definition-ops
Issue-Shape-Id: com.amazonaws.ecs#AmazonEC2ContainerServiceV20141113
Issue-Key: ecs.invalid-parameter-task-definition-ops|run#24|AssertionError|assert error == expected, f"DeleteTaskDefinitions: expected {expected!r}, got {error!r}"
Issue-Commit: 1db96a8030ce3dc5faa6b2c3a7e50004c443b0ec

# ECS DeleteTaskDefinitions refuses an over-length taskDefinitions list with ClientException and emulator wording instead of InvalidParameterException 'Size of TaskDefinition references list cannot exceed 10'

> _Note: this issue was identified against the code base at commit `1db96a8030ce3dc5faa6b2c3a7e50004c443b0ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTaskDefinitions

## Description
One net-new root cause in the ECS task-definition error taxonomy. It must be closed before submitting.

Cause 1 of 1: DeleteTaskDefinitions list-size refusal (failing check ecs.invalid-parameter-task-definition-ops).
AWS: DeleteTaskDefinitions accepts at most 10 task-definition references. Smithy (ECS 2014-11-13): DeleteTaskDefinitionsRequest.taskDefinitions -> StringList, required; member documentation 'You can specify up to 10 task definitions as a comma separated list.'; modeled errors AccessDeniedException(403) | ClientException(400) | InvalidParameterException(400) | ServerException(500). A request carrying more than 10 references is refused with HTTP 400, error code InvalidParameterException, message 'Size of TaskDefinition references list cannot exceed 10'. In that message the whole string is fixed text and the limit 10 is the service limit for this member — there are no request-derived variable parts; AWS does not echo the offending list or its length.
Emulator: the call is refused, but with error code ClientException and the emulator-authored message 'task definitions can have at most 10 items.' — a different error class, different wording, and a lower-cased 'task definitions' noun rather than AWS's 'TaskDefinition references list' phrasing.
Why: a client that branches on InvalidParameterException to split an oversized batch into chunks of 10 never takes that branch; it sees ClientException, which for this operation signals a malformed or unauthorized request, so the batch is reported as a hard failure instead of being retried in chunks. A client or operator matching the AWS refusal text does not recognize the message either.
Fix: DeleteTaskDefinitions' input validation in the ECS operation handler — raise the InvalidParameterException error class for the over-length taskDefinitions list and use AWS's message template; the surrounding refusal (400, refuse before deleting anything) is already correct.
Note: the open issue 20261029T220901Z-ecs-task-definition-and-list-operations-drop-previousstatus.md covers other DeleteTaskDefinitions divergences (missing taskDefinitions[].previousStatus, the still-ACTIVE failures[].reason wording) and the maxResults>100 list wording; it does NOT cover this list-size refusal, which the replay reported as unowned.
Traces: ecs.invalid-parameter-task-definition-ops (the check's earlier assertions on RegisterTaskDefinition cpu, DescribeTaskDefinition family characters, DeregisterTaskDefinition 'Revision is missing' and the ListTaskDefinitions/ListTaskDefinitionFamilies maxResults=0 refusals all passed; the check fails only at the DeleteTaskDefinitions assertion).

## Evidence
Behavioral suite simparity-behavioral-aws-v1, check ecs.invalid-parameter-task-definition-ops (verdict: mismatch):

  AssertionError: DeleteTaskDefinitions: expected ('InvalidParameterException', 'Size of TaskDefinition references list cannot exceed 10'), got ('ClientException', 'task definitions can have at most 10 items.')

Expected (AWS): HTTP 400, Error.Code = 'InvalidParameterException', Error.Message = 'Size of TaskDefinition references list cannot exceed 10' (entirely fixed text; 10 is the modeled service limit).
Actual (emulator): HTTP 400, Error.Code = 'ClientException', Error.Message = 'task definitions can have at most 10 items.'

Smithy (ECS 2014-11-13): DeleteTaskDefinitions input member taskDefinitions documented 'You can specify up to 10 task definitions as a comma separated list.'; InvalidParameterException is a modeled error of the operation (httpError 400), so the emulator is choosing the wrong one of two modeled client errors.

## Steps to Reproduce
1. Boot the emulator and point an ECS client at it.
2. Call DeleteTaskDefinitions with 11 task-definition references, e.g. taskDefinitions=['not-a-valid-identifier-0', ..., 'not-a-valid-identifier-10'].
3. Observe the refusal: error code ClientException with message 'task definitions can have at most 10 items.'
4. AWS refuses the same request with error code InvalidParameterException and message 'Size of TaskDefinition references list cannot exceed 10'.
5. Replay check ecs.invalid-parameter-task-definition-ops to confirm the fix (the check also exercises the sibling task-definition refusals as positive/negative controls).
