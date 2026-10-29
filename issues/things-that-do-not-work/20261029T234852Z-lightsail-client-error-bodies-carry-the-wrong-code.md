Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-alarm-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-alarm-error-paths lightsail-cloudformation-and-copy-error-paths
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|PutAlarm|4xx->4xx|$.code
Issue-Key: lightsail|PutAlarm|4xx->4xx|$.message
Issue-Key: lightsail|DeleteAlarm|4xx->4xx|$.code
Issue-Key: lightsail|DeleteAlarm|4xx->4xx|$.message
Issue-Key: lightsail|CreateCloudFormationStack|4xx->4xx|$.code
Issue-Key: lightsail|CreateCloudFormationStack|4xx->4xx|$.message
Issue-Commit: c1b8e21acf710dd592fea1840f63adefca5c3a99

# Lightsail client-error bodies carry the wrong code and non-AWS message text

> _Note: this issue was identified against the code base at commit `c1b8e21acf710dd592fea1840f63adefca5c3a99`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutAlarm

## Description
Three net-new root causes, all one family: the Lightsail error bodies the emulator serves on its client-error paths do not carry AWS's `code` discriminator or AWS's message wording, even where the HTTP status already matches. Lightsail is an awsJson1_1 service whose error structures (NotFoundException, InvalidInputException) each model members `code`, `docs`, `message`, `tip`, so `code` and `message` ARE the wire contract a client branches on, not cosmetic prose. Each cause must be closed separately.

