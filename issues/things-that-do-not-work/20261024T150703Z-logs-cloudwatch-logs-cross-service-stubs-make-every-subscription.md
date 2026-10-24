Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs.destination-lifecycle logs.destination-lifecycle-create-describe-delete logs.destination-policy-enforces-cross-account-subscription logs.put-destination-then-get logs.put-destination-invalid-target-arn-rejected logs.put-destination-policy-wildcard-principal-rejected logs.put-subscription-filter-then-describe logs.e2e-log-group-subscription-destination-delivery-flow logs.e2e-subscription-filter-invalid-role-rejected logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group logs.update-anomaly-nonexistent-fails logs.log-fields-and-record-pointer-errors-rejected logs.deploy-log-group-cfn-data-protection-policy-masks-events logs.put-bearer-token-authentication-invalid-operation-state-rejected logs.metric-extraction-policy-overlapping-selection-rejected
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|CreateStream|NotImplemented
Issue-Key: logs|GetMetricData|NotImplemented
Issue-Key: logs.update-anomaly-nonexistent-fails|run#11|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: cloudwatch-logs|GetLogGroupFields|ResourceNotFoundException
Issue-Key: logs.put-bearer-token-authentication-invalid-operation-state-rejected|run#7|AssertionError|assert enable_msg == "The specified log group does not exist.", f"got {enable_msg!r}"
Issue-Key: logs.metric-extraction-policy-overlapping-selection-rejected|run#13|AssertionError|assert code_msg == expected, f"expected message {expected!r}, got {code_msg!r}"
Issue-Related: 20261024T150703Z-logs-cloudwatch-logs-metric-filters-emit-no-cloudwatch-metric.md
Issue-Commit: 483d650f1d2fec6203edc8ec91bc65f69df3bd2d

# CloudWatch Logs: cross-service stubs make every subscription-destination check unrunnable, GetLogGroupFields 404s where AWS answers 200, and five more divergences (7 causes)

