Issue-Source: trace-sweep-divergence
Issue-Trace: redshiftserverless-namespace-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: redshiftserverless-namespace-lifecycle
Issue-Shape-Id: com.amazonaws.redshiftserverless#RedshiftServerless
Issue-Key: redshift-serverless|CreateNamespace|2xx->2xx|$.namespace.adminUsername
Issue-Key: redshift-serverless|CreateNamespace|2xx->2xx|$.namespace.creationDate
Issue-Key: redshift-serverless|CreateNamespace|2xx->2xx|$.namespace.kmsKeyId
Issue-Key: redshift-serverless|CreateNamespace|2xx->2xx|$.namespace.lakehouseRegistrationStatus
Issue-Key: redshift-serverless|GetNamespace|2xx->2xx|$.namespace.adminUsername
Issue-Key: redshift-serverless|GetNamespace|2xx->2xx|$.namespace.creationDate
Issue-Key: redshift-serverless|GetNamespace|2xx->2xx|$.namespace.kmsKeyId
Issue-Key: redshift-serverless|GetNamespace|2xx->2xx|$.namespace.lakehouseRegistrationStatus
Issue-Key: redshift-serverless|GetNamespace|2xx->2xx|$.namespace.logExports[*]
Issue-Key: redshift-serverless|CreateNamespace|4xx->4xx|$.message
Issue-Key: redshift-serverless|CreateNamespace|4xx->4xx|status
Issue-Key: redshift-serverless|GetNamespace|4xx->4xx|$.message
Issue-Key: redshift-serverless|GetNamespace|4xx->4xx|status
Issue-Commit: 837306e76dca4544e8b76426022c8d4a2dc48510

# Redshift Serverless namespace responses and error mapping diverge from AWS

> _Note: this issue was identified against the code base at commit `837306e76dca4544e8b76426022c8d4a2dc48510`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNamespace

## Description
Replay of the recorded Redshift Serverless namespace lifecycle fixture against the emulator produced 6 divergence clusters over CreateNamespace and GetNamespace. They are FOUR distinct causes, all in the redshift-serverless service (awsJson1_1, API version 2021-04-21). All four must be closed before this issue is resolved.

