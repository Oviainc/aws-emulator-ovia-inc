Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicediscovery.register-deregister-instance-lifecycle servicediscovery.deregister-instance-not-registered-error servicediscovery.custom-health-status-not-configured-error servicediscovery.custom-health-status-without-custom-health-config-rejected servicediscovery.delete-namespace-in-use-conflict servicediscovery.e2e-service-discovery-full-lifecycle-flow servicediscovery.create-service-namespace-not-found servicediscovery.get-operation-not-found servicediscovery.tag-resource-not-found-error servicediscovery.tag-untag-resource-lifecycle servicediscovery.create-service-lifecycle
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery|DeregisterInstance|InstanceNotFound
Issue-Key: servicediscovery.deregister-instance-not-registered-error|run#24|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: servicediscovery.custom-health-status-not-configured-error|_error#17|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: servicediscovery.custom-health-status-without-custom-health-config-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: servicediscovery.delete-namespace-in-use-conflict|run#17|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: servicediscovery.e2e-service-discovery-full-lifecycle-flow|run#27|AssertionError|assert ns_delete_blocked_message == "Namespace has associated services; delete the services before deleting the namespace", f"unexpected ResourceInUse message:
Issue-Key: servicediscovery.create-service-namespace-not-found|run#13|AssertionError|assert message == bogus_ns, f"expected the message to name the missing namespace {bogus_ns!r}, got {message!r}"
Issue-Key: servicediscovery.get-operation-not-found|run#13|AssertionError|assert message == "", f"expected GetOperation's OperationNotFound to carry an empty message, got {message!r}"
Issue-Key: servicediscovery.tag-resource-not-found-error|run#17|AssertionError|assert message == "", f"expected TagResource's ResourceNotFoundException to carry an empty message, got {message!r}"
Issue-Key: servicediscovery.tag-untag-resource-lifecycle|run#25|AssertionError|assert tag_message == "", f"expected TagResource's ResourceNotFoundException to carry an empty message, got {tag_message!r}"
Issue-Key: servicediscovery.create-service-lifecycle|run#24|AssertionError|assert reused_service_id == service_id, (
Issue-Commit: 2bfbb03c7bd3982d98fd2c53f75176784e229027

# Cloud Map (servicediscovery): 8 net-new behavioral causes — DeregisterInstance is not idempotent, GetInstancesHealthStatus answers 200/UNKNOWN where AWS refuses, and six error-message/identity rules diverge

> _Note: this issue was identified against the code base at commit `2bfbb03c7bd3982d98fd2c53f75176784e229027`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeregisterInstance

## Description
EIGHT independent, net-new root causes in the servicediscovery (Cloud Map) emulator, all grounded in failing checks of the behavioral replay against the bootstrapped service. They are numbered below and EACH must be closed before this issue is submitted: Cause 1 of 8 .. Cause 8 of 8, each with its own failing check ids. Model citations use services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/smithy-model/com-amazonaws-servicediscovery-route53autonaming-v20170314.json.

Cause 1 of 8: DeregisterInstance is not idempotent — a second deregister of an instance id that was registered and already deregistered under the same service is refused with InstanceNotFound instead of returning an operation that reaches SUCCESS.
AWS: Smithy DeregisterInstance{ServiceId: Arn (required), InstanceId: ResourceId (required)} -> {OperationId}; errors DuplicateRequest, InstanceNotFound, InvalidInput, ResourceInUse, ServiceNotFound. Recorded behavior only: calling DeregisterInstance again for the same ServiceId/InstanceId after the first deregistration has completed returns 200 with an OperationId whose GetOperation reaches Status 'SUCCESS' (Type DEREGISTER_INSTANCE); InstanceNotFound is reserved for an InstanceId that was NEVER registered under that service. Hypothesis: AWS treats the deregistration as a declarative 'not present' request rather than a delete of a row that must exist.
Emulator: the second call is refused with InstanceNotFound (Message = the instance id), so the check aborts.
Why: every retry and teardown path breaks — a client that retries a deregister after a timeout, or a cleanup that deregisters unconditionally, gets a hard ClientError where AWS answers an operation it can poll to SUCCESS.
Fix: DeregisterInstance's handler / the instance state it reads — a missing instance under an EXISTING service that had it registered must produce a successful DEREGISTER_INSTANCE operation, not InstanceNotFound.
Traces: servicediscovery.register-deregister-instance-lifecycle (every earlier assertion in the check — the async register operation, GetInstance/ListInstances visibility, the first deregister and its InstanceNotFound-with-instance-id message — passes).

