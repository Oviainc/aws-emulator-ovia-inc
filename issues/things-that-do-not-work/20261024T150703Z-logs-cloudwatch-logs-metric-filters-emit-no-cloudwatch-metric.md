Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.put-metric-filter-dimensions-preserved-in-metric-data logs.put-metric-filter-multiple-filters-independent-increments logs.metric-filter-default-value-emits-a-zero-datapoint-where-an-absent-default-emits-none logs.metric-filter-unit-is-attached-to-the-emitted-datapoint logs.describe-metric-filters-multiple-filters-per-group-distinct logs.disabled-cmk-reads-blocked-until-key-reenabled logs.deep-logs-2-logcmk-disabled-after-association-withholds-reads logs.deep-logs-2-logcmk-events-bound-to-ingest-key logs.deep-logs-2-logcmk-key-policy-revoked-reads-withheld logs.associate-kms-key-cross-region-mismatch-rejected logs.deep-logs-2-logcmk-malformed-entity-rejected-events-kept logs.deep-logs-3-logacct-transformer-policy-applies-to-new-group-and-group-transformer-overrides logs.deep-logs-3-logacct-transformer-policy-overlapping-prefix-rejected logs.put-data-protection-policy-roundtrip logs.data-protection-audit-findings-delivered-to-the-findings-destination logs.unmask-without-permission-denied-while-masked-read-still-served logs.delete-integration-nonexistent-fails logs.integration-lifecycle-transitions logs.list-integrations-reflects-creation-order-stable logs.get-storage-tier-policy-access-denied-without-permission logs.infrequent-access-log-group-class-restrictions
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs.put-metric-filter-dimensions-preserved-in-metric-data|_wait_for_metric_sum#21|Timeout|time.sleep(interval_s)
Issue-Key: logs.put-metric-filter-multiple-filters-independent-increments|_wait_for_both_metric_sums#19|Timeout|time.sleep(interval_s)
Issue-Key: logs.metric-filter-default-value-emits-a-zero-datapoint-where-an-absent-default-emits-none|_poll_until#20|Timeout|time.sleep(interval_s)
Issue-Key: logs.metric-filter-unit-is-attached-to-the-emitted-datapoint|_poll_until#23|Timeout|time.sleep(interval_s)
Issue-Key: logs.describe-metric-filters-multiple-filters-per-group-distinct|_poll_for_metric_sum#23|Timeout|time.sleep(10)
Issue-Key: logs.disabled-cmk-reads-blocked-until-key-reenabled|_poll#21|Timeout|time.sleep(interval_s)
Issue-Key: logs.deep-logs-2-logcmk-disabled-after-association-withholds-reads|_poll#25|Timeout|time.sleep(interval_s)
Issue-Key: logs.deep-logs-2-logcmk-events-bound-to-ingest-key|_poll#29|Timeout|time.sleep(interval_s)
Issue-Key: logs.deep-logs-2-logcmk-key-policy-revoked-reads-withheld|_poll#29|Timeout|time.sleep(interval_s)
Issue-Key: logs.associate-kms-key-cross-region-mismatch-rejected|_error#10|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded and returned {result!r}")
Issue-Key: logs.deep-logs-2-logcmk-malformed-entity-rejected-events-kept|run#18|AssertionError|assert got == want, (
Issue-Key: cloudwatch-logs|PutAccountPolicy|InvalidParameterException
Issue-Key: cloudwatch-logs|GetDataProtectionPolicy|ResourceNotFoundException
Issue-Key: logs.data-protection-audit-findings-delivered-to-the-findings-destination|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: logs.unmask-without-permission-denied-while-masked-read-still-served|_poll_until#16|Timeout|time.sleep(interval_s)
Issue-Key: logs.delete-integration-nonexistent-fails|run#7|AssertionError|assert post_delete_message == f"Integration with name {real_name} does not exist.", (
Issue-Key: logs.integration-lifecycle-transitions|run#21|AssertionError|assert get_message == f"Integration with name {integration_name} does not exist.", (
Issue-Key: logs.list-integrations-reflects-creation-order-stable|run#25|AssertionError|assert get_message == f"Integration with name {name1} does not exist.", (
Issue-Key: logs.get-storage-tier-policy-access-denied-without-permission|_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Key: logs.infrequent-access-log-group-class-restrictions|_error#14|AssertionError|raise AssertionError(f"expected a ClientError, but the call succeeded and returned {result!r}")
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# CloudWatch Logs: metric filters emit no CloudWatch metric data, CMK key state is ignored on reads, and six more control/data-plane divergences (10 causes)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricFilter

## Description
Ten independent root causes in the CloudWatch Logs emulator, all grounded in failing behavioral checks. Cause count: 10.

=== Cause 1 of 10: a metric filter never produces CloudWatch metric data ===
AWS: PutMetricFilter's metricTransformations are an ingest-time rule — every PutLogEvents event matching filterPattern publishes a datapoint for metricNamespace/metricName with metricValue, the declared dimensions resolved from the extracted fields, the declared unit, and defaultValue for non-matching ingested periods. Smithy: PutMetricFilter.metricTransformations -> MetricTransformations (metricName, metricNamespace, metricValue, defaultValue, dimensions, unit); the datapoint is then readable through cloudwatch GetMetricStatistics. Recorded behavior: the checks poll GetMetricStatistics and return as soon as Sum > 0.
Emulator: PutMetricFilter and DescribeMetricFilters round-trip, but no datapoint ever appears in the metric namespace. The polling loops never see a non-zero Sum and every check runs to the suite's 60 s cap (these loops return on the FIRST successful reading, so a correct emulator finishes in one iteration — the timeout is the absence of the datapoint, not the poll budget).
Why: a client that wires log-based alarming (metric filter -> metric -> alarm) sees a filter that exists and matches but drives no metric at all, so alarms never leave INSUFFICIENT_DATA and dashboards stay empty.
Fix: the metric-filter evaluation path in the logs ingest handler (PutLogEvents) and its emission into the CloudWatch metric store.
Traces: logs.put-metric-filter-dimensions-preserved-in-metric-data, logs.put-metric-filter-multiple-filters-independent-increments, logs.metric-filter-default-value-emits-a-zero-datapoint-where-an-absent-default-emits-none, logs.metric-filter-unit-is-attached-to-the-emitted-datapoint, logs.describe-metric-filters-multiple-filters-per-group-distinct.

=== Cause 2 of 10: the log group's associated CMK is ignored when events are read ===
AWS: a log group associated with a customer CMK serves its stored events only while that key is usable. Recorded behavior (stated in the checks, measured us-west-2): after kms DisableKey, GetLogEvents keeps answering 200 but with an EMPTY events list (the payload is withheld, not refused), and the same call returns the original events once EnableKey propagates; revoking only key A's CloudWatch Logs grant hides exactly the events ingested under key A while events ingested under key B and plaintext events keep reading. AssociateKmsKey/DisassociateKmsKey do not re-key already-stored events.
Emulator: events are stored and served as plaintext with no reference to key state or per-event key provenance. GetLogEvents keeps returning every event after DisableKey and after a grant revocation, so the "reads are withheld" arm never becomes true and each check runs to the 60 s cap.
Why: a client validating a CMK blast-radius control (disable the key, confirm the data stops being served) gets a false pass from the emulator, and the per-event key binding the API documents is not modelled at all.
Fix: the log-event store (per-event key provenance) and the read path of GetLogEvents/FilterLogEvents, gated on the KMS key's state and grants.
Traces: logs.disabled-cmk-reads-blocked-until-key-reenabled, logs.deep-logs-2-logcmk-disabled-after-association-withholds-reads, logs.deep-logs-2-logcmk-events-bound-to-ingest-key, logs.deep-logs-2-logcmk-key-policy-revoked-reads-withheld.

