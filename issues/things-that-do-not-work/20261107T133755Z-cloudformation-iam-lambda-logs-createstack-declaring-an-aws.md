Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected
Issue-Key: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected|run#29|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Commit: d9e6cac456a6ddf38a11f2ddf4f109e13159427a

# cloudformation__iam__lambda__logs: CreateStack declaring an AWS::Logs::LogGroup whose name already exists reaches CREATE_COMPLETE instead of failing NAME_CONFLICT_VALIDATION and rolling back

> _Note: this issue was identified against the code base at commit `d9e6cac456a6ddf38a11f2ddf4f109e13159427a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation runs no name-conflict pre-deployment validation for AWS::Logs::LogGroup. A stack that declares a log group at a name that already exists in CloudWatch Logs (here /aws/lambda/<function>, created implicitly by an earlier function's first invoke and deliberately left behind) is created to completion, silently adopting or ignoring the pre-existing group, where AWS refuses the whole stack before provisioning anything.

AWS: the collision is caught by CloudFormation's pre-deployment validation, so nothing in the template is ever provisioned: the stack emits exactly one stack-level CREATE_FAILED event whose DetailedStatus is "VALIDATION_FAILED", no resource ever enters CREATE_IN_PROGRESS, and the stack settles in ROLLBACK_COMPLETE. The operation event for the offending resource is an EventType "VALIDATION_ERROR" with ResourceType "AWS::Logs::LogGroup", ValidationStatus "FAILED", ValidationName "NAME_CONFLICT_VALIDATION", ValidationPath "/Resources/<logical id of the LogGroup>" and ValidationStatusReason whose fixed text is "Resource of type 'AWS::Logs::LogGroup' with identifier '" … "' already exists." — the single variable part is the identifier, i.e. the colliding log-group name taken from the template's LogGroupName (here /aws/lambda/<function name>, a per-run value, not part of the requirement). No other resource is flagged. Because nothing was provisioned, the function name in the template does not exist afterwards: lambda GetFunction is refused ResourceNotFoundException with message "Function not found: arn:aws:lambda:<region>:<account>:function:<name>". The pre-existing group is untouched — same arn and creationTime, still no retentionInDays, its existing stream and events intact — i.e. the failed stack must not apply the template's RetentionInDays 3 to a group it never created. Smithy only states the underlying refusal class for the service itself (service-com-amazonaws-cloudwatchlogs-logs-20140328: CreateLogGroup models ResourceAlreadyExistsException, httpError 400); the CloudFormation validation surface above is recorded behavior only, as the recording shows it, and the resource reference is https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/aws-resource-logs-loggroup.html.
Emulator: the colliding stack ends in CREATE_COMPLETE and reports no failed events at all (replay detail: "AssertionError: colliding stack ended in 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE': []"). There is no stack-level CREATE_FAILED, no DetailedStatus VALIDATION_FAILED, no VALIDATION_ERROR operation event, no NAME_CONFLICT_VALIDATION, and the stack's resources (the IAM role and the Lambda function) are provisioned instead of being left untouched.
Why: a client is told a deploy succeeded where AWS refuses it, and the stack now believes it owns a log group it did not create. Stack delete will then delete (or retention-mutate) a group that belongs to someone else, destroying pre-existing log data; conversely a client that relies on the refusal to detect the implicit-group trap — the documented hazard of declaring /aws/lambda/<fn> after a function has already created it — gets no signal, and no error code or validation event to branch on.
Fix: CloudFormation's pre-deployment validation pass on the create-stack path — before any resource is provisioned it must check declared physical names of name-conflict-sensitive resource types (AWS::Logs::LogGroup here) against the owning service's state, emit the VALIDATION_ERROR operation event and the stack-level CREATE_FAILED/VALIDATION_FAILED event, and take the stack to ROLLBACK_COMPLETE with no resource touched. Related but NOT covering this check: 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md (CREATE_COMPLETE for logs resources never created) and 20261107T031624Z-cloudformation-iam-lambda-logs-a-lambda-function-s.md (the implicit /aws/lambda/<fn> group); neither carries this replay key.
Traces: cloudformation__iam__lambda__logs.deploy-logs-2-declared-log-group-collides-with-implicit-rejected (mismatch at the colliding stack's terminal status, after the pre-step stack, its implicit group and that group's events had all been verified).

## Evidence
Expected (AWS), for a CreateStack of {AWS::Logs::LogGroup LogGroupName=/aws/lambda/<fn> RetentionInDays=3, AWS::IAM::Role, AWS::Lambda::Function FunctionName=<fn> DependsOn LogGroup} while /aws/lambda/<fn> already exists:
  - terminal stack status ROLLBACK_COMPLETE;
  - exactly one stack-level CREATE_FAILED event with DetailedStatus "VALIDATION_FAILED";
  - no events for any other LogicalResourceId (nothing provisioned);
  - one operation event EventType "VALIDATION_ERROR", LogicalResourceId "LogGroup", ResourceType "AWS::Logs::LogGroup", ValidationStatus "FAILED", ValidationName "NAME_CONFLICT_VALIDATION", ValidationPath "/Resources/LogGroup", ValidationStatusReason "Resource of type 'AWS::Logs::LogGroup' with identifier '<the colliding group name>' already exists.";
  - lambda GetFunction(<fn>) refused ResourceNotFoundException, message "Function not found: arn:aws:lambda:<region>:<account>:function:<fn>";
  - the pre-existing group unchanged: same arn and creationTime, retentionInDays absent, its earlier stream messages still readable.
Actual (emulator): terminal status CREATE_COMPLETE and an empty failed-event list — replay detail: "AssertionError: colliding stack ended in 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE': []" (check line 196). None of the validation events above exist, and the stack's role and function were provisioned.
Supporting, from the same run: the pre-step stack (Lambda + role only) reached CREATE_COMPLETE, its first invoke created /aws/lambda/<fn> implicitly, that group was readable with retentionInDays absent and kept its stream after the pre-step stack was deleted — so the collision premise genuinely held when the second stack was created.

## Steps to Reproduce
1. CreateStack A from a template containing only an AWS::IAM::Role (AWSLambdaBasicExecutionRole) and an AWS::Lambda::Function named <fn>; wait for CREATE_COMPLETE.
2. Invoke <fn> once so that /aws/lambda/<fn> is created implicitly, and confirm via logs DescribeLogGroups that the group exists with no retentionInDays.
3. DeleteStack A and wait: the function goes away, the implicit log group stays behind.
4. CreateStack B from a template declaring AWS::Logs::LogGroup{LogGroupName: /aws/lambda/<fn>, RetentionInDays: 3}, the role, and the function named <fn> with DependsOn the log group; wait for a terminal status.
   Expected: ROLLBACK_COMPLETE; one stack-level CREATE_FAILED with DetailedStatus VALIDATION_FAILED; one VALIDATION_ERROR operation event with ValidationName NAME_CONFLICT_VALIDATION, ValidationPath /Resources/LogGroup and ValidationStatusReason "Resource of type 'AWS::Logs::LogGroup' with identifier '/aws/lambda/<fn>' already exists."; no resource events; GetFunction(<fn>) refused ResourceNotFoundException; the pre-existing group untouched (no retentionInDays, same arn/creationTime, events intact).
   Actual: CREATE_COMPLETE with no failed events, and the stack's resources provisioned.
