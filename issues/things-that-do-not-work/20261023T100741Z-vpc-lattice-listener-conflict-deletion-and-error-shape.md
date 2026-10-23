Issue-Source: trace-sweep-divergence
Issue-Trace: vpclattice-service-and-listener-lifecycle
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: vpclattice-service-and-listener-lifecycle
Issue-Shape-Id: com.amazonaws.vpclattice#MercuryControlPlane
Issue-Key: vpc-lattice|CreateListener|4xx->2xx|$.message
Issue-Key: vpc-lattice|CreateListener|4xx->2xx|$.resourceId
Issue-Key: vpc-lattice|CreateListener|4xx->2xx|$.resourceType
Issue-Key: vpc-lattice|CreateListener|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: vpc-lattice|CreateListener|4xx->2xx|status
Issue-Key: vpc-lattice|GetListener|4xx->2xx|$.message
Issue-Key: vpc-lattice|GetListener|4xx->2xx|$.resourceId
Issue-Key: vpc-lattice|GetListener|4xx->2xx|$.resourceType
Issue-Key: vpc-lattice|GetListener|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: vpc-lattice|GetListener|4xx->2xx|status
Issue-Key: vpc-lattice|CreateListener|4xx->4xx|$.fieldList[*]
Issue-Key: vpc-lattice|CreateListener|4xx->4xx|$.message
Issue-Commit: 3b7cd50bec204a941815a8ace6b2a1778291c65c

# VPC Lattice listener conflict, deletion and error-shape divergences