Cause 1 of 4 - Namespace structure is serialized with most of its members omitted (CreateNamespace fixture redshiftserverless-namespace-lifecycle step task#1; GetNamespace steps task#2 and task#4).
AWS: Smithy shape com.amazonaws.redshiftserverless#Namespace models adminUsername (target DbUser, @sensitive), kmsKeyId, lakehouseRegistrationStatus, logExports (LogExportList, @length 0..16) alongside namespaceArn/namespaceId/namespaceName/dbName/status. CreateNamespace returns CreateNamespaceResponse.namespace and GetNamespace returns GetNamespaceResponse.namespace (@required). The recording shows AWS echoing the admin user supplied on create, a kmsKeyId of AWS_OWNED_KMS_KEY when no CMK was requested, a lakehouseRegistrationStatus of NOT_REGISTERED, and the logExports list as configured (the recording's third step carries one log-export entry). The same populated namespace is returned by the subsequent GetNamespace reads, i.e. these fields are persisted, not create-only.
Emulator: returns HTTP 200 with a Namespace object in which adminUsername, kmsKeyId, lakehouseRegistrationStatus are absent (null) and logExports is empty/absent even after the namespace was created/updated with a log-export configured.
Why: a client reading the create or get response cannot learn who the admin user is, whether the namespace is encrypted with an AWS-owned or customer key, its lakehouse registration state, or which log exports are enabled. SDK consumers and IaC tools that diff desired vs actual namespace state see every one of these as unset and will either loop re-applying the configuration or fail their own validation on a missing required attribute.
Fix: the CreateNamespace/GetNamespace handlers and the Namespace serializer in the redshift-serverless service crate - persist adminUsername, kmsKeyId (defaulting to AWS_OWNED_KMS_KEY when no key is given), lakehouseRegistrationStatus (default NOT_REGISTERED) and logExports in namespace state, and emit all of them on every operation that returns a Namespace.
Traces: redshiftserverless-namespace-lifecycle#task#1, redshiftserverless-namespace-lifecycle#task#2, redshiftserverless-namespace-lifecycle#task#4.

Cause 2 of 4 - Namespace.creationDate is serialized as an epoch number instead of the modeled date-time form (same clusters: steps task#1, task#2, task#4).
AWS: Smithy Namespace.creationDate targets Timestamp with @timestampFormat("date-time"), which overrides the awsJson1_1 epoch-seconds default, so the member is wire-serialized as an RFC3339/ISO-8601 string. The fixture asserts the field's shape rather than a literal instant (expected '<shaped-not-asserted>'), so the value is free but the form is not.
Emulator: emits creationDate as a bare JSON number (e.g. 1792625739.4766922, epoch seconds with a fractional part).
Why: a generated SDK deserializing this member expects a date-time string; a numeric token either fails parsing or is silently coerced, so clients get a parse error or a nonsense creation time for every namespace they read.
Fix: the Namespace serializer's timestamp handling in the redshift-serverless service crate - honour the member's date-time timestampFormat rather than the protocol default.
Traces: redshiftserverless-namespace-lifecycle#task#1, redshiftserverless-namespace-lifecycle#task#2, redshiftserverless-namespace-lifecycle#task#4.

Cause 3 of 4 - duplicate CreateNamespace is answered 409 where AWS answers 400 (step task#9).
AWS: recorded behavior - creating a namespace whose name already exists returns HTTP 400 with a message of the form 'The namespace <name> already exists.' (recorded example: 'The namespace sim2real-probe-rsns-4c71-953dbf1c already exists.'). The 400 status is the requirement; the message text is an example. Hypothesis: the service reports the duplicate as the 400-status ValidationException (Smithy: ValidationException @httpError 400) rather than as ConflictException, even though CreateNamespace also models ConflictException @httpError 409.
Emulator: returns HTTP 409 (ConflictException mapping) with message 'a namespace named sim2real-probe-rsns-4c71-953dbf1c already exists'.
Why: a client that branches on status code - retry/backoff on 409 conflicts, surface-to-user on 4xx validation - takes the wrong branch, and SDK error-type matching binds the failure to ConflictException instead of the 400-status error AWS actually raises, so duplicate-name handling written against AWS never fires.
Fix: the CreateNamespace handler's duplicate-name path in the redshift-serverless service crate - raise the 400-status validation error the recording shows instead of the 409 conflict.
Traces: redshiftserverless-namespace-lifecycle#task#9.

Cause 4 of 4 - GetNamespace on an absent namespace is answered 404 where AWS answers 400 (step task#10).
AWS: recorded behavior - GetNamespace for a namespace that does not exist returns HTTP 400 with a message of the form 'Serverless namespace <name> not found.' (recorded example: 'Serverless namespace sim2real-probe-rsns-absent-4c71 not found.'). The 400 status is the requirement; the message is an example. Hypothesis: the service reports the miss as the 400-status ValidationException, notwithstanding that GetNamespace also models ResourceNotFoundException @httpError 404.
Emulator: returns HTTP 404 (ResourceNotFoundException mapping) with message 'no namespace named sim2real-probe-rsns-absent-4c71'.
Why: existence probes written against AWS catch the 400-status error; against the emulator they see a 404 of a different modeled error type and treat the lookup as an unexpected failure (or, in retry wrappers, as a non-retryable resource error) instead of 'namespace absent'.
Fix: the GetNamespace handler's lookup-miss path in the redshift-serverless service crate - raise the 400-status validation error the recording shows instead of the 404 not-found.
Traces: redshiftserverless-namespace-lifecycle#task#10.

Also observed, smaller, in the same bundle (step task#11): the namespaceName pattern validation rejects correctly with HTTP 400 and the standard 'N validation error detected: Value '<v>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>' format, but renders the pattern with its anchors - '^[a-z0-9-]+$' - whereas AWS renders the constraint un-anchored, '[a-z0-9-]+' (Smithy: NamespaceName @pattern "^[a-z0-9-]+$", @length 3..64). Status and error form already match; only the pattern rendering inside the message differs, so it is LOW and travels with this issue: strip the anchors when interpolating a modeled pattern into a validation message. Trace: redshiftserverless-namespace-lifecycle#task#11.

No existing open issue covers these; the only redshift-adjacent open issues are the bootstrap issues for the separate 'redshift' and 'redshift-data' services, which are different services and do not cover redshift-serverless fidelity.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0, 14 steps, 1 passed, 13 mismatched, 6 divergence clusters, score 7.1%).

Cluster 1 - CreateNamespace, fixture redshiftserverless-namespace-lifecycle step task#1, verdict mismatch, expected 200 / actual 200:
  $.namespace.adminUsername: missing (expected 'admin', got None)
  $.namespace.creationDate: value_mismatch (expected '<shaped-not-asserted>', got 1792625739.4766922)
  $.namespace.kmsKeyId: missing (expected 'AWS_OWNED_KMS_KEY', got None)
  $.namespace.lakehouseRegistrationStatus: missing (expected 'NOT_REGISTERED', got None)

Cluster 2 - GetNamespace, step task#2, expected 200 / actual 200: same four fields missing or mis-typed.

Cluster 3 - GetNamespace, step task#4, expected 200 / actual 200: same four, plus
  $.namespace.logExports[0]: missing (expected 'userlog', got None)

Cluster 4 - CreateNamespace, step task#9:
  status: expected 400, got 409
  $.message: expected 'The namespace sim2real-probe-rsns-4c71-953dbf1c already exists.', got 'a namespace named sim2real-probe-rsns-4c71-953dbf1c already exists'

Cluster 5 - GetNamespace, step task#10:
  status: expected 400, got 404
  $.message: expected 'Serverless namespace sim2real-probe-rsns-absent-4c71 not found.', got 'no namespace named sim2real-probe-rsns-absent-4c71'

Cluster 6 - CreateNamespace, step task#11, expected 400 / actual 400 (status agrees):
  $.message: expected "1 validation error detected: Value 'SIM2REAL-Probe-RSNS-4c71' at 'namespaceName' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z0-9-]+", got the same text with pattern '^[a-z0-9-]+$'

Model consulted (services/service-com-amazonaws-redshiftserverless-redshiftserverless/smithy-model/com-amazonaws-redshiftserverless-redshiftserverless.json): service RedshiftServerless 2021-04-21, awsJson1_1, sigv4Name redshift-serverless. CreateNamespace -> CreateNamespaceResponse{namespace: Namespace}, errors ConflictException(409)/ValidationException(400)/TooManyTagsException(400)/InternalServerException(500). GetNamespace -> GetNamespaceResponse{namespace: Namespace @required}, errors ResourceNotFoundException(404)/ValidationException(400)/InternalServerException(500). Namespace members include adminUsername(DbUser,@sensitive), kmsKeyId, lakehouseRegistrationStatus, logExports(LogExportList @length 0..16), creationDate(Timestamp @timestampFormat "date-time"). NamespaceName: @length 3..64, @pattern "^[a-z0-9-]+$".

## Steps to Reproduce
1. git clone the emulator repo and build/boot the emulator from that checkout (health endpoint /__admin/health, data listener on the emulator's configured port).
2. Replay the wire fixture corpus of trace set simparity-wire-aws-v1, fixture redshiftserverless-namespace-lifecycle, against it.
3. Cause 1 and 2: observe steps task#1 (CreateNamespace with adminUsername 'admin', no kmsKeyId), task#2 (GetNamespace) and task#4 (GetNamespace after a log-export is configured). Each returns 200, but $.namespace.adminUsername, $.namespace.kmsKeyId, $.namespace.lakehouseRegistrationStatus are null/absent, $.namespace.logExports is empty at task#4, and $.namespace.creationDate is a JSON number rather than a date-time string.
4. Cause 3: at step task#9, re-issue CreateNamespace with the name already created at task#1. Expect HTTP 400; the emulator answers 409.
5. Cause 4: at step task#10, call GetNamespace for a namespace that was never created. Expect HTTP 400; the emulator answers 404.
6. LOW item: at step task#11, call CreateNamespace with namespaceName 'SIM2REAL-Probe-RSNS-4c71'. The 400 and message format match, but the interpolated pattern is '^[a-z0-9-]+$' instead of '[a-z0-9-]+'.
7. Re-replay all six steps (task#1, task#2, task#4, task#9, task#10, task#11) to confirm a fix; each is a separate cause and the first three share causes 1 and 2.
