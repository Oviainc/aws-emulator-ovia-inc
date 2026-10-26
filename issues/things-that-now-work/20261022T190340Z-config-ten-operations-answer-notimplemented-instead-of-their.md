Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.associate-disassociate-resource-types config.batch-get-aggregate-resource-config-nonexistent-aggregator-rejected config.connector-configuration-payload-round-trip config.delete-remediation-exceptions-nonexistent-rejected config.deliver-config-snapshot config.op-delete-aggregator-nonexistent-and-bad-name config.overly-permissive-put-evaluations-invalid-result-token config.start-config-rules-evaluation-transition config.external-evaluation-requires-process-check-rule config.put-external-evaluation-non-process-check-rule-rejected config.select-aggregate-resource-config-errors
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.associate-disassociate-resource-types|run#9|AssertionError|assert code == "ValidationException", code
Issue-Key: config.batch-get-aggregate-resource-config-nonexistent-aggregator-rejected|run#7|AssertionError|assert code == "NoSuchConfigurationAggregatorException", f"expected NoSuchConfigurationAggregatorException, got {code!r}"
Issue-Key: config.connector-configuration-payload-round-trip|run#4|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: config.delete-remediation-exceptions-nonexistent-rejected|run#5|AssertionError|assert code == "NoSuchRemediationExceptionException", f"expected NoSuchRemediationExceptionException, got {code!r}"
Issue-Key: config.deliver-config-snapshot|run#10|AssertionError|assert code == "NoRunningConfigurationRecorderException", code
Issue-Key: config.op-delete-aggregator-nonexistent-and-bad-name|run#5|AssertionError|assert code == "NoSuchConfigurationAggregatorException", code
Issue-Key: config.overly-permissive-put-evaluations-invalid-result-token|run#31|AssertionError|raise AssertionError(f"unexpected error starting rule evaluation: {code}")
Issue-Key: config.start-config-rules-evaluation-transition|run#15|AssertionError|assert code == "InvalidResultTokenException", code
Issue-Key: config.external-evaluation-requires-process-check-rule|run#8|AssertionError|assert code == "NoSuchConfigRuleException", (
Issue-Key: config.put-external-evaluation-non-process-check-rule-rejected|run#11|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Key: config.select-aggregate-resource-config-errors|run#4|AssertionError|assert code == "NoSuchConfigurationAggregatorException", code
Issue-Commit: 699f6ed7a7b73d87a9db3c950b6f69a120aa392c

# Config: ten operations answer NotImplemented instead of their modeled responses/errors (AssociateResourceTypes, BatchGetAggregateResourceConfig, PutConnector, DeleteRemediationExceptions, DeliverConfigSnapshot, DeleteConfigurationAggregator, StartConfigRulesEvaluation, PutEvaluations, PutExternalEvaluation, SelectAggregateResourceConfig)

