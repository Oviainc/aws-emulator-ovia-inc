Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-allocate-hosts-invalid-instance-type-error
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-allocate-hosts-invalid-instance-type-error ec2-allocate-hosts-invalid-zone-error ec2-local-gateway-route-create-search-delete ec2-flow-logs-and-batch-delete-partial-failures
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AllocateHosts|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|SearchLocalGatewayRoutes|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteLocalGatewayRoute|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteQueuedReservedInstances|2xx->2xx|$.DeleteQueuedReservedInstancesResponse.failedQueuedPurchaseDeletionSet.item.error.message
Issue-Related: 20261029T200817Z-ec2-parameter-limit-refusals-report-emulator-authored-limits.md
Issue-Commit: 78d5907a1724d3304a3021142ffd4f12ae7136a6

# EC2 host, local-gateway-route and queued-reserved-instance refusals carry emulator-authored codes and messages

> _Note: this issue was identified against the code base at commit `78d5907a1724d3304a3021142ffd4f12ae7136a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AllocateHosts

## Description
Four net-new wire divergences share one root cause: these refusal paths are reached at the right moment and with the right HTTP status, but the error Code and/or Message are the emulator's own prose instead of AWS's wording. This issue bundles 4 causes; all 4 must be closed. The open issues '20261029T215142Z-ec2-refusal-messages-reword-aws-text-across-eight.md' and '20261029T200817Z-ec2-parameter-limit-refusals-report-emulator-authored-limits.md' cover the same family on other operations but do not carry these replay keys.

Cause 1 of 4: AllocateHosts with an instance type EC2 does not offer.
AWS: recorded behavior only (AllocateHosts.errors == [] in the EC2 Smithy model; InstanceType is a plain String member). The recording shows HTTP 400, Code 'InvalidHostConfiguration', Message "The instance type '<instanceType>' is not supported." Fixed text "The instance type '" ... "' is not supported."; the value is the InstanceType from the request (recorded run: sim2real-bogus.type). The same cluster covers the invalid-availability-zone variant of the same handler.
Emulator: HTTP 400 but Code 'InvalidParameterValue' and Message "Invalid value '<instanceType>' for instanceType. EC2 offers no such instance type" - a different error class and emulator-authored text.
Why: clients branch on the error code; InvalidHostConfiguration means 'this dedicated-host configuration is impossible' while InvalidParameterValue means 'retry with a different parameter', so retry and reporting logic takes the wrong branch.
Fix: the AllocateHosts handler's validation/error-mapping component.
Traces: ec2-allocate-hosts-invalid-instance-type-error#task#1 (same cluster: ec2-allocate-hosts-invalid-zone-error#task#1).

Cause 2 of 4: SearchLocalGatewayRoutes with a malformed local-gateway route-table id.
AWS: recorded behavior only - HTTP 400 whose Message is "The local-gateway-route-table ID '<id>' is malformed". Fixed text "The local-gateway-route-table ID '" ... "' is malformed"; the value is the LocalGatewayRouteTableId from the request (recorded run: lgwrtb-00000000000000000).
Emulator: Message 'Invalid LocalGateway Route Table id <id>' - emulator-authored phrasing that drops AWS's quoting and the word malformed.
Why: message text is part of the contract clients log, match and assert on; a reworded message breaks those assertions and hides which id EC2 considered malformed.
Fix: the SearchLocalGatewayRoutes id-validation message, which should use the shared '<resource> ID '<id>' is malformed' template.
Traces: ec2-local-gateway-route-create-search-delete#task#1.

Cause 3 of 4: DeleteLocalGatewayRoute with a malformed local-gateway route-table id - the same reworded message from a second handler.
AWS: recorded behavior only - HTTP 400, Message "The local-gateway-route-table ID '<id>' is malformed" (same template as cause 2).
Emulator: 'Invalid LocalGateway Route Table id <id>'.
Why: as cause 2.
Fix: the DeleteLocalGatewayRoute id-validation message / the shared local-gateway id validator both handlers call.
Traces: ec2-local-gateway-route-create-search-delete#task#2.

Cause 4 of 4: DeleteQueuedReservedInstances partial-failure item message.
AWS: recorded behavior only - HTTP 200 whose failedQueuedPurchaseDeletionSet item carries error.message 'Reserved instances id is invalid.' This message is entirely fixed text; it names no request value.
Emulator: error.message "The Reserved Instances ID '<id>' does not exist" - a different sentence that also asserts non-existence rather than invalidity.
Why: this is a per-item error inside a 200 batch response, so clients parse the message to classify each failure; the reworded text defeats that classification.
Fix: the DeleteQueuedReservedInstances handler's per-item failure serializer.
Traces: ec2-flow-logs-and-batch-delete-partial-failures#task#3.

## Evidence
Cause 1: $.Response.Errors.Error.Code expected 'InvalidHostConfiguration', got 'InvalidParameterValue'; Message expected "The instance type 'sim2real-bogus.type' is not supported.", got "Invalid value 'sim2real-bogus.type' for instanceType. EC2 offers no such instance type" (status 400 in both).
Cause 2: Message expected "The local-gateway-route-table ID 'lgwrtb-00000000000000000' is malformed", got 'Invalid LocalGateway Route Table id lgwrtb-00000000000000000'.
Cause 3: identical expected/actual pair on DeleteLocalGatewayRoute.
Cause 4: $.DeleteQueuedReservedInstancesResponse.failedQueuedPurchaseDeletionSet.item.error.message expected 'Reserved instances id is invalid.', got "The Reserved Instances ID '00000000-0000-0000-0000-000000000000' does not exist".

## Steps to Reproduce
Replay simparity-wire-aws-v1 and inspect ec2-allocate-hosts-invalid-instance-type-error#task#1, ec2-local-gateway-route-create-search-delete#task#1 and #task#2, and ec2-flow-logs-and-batch-delete-partial-failures#task#3. Compare the Code/Message of each refusal (and the per-item error.message in the last one) against the recorded AWS response.
