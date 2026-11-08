Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ivschat.create-logging-configuration-invalid-s3-destination-not-found ivschat.delete-logging-configuration-lifecycle ivschat.delete-logging-configuration-conflict-while-attached-rejected
Issue-Shape-Id: com.amazonaws.ivschat#AmazonInteractiveVideoServiceChat
Issue-Key: ivschat.create-logging-configuration-invalid-s3-destination-not-found|_error_code#17|AssertionError|raise AssertionError(f"expected a ClientError but the call succeeded: {result!r}")
Issue-Key: ivschat.delete-logging-configuration-lifecycle|run#12|AssertionError|assert final_message == f"Resource: {arn} not found", f"unexpected message: {final_message!r}"
Issue-Key: ivschat.delete-logging-configuration-conflict-while-attached-rejected|run#16|AssertionError|assert message == (
Issue-Commit: 1d15d4851343e3a31be69cd8cea36918aab666fe

# ivschat logging-configuration fidelity: missing S3 destination validation on CreateLoggingConfiguration, plus wrong ResourceNotFoundException and ConflictException message rules

> _Note: this issue was identified against the code base at commit `1d15d4851343e3a31be69cd8cea36918aab666fe`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLoggingConfiguration

## Description
Three distinct root causes in the ivschat logging-configuration surface, all reached through the emulator's own responses during behavioral replay. No existing open issue mentions ivschat (list_existing_issues query='ivschat' returns none).

Cause 1 of 3: CreateLoggingConfiguration accepts an S3 destination bucket that does not exist.
AWS: CreateLoggingConfiguration models ResourceNotFoundException (httpError 404) among its errors (Smithy: services/service-com-amazonaws-ivschat-amazoninteractivevideoservicechat -- CreateLoggingConfiguration errors = AccessDeniedException 403, ConflictException 409, PendingVerification 403, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ValidationException 400; destinationConfiguration is @required). Recorded behavior: pointing destinationConfiguration.s3.bucketName at a bucket that does not exist is rejected with HTTP 404 / error code ResourceNotFoundException and NO logging configuration is created (ListLoggingConfigurations does not contain the requested name). The recorded message is, as an example, "Resource: <bucketName> not found" (same template as Cause 2).
Emulator: returns HTTP 200 with a fully-formed logging configuration (arn, id, createTime, state=ACTIVE) for a nonexistent bucket, and the configuration is persisted. The replay shows the call succeeding where a ClientError was required.
Why: an overly permissive create. A client that relies on AWS validating the destination at create time gets a configuration that looks ACTIVE but can never deliver logs; negative-path code (error handling, rollback, retry with a corrected bucket) is never exercised, and the emulator additionally leaks a resource AWS never creates.
Fix: the CreateLoggingConfiguration handler's validation in the ivschat service crate -- the destinationConfiguration branch must resolve the referenced S3 bucket (and, symmetrically, the cloudWatchLogs log group / firehose stream) against the emulator's cross-service state and raise ResourceNotFoundException 404 before any state is written.
Traces: ivschat.create-logging-configuration-invalid-s3-destination-not-found

Cause 2 of 3: ResourceNotFoundException for a logging configuration uses a non-AWS message template.
AWS: recorded behavior only -- the modeled shape carries only the error kind/status (ResourceNotFoundException, httpError 404). The recordings show the message is built deterministically from the request identifier as "Resource: <identifier> not found"; that request-derived template is the requirement, not the recorded literal. This applies to GetLoggingConfiguration, UpdateLoggingConfiguration and a repeat DeleteLoggingConfiguration against a deleted identifier (all three model ResourceNotFoundException 404).
Emulator: returns the correct code/status but the message 'Logging configuration not found for identifier arn:aws:ivschat:us-west-2:000000000000:logging-configuration/032oOeu717rL.' instead of the "Resource: <identifier> not found" form.
Why: clients and test suites that match on the not-found message (or parse the identifier out of it) diverge; the error text is the only part of a 404 that identifies the subject resource.
Fix: the shared ivschat error construction for ResourceNotFoundException (the not-found path of the GetLoggingConfiguration / UpdateLoggingConfiguration / DeleteLoggingConfiguration handlers) must format the message from the request identifier using AWS's template.
Traces: ivschat.delete-logging-configuration-lifecycle