> _Note: this issue was identified against the code base at commit `699f6ed7a7b73d87a9db3c950b6f69a120aa392c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateResourceTypes

## Description
Ten AWS Config operations are still scaffold stubs: every call answers the emulator-specific error code `NotImplemented` ("operation X is scaffolded but not implemented in this emulator") instead of the modeled success shape or the modeled 400 client error. None of these ten operations is covered by an existing open issue (the suite's auto-filed stub issues cover a different set of operations: GetResourceConfigHistory, PutConfigurationAggregator, PutAggregationAuthorization, DescribeAggregationAuthorizations, ListDiscoveredResources, DescribeComplianceByConfigRule, ListConnectors, ListConfigurationRecorders, GetCustomRulePolicy, DescribeConfigRuleEvaluationStatus, PutConformancePack, TagResource, PutServiceLinkedConfigurationRecorder, DescribeDeliveryChannelStatus, PutRemediationConfigurations, GetDiscoveredResourceCounts, StartResourceEvaluation, PutStoredQuery, DescribeRetentionConfigurations, DescribePendingAggregationRequests, DescribeRemediationConfigurations, GetComplianceDetailsByConfigRule, PutResourceConfig, SelectResourceConfig, ListResourceEvaluations, PutThirdPartyServiceLinkedConfigurationRecorder). TEN CAUSES must be closed.

Cause 1 of 10: AssociateResourceTypes / DisassociateResourceTypes.
AWS: AssociateResourceTypes and DisassociateResourceTypes each take a required ConfigurationRecorderArn (@length 1..1000) and ResourceTypes, return the updated ConfigurationRecorder, and model ConflictException, NoSuchConfigurationRecorderException and ValidationException, all client errors with httpError 400 (Smithy: AssociateResourceTypes / DisassociateResourceTypes). ValidationException's own doc states the case the check exercises: "For AssociateResourceTypes and DisassociateResourceTypes ... Your configuraiton recorder has a recording strategy that does not allow the association or disassociation of resource types." So associating a resource type onto an ALL_SUPPORTED-strategy recorder is a 400 ValidationException, and associating onto an EXCLUSION/INCLUSION recorder succeeds and is visible in DescribeConfigurationRecorders.
Emulator: answers error code `NotImplemented` for the operation, so neither the rejection nor the successful mutation happens.
Why: a client cannot narrow a recorder's recorded resource types at all, and its error handling never sees ValidationException/NoSuchConfigurationRecorderException — it sees an error code that does not exist in the Config API and that no SDK models, so retry/branch logic on the modeled errors is dead code.
Fix: the AssociateResourceTypes and DisassociateResourceTypes handlers and their validation (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/associate_resource_types.rs, .../disassociate_resource_types.rs).
Traces: config.associate-disassociate-resource-types (assert code == "ValidationException" got 'NotImplemented').

Cause 2 of 10: BatchGetAggregateResourceConfig.
AWS: models exactly two errors, NoSuchConfigurationAggregatorException and ValidationException, both client/400 (Smithy: BatchGetAggregateResourceConfig.errors); NoSuchConfigurationAggregatorException doc: "You have specified a configuration aggregator that does not exist." A call naming an aggregator that does not exist is a 400 NoSuchConfigurationAggregatorException; a call against a real aggregator returns BaseConfigurationItems/UnprocessedResourceIdentifiers.
Emulator: answers `NotImplemented`.
Why: the aggregator-existence boundary is unobservable — a client cannot distinguish a bad aggregator name from an empty aggregate result, and never gets the modeled 400.
Fix: the BatchGetAggregateResourceConfig handler and its aggregator lookup (.../src/ops/batch_get_aggregate_resource_config.rs).
Traces: config.batch-get-aggregate-resource-config-nonexistent-aggregator-rejected (expected NoSuchConfigurationAggregatorException, got 'NotImplemented').

Cause 3 of 10: PutConnector / GetConnector.
AWS: PutConnector takes a required ConnectorConfiguration, returns the connector Arn, and models ConflictException, InsufficientPermissionsException, MaxNumberOfConnectorsExceededException and ValidationException (all client/400). GetConnector takes a required Arn and models ResourceNotFoundException and ValidationException (client/400) (Smithy: PutConnector, GetConnector). A ConnectorConfiguration carrying a malformed tenant/client identifier is rejected 400 ValidationException (recorded messages, examples: "Invalid tenant identifier", "Invalid client identifier"); GetConnector on a nonexistent connector ARN is rejected rather than answered.
Emulator: answers `NotImplemented` for PutConnector (and therefore GetConnector is never reached).
Why: the whole connector surface is unusable and its payload round-trip and rejection classes cannot be exercised.
Fix: the PutConnector and GetConnector handlers plus ConnectorConfiguration validation (.../src/ops/put_connector.rs, .../src/ops/get_connector.rs).
Traces: config.connector-configuration-payload-round-trip (expected ValidationException, got 'NotImplemented').

Cause 4 of 10: DeleteRemediationExceptions.
AWS: models exactly one error, NoSuchRemediationExceptionException (client/400), and returns FailedBatches otherwise (Smithy: DeleteRemediationExceptions). Deleting a remediation exception that does not exist is rejected 400 NoSuchRemediationExceptionException (recorded message, an example: "Specified RemediationException could not be found.").
Emulator: answers `NotImplemented`.
Why: teardown/reconciliation code that deletes a remediation exception and catches the not-found class gets an unmodeled error instead, and real deletions never take effect.
Fix: the DeleteRemediationExceptions handler and its state lookup (.../src/ops/delete_remediation_exceptions.rs).
Traces: config.delete-remediation-exceptions-nonexistent-rejected (expected NoSuchRemediationExceptionException, got 'NotImplemented').

Cause 5 of 10: DeliverConfigSnapshot.
AWS: returns configSnapshotId and models NoAvailableConfigurationRecorderException, NoRunningConfigurationRecorderException and NoSuchDeliveryChannelException, all client/400 (Smithy: DeliverConfigSnapshot). With a delivery channel present but the recorder stopped, the call is rejected 400 NoRunningConfigurationRecorderException (recorded message, an example: "Configuration recorder is turned off. Turn on configuration recorder."); with the recorder running it returns a configSnapshotId.
Emulator: answers `NotImplemented`.
Why: on-demand snapshot delivery is entirely unavailable, and the recorder-state precondition is not reportable.
Fix: the DeliverConfigSnapshot handler, its recorder/channel precondition checks and snapshot-id generation (.../src/ops/deliver_config_snapshot.rs).
Traces: config.deliver-config-snapshot (assert code == "NoRunningConfigurationRecorderException" got 'NotImplemented').

Cause 6 of 10: DeleteConfigurationAggregator.
AWS: models exactly one error, NoSuchConfigurationAggregatorException (client/400), with an empty output (Smithy: DeleteConfigurationAggregator). Deleting an aggregator that does not exist is a 400 NoSuchConfigurationAggregatorException; deleting a real one removes it from DescribeConfigurationAggregators.
Emulator: answers `NotImplemented`.
Why: aggregators cannot be torn down and the absence boundary is unreportable, so cleanup leaks state and clients cannot detect double deletion.
Fix: the DeleteConfigurationAggregator handler and its state lookup (.../src/ops/delete_configuration_aggregator.rs).
Traces: config.op-delete-aggregator-nonexistent-and-bad-name (assert code == "NoSuchConfigurationAggregatorException" got 'NotImplemented').

Cause 7 of 10: StartConfigRulesEvaluation.
AWS: takes ConfigRuleNames (1..25), returns an empty response, and models InvalidParameterValueException, LimitExceededException, NoSuchConfigRuleException and ResourceInUseException (all client/400) (Smithy: StartConfigRulesEvaluation). Re-evaluating an existing rule succeeds (and drives a fresh evaluation); naming a nonexistent rule is a modeled 400.
Emulator: answers `NotImplemented`.
Why: there is no way to trigger a re-evaluation, so every flow that asks Config to re-evaluate and then reads the verdict stalls, and the four modeled rejection classes are unreachable.
Fix: the StartConfigRulesEvaluation handler and the evaluation trigger it drives (.../src/ops/start_config_rules_evaluation.rs).
Traces: config.overly-permissive-put-evaluations-invalid-result-token ("unexpected error starting rule evaluation: NotImplemented").

Cause 8 of 10: PutEvaluations.
AWS: requires ResultToken, accepts up to 100 Evaluations, returns FailedEvaluations, and models InvalidParameterValueException, InvalidResultTokenException and NoSuchConfigRuleException (all client/400) (Smithy: PutEvaluations; InvalidResultTokenException doc: "The specified ResultToken is not valid."). A PutEvaluations carrying a result token that was not issued by Config is rejected 400 InvalidResultTokenException (recorded message, an example: "Result Token provided is invalid").
Emulator: answers `NotImplemented`.
Why: custom-rule evaluation reporting does not work at all, and a Lambda-backed custom rule can never publish a verdict; an invalid token is also not distinguishable from a transport failure.
Fix: the PutEvaluations handler, its ResultToken validation and the evaluation-result state it writes (.../src/ops/put_evaluations.rs).
Traces: config.start-config-rules-evaluation-transition (assert code == "InvalidResultTokenException" got 'NotImplemented').

Cause 9 of 10: PutExternalEvaluation.
AWS: requires ConfigRuleName and ExternalEvaluation and models exactly InvalidParameterValueException and NoSuchConfigRuleException, both client/400 (Smithy: PutExternalEvaluation). Two distinct boundaries are modeled: a rule name that does not exist at all is NoSuchConfigRuleException, while an existing rule that is not a Config Process Check rule is InvalidParameterValueException (the refusal is about the rule's type, not its existence).
Emulator: answers `NotImplemented` for both, collapsing the two boundaries into one unmodeled error.
Why: external (process-check) evaluation reporting is unavailable, and a client cannot tell "no such rule" from "wrong rule type" — the two cases demand different remediation.
Fix: the PutExternalEvaluation handler and its rule-existence/rule-type validation (.../src/ops/put_external_evaluation.rs).
Traces: config.external-evaluation-requires-process-check-rule, config.put-external-evaluation-non-process-check-rule-rejected.

Cause 10 of 10: SelectAggregateResourceConfig.
AWS: requires Expression (1..4096) and ConfigurationAggregatorName, is paginated on NextToken/Results, and models InvalidExpressionException, InvalidLimitException, InvalidNextTokenException and NoSuchConfigurationAggregatorException (all client/400) (Smithy: SelectAggregateResourceConfig). A nonexistent aggregator is 400 NoSuchConfigurationAggregatorException; a syntactically malformed expression against a real aggregator is 400 InvalidExpressionException (recorded message, an example: "syntax error at line 1, column 19"); a well-formed query answers Results.
Emulator: answers `NotImplemented`.
Why: the aggregate query surface is unusable and malformed queries cannot be distinguished from missing aggregators or from an empty result set.
Fix: the SelectAggregateResourceConfig handler, its expression parser and aggregator lookup (.../src/ops/select_aggregate_resource_config.rs).
Traces: config.select-aggregate-resource-config-errors (assert code == "NoSuchConfigurationAggregatorException" got 'NotImplemented').

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator was serving: it returned AWS-shaped JSON 1.1 error envelopes; bootstrap_signal false; 7/179 checks passed).

All ten causes present as the same observable: the operation's response carries Error.Code == "NotImplemented" with message "operation <Op> is scaffolded but not implemented in this emulator", where AWS returns either the modeled 2xx output or a modeled 400 client error.

Cause 1 — config.associate-disassociate-resource-types:
  AssertionError: NotImplemented   (at `assert code == "ValidationException", code`)
  expected: 400 ValidationException for AssociateResourceTypes against an ALL_SUPPORTED-strategy recorder, then a successful associate/disassociate round trip visible in DescribeConfigurationRecorders.

Cause 2 — config.batch-get-aggregate-resource-config-nonexistent-aggregator-rejected:
  AssertionError: expected NoSuchConfigurationAggregatorException, got 'NotImplemented'

Cause 3 — config.connector-configuration-payload-round-trip:
  AssertionError: expected ValidationException, got 'NotImplemented'   (PutConnector with a malformed tenant identifier)

Cause 4 — config.delete-remediation-exceptions-nonexistent-rejected:
  AssertionError: expected NoSuchRemediationExceptionException, got 'NotImplemented'

Cause 5 — config.deliver-config-snapshot:
  AssertionError: NotImplemented   (at `assert code == "NoRunningConfigurationRecorderException", code`)

Cause 6 — config.op-delete-aggregator-nonexistent-and-bad-name:
  AssertionError: NotImplemented   (at `assert code == "NoSuchConfigurationAggregatorException", code`)

Cause 7 — config.overly-permissive-put-evaluations-invalid-result-token:
  AssertionError: unexpected error starting rule evaluation: NotImplemented   (StartConfigRulesEvaluation)

Cause 8 — config.start-config-rules-evaluation-transition:
  AssertionError: NotImplemented   (at `assert code == "InvalidResultTokenException", code`, PutEvaluations with ResultToken="bogus-result-token-<rule>")

Cause 9 — config.external-evaluation-requires-process-check-rule:
  AssertionError: PutExternalEvaluation against a nonexistent rule must be NoSuchConfigRuleException; got 'NotImplemented'
  config.put-external-evaluation-non-process-check-rule-rejected:
  AssertionError: PutExternalEvaluation against an existing non-process-check rule must be rejected with InvalidParameterValueException ...; got 'NotImplemented'

Cause 10 — config.select-aggregate-resource-config-errors:
  AssertionError: NotImplemented   (at `assert code == "NoSuchConfigurationAggregatorException", code`)

Model grounding: services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json — every error cited above is errorKind client with httpError 400 (defaulted).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the behavioral suite simparity-behavioral-aws-v1.
3. Re-replay exactly the check ids in trace_ids; each one calls one of the ten operations and asserts on the modeled response/error class.
4. Equivalently, by hand against the booted emulator: config associate-resource-types, config batch-get-aggregate-resource-config --configuration-aggregator-name <absent>, config put-connector, config delete-remediation-exceptions, config deliver-config-snapshot, config delete-configuration-aggregator --configuration-aggregator-name <absent>, config start-config-rules-evaluation, config put-evaluations --result-token bogus, config put-external-evaluation, config select-aggregate-resource-config — each returns Error.Code NotImplemented instead of the modeled output or modeled 400.