Cause 1 of 3: PutAlarm with a monitoredResourceName that names no monitorable resource answers the wrong error code and a hand-written message.
AWS: Smithy (com.amazonaws.lightsail#PutAlarm) models errors NotFoundException and InvalidInputException; NotFoundException is documented "Lightsail throws this exception when it cannot find a resource" and carries members code/docs/message/tip. Recorded behavior shows the not-found refusal served as HTTP 400 with body `code` = "NotFoundException" and `message` = "The specified resource <monitoredResourceName> does not exist within types [Instance, LoadBalancer, RelationalDatabase, Distribution, Bucket]. Please provide a valid resource name." Fixed text: "The specified resource " ... " does not exist within types [Instance, LoadBalancer, RelationalDatabase, Distribution, Bucket]. Please provide a valid resource name."; the only variable part is the monitoredResourceName copied from the request. The bracketed type list is fixed text, and it names FIVE monitorable types.
Emulator: same status (400) but `code` = "DoesNotExist" and `message` = "monitoredResourceName '<name>' does not name an Instance, LoadBalancer or RelationalDatabase of this account" — a code AWS never emits, and a message that both reads differently and advertises only three of the five types.
Why: a client that catches `lightsail.exceptions.NotFoundException` (botocore dispatches on the modeled code) or that tests `Error.Code == "NotFoundException"` never matches, so a recoverable not-found is re-raised as an unknown error and retry/cleanup paths break. The truncated type list also misinforms callers about which resources an alarm may monitor.
Fix: the PutAlarm handler's monitored-resource resolution error path — raise the modeled NotFoundException with AWS's message template instead of a locally invented `DoesNotExist` code. Related open issue (different cause, same operation, not a duplicate): "No alarm can be created on a load balancer" (20261028T151124Z-lightsail-putalarm-wrong-behavior.md) — that one is about which resource types resolve at all; this one is about the error body served when none resolves.
Traces: lightsail-alarm-error-paths#task#1.

Cause 2 of 3: DeleteAlarm for an alarm name that does not exist answers the wrong error code and message.
AWS: Smithy (com.amazonaws.lightsail#DeleteAlarm) — input alarmName (ResourceName, required, pattern ^\w[\w\-]*\w$); modeled errors include NotFoundException. Recorded behavior: HTTP 400 with body `code` = "NotFoundException" and `message` = "The specified resource <alarmName> does not exist within types [Alarm]. Please provide a valid resource name." — the same fixed template as cause 1 with the type list "[Alarm]", the alarm name taken from the request.
Emulator: `code` = "DoesNotExist", `message` = "The Alarm does not exist: <alarmName>" — a different code and a different message shape entirely.
Why: idempotent teardown (delete, ignore not-found) is the normal use of DeleteAlarm; with the wrong code the client cannot recognise the condition it is meant to ignore, and log/alert matching on AWS's message template fails.
Fix: the DeleteAlarm handler's lookup-miss path — emit NotFoundException with the shared "The specified resource ... does not exist within types [...]" template that PutAlarm must also use; the template belongs in one place in the crate's error construction, not per handler.
Traces: lightsail-alarm-error-paths#task#3.

Cause 3 of 3: CreateCloudFormationStack rejects an invalid instance type with NO `code` member at all, plus non-AWS message text.
AWS: Smithy (com.amazonaws.lightsail#CreateCloudFormationStack) — input `instances` (InstanceEntryList, required); modeled errors include InvalidInputException (HTTP 400), documented "Lightsail throws this exception when user input does not conform to the validation rules of an input field", with members code/docs/message/tip. Recorded behavior: an entry whose instanceType is a Lightsail bundle id rather than an EC2 instance type is refused with HTTP 400 and body `code` = "InvalidParams", `message` = "Sorry, the input instance type <instanceType> is invalid. See the Lightsail API documentation for examples of valid instance types." Fixed text: "Sorry, the input instance type " ... " is invalid. See the Lightsail API documentation for examples of valid instance types."; the variable part is the rejected instanceType from the request.
Emulator: status 400 is right, but the response body contains NO `code` member (the field is absent, so a client reading it gets null), and `message` = "instances[].instanceType '<value>' is not an Amazon EC2 instance type: it is an instance family and a size separated by a dot (t2.micro)".
Why: Lightsail's `code` is the sub-classification clients and the console use to tell an input-validation refusal from the other 400s this operation can return (OperationFailureException is also 400). With `code` missing, every 400 from this operation is indistinguishable, and message-based detection also fails.
Fix: the CreateCloudFormationStack input validation path and the serializer for Lightsail error structures — always populate `code` on the error body, and use AWS's wording for the invalid-instance-type refusal. Related open issue (different cause, same operation, not a duplicate): 20261029T101758Z-lightsail-createcloudformationstack-validation.md (accepting a DISK export-snapshot record).
Traces: lightsail-cloudformation-and-copy-error-paths#task#2.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from this checkout (score 44.1%, 18 mismatched steps).

Cause 1 — lightsail-alarm-error-paths#task#1, PutAlarm, verdict mismatch, status 400 -> 400:
  $.code: expected 'NotFoundException', got 'DoesNotExist'
  $.message: expected 'The specified resource <resourceName> does not exist within types [Instance, LoadBalancer, RelationalDatabase, Distribution, Bucket]. Please provide a valid resource name.', got "monitoredResourceName '<resourceName>' does not name an Instance, LoadBalancer or RelationalDatabase of this account"

Cause 2 — lightsail-alarm-error-paths#task#3, DeleteAlarm, verdict mismatch, status 400 -> 400:
  $.code: expected 'NotFoundException', got 'DoesNotExist'
  $.message: expected 'The specified resource <alarmName> does not exist within types [Alarm]. Please provide a valid resource name.', got 'The Alarm does not exist: <alarmName>'

Cause 3 — lightsail-cloudformation-and-copy-error-paths#task#2, CreateCloudFormationStack, verdict mismatch, status 400 -> 400:
  $.code: expected 'InvalidParams', got None (member absent from the body)
  $.message: expected 'Sorry, the input instance type <instanceType> is invalid. See the Lightsail API documentation for examples of valid instance types.', got "instances[].instanceType '<instanceType>' is not an Amazon EC2 instance type: it is an instance family and a size separated by a dot (t2.micro)"

(Resource names, alarm names and the rejected instance type above are request-supplied values; the template around them is the requirement.)

## Steps to Reproduce
Boot the emulator and point a botocore lightsail client at its data listener in the emulator's region.

Cause 1: ls.put_alarm(alarmName="a1", metricName="CPUUtilization", monitoredResourceName="<name-of-no-existing-resource>", comparisonOperator="GreaterThanThreshold", threshold=80.0, evaluationPeriods=1). Expect HTTP 400 with body code="NotFoundException" and message "The specified resource <name> does not exist within types [Instance, LoadBalancer, RelationalDatabase, Distribution, Bucket]. Please provide a valid resource name.". Observed code "DoesNotExist" and a different message.

Cause 2: ls.delete_alarm(alarmName="<never-created-alarm>"). Expect HTTP 400 with code="NotFoundException" and message "The specified resource <alarmName> does not exist within types [Alarm]. Please provide a valid resource name.". Observed code "DoesNotExist", message "The Alarm does not exist: <alarmName>".

Cause 3: ls.create_cloud_formation_stack(instances=[{"sourceName": "<export-snapshot-source>", "instanceType": "nano_2_0", "portInfoSource": "INSTANCE", "availabilityZone": "us-west-2a"}]). Expect HTTP 400 with code="InvalidParams" and message "Sorry, the input instance type nano_2_0 is invalid. See the Lightsail API documentation for examples of valid instance types.". Observed a 400 whose body has no code member and a different message.

Re-replay: lightsail-alarm-error-paths#task#1, lightsail-alarm-error-paths#task#3, lightsail-cloudformation-and-copy-error-paths#task#2.
