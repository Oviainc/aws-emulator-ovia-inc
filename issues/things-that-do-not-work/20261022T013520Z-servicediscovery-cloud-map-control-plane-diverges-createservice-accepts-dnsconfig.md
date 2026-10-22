Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: servicediscovery.create-service-dnsconfig-in-http-namespace-rejected servicediscovery.create-namespace-invalid-dns-name-rejected servicediscovery.namespace-already-exists-error servicediscovery.namespace-description-empty-string-preserved servicediscovery.get-namespace-not-found servicediscovery.service-already-exists-error servicediscovery.delete-namespace-in-use-conflict servicediscovery.delete-namespace-with-active-service-rejected servicediscovery.delete-service-in-use-conflict servicediscovery.e2e-service-discovery-full-lifecycle-flow servicediscovery.custom-health-status-not-configured-error servicediscovery.custom-health-status-without-custom-health-config-rejected servicediscovery.list-namespaces-invalid-filter-error
Issue-Shape-Id: com.amazonaws.servicediscovery#Route53AutoNaming_v20170314
Issue-Key: servicediscovery.create-service-dnsconfig-in-http-namespace-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: servicediscovery.create-namespace-invalid-dns-name-rejected|run#4|AssertionError|assert code == "InvalidInput", f"expected InvalidInput for name {bad_name!r}, got {code!r}"
Issue-Key: servicediscovery.namespace-already-exists-error|run#14|AssertionError|assert code == "DuplicateRequest", f"duplicate namespace name must be rejected as DuplicateRequest, got {code}"
Issue-Key: servicediscovery.namespace-description-empty-string-preserved|run#13|AssertionError|assert "Description" not in resp_with_empty, (
Issue-Key: servicediscovery.get-namespace-not-found|run#14|AssertionError|assert message == "", f"unexpected NamespaceNotFound message: {message!r}"
Issue-Key: servicediscovery.service-already-exists-error|run#16|AssertionError|assert error.get("Message") == "Service already exists.", (
Issue-Key: servicediscovery.delete-namespace-in-use-conflict|run#14|AssertionError|assert code == "ResourceInUse", f"namespace delete with active service should fail ResourceInUse, got {code}"
Issue-Key: servicediscovery.delete-namespace-with-active-service-rejected|run#8|AssertionError|assert code == "ResourceInUse", f"expected ResourceInUse, got {code}"
Issue-Key: servicediscovery.delete-service-in-use-conflict|run#16|AssertionError|assert code == "ResourceInUse", f"service delete with registered instance should fail ResourceInUse, got {code}"
Issue-Key: servicediscovery.e2e-service-discovery-full-lifecycle-flow|run#18|AssertionError|assert delete_blocked_code == "ResourceInUse", (
Issue-Key: servicediscovery.custom-health-status-not-configured-error|run#15|AssertionError|assert code == "ServiceNotFound", (
Issue-Key: servicediscovery.custom-health-status-without-custom-health-config-rejected|run#9|AssertionError|assert code == "ServiceNotFound", f"expected ServiceNotFound, got {code}"
Issue-Key: servicediscovery.list-namespaces-invalid-filter-error|run#3|AssertionError|assert code == "InvalidInput", f"expected InvalidInput for NAME+BETWEEN filter, got {code!r}"
Issue-Commit: 5b247f22dea7573e9fb6c4376c776b3a345a666f

# Cloud Map control plane diverges: CreateService accepts DnsConfig in an HTTP namespace, duplicate-namespace code wrong, empty Description echoed, error message rules wrong, and in-use/validation refusals answer 501