=== Cause 3 of 10: AssociateKmsKey accepts a KMS key from another region ===
AWS: Smithy AssociateKmsKey.kmsKeyId is @required (length 0-256) and the operation models InvalidParameterException (400). A log group can only be associated with a CMK in the same region as the log group; a cross-region key ARN is refused.
Emulator: the call returns HTTP 200 with an empty body ({}), associating a key whose ARN names a different region.
Why: a client that relies on the API to catch a mis-regioned key ARN gets a log group that appears CMK-protected by a key the service could never use.
Fix: AssociateKmsKey's input validation (region component of the kmsKeyId ARN vs the request region).
Traces: logs.associate-kms-key-cross-region-mismatch-rejected.

=== Cause 4 of 10: PutLogEvents omits rejectedEntityInfo for a malformed entity ===
AWS: Smithy PutLogEventsResponse has member rejectedEntityInfo -> RejectedEntityInfo{errorType}. When the request's `entity` is malformed, the log events are still accepted (200) and the response reports why the entity was rejected — errorType 'InvalidTypeValue' for a bad entity type.
Emulator: PutLogEvents answers 200 but rejectedEntityInfo is absent entirely (the check reads rejectedEntityInfo=None, errorType=None).
Why: a client cannot distinguish "entity accepted" from "entity silently dropped"; the only signal AWS gives for a bad entity is missing from the response.
Fix: PutLogEvents' response serializer and its entity validation.
Traces: logs.deep-logs-2-logcmk-malformed-entity-rejected-events-kept.

=== Cause 5 of 10: PutAccountPolicy rejects the documented TRANSFORMER_POLICY selectionCriteria ===
AWS: Smithy PutAccountPolicy.selectionCriteria documentation: "If policyType is TRANSFORMER_POLICY, the only supported selectionCriteria filter is LogGroupNamePrefix". The request form is `LogGroupNamePrefix = "<prefix>"`, and PutAccountPolicy echoes it back in accountPolicy.selectionCriteria.
Emulator: that request is refused with HTTP 400 InvalidParameterException and the message `selectionCriteria must be one or more filters of the form 'Field IN ["value", ...]' or 'Field NOT IN ["value", ...]', joined by AND` — i.e. only the SUBSCRIPTION_FILTER_POLICY grammar is accepted, for every policy type.
Why: account-level transformer policies cannot be created at all; every scoped TRANSFORMER_POLICY (and the FIELD_INDEX_POLICY LogGroupNamePrefix form) is unreachable.
Fix: PutAccountPolicy's selectionCriteria parser/validation, keyed on policyType.
Traces: logs.deep-logs-3-logacct-transformer-policy-applies-to-new-group-and-group-transformer-overrides, logs.deep-logs-3-logacct-transformer-policy-overlapping-prefix-rejected.

=== Cause 6 of 10: GetDataProtectionPolicy 404s after a successful PutDataProtectionPolicy ===
AWS: Smithy GetDataProtectionPolicy returns {logGroupIdentifier, policyDocument, lastUpdatedTime} and models ResourceNotFoundException (400) only for an unknown resource. After PutDataProtectionPolicy on an existing log group, Get returns the stored document; recorded behavior: after DeleteDataProtectionPolicy, AWS still answers 200 (the response may fall back to an account-level policy) rather than 404.
Emulator: GetDataProtectionPolicy raises ResourceNotFoundException with message 'The specified data protection policy does not exist.' on the group whose policy was just written.
Why: a data-protection policy cannot be read back, so a client cannot confirm the control it just applied, and the documented 200-after-delete contract is replaced by an error.
Fix: PutDataProtectionPolicy's state write and GetDataProtectionPolicy's lookup/not-found classification.
Traces: logs.put-data-protection-policy-roundtrip.