> _Note: this issue was identified against the code base at commit `3b7cd50bec204a941815a8ace6b2a1778291c65c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateListener

## Description
Replay of the vpc-lattice listener-lifecycle fixture shows four distinct defects in the Listener resource: the emulator never detects a port conflict, it keeps serving a listener after it was deleted, and both of its listener error bodies are shaped differently from AWS. All four share the CreateListener/GetListener listener path of the vpc-lattice emulator. There are 4 causes to close.

Cause 1 of 4 — CreateListener does not reject a second listener on an already-occupied port (fixture vpclattice-service-and-listener-lifecycle, step task#7).
AWS: a listener's port must be unique within one service; creating a second listener on a port already bound by an existing listener of that service fails. Smithy: CreateListener binds ConflictException (httpError 409), whose members message, resourceId and resourceType are all @required, and CreateListener.port targets Port @range min 1 max 65535 (port uniqueness itself is not stated in the model — recorded behavior only). The recording shows HTTP 409 with resourceType 'LISTENER', the conflicting listener id in resourceId, an x-amzn-errortype header, and a message of the form 'Port <port> is occupied'.
Emulator: returns HTTP 201 and creates a second listener on the same port; message, resourceId, resourceType and x-amzn-errortype are all absent because no error is raised at all.
Why: a client that relies on the port-uniqueness invariant gets two listeners silently bound to one port instead of a ConflictException, so provisioning code (and any retry/reconcile loop that treats 409 as "already configured") proceeds against a service topology AWS would never produce, and duplicate listeners leak into subsequent list/get calls.
Fix: the CreateListener handler's pre-insert conflict check and the listener state keyed by (serviceId, port) — reject with ConflictException(resourceType='LISTENER') before inserting.
Traces: vpclattice-service-and-listener-lifecycle#task#7.

Cause 2 of 4 — GetListener still returns a deleted listener (fixture vpclattice-service-and-listener-lifecycle, step task#12).
AWS: after DeleteListener succeeds, the listener is gone and GetListener on that identifier fails. Smithy: GetListener binds ResourceNotFoundException (httpError 404) with @required message, resourceId and resourceType; DeleteListener is @idempotent with http code 204. The recording shows HTTP 404, resourceType 'LISTENER', resourceId set to the deleted listener id, an x-amzn-errortype header, and a message of the form 'Listener <id> cannot be found.'
Emulator: returns HTTP 200 with the full listener body; no message/resourceId/resourceType/x-amzn-errortype.
Why: deletion is not observable. A client polling GetListener to confirm teardown never converges, and any code that treats a successful Get as proof of existence will keep operating on (or refuse to recreate) a listener that no longer exists.
Fix: the DeleteListener handler's state mutation (actually remove the listener from the service's listener map / mark it absent) and the GetListener lookup path, which must raise ResourceNotFoundException on a miss.
Traces: vpclattice-service-and-listener-lifecycle#task#12.

Cause 3 of 4 — ValidationException for an unsupported fixedResponse status code is shaped generically instead of service-specifically (fixture vpclattice-service-and-listener-lifecycle, step task#9).
AWS: Smithy FixedResponseAction.statusCode targets HttpStatusCode @range min 100 max 599 and is documented 'The HTTP response code. Only 404 and 500 status codes are supported.' CreateListener binds ValidationException (httpError 400) carrying fieldList of ValidationExceptionField{name,message}. Both sides agree on HTTP 400, but the recording reports exactly one fieldList entry whose name is the bare member 'statusCode' and whose message equals the top-level message, 'Provided status code is not supported.' — a service-level rule rejection, not a range/enum constraint report.
Emulator: returns a generic constraint-violation body — fieldList[0].name 'defaultAction.fixedResponse.statusCode' and message "Value '200' at 'defaultAction.fixedResponse.statusCode' failed to satisfy constraint: Member must satisfy enum value set: [404, 500]", with top-level message "1 validation error detected: ...".
Why: the fieldList name is the key clients use to map a validation failure back to an input member; emitting a dotted path where AWS emits the member name breaks that mapping, and the generic "N validation errors detected" wrapper misreports a service rule as a modeled enum/range constraint. Both the name and the message here are built from the request, so the template is the requirement (the literal recorded text is an example).
Fix: the CreateListener validation layer for defaultAction.fixedResponse.statusCode — raise the service-specific ValidationException with fieldList[0].name='statusCode' rather than letting the generic constraint-violation serializer format it.
Traces: vpclattice-service-and-listener-lifecycle#task#9.

Cause 4 of 4 — duplicate-listener-name ConflictException message uses a generic template (fixture vpclattice-service-and-listener-lifecycle, step task#8).
AWS: creating a listener whose name already exists in the service returns ConflictException. Status (409) and the error type match the emulator; only the message differs. The message is built from the request and names both the submitted listener name and the target service, following the template 'Listener with name <name> already exists in <serviceId> service.'
Emulator: returns 409 with the generic template 'Resource with name <name> already exists in Service <serviceId>.'
Why: lowest-impact of the four — status and code are correct, so well-behaved SDK clients are unaffected; but callers and test harnesses that match the documented listener-specific wording (or distinguish listener-name conflicts from other resource conflicts by message) misclassify the error. The requirement is the template, not the recorded sentence.
Fix: the ConflictException construction in the CreateListener handler — use the Listener-specific message template instead of the shared resource-conflict one.
Traces: vpclattice-service-and-listener-lifecycle#task#8.

## Evidence
Fixture vpclattice-service-and-listener-lifecycle, replayed against the emulator (emulator_up=true, state_reset_failures=0, score 37%).

Cause 1 — step task#7, CreateListener: expected status 409, got 201. detail: '$.message: missing (expected \'Port 80 is occupied\', got None); $.resourceId: missing; $.resourceType: missing (expected \'LISTENER\', got None); $headers.x-amzn-errortype: missing'. Model: ConflictException httpError 409, members message/resourceId/resourceType all @required.

Cause 2 — step task#12, GetListener (after DeleteListener): expected status 404, got 200. detail: '$.message: missing (expected \'Listener <id> cannot be found.\', got None); $.resourceId: missing (expected the deleted listener id); $.resourceType: missing (expected \'LISTENER\'); $headers.x-amzn-errortype: missing'. Model: GetListener errors include ResourceNotFoundException httpError 404; DeleteListener http code 204, @idempotent.

Cause 3 — step task#9, CreateListener with defaultAction.fixedResponse.statusCode=200: status 400 on both sides, bodies differ. Expected fieldList[0] = {'name': 'statusCode', 'message': 'Provided status code is not supported.'} and top-level message 'Provided status code is not supported.'; actual fieldList[0] = {'name': 'defaultAction.fixedResponse.statusCode', 'message': "Value '200' at 'defaultAction.fixedResponse.statusCode' failed to satisfy constraint: Member must satisfy enum value set: [404, 500]"} with top-level message "1 validation error detected: ...". Model: FixedResponseAction.statusCode HttpStatusCode @range 100..599, doc 'Only 404 and 500 status codes are supported'; ValidationException httpError 400 with fieldList of ValidationExceptionField{name,message}.

Cause 4 — step task#8, CreateListener with a duplicate name: status 409 on both sides. Expected $.message 'Listener with name <name> already exists in <serviceId> service.'; actual 'Resource with name <name> already exists in Service <serviceId>.'

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture vpclattice-service-and-listener-lifecycle.
2. Cause 1 (task#7): CreateService, then CreateListener (protocol HTTP, port 80), then CreateListener again on the SAME service with a different name and port 80. AWS returns 409 ConflictException with resourceType 'LISTENER'; the emulator returns 201 and creates a duplicate.
3. Cause 4 (task#8): CreateListener again on that service reusing an EXISTING listener name. Both return 409; compare $.message against the 'Listener with name <name> already exists in <serviceId> service.' template.
4. Cause 3 (task#9): CreateListener with defaultAction.fixedResponse.statusCode = 200 (anything other than 404/500). Both return 400; compare $.message and $.fieldList[0].name — AWS reports name 'statusCode', the emulator reports 'defaultAction.fixedResponse.statusCode' with a generic constraint-violation message.
5. Cause 2 (task#12): DeleteListener on the created listener, then GetListener with the same serviceIdentifier/listenerIdentifier. AWS returns 404 ResourceNotFoundException (resourceType 'LISTENER', resourceId = the listener id); the emulator returns 200 with the listener body.
