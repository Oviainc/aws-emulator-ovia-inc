Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.deep-logs-3-logacct-transformer-csv-splits-quoted-columns-on-ingest logs.deep-logs-3-logacct-transformer-datetimeconverter-shifts-timezone-on-ingest logs.deep-logs-3-logacct-transformer-listtomap-builds-keyed-map-on-ingest logs.deep-logs-3-logacct-transformer-typeconverter-casts-fields-on-ingest logs.account-data-protection-policy-masks-a-new-log-group logs.data-protection-policy-masks-log-events logs.data-protection-policy-masks-sensitive-data-server-side logs.delete-data-protection-policy-unmasks-subsequent-events logs.delivery-link-create-update-delete-lifecycle logs.get-delivery-reflects-actual-record-fields-after-events-flow logs.delivery-destination-policy-lifecycle logs.put-delivery-destination-policy-lifecycle logs.delivery-source-conflict-different-resource-rejected logs.deploy-metric-filter-cfn-then-emits-metric-datapoint logs.deep-logs-3-logacct-subscription-filter-policy-not-in-excludes-named-group logs.export-task-server-managed-completion
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.deep-logs-3-logacct-transformer-csv-splits-quoted-columns-on-ingest|run#23|AssertionError|raise AssertionError(
Issue-Key: logs.deep-logs-3-logacct-transformer-datetimeconverter-shifts-timezone-on-ingest|run#24|AssertionError|assert sorted(map(_canon, parsed)) == sorted(map(_canon, _EXPECTED)), (
Issue-Key: logs.deep-logs-3-logacct-transformer-listtomap-builds-keyed-map-on-ingest|run#24|AssertionError|assert sorted(map(_canon, parsed)) == sorted(map(_canon, _EXPECTED)), (
Issue-Key: logs.deep-logs-3-logacct-transformer-typeconverter-casts-fields-on-ingest|run#24|AssertionError|assert sorted(map(_canon, parsed)) == sorted(map(_canon, _EXPECTED)), (
Issue-Key: logs.account-data-protection-policy-masks-a-new-log-group|_poll_until#15|Timeout|time.sleep(interval_s)
Issue-Key: logs.data-protection-policy-masks-log-events|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Key: logs.data-protection-policy-masks-sensitive-data-server-side|run#14|Timeout|time.sleep(5.0)
Issue-Key: logs.delete-data-protection-policy-unmasks-subsequent-events|_poll_until_masked#9|Timeout|time.sleep(5)
Issue-Key: logs.delivery-link-create-update-delete-lifecycle|run#23|AssertionError|assert default_fields, "expected the delivery to have default recordFields assigned"
Issue-Key: logs.get-delivery-reflects-actual-record-fields-after-events-flow|run#19|AssertionError|assert created_record_fields, f"expected CreateDelivery to populate default recordFields, got {created}"
Issue-Key: logs.delivery-destination-policy-lifecycle|run#22|AssertionError|raise AssertionError("expected ResourceNotFoundException re-deleting an already-absent policy")
Issue-Key: logs.put-delivery-destination-policy-lifecycle|_post_delete_policy#19|KeyError|return svc.get_delivery_destination_policy(deliveryDestinationName=dest_name)["policy"].get(
Issue-Key: logs.delivery-source-conflict-different-resource-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cloudwatch-logs|DescribeMetricFilters|ResourceNotFoundException
Issue-Key: logs.deep-logs-3-logacct-subscription-filter-policy-not-in-excludes-named-group|run#22|Timeout|time.sleep(4.0)
Issue-Key: logs.export-task-server-managed-completion|_poll_until#19|Timeout|time.sleep(interval_s)
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# logs: transformers are not applied on ingest, data-protection masking never happens, delivery records lack defaults, and four more delivery-path causes (8 causes)

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTransformer

## Description
Eight independent root causes on the logs ingest/delivery paths. Cause count: 8. Each has its own failing check ids; all eight must be closed.

=== Cause 1 of 8: a configured transformer's processors are not applied to events on ingest ===
AWS: a transformer put on a log group (or inherited from a TRANSFORMER_POLICY account policy) rewrites each event at ingest time: csv splits the message into named columns honouring quoted fields containing the delimiter, dateTimeConverter rewrites a timestamp field into the target timezone/format, listToMap builds a keyed map from a list of objects, and typeConverter casts fields to the declared types. A subscription with applyOnTransformedLogs=true then delivers the TRANSFORMED (JSON) event. Recorded behavior only (processor semantics are documented, not modeled in Smithy).
Emulator: the raw message is delivered unchanged — 'a subscription with applyOnTransformedLogs=true must deliver the transformed JSON event, got non-JSON message \'alice,42,"x,y"\'' — and for the other three processors the delivered events 'differ from the processor\'s contract'. The transformer is accepted and stored but never runs.
Why: every transformer-dependent consumer (a subscription, a metric filter, an Insights query) sees unparsed raw text, so a client that moved parsing into the service silently loses it; nothing reports an error.
Fix: the transformer evaluation stage of the log-ingest path (apply the group's/policy's processor chain before the event is stored and before subscription delivery when applyOnTransformedLogs=true).
Traces: logs.deep-logs-3-logacct-transformer-csv-splits-quoted-columns-on-ingest, logs.deep-logs-3-logacct-transformer-datetimeconverter-shifts-timezone-on-ingest, logs.deep-logs-3-logacct-transformer-listtomap-builds-keyed-map-on-ingest, logs.deep-logs-3-logacct-transformer-typeconverter-casts-fields-on-ingest.

