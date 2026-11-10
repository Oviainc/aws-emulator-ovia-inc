Issue-Source: trace-sweep-divergence
Issue-Trace: entityresolution-resource-policy-lifecycle
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: entityresolution-resource-policy-lifecycle
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution|CreateIdNamespace|2xx->4xx|status
Issue-Key: entityresolution|DeleteIdNamespace|2xx->4xx|status
Issue-Commit: 0d948e25b0f3e5bfbdfdba7e7f14d68cf70134c2

# entityresolution ID namespace lifecycle unusable: CreateIdNamespace answers 403 and DeleteIdNamespace answers 404

> _Note: this issue was identified against the code base at commit `0d948e25b0f3e5bfbdfdba7e7f14d68cf70134c2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIdNamespace

## Description
The emulator cannot serve the AWS Entity Resolution ID-namespace resource lifecycle: creating an ID namespace is refused outright, and deleting one that the same recorded fixture created is reported as absent. There are 2 distinct causes in this issue and BOTH must be closed, and BOTH replay steps must pass before submitting — do not assume the second is only a consequence of the first.

Cause 1 of 2 — CreateIdNamespace returns HTTP 403 instead of creating the ID namespace.
AWS: CreateIdNamespace (POST /idnamespaces, Smithy: services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model/com-amazonaws-entityresolution-awsveniceservice.json) creates an ID namespace from the @required idNamespaceName (EntityName, @length 1..255, pattern ^[a-zA-Z_0-9-]*$) and the @required type (IdNamespaceType enum), with optional description, inputSourceConfig (0..20), idMappingWorkflowProperties (1..1), roleArn (RoleArn, @length 32..512, pattern ^arn:aws:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$) and tags, and answers HTTP 200 with the @required idNamespaceName, idNamespaceArn (pattern ^arn:(aws|aws-us-gov|aws-cn):entityresolution:<region>:<account>:idnamespace/<name>$), type, createdAt and updatedAt. The operation models AccessDeniedException (403, "You do not have sufficient access to perform this action"), ConflictException (400), ExceedsLimitException (402), ValidationException (400), ThrottlingException (429) and InternalServerException (500); 403 is reserved for a caller whose identity is not permitted the action. The recording for this step is a well-formed create by a fully-permitted caller and AWS answered 200 with the namespace body.
Emulator: answers HTTP 403 to the same request, so no ID namespace is created and no response body is served. Hypothesis: the ID-namespace write path applies an authorization/role check (most plausibly on the request's credentials or on the supplied roleArn) that denies a caller AWS permits — note the open behavioral issue 20261022T153803Z-entityresolution-control-plane-missing-reference-duplicate-validation-no.md records the OPPOSITE defect on the read path (GetSchemaMapping performs no authorization check at all), so this 403 is not that issue's cause and is not covered by it.
Why: a client that creates an ID namespace as the first step of any ID-mapping or resource-policy flow gets AccessDeniedException where AWS returns 200, so every downstream call (GetIdNamespace, ListIdNamespaces, resource-policy statements scoped to the namespace ARN, ID-mapping workflows that reference it) is unreachable against the emulator, and an SDK error handler sees a permissions failure that no policy change can clear.
Fix: the CreateIdNamespace handler in the entityresolution service crate — its request-authorization/role precondition must not deny a permitted caller; a valid create must persist the namespace in state and serialize the modeled 200 output (idNamespaceName, idNamespaceArn, type, createdAt, updatedAt, plus the echoed optional members). Error selection for genuinely invalid input belongs to ValidationException (400) / ConflictException (400), not 403.
Traces: entityresolution-resource-policy-lifecycle#setup#2 (CreateIdNamespace, expected 200, got 403).

Cause 2 of 2 — DeleteIdNamespace returns HTTP 404 where the recording deletes the namespace successfully.
AWS: DeleteIdNamespace (DELETE /idnamespaces/{idNamespaceName}, @idempotent) deletes the named ID namespace and answers HTTP 200 with the @required message member (String). It models ResourceNotFoundException (404) only for an idNamespaceName that does not exist, plus AccessDeniedException (403), ValidationException (400), ThrottlingException (429) and InternalServerException (500). The recording shows the teardown delete of a namespace created earlier in the same fixture answering 200.
Emulator: answers HTTP 404 (ResourceNotFoundException) for that teardown delete, i.e. it reports the namespace as nonexistent. The fixed text of the 404 body is AWS's not-found wording for this resource class and its variable part is the requested idNamespaceName taken from the {idNamespaceName} URI label — but on this step no 404 of any wording is correct.
Why: the ID-namespace lifecycle never closes. A client that creates and then deletes a namespace (and any test that cleans up after itself) gets a not-found error on a resource it just created, so teardown and idempotent re-delete logic written against AWS fails, and the emulator's namespace store cannot be trusted to hold what it accepted.
Fix: the DeleteIdNamespace handler and the shared ID-namespace state in the entityresolution service crate — look the idNamespaceName up in the namespace store, remove it and return the modeled 200 { message } when present, reserving 404 ResourceNotFoundException for a name that was never created. If the namespace is missing here only because Cause 1 refused to create it, this step must still be re-replayed and shown green after Cause 1 is fixed.
Traces: entityresolution-resource-policy-lifecycle#teardown#10 (DeleteIdNamespace, expected 200, got 404).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, 40/55 steps passed, so entityresolution is serving): 5 divergence clusters, 3 of them owned by the open issue 20261029T172827Z-entityresolution-provider-service-catalog-is-fabricated-listproviderservices-and.md. The 2 unowned clusters are the causes above.

Cause 1 — fixture entityresolution-resource-policy-lifecycle, step setup#2, operation CreateIdNamespace, verdict mismatch, divergence_key "service=entityresolution|CreateIdNamespace|status=200->403|keys=status": detail "status: expected 200, got 403". Expected status 200 (modeled CreateIdNamespaceOutput), actual status 403. affected_count=1, stub_path empty, owned_by empty.

Cause 2 — fixture entityresolution-resource-policy-lifecycle, step teardown#10, operation DeleteIdNamespace, verdict mismatch, divergence_key "service=entityresolution|DeleteIdNamespace|status=200->404|keys=status": detail "status: expected 200, got 404". Expected status 200 (modeled DeleteIdNamespaceOutput, @required message), actual status 404. affected_count=1, stub_path empty, owned_by empty.

Smithy citations (services/service-com-amazonaws-entityresolution-awsveniceservice/smithy-model/com-amazonaws-entityresolution-awsveniceservice.json): CreateIdNamespace POST /idnamespaces, http code 200, input idNamespaceName @required (EntityName 1..255, ^[a-zA-Z_0-9-]*$) and type @required (IdNamespaceType), output idNamespaceName/idNamespaceArn/type/createdAt/updatedAt @required, errors AccessDeniedException(403), ConflictException(400), ExceedsLimitException(402), ValidationException(400), ThrottlingException(429), InternalServerException(500). DeleteIdNamespace DELETE /idnamespaces/{idNamespaceName} @idempotent, http code 200, input idNamespaceName @required @httpLabel, output message @required, errors AccessDeniedException(403), ResourceNotFoundException(404), ValidationException(400), ThrottlingException(429), InternalServerException(500).

No host, port, generated namespace name, account id, timestamp or request id from the recording is part of either requirement; the requirements are the statuses (200 on both steps), the modeled response shapes, and 403/404 being reserved for a denied caller and an absent name respectively.

## Steps to Reproduce
1. git clone the repository and build/boot the emulator from that checkout (replay_wire_corpus does this; emulator_up was true and /__admin/health answered 200).
2. Replay the wire fixture corpus simparity-wire-aws-v1 and filter the divergences to service "entityresolution".
3. Cause 1: replay fixture entityresolution-resource-policy-lifecycle step setup#2 — POST /idnamespaces with a valid idNamespaceName (matching ^[a-zA-Z_0-9-]*$, 1..255) and a valid type, by the fixture's fully-permitted caller. The emulator answers HTTP 403; the recording expects HTTP 200 with idNamespaceName, idNamespaceArn, type, createdAt and updatedAt.
4. Cause 2: replay the same fixture through to step teardown#10 — DELETE /idnamespaces/{idNamespaceName} for the namespace the fixture created. The emulator answers HTTP 404 (ResourceNotFoundException); the recording expects HTTP 200 with the required message member.
5. After fixing, re-replay BOTH entityresolution-resource-policy-lifecycle#setup#2 and entityresolution-resource-policy-lifecycle#teardown#10 and confirm both pass; the delete step is a separate assertion from the create step and must be verified on its own.
