Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: vpc-lattice.delete-service-network-with-active-service-association-rejected vpc-lattice.deleting-a-service-network-is-rejected-while-an-association-survives-it vpc-lattice.delete-service-with-active-service-network-association-rejected
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice.delete-service-network-with-active-service-association-rejected|run#16|AssertionError|assert message == expected, (
Issue-Key: vpc-lattice.deleting-a-service-network-is-rejected-while-an-association-survives-it|run#14|AssertionError|assert refusal_message == expected, (
Issue-Key: vpc-lattice.delete-service-with-active-service-network-association-rejected|run#16|AssertionError|assert message == expected, (
Issue-Commit: 5598afbcdd0af564c379fb305d1563cf7235aea7

# VPC Lattice referential-integrity ConflictException messages do not match AWS for DeleteServiceNetwork and DeleteService

> _Note: this issue was identified against the code base at commit `5598afbcdd0af564c379fb305d1563cf7235aea7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteServiceNetwork

## Description
The two referential-integrity delete guards in VPC Lattice raise the right error (ConflictException / HTTP 409) and correctly leave the resource and its association intact, but the ConflictException `message` they emit is emulator-invented prose rather than the message real AWS produces. The message in both cases is a FIXED TEMPLATE parameterized only by the resolved resource id from the request path, so the template is the requirement; only the id inside it varies per environment. 2 causes.

Cause 1 of 2: DeleteServiceNetwork — conflict message template.
AWS: Smithy (services/service-com-amazonaws-vpclattice-mercurycontrolplane/smithy-model/com-amazonaws-vpclattice-mercurycontrolplane.json): DeleteServiceNetwork DELETE /servicenetworks/{serviceNetworkIdentifier}, success 204, modeled error ConflictException @httpError 409 with required members message/resourceId/resourceType; the operation doc states "You can only delete the service network if there is no service or VPC associated with it". The model does not state the message text — recorded behavior only: real AWS refuses with ConflictException whose message is built from the request's resolved service-network id as `ServiceNetwork <sn-id> has service(s) associated`. Two independent recordings (ids sn-4oub5vinlrb3lr9bg and sn-1kg41yi9n6x2kxqlq) show the identical template with only the id substituted, which is what establishes it as a rule rather than one recorded string.
Emulator: returns ConflictException/409 (correct code and status) but with message `service network <sn-id> still has <n> service association(s) and cannot be deleted` — a different template that additionally leaks an association COUNT AWS never reports.
Why: a client that matches or logs the refusal reason (error-message assertions in deployment tooling, IaC drift reporting, support/debug surfaces that render the AWS wording) sees text no AWS region ever emits, and any parser of the `ServiceNetwork ... has service(s) associated` form fails to classify the refusal.
Fix: the ConflictException construction in the DeleteServiceNetwork handler's association guard — emit the AWS template over the resolved id, and drop the association count from the message.
Traces: vpc-lattice.delete-service-network-with-active-service-association-rejected, vpc-lattice.deleting-a-service-network-is-rejected-while-an-association-survives-it; handler services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/delete_service_network.rs lines 181-193.

Cause 2 of 2: DeleteService — conflict message template.
AWS: Smithy: DeleteService DELETE /services/{serviceIdentifier}, success 200, modeled error ConflictException @httpError 409; the handler's own quoted doc states "A service can't be deleted if it's associated with a service network". Message text is recorded behavior only: real AWS refuses with ConflictException whose message is built from the request's resolved service id as `Service <svc-id> has one or more associated service networks.` (trailing period included).
Emulator: returns ConflictException/409 (correct code and status) but with message `service <svc-id> is still associated with <n> service network(s) and cannot be deleted`.
Why: same as Cause 1 — the refusal is classified correctly by code but its reason string diverges from AWS, so client-side wording checks and human-facing renderings disagree with every real region.
Fix: the ConflictException construction in the DeleteService handler's association guard.
Traces: vpc-lattice.delete-service-with-active-service-network-association-rejected; handler services/service-com-amazonaws-vpclattice-mercurycontrolplane/src/ops/delete_service.rs lines 194-206.

## Evidence
Cause 1 — vpc-lattice.delete-service-network-with-active-service-association-rejected:
  AssertionError: the ConflictException message is 'service network sn-4oub5vinlrb3lr9bg still has 1 service association(s) and cannot be deleted'; real AWS says 'ServiceNetwork sn-4oub5vinlrb3lr9bg has service(s) associated'
Cause 1 — vpc-lattice.deleting-a-service-network-is-rejected-while-an-association-survives-it:
  AssertionError: the ConflictException message is 'service network sn-1kg41yi9n6x2kxqlq still has 1 service association(s) and cannot be deleted'; real AWS says 'ServiceNetwork sn-1kg41yi9n6x2kxqlq has service(s) associated'
  (same template, different id -> the expectation is a template, not a literal)

Expected (AWS): HTTP 409, Error.Code=ConflictException, Error.Message="ServiceNetwork {serviceNetworkId} has service(s) associated"
Actual (emulator): HTTP 409, Error.Code=ConflictException, Error.Message="service network {serviceNetworkId} still has {count} service association(s) and cannot be deleted"

Cause 2 — vpc-lattice.delete-service-with-active-service-network-association-rejected:
  AssertionError: the ConflictException message is 'service svc-5j02qfjsak79k6oks is still associated with 1 service network(s) and cannot be deleted'; real AWS says 'Service svc-5j02qfjsak79k6oks has one or more associated service networks.'

Expected (AWS): HTTP 409, Error.Code=ConflictException, Error.Message="Service {serviceId} has one or more associated service networks."
Actual (emulator): HTTP 409, Error.Code=ConflictException, Error.Message="service {serviceId} is still associated with {count} service network(s) and cannot be deleted"

In all three checks the error CODE, HTTP status, and the post-refusal state (service network still gettable, association still listed, delete succeeding once the association is removed) are correct; the divergence is confined to the message template.

## Steps to Reproduce
Cause 1 (DeleteServiceNetwork):
1. CreateServiceNetwork(name=..., authType=NONE) -> note id sn-...
2. CreateService(name=..., authType=NONE); poll GetService until status==ACTIVE.
3. CreateServiceNetworkServiceAssociation(serviceNetworkIdentifier=sn-..., serviceIdentifier=svc-...); poll GetServiceNetworkServiceAssociation until status==ACTIVE.
4. DeleteServiceNetwork(serviceNetworkIdentifier=sn-...).
5. Inspect the ConflictException message: it is "service network sn-... still has 1 service association(s) and cannot be deleted" instead of "ServiceNetwork sn-... has service(s) associated".

Cause 2 (DeleteService):
1-3. As above, with an ACTIVE service-network/service association in place.
4. DeleteService(serviceIdentifier=svc-...).
5. Inspect the ConflictException message: it is "service svc-... is still associated with 1 service network(s) and cannot be deleted" instead of "Service svc-... has one or more associated service networks."
