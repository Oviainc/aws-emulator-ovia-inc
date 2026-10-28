Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.conflict-exception-concurrent-state
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.conflict-exception-concurrent-state|run#39|AssertionError|assert msg == (
Issue-Commit: 5df60a3e41d7e0b7654dd2b65ebe4fbcad6c7940

# logs: DeleteDeliveryDestination's in-use ConflictException message is invented — it names the blocking delivery instead of AWS's fixed text (1 cause)

> _Note: this issue was identified against the code base at commit `5df60a3e41d7e0b7654dd2b65ebe4fbcad6c7940`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteDeliveryDestination

## Description
One net-new root cause.

Cause 1 of 1: DeleteDeliveryDestination refuses correctly but with the wrong ConflictException message.
Failing check: logs.conflict-exception-concurrent-state

AWS: Smithy (services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json) models DeleteDeliveryDestination with `name` (DeliveryDestinationName, len 1..60, pattern ^[\w-]*$) required, and declares ConflictException (client, HTTP 400) among its errors. Its documentation states the rule: "You can't delete a delivery destination if any current deliveries are associated with it. To find whether any deliveries are associated with this delivery destination, use the DescribeDeliveries operation and check the deliveryDestinationArn field in the results." The model does not state the message text, so the requirement is the recorded wording: HTTP 400, Code `ConflictException`, Message exactly
  "Could not delete this Delivery Destination as it is currently in use. Please delete any Deliveries that reference this resource and try again."
This message is entirely FIXED text for this error class — it interpolates nothing from the request or from state: no destination name, no delivery id. AWS deliberately points the caller at DescribeDeliveries rather than naming one blocking delivery.

Emulator: the refusal fires with the right code and status, but with a message of its own construction:
  "The delivery destination <name> cannot be deleted while delivery <id> is associated with it."
In the replay that read 'The delivery destination sim2real-behav-dlvdst-2d2ee761b8df cannot be deleted while delivery 0000000000000000 is associated with it.' — the destination name comes from the request and the id from the first matching delivery in the store; neither belongs in AWS's text. The destination itself is correctly left intact (GetDeliveryDestination still returns it), and the rest of the delivery pipeline this check exercises (PutDeliverySource, PutDeliveryDestination, Put/Get/DeleteDeliveryDestinationPolicy round-trip, idempotent CreateDelivery upsert, UpdateDeliveryConfiguration) all match — the message is the only divergence reached.

Why: a client that matches on the error message to distinguish "destination still in use" from other ConflictException causes (vended-log teardown scripts, CloudFormation custom resources and CDK/Terraform destroy paths that retry only on the in-use text) never matches and either fails the teardown or retries the wrong path. Worse, the emulator's wording promises a single blocking delivery, so a caller that parses the id out of it and deletes just that delivery can still be refused when more than one delivery references the destination — AWS's wording directs the caller to enumerate them all via DescribeDeliveries.

Fix: the DeleteDeliveryDestination handler's refusal path — it should emit the fixed ConflictException text with no interpolation. Note also that the id it interpolates read as all-zeros, which suggests the delivery-id minting the store uses is not the AWS id shape; that is not asserted by this check and is mentioned only so the fixer does not mistake it for part of this contract.

Traces: logs.conflict-exception-concurrent-state (code path: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_delivery_destination.rs:206-221).

## Evidence
Expected (AWS): HTTP 400, Error.Code = "ConflictException", Error.Message = "Could not delete this Delivery Destination as it is currently in use. Please delete any Deliveries that reference this resource and try again." (wholly fixed text — no request or state values interpolated).

Actual (emulator): HTTP 400, Error.Code = "ConflictException" (correct), Error.Message = "The delivery destination {name} cannot be deleted while delivery {delivery-id} is associated with it." Replay output:
  File "check_logs_conflict_exception_concurrent_state.py", line 217, in run
    assert msg == (
  AssertionError: unexpected DeleteDeliveryDestination in-use message: 'The delivery destination sim2real-behav-dlvdst-2d2ee761b8df cannot be deleted while delivery 0000000000000000 is associated with it.'
(The destination name and delivery id in that line are per-run values filling the emulator's template; the defect is the template itself.)

Source: services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/delete_delivery_destination.rs lines 206-221 build the ConflictException with `format!("The delivery destination {} cannot be deleted while delivery {} is associated with it.", input.name, delivery.id)`.
Smithy citation: DeleteDeliveryDestination errors include ConflictException (client, httpError 400); operation doc states the in-use rule and points at DescribeDeliveries.

## Steps to Reproduce
1. logs.CreateLogGroup(logGroupName=<group>); read its logGroupArn via DescribeLogGroups.
2. s3.CreateBucket(<bucket>); logs.PutDeliverySource(name=<src>, resourceArn=arn:aws:s3:::<bucket>, logType=S3_SERVER_ACCESS_LOGS).
3. logs.PutDeliveryDestination(name=<dst>, deliveryDestinationType=CWL, deliveryDestinationConfiguration={destinationResourceArn: <logGroupArn>}).
4. logs.CreateDelivery(deliverySourceName=<src>, deliveryDestinationArn=<dst arn>).
5. logs.DeleteDeliveryDestination(name=<dst>). It is correctly rejected with ConflictException, but the Message is the emulator's own sentence naming <dst> and the delivery id instead of AWS's fixed "Could not delete this Delivery Destination as it is currently in use. Please delete any Deliveries that reference this resource and try again."
6. Re-replay check logs.conflict-exception-concurrent-state.