Cause 2 of 8: DeregisterInstance's InstanceNotFound Message for an id never registered under the service is the bare instance id instead of AWS's sentence.
AWS: recorded message template "instanceId <InstanceId> doesn't exists for service <service ARN>" — fixed text 'instanceId ', " doesn't exists for service " (AWS's own grammar, including the missing apostrophe-s agreement, is literal); variable parts: the InstanceId taken from the request and the service ARN built from the requested ServiceId (arn:aws:servicediscovery:<region>:<account>:service/<serviceId>).
Emulator: InstanceNotFound whose Message is just the instance id.
Why: this template is the only place AWS reports WHICH service the lookup was scoped to; clients and tests matching the documented refusal text diverge. Distinct from the open wire issue 20261029T203804Z-servicediscovery-cloud-map-instance-level-operations-report-the-wrong.md, whose Cause 3 covers DeregisterInstance when the SERVICE is missing (Message = the service ARN alone); this is the service-exists/instance-never-registered path.
Fix: the InstanceNotFound construction in DeregisterInstance's handler (per-operation template, not the shared not-found prose).
Traces: servicediscovery.deregister-instance-not-registered-error (the InstanceNotFound code itself is correct).

Cause 3 of 8: GetInstancesHealthStatus answers HTTP 200 with an UNKNOWN placeholder for an instance of a service created WITHOUT HealthCheckCustomConfig, where AWS refuses with InstanceNotFound.
AWS: Smithy GetInstancesHealthStatus{ServiceId: Arn (required), Instances: InstanceIdList, MaxResults, NextToken} -> {Status: InstanceHealthStatusMap, NextToken}; errors InstanceNotFound, InvalidInput, ServiceNotFound. Recorded behavior: a service with no HealthCheckCustomConfig (and no HealthCheckConfig) is never indexed by the custom health-check subsystem, so reading health for one of its instances is refused with InstanceNotFound, Message "Instance(s) not found for service '<service ARN>' when trying to retreive: <requested scope>" — fixed text 'Instance(s) not found for service ', the single-quoted ARN, ' when trying to retreive: ' (AWS's own misspelling of 'retreive' is part of the literal template); variable parts: the service ARN built from the requested ServiceId and the requested instance ids (the literal 'All instances' when Instances is omitted). The emulator already answers the sibling UpdateInstanceCustomHealthStatus call correctly (ServiceNotFound, "no service '<service ARN>'"), so only the read path diverges.
Emulator: HTTP 200 with {"Status": {"<instanceId>": "UNKNOWN"}}.
Why: a client cannot distinguish 'not tracked for custom health' from 'tracked but health unknown' — the two states drive different remediation. Negative tests of the refusal pass vacuously against the emulator and fail against AWS.
Fix: GetInstancesHealthStatus' handler — gate the read on the target service carrying a health-check configuration and raise InstanceNotFound with the template above; a service that DOES carry HealthCheckCustomConfig must keep answering UNKNOWN for a freshly registered instance.
Traces: servicediscovery.custom-health-status-not-configured-error, servicediscovery.custom-health-status-without-custom-health-config-rejected.

Cause 4 of 8: DeleteNamespace's ResourceInUse Message is emulator prose instead of AWS's fixed sentence.
AWS: Smithy DeleteNamespace{Id (required)} -> {OperationId}; errors DuplicateRequest, InvalidInput, NamespaceNotFound, ResourceInUse. Recorded message for a namespace that still contains a service: 'Namespace has associated services; delete the services before deleting the namespace' — the WHOLE message is fixed text, with no namespace id and no service count in it.
Emulator: "The namespace <namespaceId> can't be deleted because it still contains <n> service(s)." — a different template whose variable parts are the namespace id and a count AWS never reports.
Why: clients and tests that assert the documented conflict text diverge; the id/count the emulator embeds are not part of AWS's message.
Fix: the ResourceInUse message constant used by DeleteNamespace's handler. The open behavioral issue 20261023T124702Z-servicediscovery-cloud-map-servicediscovery-8-fidelity-divergences-missing-dnsconfig.md names this same template inside its Cause 8 for check servicediscovery.delete-namespace-with-active-service-rejected (which this replay reports as OWNED by it); the two checks below are NOT carried by that issue's replay keys and still fail, so they are filed here and the fix must satisfy them too.
Traces: servicediscovery.delete-namespace-in-use-conflict, servicediscovery.e2e-service-discovery-full-lifecycle-flow.

Cause 5 of 8: CreateService's NamespaceNotFound carries an EMPTY Message where AWS echoes the requested NamespaceId.
AWS: Smithy CreateService{Name (required), NamespaceId: Arn, CreatorRequestId, Description, DnsConfig, HealthCheckConfig, HealthCheckCustomConfig, Tags, Type} -> {Service}; errors InvalidInput, NamespaceNotFound, ResourceLimitExceeded, ServiceAlreadyExists, TooManyTagsException. Recorded message rule for this operation: the Message IS the NamespaceId value taken from the request (no surrounding prose).
Emulator: NamespaceNotFound with an empty Message.
Why: the request-echo is how a caller correlates the failure with the namespace it asked for when several creates are in flight; an empty message leaves botocore's exception text, logs and user-facing errors blank. Related to the open issue 20261029T172728Z-servicediscovery-cloud-map-servicediscovery-5-fidelity-divergences-servicenotfound-namespacenotfound.md, which carries the Update*Namespace checks for this same empty-message defect; CreateService is a separate operation and a separate failing check that issue does not carry.
Fix: the NamespaceNotFound construction in CreateService's handler.
Traces: servicediscovery.create-service-namespace-not-found (the positive control — CreateService against a real namespace — succeeds, and the NamespaceNotFound code is correct).

Cause 6 of 8: GetOperation's OperationNotFound carries emulator prose where AWS sends no message text at all.
AWS: Smithy GetOperation{OperationId (required), OwnerAccount} -> {Operation}; errors InvalidInput, OperationNotFound. Recorded message rule: OperationNotFound from GetOperation has NO Message text (a client reads it as empty) — AWS deliberately reports nothing beyond the error code for this class.
Emulator: 'Operation <operationId> was not found.'
Why: tests and clients that key off the documented empty message see unexpected text; the emulator is inventing a template AWS does not serve.
Fix: the OperationNotFound construction in GetOperation's handler (and the shared not-found message helper it uses).
Traces: servicediscovery.get-operation-not-found.

Cause 7 of 8: TagResource/UntagResource's ResourceNotFoundException carries emulator prose where AWS sends no message text.
AWS: Smithy TagResource{ResourceARN (required, length 1..1011), Tags (required)} -> {}; errors InvalidInput, ResourceNotFoundException, TooManyTagsException (UntagResource is modelled with the same not-found error). Recorded message rule: ResourceNotFoundException for a tagging call against a namespace/service ARN that does not exist carries NO Message text.
Emulator: "The operation can't be completed because the resource <resourceArn> was not found."
Why: as cause 6 — the emulator serves a template AWS does not, so assertions on the documented refusal fail and the ARN echo is not something a client may rely on. Distinct from the open wire issue 20261029T203804Z-servicediscovery-cloud-map-400-responses-carry-wrong-or-absent.md, whose Cause 4 covers TagResource's tag-LIMIT message, not its not-found message.
Fix: the ResourceNotFoundException construction shared by TagResource/UntagResource (and ListTagsForResource if it uses the same helper).
Traces: servicediscovery.tag-resource-not-found-error, servicediscovery.tag-untag-resource-lifecycle.

Cause 8 of 8: a Service Id is minted at random instead of being derived from Name + NamespaceId, so re-creating a deleted service with the same Name in the same namespace returns a different Id.
AWS: recorded behavior only — after DeleteService, CreateService with the SAME Name in the SAME NamespaceId returns the SAME Service Id as the first create. Hypothesis: AWS derives the service identifier from Name+NamespaceId rather than from a random sequence. Nothing in the model constrains the id beyond ResourceId.
Emulator: the second CreateService returns a fresh random srv-... id.
Why: an IaC layer or client that caches/reconstructs the Service Id for a known Name+Namespace pair, or that asserts an idempotent re-create, diverges after any delete/re-create cycle.
Fix: the Service Id derivation in CreateService (the id allocator it calls in the service's state module).
Traces: servicediscovery.create-service-lifecycle (every other assertion — synchronous create, GetService shape, empty-message ServiceNotFound after delete — passes). The open issue 20261023T124702Z-servicediscovery-cloud-map-servicediscovery-8-fidelity-divergences-missing-dnsconfig.md describes this same cause as its Cause 6, but its replay key for this check no longer matches, so the check is still reported as an unowned failure here; closing either must satisfy servicediscovery.create-service-lifecycle.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against this checkout: 36/93 passed; servicediscovery 33/74 (45%). Per cause, the failing assertion exactly as replayed (expected = recorded AWS behavior):

Cause 1 — servicediscovery.register-deregister-instance-lifecycle: botocore.errorfactory.InstanceNotFound: An error occurred (InstanceNotFound) when calling the DeregisterInstance operation: sim2real-behav-sd-inst-812cfcb6f9ff, raised by the re-deregister of an already-deregistered instance. Expected: 200 + OperationId whose GetOperation reaches Status 'SUCCESS'.
Cause 2 — servicediscovery.deregister-instance-not-registered-error: AssertionError: expected "instanceId sim2real-behav-sd-inst-never-2db43d95aaef doesn't exists for service arn:aws:servicediscovery:us-west-2:000000000000:service/srv-z2d7acnmdqdfg224", got 'sim2real-behav-sd-inst-never-2db43d95aaef' (ids/region/account in that line are this run's values filling the template).
Cause 3 — servicediscovery.custom-health-status-not-configured-error and servicediscovery.custom-health-status-without-custom-health-config-rejected: AssertionError: expected the call to be rejected, but it succeeded: {'Status': {'<instanceId>': 'UNKNOWN'}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} on get_instances_health_status. Expected: InstanceNotFound, Message "Instance(s) not found for service '<service ARN>' when trying to retreive: <instanceId>". In both checks the preceding UpdateInstanceCustomHealthStatus assertions (ServiceNotFound + "no service '<arn>'") PASS.
Cause 4 — servicediscovery.delete-namespace-in-use-conflict: AssertionError: expected 'Namespace has associated services; delete the services before deleting the namespace', got "The namespace ns-woghvvv06bpc54wx can't be deleted because it still contains 1 service(s)."; servicediscovery.e2e-service-discovery-full-lifecycle-flow: same, got "The namespace ns-624w95p6g2yt5we0 can't be deleted because it still contains 1 service(s)."
Cause 5 — servicediscovery.create-service-namespace-not-found: AssertionError: expected the message to name the missing namespace 'ns-sim2realbeha', got ''.
Cause 6 — servicediscovery.get-operation-not-found: AssertionError: expected GetOperation's OperationNotFound to carry an empty message, got 'Operation sim2real-behav-op-bogus-7f8112e2e891 was not found.'
Cause 7 — servicediscovery.tag-resource-not-found-error: AssertionError: expected TagResource's ResourceNotFoundException to carry an empty message, got "The operation can't be completed because the resource arn:aws:servicediscovery:us-west-2:000000000000:namespace/ns-tf6u22nr01q7isy4 was not found."; servicediscovery.tag-untag-resource-lifecycle: same text for ns-uyirqjuwd0yq6lqa.
Cause 8 — servicediscovery.create-service-lifecycle: AssertionError: recreating 'sim2real-behav-sd-svc-eca56530ad86' in the same namespace after delete should reuse Service Id 'srv-43wi0nghugjj5k8k', got 'srv-h516g29accp1klkk'.

## Steps to Reproduce
Boot the emulator from this checkout and replay the behavioral checks listed in trace_ids, or drive the endpoint with botocore (sd = boto3.client('servicediscovery', endpoint_url=<emulator>)); read e.response['Error']['Code'] and ['Message']:
1. Cause 1: create an HTTP namespace (poll its operation to SUCCESS), create a service, register an instance, deregister it and poll to SUCCESS, then call sd.deregister_instance(ServiceId=svc, InstanceId=<same id>) again -> emulator raises InstanceNotFound; AWS returns an OperationId that reaches SUCCESS.
2. Cause 2: sd.deregister_instance(ServiceId=<existing svc>, InstanceId='<never-registered id>') -> compare Message with "instanceId <that id> doesn't exists for service arn:aws:servicediscovery:<region>:<account>:service/<svc>".
3. Cause 3: create a service WITHOUT HealthCheckCustomConfig, register an instance, then sd.get_instances_health_status(ServiceId=svc, Instances=[inst]) -> emulator 200 {'Status': {inst: 'UNKNOWN'}}; AWS refuses with InstanceNotFound and the "Instance(s) not found for service '<arn>' when trying to retreive: <inst>" message. Repeat against a service created WITH HealthCheckCustomConfig={'FailureThreshold':1} to confirm that path still answers UNKNOWN/UNHEALTHY.
4. Cause 4: with one service still in the namespace, sd.delete_namespace(Id=ns) -> compare the ResourceInUse Message with 'Namespace has associated services; delete the services before deleting the namespace'.
5. Cause 5: sd.create_service(Name='<any>', NamespaceId='ns-doesnotexist') -> compare the NamespaceNotFound Message with the NamespaceId sent in the request.
6. Cause 6: sd.get_operation(OperationId='<bogus>') -> emulator 'Operation <id> was not found.'; AWS carries no message text.
7. Cause 7: sd.tag_resource(ResourceARN='arn:aws:servicediscovery:<region>:<account>:namespace/ns-doesnotexist', Tags=[{'Key':'k','Value':'v'}]) and the matching sd.untag_resource(...) -> emulator serves prose; AWS carries no message text.
8. Cause 8: create a service, sd.delete_service(Id=svc), then create the same Name in the same NamespaceId again and compare Service.Id with the first one.
