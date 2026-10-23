Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.conformance-pack-lifecycle config.put-conformance-pack-invalid-template-rejected config.select-and-batch-get-resource-config config.get-aggregate-resource-config-not-discovered-rejected config.get-aggregate-resource-config-oversized-item-rejected config.third-party-service-linked-recorder-creation-validation config.third-party-service-linked-recorder-direct-customer-call-rejected
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.conformance-pack-lifecycle|run#8|AssertionError|assert code == "ConformancePackTemplateValidationException", code
Issue-Key: config.put-conformance-pack-invalid-template-rejected|run#5|AssertionError|assert code == "ConformancePackTemplateValidationException", (
Issue-Key: config.select-and-batch-get-resource-config|run#3|AssertionError|assert code == "InvalidExpressionException", (
Issue-Key: config.get-aggregate-resource-config-not-discovered-rejected|run#11|AssertionError|assert code == "ResourceNotDiscoveredException", f"expected ResourceNotDiscoveredException, got {code}"
Issue-Key: config.get-aggregate-resource-config-oversized-item-rejected|run#11|AssertionError|assert code_short == "ResourceNotDiscoveredException", f"expected ResourceNotDiscoveredException, got {code_short}"
Issue-Key: config.third-party-service-linked-recorder-creation-validation|run#12|AssertionError|assert control_code == "ValidationException", control_code
Issue-Key: config.third-party-service-linked-recorder-direct-customer-call-rejected|run#9|AssertionError|assert code == "ValidationException", code
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Config: four more operations still answer NotImplemented on paths the existing stub issues do not carry (PutConformancePack, SelectResourceConfig, GetAggregateResourceConfig, PutThirdPartyServiceLinkedConfigurationRecorder) (4 causes)

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutConformancePack

## Description
Four operations answer the emulator-specific error code `NotImplemented` ("operation X is scaffolded but not implemented in this emulator") instead of their modeled output or modeled 400 client error. Open stub issues exist for these operations (20261022T190340Z-config-putconformancepack-stub.md, -config-selectresourceconfig-stub.md, -config-putthirdpartyservicelinkedconfigurationrecorder-stub.md, and the suite's auto-filed GetAggregateResourceConfig stub), but none of them carries the replay keys of the seven checks listed here, so these rejection paths are not gated by any filed work item. FOUR CAUSES must be closed.

Cause 1 of 4: PutConformancePack.
AWS: PutConformancePack returns ConformancePackArn and models ConformancePackTemplateValidationException, InsufficientPermissionsException, MaxNumberOfConformancePacksExceededException, ResourceInUseException and ValidationException (client/400). Recorded behavior: a TemplateBody that is not valid YAML/JSON is refused 400 ConformancePackTemplateValidationException with the message "Template passed in the input parameter is invalid", and no pack is left behind for that name; a valid template creates the pack and DescribeConformancePackStatus reaches CREATE_COMPLETE.
Emulator: answers NotImplemented, so neither the refusal nor the creation happens.
Why: conformance packs cannot be created at all and the template-validation boundary is unobservable; the error code returned does not exist in the Config API, so SDK error handling for the modeled classes is dead code.
Fix: the PutConformancePack handler, its template validation and the pack state behind DescribeConformancePackStatus (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_conformance_pack.rs).
Traces: config.conformance-pack-lifecycle, config.put-conformance-pack-invalid-template-rejected.

Cause 2 of 4: SelectResourceConfig.
AWS: SelectResourceConfig models InvalidExpressionException, InvalidLimitException and InvalidNextTokenException (client/400) and returns Results/QueryInfo. Recorded behavior: a syntactically malformed Expression is refused 400 InvalidExpressionException — it is NOT answered with an empty result set.
Emulator: answers NotImplemented.
Why: a client cannot tell a malformed query from a query that matched nothing, and the whole resource-query surface is unavailable.
Fix: the SelectResourceConfig handler and its expression parser (.../src/ops/select_resource_config.rs).
Traces: config.select-and-batch-get-resource-config.

Cause 3 of 4: GetAggregateResourceConfig.
AWS: GetAggregateResourceConfig models NoSuchConfigurationAggregatorException, OversizedConfigurationItemException, ResourceNotDiscoveredException and ValidationException (client/400) and otherwise returns ConfigurationItem. Recorded behavior: a resource identifier that the aggregator has not discovered is refused 400 ResourceNotDiscoveredException.
Emulator: answers NotImplemented for both the not-discovered and the oversized-item legs.
Why: the aggregate read surface is unusable and the two distinct modeled boundaries collapse into one unmodeled error.
Fix: the GetAggregateResourceConfig handler and its aggregator/inventory lookup (.../src/ops/get_aggregate_resource_config.rs).
Traces: config.get-aggregate-resource-config-not-discovered-rejected, config.get-aggregate-resource-config-oversized-item-rejected.

Cause 4 of 4: PutThirdPartyServiceLinkedConfigurationRecorder.
AWS: the operation models ConflictException, InsufficientPermissionsException, LimitExceededException and ValidationException (client/400). Recorded behavior: a ServicePrincipal that is malformed, and equally a well-formed but unrecognized control principal, are both refused 400 ValidationException; a direct customer call for a principal the caller does not own is refused the same way.
Emulator: answers NotImplemented, so none of the three refusal paths is reachable.
Why: the third-party recorder surface cannot be exercised and a client sees an error code that is not in the API instead of the modeled ValidationException.
Fix: the PutThirdPartyServiceLinkedConfigurationRecorder handler and its ServicePrincipal validation (.../src/ops/put_third_party_service_linked_configuration_recorder.rs).
Traces: config.third-party-service-linked-recorder-creation-validation, config.third-party-service-linked-recorder-direct-customer-call-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from mainline (emulator serving; 30/179 passed). None of these seven checks was marked ALREADY FILED or OWNED by the replay report.

Cause 1:
  config.conformance-pack-lifecycle: AssertionError: NotImplemented (at `assert code == "ConformancePackTemplateValidationException", code`; call: put_conformance_pack(ConformancePackName=<name>, TemplateBody="not: [ valid :: yaml {{"), expected message "Template passed in the input parameter is invalid")
  config.put-conformance-pack-invalid-template-rejected: AssertionError: expected ConformancePackTemplateValidationException, got 'NotImplemented'

Cause 2:
  config.select-and-batch-get-resource-config: AssertionError: a syntactically malformed Select expression must be refused with InvalidExpressionException, not silently answered with an empty result set; got 'NotImplemented'

Cause 3:
  config.get-aggregate-resource-config-not-discovered-rejected: AssertionError: expected ResourceNotDiscoveredException, got NotImplemented
  config.get-aggregate-resource-config-oversized-item-rejected: AssertionError: expected ResourceNotDiscoveredException, got NotImplemented

Cause 4:
  config.third-party-service-linked-recorder-creation-validation: AssertionError: NotImplemented (at `assert control_code == "ValidationException"`)
  config.third-party-service-linked-recorder-direct-customer-call-rejected: AssertionError: NotImplemented (at `assert code == "ValidationException"`)
  (the sibling check config.put-third-party-service-linked-recorder-invalid-principal-rejected reports the full envelope: Error.Code 'NotImplemented', message 'operation PutThirdPartyServiceLinkedConfigurationRecorder is scaffolded but not implemented in this emulator')

Model grounding: com-amazonaws-configservice-starlingdoveservice.json — every error named above is errorKind client with httpError 400 (defaulted).

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline; replay simparity-behavioral-aws-v1 and re-replay the check ids in trace_ids.
2. config put-conformance-pack --conformance-pack-name x --template-body 'not: [ valid :: yaml {{' — Error.Code NotImplemented instead of ConformancePackTemplateValidationException.
3. config select-resource-config --expression 'SELECT WHERE' — Error.Code NotImplemented instead of InvalidExpressionException.
4. config get-aggregate-resource-config against a real aggregator with an undiscovered ResourceIdentifier — Error.Code NotImplemented instead of ResourceNotDiscoveredException.
5. config put-third-party-service-linked-configuration-recorder with a malformed and with a well-formed-but-unrecognized ServicePrincipal — Error.Code NotImplemented instead of ValidationException.