=== Cause 2 of 8: data-protection policies never mask anything, so four checks hang to the 60 s cap ===
AWS: a DATA_PROTECTION_POLICY (account-level or per log group) with an Audit/Deidentify statement over a managed data identifier masks the matched substring in every read of the events (GetLogEvents/FilterLogEvents) for a caller without logs:Unmask, and deleting the policy un-masks subsequent events. The checks poll for the masked form. Recorded behavior only.
Emulator: all four checks are terminated with 'Error: Check timed out after 60.0s' — the masked form never appears, so the bounded poll runs to the deadline.
Why: sensitive data a client believes is masked is served in the clear, and the masking contract cannot be observed at all; a caller waiting for the masked read hangs instead of getting an answer.
Fix: the data-protection masking stage of the log read path (and the Audit findings emitter), honouring the account-level policy for newly created groups.
Traces: logs.account-data-protection-policy-masks-a-new-log-group, logs.data-protection-policy-masks-log-events, logs.data-protection-policy-masks-sensitive-data-server-side, logs.delete-data-protection-policy-unmasks-subsequent-events.

=== Cause 3 of 8: CreateDelivery does not assign the delivery's default recordFields ===
AWS: CreateDelivery pairing a delivery source with a destination returns (and GetDelivery reports) a delivery whose recordFields are populated with the default record-field set for that source's logType.
Emulator: the delivery object carries only id, arn, deliverySourceName, deliveryDestinationArn and deliveryDestinationType — 'expected CreateDelivery to populate default recordFields' / 'expected the delivery to have default recordFields assigned'.
Why: a client reading recordFields to learn (or to diff) what a vended delivery writes sees an absent member where AWS always returns the defaults, so UpdateDeliveryConfiguration round-trips and drift detection break.
Fix: the CreateDelivery handler's record-field defaulting (and the GetDelivery serializer that must report it).
Traces: logs.delivery-link-create-update-delete-lifecycle, logs.get-delivery-reflects-actual-record-fields-after-events-flow.

=== Cause 4 of 8: DeleteDeliveryDestinationPolicy succeeds when the policy is already gone ===
AWS: re-deleting an absent delivery-destination policy is refused with ResourceNotFoundException.
Emulator: the second delete succeeds — 'expected ResourceNotFoundException re-deleting an already-absent policy'.
Why: a client cannot tell whether a policy existed, so convergence logic that relies on the not-found refusal (and audit trails of what was actually removed) is wrong.
Fix: the not-found path of the DeleteDeliveryDestinationPolicy handler.
Traces: logs.delivery-destination-policy-lifecycle.

=== Cause 5 of 8: PutDeliveryDestinationPolicy's response omits the policy ===
AWS: PutDeliveryDestinationPolicy returns a policy member carrying the stored deliveryDestinationPolicy; the check reads response['policy'].
Emulator: the member is absent — the check dies with KeyError: 'policy'.
Why: a caller cannot confirm what policy was stored from the response, and any SDK code dereferencing the documented member crashes instead of receiving the policy.
Fix: the PutDeliveryDestinationPolicy response serializer (populate the policy member from the stored document).
Traces: logs.put-delivery-destination-policy-lifecycle.