> _Note: this issue was identified against the code base at commit `5b247f22dea7573e9fb6c4376c776b3a345a666f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateService

## Description
Eight distinct root causes in the servicediscovery (AWS Cloud Map) emulator, all grounded in failing behavioral checks of this suite. They are bundled because they share one service; a fixer must close ALL EIGHT before submitting.

Cause 1 of 8 — CreateService accepts a DnsConfig element against an HTTP namespace.
AWS: Smithy CreateService models errors InvalidInput (HTTP 400), NamespaceNotFound (404), ResourceLimitExceeded (400), ServiceAlreadyExists (400), TooManyTagsException (400). An HTTP namespace has no hosted zone, so a DnsConfig is meaningless there and the call is refused with 400 InvalidInput; recorded message example: "When you create a service using a namespace that has a type of HTTP, you can't include a DnsConfig element." No service is created.
Emulator: returns HTTP 200 and creates the service, echoing Type=HTTP together with the DnsConfig the caller sent (observed Service {Id: srv-..., Type: 'HTTP', DnsConfig: {DnsRecords: [{Type: 'A', TTL: 60}]}}).
Why: a client that mis-wires a DNS record set onto an HTTP namespace gets a success and a service object that promises DNS records no resolver will ever answer; against real AWS the same deploy fails fast at CreateService.
Fix: CreateService's input validation/handler must reject DnsConfig when the parent namespace Type is HTTP, before any state mutation.
Traces: servicediscovery.create-service-dnsconfig-in-http-namespace-rejected.

Cause 2 of 8 — CreatePrivateDnsNamespace is a scaffolded stub, so neither its name validation nor its async operation exists.
AWS: Smithy CreatePrivateDnsNamespace.Name targets NamespaceNamePrivate with pattern ^(?!arn:)[!-~]{1,253}$ and length 0..253; the operation models InvalidInput (400), DuplicateRequest (409), NamespaceAlreadyExists (400), ResourceLimitExceeded (400), TooManyTagsException (400), and returns an OperationId that GetOperation drives to SUCCESS/FAIL. A Name containing a space violates the pattern and is refused with 400 InvalidInput before anything is created; recorded message example: "1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?!arn:)[!-~]{1,253}$".
Emulator: every CreatePrivateDnsNamespace call answers HTTP 501 with error code NotImplemented ("operation CreatePrivateDnsNamespace is scaffolded but not implemented in this emulator"); services/service-com-amazonaws-servicediscovery-route53autonaming-v20170314/src/ops/create_private_dns_namespace.rs still carries the SCAFFOLDED marker and returns Scaffolded::not_implemented. The check caught the ClientError, so the suite reported this as a mismatch rather than flagging it as an already-filed stub — no open issue names this operation.
Why: private DNS namespaces are the primary Cloud Map shape used by ECS service discovery; a client cannot create one at all, and a bad name is answered with a non-modeled 501/NotImplemented instead of 400/InvalidInput.
Fix: implement the CreatePrivateDnsNamespace handler (pattern validation -> InvalidInput; otherwise create the async Operation with a NAMESPACE target reachable through GetOperation).
Traces: servicediscovery.create-namespace-invalid-dns-name-rejected.

Cause 3 of 8 — a duplicate namespace Name is rejected with the wrong error code.
AWS: CreateHttpNamespace models both DuplicateRequest (HTTP 409) and NamespaceAlreadyExists (HTTP 400). Recorded behavior: a second CreateHttpNamespace with a Name already live in the account is refused with code DuplicateRequest, whose message names the already-completed create operation (example: "Another operation of type CreateHttpNamespace and id <opId> has completed"). Deleting the original frees the name and the identical call then succeeds.
Emulator: refuses the duplicate with code NamespaceAlreadyExists.
Why: clients branch on the error code — DuplicateRequest is retryable/idempotency-shaped (409) while NamespaceAlreadyExists is a 400 terminal validation failure, so retry logic and CFN-style reconcilers take the wrong path.
Fix: CreateHttpNamespace's duplicate-name detection in the operation handler/state must raise DuplicateRequest for a name already owned by a completed create.
Traces: servicediscovery.namespace-already-exists-error.

Cause 4 of 8 — an explicit empty-string Description is persisted and echoed instead of normalized away.
AWS: recorded behavior only (Description is ResourceDescription, length 0..1024, no default in the model). A namespace created with Description="" is indistinguishable on GetNamespace from one created with no Description at all: the field is absent from the Namespace structure. A non-empty Description round-trips.
Emulator: GetNamespace returns Description present with value '' for the namespace created with Description="".
Why: a client that tests presence of Description (or diffs a read against desired state) sees a field AWS never returns, producing permanent spurious drift on every reconcile.
Fix: CreateHttpNamespace/namespace state normalization must drop an empty-string Description so the read projection omits the member.
Traces: servicediscovery.namespace-description-empty-string-preserved.

Cause 5 of 8 — error Message rules diverge: the emulator synthesizes descriptive messages where AWS returns a fixed or empty one.
AWS: recorded behavior only, but the rule is fixed text, not request-derived. GetNamespace on a well-formed but never-created Id -> code NamespaceNotFound (404) with an EMPTY Message. A duplicate CreateService in the same namespace -> code ServiceAlreadyExists (400) with the constant Message "Service already exists." (the message does not name the service or the namespace).
Emulator: NamespaceNotFound carries a constructed message echoing the requested id (example: "No namespace exists with the specified ID ns-sim2realbeha."), and ServiceAlreadyExists carries a constructed message naming both resources (example: "The service <name> already exists in namespace ns-0ci80nu2mho4pr41."). The codes and statuses are correct; only the message rule is wrong.
Why: Cloud Map's not-found/conflict messages are fixed strings, not request echoes; clients and log scrapers built against AWS see text AWS never emits, and the emulator leaks request identifiers into bodies AWS keeps generic.
Fix: the error construction for NamespaceNotFound (GetNamespace handler/serializer) must emit no Message, and ServiceAlreadyExists (CreateService handler) must emit the constant message instead of formatting the request.
Traces: servicediscovery.get-namespace-not-found, servicediscovery.service-already-exists-error.

Cause 6 of 8 — in-use state is not enforced on delete: DeleteNamespace/DeleteService answer 501 instead of 409 ResourceInUse.
AWS: Smithy DeleteNamespace models ResourceInUse (HTTP 409), NamespaceNotFound (404), DuplicateRequest (409), InvalidInput (400); DeleteService models ResourceInUse (409), ServiceNotFound (404), InvalidInput (400). Deleting a namespace that still contains an ACTIVE service, or a service that still has a registered instance, is refused with ResourceInUse; the delete succeeds once the child is removed.
Emulator: both calls answer HTTP 501 with code NotImplemented, so the in-use precondition is never evaluated.
Why: teardown/reconcile logic that depends on ResourceInUse to retry-until-drained instead sees a non-modeled 501 and aborts; nothing ever proves the ordering constraint AWS enforces.
Fix: implement the DeleteNamespace and DeleteService handlers including the in-use precondition (namespace with live services; service with live instances) -> ResourceInUse. Open stub issues name these two operations and their files as bare stubs; this cause records the ResourceInUse behavior those stubs must implement, which the stub issues do not state.
Traces: servicediscovery.delete-namespace-in-use-conflict, servicediscovery.delete-namespace-with-active-service-rejected, servicediscovery.delete-service-in-use-conflict, servicediscovery.e2e-service-discovery-full-lifecycle-flow.

Cause 7 of 8 — UpdateInstanceCustomHealthStatus does not classify a service created without HealthCheckCustomConfig.
AWS: Smithy UpdateInstanceCustomHealthStatus models ServiceNotFound (404), InstanceNotFound (404), CustomHealthNotFound (404), InvalidInput (400). Recorded behavior: for a service created WITHOUT HealthCheckCustomConfig the call is refused with code ServiceNotFound (the service is absent from the custom-health registry), while an otherwise identical service that carries HealthCheckCustomConfig accepts the call and the status is visible on a later read.
Emulator: answers HTTP 501 code NotImplemented for every call, so neither the rejection nor the accepted transition exists.
Why: health-status wiring cannot be exercised at all, and an overly permissive future implementation that accepts the call for a service with no custom health config would silently diverge — the refusal is the contract.
Fix: implement the UpdateInstanceCustomHealthStatus handler with the custom-health-registry lookup (no HealthCheckCustomConfig -> ServiceNotFound). An open stub issue names this operation/file as a bare stub; this cause records the required classification it must implement.
Traces: servicediscovery.custom-health-status-not-configured-error, servicediscovery.custom-health-status-without-custom-health-config-rejected.

Cause 8 of 8 — ListNamespaces does not validate filter name/condition combinations.
AWS: Smithy ListNamespaces models exactly one error, InvalidInput (HTTP 400). A NamespaceFilter with Name=NAME and Condition=BETWEEN is not a supported combination and is refused with 400 InvalidInput.
Emulator: answers HTTP 501 code NotImplemented, so the filter is never validated.
Why: a client sending an unsupported filter combination gets a non-modeled 501 instead of the modeled 400, and filtered listing (the way namespaces are looked up by name) does not work at all.
Fix: implement the ListNamespaces handler including filter validation (unsupported Name+Condition pairs -> InvalidInput). An open stub issue names this operation/file as a bare stub; this cause records the filter-validation behavior it must implement.
Traces: servicediscovery.list-namespaces-invalid-filter-error.

## Evidence
Suite: simparity-behavioral-aws-v1, servicediscovery 6/72 passed.

Cause 1 — expected ClientError InvalidInput, got HTTP 200: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'Service': {'Id': 'srv-7x48m13godwe2qn4', 'NamespaceId': 'ns-zy2k4tsoy8sb0ocw', 'DnsConfig': {'DnsRecords': [{'Type': 'A', 'TTL': 60}]}, 'Type': 'HTTP', ...}, 'ResponseMetadata': {'HTTPStatusCode': 200}}

Cause 2 — AssertionError: expected InvalidInput for name 'not a valid dns name!!', got 'NotImplemented' (HTTP 501). create_private_dns_namespace.rs still returns Scaffolded::not_implemented("CreatePrivateDnsNamespace") and carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker.

Cause 3 — AssertionError: duplicate namespace name must be rejected as DuplicateRequest, got NamespaceAlreadyExists.

Cause 4 — AssertionError: expected empty-string Description to be normalized away, got ''.

Cause 5 — AssertionError: unexpected NamespaceNotFound message: 'No namespace exists with the specified ID ns-sim2realbeha.' (AWS: empty message); AssertionError: unexpected ServiceAlreadyExists message: 'The service sim2real-behav-sd-svc-dup-e92c5a06c140 already exists in namespace ns-0ci80nu2mho4pr41.' (AWS: 'Service already exists.').

Cause 6 — AssertionError: namespace delete with active service should fail ResourceInUse, got NotImplemented; AssertionError: expected ResourceInUse, got NotImplemented; AssertionError: service delete with registered instance should fail ResourceInUse, got NotImplemented; AssertionError: expected ResourceInUse deleting a service with a registered instance, got 'NotImplemented'.

Cause 7 — AssertionError: custom health update on a service without HealthCheckCustomConfig must be rejected with ServiceNotFound, got NotImplemented; AssertionError: expected ServiceNotFound, got NotImplemented.

Cause 8 — AssertionError: expected InvalidInput for NAME+BETWEEN filter, got 'NotImplemented'.

## Steps to Reproduce
1. Boot the emulator and point a botocore servicediscovery client at it.
2. Cause 1: CreateHttpNamespace, wait GetOperation -> SUCCESS, then CreateService(Name=..., NamespaceId=<http ns>, DnsConfig={'NamespaceId': <ns>, 'DnsRecords': [{'Type':'A','TTL':60}]}) -> emulator returns 200 + a service; AWS returns 400 InvalidInput.
3. Cause 2: CreatePrivateDnsNamespace(Name='not a valid dns name!!', Vpc='vpc-0123456789abcdef0') -> 501 NotImplemented; AWS returns 400 InvalidInput. The same call with a valid name also 501s instead of returning an OperationId.
4. Cause 3: CreateHttpNamespace(Name=N) to SUCCESS, then CreateHttpNamespace(Name=N) again -> NamespaceAlreadyExists; AWS returns 409 DuplicateRequest.
5. Cause 4: CreateHttpNamespace(Name=..., Description='') then GetNamespace -> Description present as ''; AWS omits the member.
6. Cause 5: GetNamespace(Id='ns-<well-formed but unknown>') -> NamespaceNotFound with a constructed message (AWS: empty). CreateService twice with the same Name in one namespace -> ServiceAlreadyExists with a constructed message (AWS: 'Service already exists.').
7. Cause 6: with a service (and a registered instance) inside a namespace, call DeleteNamespace(Id=<ns>) and DeleteService(Id=<svc>) -> 501 NotImplemented; AWS returns 409 ResourceInUse.
8. Cause 7: CreateService without HealthCheckCustomConfig, RegisterInstance, then UpdateInstanceCustomHealthStatus(Status='UNHEALTHY') -> 501 NotImplemented; AWS returns 404 ServiceNotFound.
9. Cause 8: ListNamespaces(Filters=[{'Name':'NAME','Values':[...],'Condition':'BETWEEN'}]) -> 501 NotImplemented; AWS returns 400 InvalidInput.
10. Replay the check ids in trace_ids to confirm.