=== Cause 7 of 10: data-protection audit findings are never delivered and unmask authorization is not enforced ===
AWS: a data protection policy with an Audit statement and a FindingsDestination delivers a finding record to that destination for each matched event; a Deidentify/MaskConfig statement masks the identifier on read, and reading with unmask=true requires logs:Unmask — without it AWS refuses the unmasked read while the masked read keeps succeeding.
Emulator: the findings destination never receives a record and the unmask arm is not refused, so both checks run to the 60 s cap waiting for an observable that never appears.
Why: a client cannot verify either half of a data-protection control — neither that sensitive matches are audited, nor that unmasking is privileged.
Fix: the data-protection policy evaluation in the logs ingest/read path (findings emission and the Unmask authorization check).
Traces: logs.data-protection-audit-findings-delivered-to-the-findings-destination, logs.unmask-without-permission-denied-while-masked-read-still-served.

=== Cause 8 of 10: integration not-found messages do not name the integration ===
AWS: Smithy GetIntegration/DeleteIntegration take integrationName (@required) and model ResourceNotFoundException (400). The message is built from the request and names the integration: `Integration with name <integrationName> does not exist.`
Emulator: the status (400) and code (ResourceNotFoundException) match, but the message is the constant 'The specified integration does not exist.' — it does not carry the requested name.
Why: a client (or operator reading a log) cannot tell WHICH integration was missing when several are managed in one workflow; the request-derived part of the message is lost.
Fix: the error construction in the GetIntegration/DeleteIntegration handlers.
Traces: logs.delete-integration-nonexistent-fails, logs.integration-lifecycle-transitions, logs.list-integrations-reflects-creation-order-stable.

=== Cause 9 of 10: GetStorageTierPolicy is served to a principal AWS denies ===
AWS: logs read operations are authorized; a session whose identity policy does not grant the action is refused with AccessDeniedException (400) rather than served.
Emulator: GetStorageTierPolicy answers HTTP 200 with {'storageTier': 'STANDARD'} for the unauthorized session. (Related open issue 20261023T113357Z-logs-cloudwatch-logs-accepts-requests-real-aws-refuses-putresourcepolicy.md records the general "no IAM authorization" gap for other operations; this operation is not among them.)
Why: a client validating least-privilege for log storage-tier configuration gets a false pass — the emulator reports an authorization boundary that does not exist.
Fix: the authorization check in the GetStorageTierPolicy handler.
Traces: logs.get-storage-tier-policy-access-denied-without-permission.

