Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.disabled-operation-gating-across-readonly-apis opensearch.domain-config-change-lifecycle opensearch.domain-health-reporting opensearch.reserved-instance-offerings-readonly-semantics opensearch.reserved-instance-purchase-and-list
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.disabled-operation-gating-across-readonly-apis|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: opensearch.domain-config-change-lifecycle|run#17|AssertionError|assert cancel_code in ("ResourceNotFoundException", "ValidationException"), (
Issue-Key: opensearch.domain-health-reporting|run#15|AssertionError|assert code == "BaseException", f"expected BaseException, got {code}"
Issue-Key: opensearch.reserved-instance-offerings-readonly-semantics|run#18|AssertionError|assert code == 'ResourceNotFoundException', f'expected ResourceNotFoundException, got {code}'
Issue-Key: opensearch.reserved-instance-purchase-and-list|run#7|AssertionError|assert message == (
Issue-Commit: 7b52a53863c32195473bcccd44fb6923fe37ec7d

# opensearch control plane: StartDomainMaintenance and CancelDomainConfigChange answer NotImplemented, DescribeDomainHealth/reserved-instance reads return the wrong error class (5 causes)

> _Note: this issue was identified against the code base at commit `7b52a53863c32195473bcccd44fb6923fe37ec7d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartDomainMaintenance

## Description
Five distinct root causes in the opensearch service, all reached on an existing, Active domain (or on the account-level reserved-instance catalog) — none of them is covered by the already-filed scaffold-stub issues or by the two open CreateDomain/data-plane issues (20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md, 20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md), which name CreateDomain validation, the DomainStatus serializer, StartServiceSoftwareUpdate and the data plane. All five must be closed before this issue is resubmitted.

Cause 1 of 5: StartDomainMaintenance answers NotImplemented instead of the operation's modeled refusal.
AWS: StartDomainMaintenance(DomainName, Action=RESTART_SEARCH_PROCESS, NodeId=<a real Data NodeId just read from DescribeDomainNodes>) on a domain with a single data node is refused — the node-count gate is enforced through NodeId eligibility and the refusal is ResourceNotFoundException (recorded message example: "The requested node ID is invalid"). Smithy: opensearch#StartDomainMaintenance POST /2021-01-01/opensearch/domain/{DomainName}/domainMaintenance, errors = BaseException(400), DisabledOperationException(409), InternalException(500), ResourceNotFoundException(409), ValidationException(400). NotImplemented/501 is not in the operation's contract.
Emulator: the call returns error code 'NotImplemented' ("operation ... is scaffolded but not implemented"), so the check's assertion on ResourceNotFoundException fails with "expected ResourceNotFoundException, got 'NotImplemented'".
Why: a client cannot distinguish "this node/domain is not eligible for maintenance" from "the emulator has no handler". Negative tests for the 3-data-node requirement cannot be written, and no maintenance id is ever produced, so the whole maintenance surface (GetDomainMaintenanceStatus, ListDomainMaintenances) is unreachable.
Fix: the StartDomainMaintenance handler in the OpenSearch service crate — resolve the domain and the supplied NodeId and return the modeled ResourceNotFoundException for an ineligible node, never a 501/NotImplemented.
Traces: opensearch.disabled-operation-gating-across-readonly-apis (handler file: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/start_domain_maintenance.rs).

Cause 2 of 5: CancelDomainConfigChange answers NotImplemented instead of cancelling (or reporting no pending change).
AWS: after UpdateDomainConfig kicks off a change, CancelDomainConfigChange(DomainName) either succeeds (HTTP 200 with CancelledChangeIds/CancelledChangeProperties) or, when there is nothing pending, is refused with ResourceNotFoundException/ValidationException. Smithy: opensearch#CancelDomainConfigChange POST /2021-01-01/opensearch/domain/{DomainName}/config/cancel, output {CancelledChangeIds, CancelledChangeProperties, DryRun}, errors = BaseException(400), DisabledOperationException(409), InternalException(500), ResourceNotFoundException(409), ValidationException(400). NotImplemented/501 is not in the contract.
Emulator: the call returns error code 'NotImplemented' — "expected ResourceNotFoundException/ValidationException (no pending change) or success, got NotImplemented".
Why: a client that starts a blue/green config change has no way to abort it, and its error handling cannot tell "nothing to cancel" from "operation missing". The cancel half of the config-change lifecycle is simply absent.
Fix: the CancelDomainConfigChange handler in the OpenSearch service crate — operate on the domain's pending-change state and return the modeled success/refusal.
Traces: opensearch.domain-config-change-lifecycle (handler file: services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/cancel_domain_config_change.rs).

Cause 3 of 5: DescribeDomainHealth on a domain that does not exist returns the wrong error class.
AWS: DescribeDomainHealth for a nonexistent domain is refused with the generic envelope — error code BaseException, HTTP 400, message built from the request ("Domain not found: <DomainName>"), not ResourceNotFoundException. Smithy: opensearch#DescribeDomainHealth GET /2021-01-01/opensearch/domain/{DomainName}/health models both BaseException (httpError 400, defaulted) and ResourceNotFoundException (httpError 409), so both are in the model and the recording is what distinguishes them: this operation uses BaseException/400.
Emulator: the call is refused with error code ResourceNotFoundException (409) — "expected BaseException, got ResourceNotFoundException".
Why: the status code and error code are both wrong (400/BaseException vs 409/ResourceNotFoundException). Clients keying on the error code or retrying on 409 take the wrong branch, and SDK error-type matching (botocore errorfactory) raises a different exception class than on AWS.
Fix: the DescribeDomainHealth handler's missing-domain path in the OpenSearch service crate — raise the BaseException envelope with HTTP 400 and the "Domain not found: <name>" message derived from the request.
Traces: opensearch.domain-health-reporting.

Cause 4 of 5: DescribeReservedInstances filtered by an unknown ReservedInstanceId is refused as a validation error instead of a not-found.
AWS: DescribeReservedInstances(ReservedInstanceId=<well-formed GUID that does not exist>) is refused with ResourceNotFoundException (recorded message example: "Reservation id not found"). Smithy: opensearch#DescribeReservedInstances GET /2021-01-01/opensearch/reservedInstances, ReservedInstanceId is an httpQuery 'reservationId' GUID with @length 36 and the full UUID @pattern, and the operation models ResourceNotFoundException(409), ValidationException(400), DisabledOperationException(409), InternalException(500). The submitted id satisfies the modeled length/pattern, so validation has nothing to reject and the correct refusal is the not-found.
Emulator: the call is refused with error code ValidationException — "expected ResourceNotFoundException, got ValidationException".
Why: a syntactically valid id that merely does not exist is reported as a malformed request. A client cannot tell "you sent garbage" from "no such reservation", and code that treats ValidationException as non-retryable client-bug versus ResourceNotFoundException as an expected empty result behaves differently than on AWS.
Fix: the DescribeReservedInstances handler in the OpenSearch service crate — apply the GUID length/pattern validation first, then resolve the id against reserved-instance state and return ResourceNotFoundException when it is absent.
Traces: opensearch.reserved-instance-offerings-readonly-semantics, opensearch.reserved-instance-purchase-and-list.

Cause 5 of 5: PurchaseReservedInstanceOffering resolves the offering before applying the InstanceCount range constraint, so the modeled input validation never fires.
AWS: PurchaseReservedInstanceOffering(ReservedInstanceOfferingId=<unknown GUID>, ReservationName=<valid>) with InstanceCount omitted is refused by input validation on instanceCount BEFORE the offering id is resolved: ValidationException whose message is built from the request and names the member and its constraint — "1 validation error detected: Value '0' at 'instanceCount' failed to satisfy constraint: Member must have value greater than or equal to 1". Smithy: opensearch#PurchaseReservedInstanceOffering$InstanceCount targets InstanceCount with @range min 1, and ValidationException(400) is modeled.
Emulator: the same call is refused with ValidationException but on offering existence — message "Reserved Instance offering not found: 00000000-0000-0000-0000-000000000000" — proving the instanceCount >= 1 range check is not applied at all (the request never gets as far as a constraint violation).
Why: the requirement here is the ordering and existence of the modeled input validation, not the recorded wording: a client omitting a required count sees a resource error instead of the constraint error that tells it which member to fix, and any request that DOES name a real offering while omitting InstanceCount would be accepted by the emulator where AWS refuses it — a non-cancellable billing commitment created from input AWS rejects.
Fix: PurchaseReservedInstanceOffering's input validation in the OpenSearch service crate — enforce the modeled @range on InstanceCount (treating an absent value as 0) before the offering id is looked up.
Traces: opensearch.reserved-instance-purchase-and-list.

## Evidence
Cause 1: AssertionError: expected ResourceNotFoundException, got 'NotImplemented' — opensearch.start_domain_maintenance(DomainName=<1-data-node active domain>, Action='RESTART_SEARCH_PROCESS', NodeId=<NodeId from DescribeDomainNodes>). Modeled errors for StartDomainMaintenance: BaseException(400)/DisabledOperationException(409)/InternalException(500)/ResourceNotFoundException(409)/ValidationException(400); 'NotImplemented' is in none of them.
Cause 2: AssertionError: expected ResourceNotFoundException/ValidationException (no pending change) or success, got NotImplemented — opensearch.cancel_domain_config_change(DomainName=<domain with a change just started by UpdateDomainConfig, DescribeDomainChangeProgress Status=PENDING/PROCESSING>).
Cause 3: AssertionError: expected BaseException, got ResourceNotFoundException — opensearch.describe_domain_health(DomainName=<nonexistent>); AWS answers HTTP 400 code BaseException with message 'Domain not found: <name>', emulator answers code ResourceNotFoundException (modeled httpError 409).
Cause 4: AssertionError: expected ResourceNotFoundException, got ValidationException — opensearch.describe_reserved_instances(ReservedInstanceId='11111111-2222-3333-4444-555555555555') (a well-formed GUID; AWS: ResourceNotFoundException, message example 'Reservation id not found').
Cause 5: AssertionError: unexpected validation message: 'Reserved Instance offering not found: 00000000-0000-0000-0000-000000000000' — expected the modeled constraint refusal "1 validation error detected: Value '0' at 'instanceCount' failed to satisfy constraint: Member must have value greater than or equal to 1". Same error code (ValidationException), different rule: the emulator resolved the offering instead of validating InstanceCount's @range min 1.

## Steps to Reproduce
Cause 1: create a minimal domain with ClusterConfig.InstanceCount=1 and wait for Active; node_id = opensearch.describe_domain_nodes(DomainName=d)['DomainNodesStatusList'][0]['NodeId']; opensearch.start_domain_maintenance(DomainName=d, Action='RESTART_SEARCH_PROCESS', NodeId=node_id) -> NotImplemented (expected ResourceNotFoundException). Replay opensearch.disabled-operation-gating-across-readonly-apis.
Cause 2: create a domain, wait Active, opensearch.update_domain_config(DomainName=d, ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':2}); opensearch.describe_domain_change_progress(DomainName=d) -> Status PENDING/PROCESSING; opensearch.cancel_domain_config_change(DomainName=d) -> NotImplemented (expected 200, or ResourceNotFoundException/ValidationException if nothing is pending). Replay opensearch.domain-config-change-lifecycle.
Cause 3: opensearch.describe_domain_health(DomainName='sim2real-behav-os-missing-xyz') -> ResourceNotFoundException (expected HTTP 400, code BaseException, message 'Domain not found: sim2real-behav-os-missing-xyz'). Replay opensearch.domain-health-reporting.
Cause 4: opensearch.describe_reserved_instances(ReservedInstanceId='11111111-2222-3333-4444-555555555555') -> ValidationException (expected ResourceNotFoundException 'Reservation id not found'). Replay opensearch.reserved-instance-offerings-readonly-semantics.
Cause 5: opensearch.purchase_reserved_instance_offering(ReservedInstanceOfferingId='00000000-0000-0000-0000-000000000000', ReservationName='ri-xxxxx') with InstanceCount omitted -> ValidationException 'Reserved Instance offering not found: ...' (expected the instanceCount >= 1 constraint message). Replay opensearch.reserved-instance-purchase-and-list.

Replay the check ids listed in trace_ids.