Cause 3 of 3: ConflictException for deleting a logging configuration still attached to a room uses a non-AWS message template.
AWS: recorded behavior only for the text -- DeleteLoggingConfiguration models ConflictException with httpError 409 (Smithy, as above), and the emulator already returns that code correctly. The recording shows the message is built deterministically from the operation name and the request identifier: "Unable to perform ivschat:DeleteLoggingConfiguration while <identifier> is still attached to a chat room". That request-derived template is the requirement; the recorded literal is an example.
Emulator: message 'logging configuration arn:aws:ivschat:us-west-2:000000000000:logging-configuration/032oOeu717rL is attached to 1 room(s) and cannot be deleted.' -- a different template that also exposes an attachment count AWS does not report.
Why: a client that distinguishes conflict causes by message text, or that surfaces the AWS wording, sees text AWS never emits. Status (409) and code (ConflictException) are correct, so this is message-rule-only, hence MEDIUM.
Fix: the conflict path of the DeleteLoggingConfiguration handler in the ivschat service crate -- format the ConflictException message as "Unable to perform ivschat:DeleteLoggingConfiguration while <identifier> is still attached to a chat room". The rest of this check's behavior (409, config left ACTIVE and gettable, delete succeeding after detach) already matches.
Traces: ivschat.delete-logging-configuration-conflict-while-attached-rejected

## Evidence
Cause 1 -- ivschat.create-logging-configuration-invalid-s3-destination-not-found:
  Expected: ClientError, HTTP 404, Error.Code = "ResourceNotFoundException" (message example: "Resource: sim2real-ivschat-nobucketba9bd3e4c5b2 not found"), and no logging configuration left behind.
  Actual:   HTTP 200 with {'arn': 'arn:aws:ivschat:us-west-2:000000000000:logging-configuration/02fMvZLCBbBp', 'id': '02fMvZLCBbBp', 'name': 'sim2real-ivschat-badlogcfg...', 'destinationConfiguration': {'s3': {'bucketName': 'sim2real-ivschat-nobucket...'}}, 'state': 'ACTIVE', 'tags': {}}.
  Harness: AssertionError: expected a ClientError but the call succeeded. A positive control against a real existing bucket succeeded in the same check, so the operation is otherwise healthy.

Cause 2 -- ivschat.delete-logging-configuration-lifecycle:
  Expected: GetLoggingConfiguration on a deleted identifier -> ResourceNotFoundException with message "Resource: <arn> not found".
  Actual:   code ResourceNotFoundException (correct) with message 'Logging configuration not found for identifier arn:aws:ivschat:us-west-2:000000000000:logging-configuration/032oOeu717rL.'
  Harness: AssertionError: unexpected message: 'Logging configuration not found for identifier ...'

Cause 3 -- ivschat.delete-logging-configuration-conflict-while-attached-rejected:
  Expected: DeleteLoggingConfiguration on an attached config -> ConflictException (409) with message "Unable to perform ivschat:DeleteLoggingConfiguration while <arn> is still attached to a chat room".
  Actual:   ConflictException (correct) with message 'logging configuration arn:aws:ivschat:us-west-2:000000000000:logging-configuration/032oOeu717rL is attached to 1 room(s) and cannot be deleted.'
  Harness: AssertionError: unexpected message: 'logging configuration ... is attached to 1 room(s) and cannot be deleted.'

## Steps to Reproduce
Cause 1:
1. s3.create_bucket(Bucket=<good-bucket>) (with CreateBucketConfiguration.LocationConstraint outside us-east-1).
2. ivschat.create_logging_configuration(name=<good>, destinationConfiguration={'s3': {'bucketName': <good-bucket>}}) -> succeeds (positive control).
3. ivschat.create_logging_configuration(name=<bad>, destinationConfiguration={'s3': {'bucketName': <bucket-that-was-never-created>}}).
   Expect HTTP 404 ResourceNotFoundException; emulator returns HTTP 200 and a persisted ACTIVE configuration.
4. ivschat.list_logging_configurations() -> the <bad> name must be absent; in the emulator it is present.

Cause 2:
1. logs.create_log_group(logGroupName=<lg>).
2. ivschat.create_logging_configuration(name=<n>, destinationConfiguration={'cloudWatchLogs': {'logGroupName': <lg>}}); poll until state == ACTIVE.
3. ivschat.delete_logging_configuration(identifier=<arn>).
4. ivschat.get_logging_configuration(identifier=<arn>) -> ResourceNotFoundException; inspect Error.Message: must be "Resource: <arn> not found". Same for update_logging_configuration and a second delete_logging_configuration against <arn>.

Cause 3:
1. Create a cloudWatchLogs-destination logging configuration and wait for ACTIVE.
2. ivschat.create_room(name=<r>, loggingConfigurationIdentifiers=[<cfg-arn>]).
3. ivschat.delete_logging_configuration(identifier=<cfg-arn>) -> ConflictException; inspect Error.Message: must be "Unable to perform ivschat:DeleteLoggingConfiguration while <cfg-arn> is still attached to a chat room".
4. ivschat.update_room(identifier=<room-arn>, loggingConfigurationIdentifiers=[]) then retry the delete -> succeeds (control, already passing).