=== Cause 10 of 10: INFREQUENT_ACCESS log-group class restrictions are not enforced ===
AWS: a log group created with logGroupClass INFREQUENT_ACCESS supports only a reduced operation set; the operations that are not available on that class are refused (a 400 client error) rather than applied.
Emulator: the restricted call answers HTTP 200 with an empty body, so the class carries no restriction at all.
Why: a client choosing the INFREQUENT_ACCESS class to trade features for cost gets no signal that a feature is unavailable, and discovers it only in production.
Fix: the logGroupClass gate in the affected logs operation handlers (and CreateLogGroup's record of the class).
Traces: logs.infrequent-access-log-group-class-restrictions.

## Evidence
Cause 1 — logs.put-metric-filter-dimensions-preserved-in-metric-data: expected a GetMetricStatistics Sum >= 512 for the dimensioned metric; got no datapoint at all and the check ran to 'Check timed out after 60.0s'. The poll helper returns on the first reading with Sum > 0, so one correct iteration would end the check.
Cause 2 — logs.disabled-cmk-reads-blocked-until-key-reenabled: after kms DisableKey (DescribeKey KeyState == 'Disabled'), GetLogEvents must converge to an empty events list; the emulator kept serving both stored events until the 60 s cap. logs.deep-logs-2-logcmk-events-bound-to-ingest-key: revoking key A's grant must hide only the key-A event; no event was ever hidden.
Cause 3 — logs.associate-kms-key-cross-region-mismatch-rejected: AssertionError: expected the call to be rejected, but it succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}}. Expected 400 InvalidParameterException.
Cause 4 — logs.deep-logs-2-logcmk-malformed-entity-rejected-events-kept: AssertionError: bad-type: rejectedEntityInfo.errorType=None, expected 'InvalidTypeValue' (full rejectedEntityInfo=None).
Cause 5 — logs.deep-logs-3-logacct-transformer-policy-applies-to-new-group-and-group-transformer-overrides: PutAccountPolicy(policyType='TRANSFORMER_POLICY', selectionCriteria='LogGroupNamePrefix = "<prefix>"') raised InvalidParameterException: selectionCriteria must be one or more filters of the form 'Field IN ["value", ...]' or 'Field NOT IN ["value", ...]', joined by AND. Expected 200 with accountPolicy.selectionCriteria echoed verbatim.
Cause 6 — logs.put-data-protection-policy-roundtrip: GetDataProtectionPolicy raised ResourceNotFoundException: The specified data protection policy does not exist., on the log group whose policy PutDataProtectionPolicy had just accepted. Expected 200 with policyDocument.
Cause 7 — logs.data-protection-audit-findings-delivered-to-the-findings-destination and logs.unmask-without-permission-denied-while-masked-read-still-served: both 'Check timed out after 60.0s'; no finding record reached the findings destination and the unmasked read was never refused.
Cause 8 — logs.delete-integration-nonexistent-fails: AssertionError: unexpected message: 'The specified integration does not exist.' (expected 'Integration with name <name> does not exist.'). Same message on GetIntegration in logs.integration-lifecycle-transitions and logs.list-integrations-reflects-creation-order-stable.
Cause 9 — logs.get-storage-tier-policy-access-denied-without-permission: AssertionError: expected a ClientError, but the call succeeded and returned {'storageTier': 'STANDARD', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}.
Cause 10 — logs.infrequent-access-log-group-class-restrictions: AssertionError: expected a ClientError, but the call succeeded and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}} (empty body).

## Steps to Reproduce
Boot the emulator and replay the behavioral suite, or run the listed checks individually.

Cause 1: CreateLogGroup + CreateLogStream; PutMetricFilter with metricTransformations=[{metricName, metricNamespace, metricValue:'$size', dimensions:{'StatusCode':'$status_code'}}] and filterPattern '[ip, status_code, size]'; PutLogEvents a matching line; poll cloudwatch GetMetricStatistics(Namespace, MetricName, Dimensions=[{StatusCode:200}], Period=60, Statistics=['Sum']) — no datapoint appears.
Cause 2: kms CreateKey with a key policy granting logs.<region>.amazonaws.com; CreateLogGroup with kmsKeyId; PutLogEvents two events; GetLogEvents returns them; kms DisableKey; poll GetLogEvents — it keeps returning both events instead of converging to [].
Cause 3: kms CreateKey in region A; CreateLogGroup in region B; AssociateKmsKey(logGroupName, kmsKeyId=<region-A key ARN>) — returns 200.
Cause 4: PutLogEvents with a malformed `entity` — response has no rejectedEntityInfo member.
Cause 5: PutAccountPolicy(policyName, policyType='TRANSFORMER_POLICY', policyDocument=<processors>, selectionCriteria='LogGroupNamePrefix = "sim2real-"') — 400 InvalidParameterException.
Cause 6: CreateLogGroup; PutDataProtectionPolicy(logGroupIdentifier, policyDocument); GetDataProtectionPolicy(logGroupIdentifier) — 400 ResourceNotFoundException.
Cause 7: PutDataProtectionPolicy with an Audit statement + FindingsDestination and a Deidentify/MaskConfig statement; PutLogEvents an email address; poll the findings destination (empty) and GetLogEvents(unmask=True) without logs:Unmask (not refused).
Cause 8: DeleteIntegration / GetIntegration for a name that does not exist — message does not contain the name.
Cause 9: GetStorageTierPolicy under a session without the action granted — 200.
Cause 10: CreateLogGroup(logGroupClass='INFREQUENT_ACCESS'), then invoke an operation unavailable on that class — 200.