> _Note: this issue was identified against the code base at commit `483d650f1d2fec6203edc8ec91bc65f69df3bd2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDestination

## Description
Seven further root causes in the CloudWatch Logs surface, distinct from the ten in the companion logs issue. Cause count: 7.

=== Cause 1 of 7: every CloudWatch Logs subscription-destination path is blocked by the kinesis CreateStream stub ===
AWS: a CWL subscription filter delivers to a Kinesis data stream, either directly or through a cross-account Destination (PutDestination/PutDestinationPolicy). The destination's targetArn names a stream that must exist, so every one of these behaviours needs kinesis CreateStream to work.
Emulator: kinesis CreateStream answers NotImplemented ('operation CreateStream is scaffolded but not implemented in this emulator'), so the checks never reach the logs operation under test. The CWL side of the contract (PutDestination round-trip, destination policy enforcement, record envelope shape, ForwardedLogEvents / DeliveryErrors metrics, invalid-targetArn and invalid-role rejection) is therefore completely unverified.
Why: subscription-filter delivery is the main CWL fan-out mechanism; a client cannot stand up or validate any of it. Note: the kinesis service is still in the project's PENDING bootstrap set, so this cause may be resolved by that bootstrap — it is recorded here because these are logs checks and the logs-side assertions remain unexercised either way.
Fix: kinesis CreateStream (services/service-com-amazonaws-kinesis-kinesis-20131202), then re-replay the listed logs checks to grade the logs handlers.
Traces: logs.destination-lifecycle, logs.destination-lifecycle-create-describe-delete, logs.destination-policy-enforces-cross-account-subscription, logs.put-destination-then-get, logs.put-destination-invalid-target-arn-rejected, logs.put-destination-policy-wildcard-principal-rejected, logs.put-subscription-filter-then-describe, logs.e2e-log-group-subscription-destination-delivery-flow, logs.e2e-subscription-filter-invalid-role-rejected, logs.subscription-filter-record-envelope-carries-owner-stream-and-only-matching-events, logs.subscription-filter-forwardedlogevents-counts-only-the-matching-events, logs.subscription-filter-delivery-failure-counts-a-deliveryerrors-datapoint, logs.deep-logs-3-logacct-put-transformer-vended-parser-not-first-rejected, logs.deep-logs-3-logacct-transformer-parsevpc-extracts-typed-flow-fields-on-ingest.

=== Cause 2 of 7: the cloudwatch GetMetricData stub blocks account-level METRIC_EXTRACTION_POLICY verification ===
AWS: an account METRIC_EXTRACTION_POLICY changes which EMF metrics materialise; the only way to observe it is cloudwatch GetMetricData over the affected namespaces.
Emulator: cloudwatch GetMetricData answers NotImplemented ('operation GetMetricData is scaffolded but not implemented in this emulator'), so the policy's effect (and the EMFDisabledErrors signal) cannot be read at all.
Why: a client cannot confirm that an account policy suppressed or enabled EMF extraction for the selected groups.
Fix: cloudwatch GetMetricData (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801), then re-replay these two checks.
Traces: logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected, logs.deep-logs-3-logacct-metric-extraction-policy-suppresses-emf-only-for-selected-group.

=== Cause 3 of 7: UpdateAnomaly answers NotImplemented instead of its modelled not-found error ===
AWS: Smithy UpdateAnomaly takes anomalyDetectorArn (@required) plus anomalyId/patternId and models InvalidParameterException, OperationAbortedException, ResourceNotFoundException (400) and ServiceUnavailableException (500). An anomaly that does not exist is refused with ResourceNotFoundException (400).
Emulator: the call answers error code 'NotImplemented'.
Why: a client's not-found handling never runs; a retry/catch keyed on ResourceNotFoundException sees an unmodelled code instead. No existing stub issue names UpdateAnomaly.
Fix: the UpdateAnomaly handler and its not-found classification.
Traces: logs.update-anomaly-nonexistent-fails.

=== Cause 4 of 7: GetLogGroupFields raises ResourceNotFoundException for an unknown log group ===
AWS: Smithy GetLogGroupFields returns {logGroupFields} and models ResourceNotFoundException (400). Recorded behavior: GetLogGroupFields does not validate log-group existence up front — it queries the (empty) field index for the name given, so an UNKNOWN log group name answers 200 with an empty logGroupFields list, and an existing group with no indexed fields yet likewise answers 200 with [].
Emulator: the call raises ResourceNotFoundException (400) with 'The specified log group does not exist.'.
Why: the operation is the normal way to probe a group's queryable schema; turning an empty result into an error breaks any caller that enumerates fields before building a query, and misplaces the not-found boundary (which AWS applies to pointers and data sources, not to this name).
Fix: the GetLogGroupFields handler — drop the existence pre-check and return an empty list.
Traces: logs.log-fields-and-record-pointer-errors-rejected.

=== Cause 5 of 7: a CloudFormation AWS::Logs::LogGroup with a DataProtectionPolicy reaches CREATE_COMPLETE without the group existing ===
AWS: a stack declaring AWS::Logs::LogGroup (with DataProtectionPolicy) and AWS::Logs::LogStream reaches CREATE_COMPLETE only once those resources exist; the group is then readable through logs APIs and the policy is live on the data plane.
Emulator: the stack reports CREATE_COMPLETE, but GetDataProtectionPolicy on the declared group raises ResourceNotFoundException 'The specified log group does not exist.' — the group was never provisioned.
Why: a client deploying log infrastructure by template gets a green stack and no log group, so the subsequent masking/ingest work fails with a not-found error far from the cause. Open issue 20261023T113357Z-cloudformation-reports-create-complete-for-logs-cloudwatch-ecs.md records this pattern for other checks; this check is not among the ones it carries.
Fix: the CloudFormation resource provisioner for AWS::Logs::LogGroup / AWS::Logs::LogStream (and the DataProtectionPolicy property).
Traces: logs.deploy-log-group-cfn-data-protection-policy-masks-events.

=== Cause 6 of 7: the log-group not-found message is missing its trailing period ===
AWS: on the not-found path of the bearer-token-authentication operations, the ResourceNotFoundException message is the fixed service string 'The specified log group does not exist.' — with the terminating period. Other CWL operations in the same emulator already emit exactly that string (see cause 5 above and cause 4's message), so the divergence is inconsistent within the service rather than a message whose rule is unknown.
Emulator: this path emits 'The specified log group does not exist' (no period). Status 400 and code ResourceNotFoundException match.
Why: a client that string-matches the service message, or a test fixture recorded against AWS, diverges on one operation while agreeing on the rest. Open issue 20261023T113357Z-logs-cloudwatch-logs-startlivetail-and-getlogobject-answer-501-three.md covers CWL error-message divergences on other operations; it does not carry this one. Severity LOW on its own.
Fix: the error construction in the PutBearerTokenAuthentication handler.
Traces: logs.put-bearer-token-authentication-invalid-operation-state-rejected.

=== Cause 7 of 7: the overlapping-METRIC_EXTRACTION_POLICY rejection does not name the conflicting policy in AWS's form ===
AWS: PutAccountPolicy refuses a METRIC_EXTRACTION_POLICY whose selectionCriteria overlaps an existing one. The message is built from the request and the conflicting resource: 'SelectionCriteria is overlapping with an existing policy with policy name: <policyName>'.
Emulator: the status (400) and code (InvalidParameterException) match, but the message is the emulator's own prose: "selectionCriteria conflicts with the account-level METRIC_EXTRACTION_POLICY policy '<policyName>': no two of them can have overlapping criteria".
Why: the conflict IS reported, so severity is LOW, but the request-derived message shape differs from AWS's, which breaks callers and fixtures that match on it.
Fix: the error construction in the PutAccountPolicy overlap check.
Traces: logs.metric-extraction-policy-overlapping-selection-rejected.

## Evidence
Cause 1 — all fourteen listed checks end in: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateStream operation: operation CreateStream is scaffolded but not implemented in this emulator. Expected a created Kinesis stream and then the logs assertions (e.g. PutDestination round-trip, 'ForwardedLogEvents counts only matching events').
Cause 2 — logs.deep-logs-3-logacct-metric-extraction-policy-enabled-status-rejected and ...-suppresses-emf-only-for-selected-group: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the GetMetricData operation: operation GetMetricData is scaffolded but not implemented in this emulator.
Cause 3 — logs.update-anomaly-nonexistent-fails: AssertionError: expected ResourceNotFoundException, got NotImplemented.
Cause 4 — logs.log-fields-and-record-pointer-errors-rejected: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the GetLogGroupFields operation: The specified log group does not exist. Expected HTTP 200 with logGroupFields == [] for the unknown name (the check asserts exactly that, citing real AWS).
Cause 5 — logs.deploy-log-group-cfn-data-protection-policy-masks-events: the stack reached a terminal CREATE_COMPLETE, then botocore.errorfactory.ResourceNotFoundException: ... when calling the GetDataProtectionPolicy operation: The specified log group does not exist.
Cause 6 — logs.put-bearer-token-authentication-invalid-operation-state-rejected: AssertionError: got 'The specified log group does not exist' (expected 'The specified log group does not exist.').
Cause 7 — logs.metric-extraction-policy-overlapping-selection-rejected: AssertionError: expected message 'SelectionCriteria is overlapping with an existing policy with policy name: <name>', got "selectionCriteria conflicts with the account-level METRIC_EXTRACTION_POLICY policy '<name>': no two of them can have overlapping criteria".

## Steps to Reproduce
Cause 1: kinesis CreateStream(StreamName=..., ShardCount=1) — NotImplemented. Then (once it works) PutDestination / PutSubscriptionFilter against that stream ARN and replay the fourteen listed checks.
Cause 2: cloudwatch GetMetricData(MetricDataQueries=[...]) — NotImplemented.
Cause 3: UpdateAnomaly(anomalyDetectorArn=<existing detector>, anomalyId=<36-char id that does not exist>, suppressionType=...) — error code NotImplemented instead of ResourceNotFoundException.
Cause 4: GetLogGroupFields(logGroupName='a-name-that-does-not-exist') — 400 ResourceNotFoundException instead of 200 {'logGroupFields': []}.
Cause 5: cfn CreateStack with Resources {Protected: AWS::Logs::LogGroup with LogGroupName/RetentionInDays/DataProtectionPolicy, Control: AWS::Logs::LogGroup, PStream: AWS::Logs::LogStream}; wait for CREATE_COMPLETE; GetDataProtectionPolicy(logGroupIdentifier=<Protected>) — 400 ResourceNotFoundException.
Cause 6: PutBearerTokenAuthentication against a log group that does not exist — message lacks the trailing period.
Cause 7: PutAccountPolicy two METRIC_EXTRACTION_POLICY policies with overlapping selectionCriteria — second is refused with a non-AWS message.