=== Cause 6 of 8: PutDeliverySource's resource-conflict message is worded by the emulator ===
AWS: PutDeliverySource re-using an existing source name with a DIFFERENT resourceArn is refused with the fixed text 'Update to existing Delivery Source with new ResourceId is not allowed. Please create a new Delivery Source instead.' (no variable parts).
Emulator: 'A delivery source named <name> already exists and represents <arn>' — an emulator-authored sentence that also echoes the stored ARN.
Why: the message no longer states the rule (a source's resource is immutable; create a new one), so a client cannot act on it, and tooling matching AWS's text does not recognise the refusal.
Fix: the name-collision branch of the PutDeliverySource handler.
Traces: logs.delivery-source-conflict-different-resource-rejected.

=== Cause 7 of 8: a log group declared by a CREATE_COMPLETE CloudFormation stack does not exist ===
AWS: when a stack declaring an AWS::Logs::LogGroup (plus an AWS::Logs::MetricFilter on it) reaches CREATE_COMPLETE, DescribeMetricFilters/DescribeLogGroups on that group answer 200.
Emulator: DescribeMetricFilters raises ResourceNotFoundException 'The specified log group does not exist.' after the stack reported CREATE_COMPLETE.
Why: the stack's success status is not a readiness signal — the very resource it claims to have created is missing, so everything downstream of the deploy fails with not-found.
Fix: the CloudFormation provisioning path for AWS::Logs::LogGroup / AWS::Logs::MetricFilter (create the resources before marking them, and the stack, complete). Same family as the open issue 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md, whose replay keys do not include this check.
Traces: logs.deploy-metric-filter-cfn-then-emits-metric-datapoint.

=== Cause 8 of 8: two more logs checks hang to the 60 s cap — account-level subscription-filter policy delivery, and server-managed export-task completion ===
AWS: a SUBSCRIPTION_FILTER_POLICY account policy delivers events from every log group it does not exclude, and a CreateExportTask moves to COMPLETED on its own and writes the objects; both checks poll for those observables.
Emulator: both are terminated with 'Error: Check timed out after 60.0s' — no delivery from the account policy, and the export task never settles with content.
Why: account-wide subscription policies are inert and export tasks never complete, so clients wait forever instead of receiving data or an error.
Fix: the account-policy subscription delivery path, and the export-task runner that must transition the task and write its objects.
Traces: logs.deep-logs-3-logacct-subscription-filter-policy-not-in-excludes-named-group, logs.export-task-server-managed-completion.

## Evidence
Replay quotes (generated names are per-run values):
Cause 1: 'a subscription with applyOnTransformedLogs=true must deliver the transformed JSON event, got non-JSON message \'alice,42,"x,y"\' (Expecting value: line 1 column 1 (char 0))'; 'transformed events delivered from <group> differ from the processor\'s contract:' (dateTimeConverter, listToMap and typeConverter checks).
Cause 2: all four checks report exactly 'Error: Check timed out after 60.0s'; each polls GetLogEvents for the masked form of a sensitive value after PutAccountPolicy/PutDataProtectionPolicy.
Cause 3: 'expected CreateDelivery to populate default recordFields, got {\'id\': ..., \'arn\': ..., \'deliverySourceName\': ..., \'deliveryDestinationArn\': ..., \'deliveryDestinationType\': \'CWL\'}' and 'expected the delivery to have default recordFields assigned'.
Cause 4: 'expected ResourceNotFoundException re-deleting an already-absent policy'.
Cause 5: "KeyError: 'policy'".
Cause 6: "expected message 'Update to existing Delivery Source with new ResourceId is not allowed. Please create a new Delivery Source instead.', got 'A delivery source named <name> already exists and represents <arn>'".
Cause 7: 'botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeMetricFilters operation: The specified log group does not exist.'
Cause 8: both checks report 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
Cause 1: PutTransformer (or an account TRANSFORMER_POLICY) with a csv / dateTimeConverter / listToMap / typeConverter processor chain on a log group; PutSubscriptionFilter with applyOnTransformedLogs=true; PutLogEvents with a raw line (e.g. 'alice,42,"x,y"'); read the delivered record. Observed: the raw message; expected: the transformed JSON event.
Cause 2: PutAccountPolicy(DATA_PROTECTION_POLICY) or PutDataProtectionPolicy with an EmailAddress data identifier; PutLogEvents containing an address; GetLogEvents as a caller without logs:Unmask. Observed: the poll for the masked form never succeeds; expected: the masked value.
Cause 3: PutDeliverySource + PutDeliveryDestination + CreateDelivery; read recordFields from the response and from GetDelivery. Observed: absent; expected: the logType's default record fields.
Cause 4: PutDeliveryDestinationPolicy, DeleteDeliveryDestinationPolicy, then DeleteDeliveryDestinationPolicy again. Observed: success; expected: ResourceNotFoundException.
Cause 5: PutDeliveryDestinationPolicy and read response['policy']. Observed: the member is missing.
Cause 6: PutDeliverySource name=<n> resourceArn=<A>, then PutDeliverySource name=<n> resourceArn=<B>. Compare the refusal message.
Cause 7: deploy a CFN template with an AWS::Logs::LogGroup and an AWS::Logs::MetricFilter; after CREATE_COMPLETE call DescribeMetricFilters logGroupName=<declared group>. Observed: ResourceNotFoundException.
Cause 8: (a) PutAccountPolicy(SUBSCRIPTION_FILTER_POLICY) whose selectionCriteria does not exclude a named group, write events and poll the destination; (b) CreateExportTask and poll DescribeExportTasks for COMPLETED plus the written objects. Both polls never complete.
