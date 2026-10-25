Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.get-function-concurrency-field-scoped-to-unqualified-identity lambda.get-function-configuration-representation-matches-created-config lambda.publish-version-immutable lambda.image-uri-preserved-through-create-and-get lambda.layer-omitted-options-stay-absent-from-list-and-description-defaults-empty lambda.durable-config-defaults-and-partial-update lambda.code-location-serves-the-exact-bytes-and-a-published-version-is-byte-frozen lambda.dry-run-does-not-invoke-handler lambda.create-event-source-mapping-duplicate-conflict lambda.layer-permission-lifecycle lambda.layer-version-permission-duplicate-statement-rejected lambda.layer-version-permission-lifecycle
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.get-function-concurrency-field-scoped-to-unqualified-identity|run#11|AssertionError|assert qualified.get("Concurrency") is None, (
Issue-Key: lambda.get-function-configuration-representation-matches-created-config|run#25|AssertionError|assert published["RevisionId"] != latest_revision, "published version reused $LATEST's revision id"
Issue-Key: lambda.publish-version-immutable|run#6|KeyError|baseline_description = lam.get_function_configuration(
Issue-Key: lambda.image-uri-preserved-through-create-and-get|run#20|AssertionError|assert resolved.startswith(f"{expected_repo_prefix}@sha256:"), (
Issue-Key: lambda.layer-omitted-options-stay-absent-from-list-and-description-defaults-empty|run#4|AssertionError|assert resp.get("Description") == "", (
Issue-Key: lambda.durable-config-defaults-and-partial-update|run#7|KeyError|assert created["DurableConfig"]["RetentionPeriodInDays"] == 14, (
Issue-Key: lambda.code-location-serves-the-exact-bytes-and-a-published-version-is-byte-frozen|_download#16|AssertionError|raise AssertionError(
Issue-Key: lambda.dry-run-does-not-invoke-handler|run#9|IncompleteReadError|assert dry["Payload"].read() == b"", "DryRun must return an empty response body"
Issue-Key: lambda.create-event-source-mapping-duplicate-conflict|run#12|AssertionError|assert message == expected, (
Issue-Key: lambda.layer-permission-lifecycle|run#9|AssertionError|assert none_msg == "No policy is associated with the given resource.", f"unexpected message for a layer with no policy: {none_msg!r}"
Issue-Key: lambda.layer-version-permission-duplicate-statement-rejected|run#21|AssertionError|assert gone_msg == expected_gone, f"expected message {expected_gone!r}, got {gone_msg!r}"
Issue-Key: lambda.layer-version-permission-lifecycle|run#20|AssertionError|assert message == expected_gone, f"expected message {expected_gone!r}, got {message!r}"
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda: ten read/representation and data-plane divergences — qualified reads, version metadata, Code.Location and layer-policy diagnosis

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunction

## Description
Ten net-new root causes in how the lambda service represents what it stored and serves its data-plane URLs.

Cause 1 of 10 — GetFunction with Qualifier='$LATEST' reports Concurrency.
AWS: reserved concurrency is a property of the unqualified function; GetFunction for a qualified identity (a version or $LATEST) omits the Concurrency block entirely. Emulator: it returns {'ReservedConcurrentExecutions': 2}. Why: a client reading a qualified identity concludes the qualifier carries its own limit. Fix: the GetFunction serializer's qualifier scoping. Traces: lambda.get-function-concurrency-field-scoped-to-unqualified-identity.

Cause 2 of 10 — a published version reuses $LATEST's RevisionId.
AWS: PublishVersion mints a new RevisionId for the version; the version's RevisionId differs from $LATEST's, which is what makes revision preconditions meaningful per identity. Emulator: the published version carries $LATEST's RevisionId. Why: optimistic-concurrency preconditions cannot distinguish the two identities. Fix: RevisionId generation in the PublishVersion handler/state. Traces: lambda.get-function-configuration-representation-matches-created-config.

Cause 3 of 10 — the PublishVersion response omits Description.
AWS: Smithy (lambda) FunctionConfiguration includes Description and PublishVersion returns the version's configuration, so the member is present. Emulator: the key is absent (KeyError 'Description'). Why: a client cannot read back what it published without a second call, and a strongly-typed SDK sees a missing member. Fix: the PublishVersion response serializer. Traces: lambda.publish-version-immutable.

Cause 4 of 10 — ResolvedImageUri is not digest-pinned.
AWS: for an Image package type, ImageConfigResponse/ResolvedImageUri pins the same repository to a digest (repo@sha256:...), not the mutable tag that was supplied. Emulator: it echoes '<repo>:v1'. Why: a client cannot tell which image the function is actually frozen to, which is the entire purpose of the resolved URI. Fix: image resolution in the CreateFunction/GetFunction path. Traces: lambda.image-uri-preserved-through-create-and-get.

Cause 5 of 10 — an omitted layer Description does not default to ''.
AWS: Smithy (lambda) PublishLayerVersion/GetLayerVersion include Description; when it was omitted AWS returns it as an empty string, present in the response and in ListLayerVersions. Emulator: the member is absent ('<absent>'). Why: clients that read the member unconditionally break, and list/describe disagree. Fix: the layer-version serializers' defaulting. Traces: lambda.layer-omitted-options-stay-absent-from-list-and-description-defaults-empty.

Cause 6 of 10 — the durable-execution config read omits RetentionPeriodInDays.
AWS: the configuration read returns the retention member (defaulted when it was never set). Recorded behavior only for the default value. Emulator: the key is absent (KeyError 'RetentionPeriodInDays'). Why: a client cannot discover the effective retention or partially update it. Fix: the durable-execution config read serializer/defaults. Traces: lambda.durable-config-defaults-and-partial-update.

Cause 7 of 10 — the presigned Code.Location is not served.
AWS: GetFunction returns Code.Location as a presigned URL that serves exactly the uploaded bytes, and a published version's location keeps serving the frozen bytes. Emulator: fetching it answers HTTP 404 with '{"__type":"UnknownService","message":"no emulated service is registered for \'awslambda-us-west-2-tasks\'"}' — the URL names a host/service the emulator does not route. Why: every client that downloads its own deployment package (CI diffing, byte-freeze verification) gets a 404 from a URL the API just handed it. The address announced is not one the emulator answers on. Fix: the code-store data-plane route and the presigned-URL builder in the lambda service. Traces: lambda.code-location-serves-the-exact-bytes-and-a-published-version-is-byte-frozen.

Cause 8 of 10 — Invoke with InvocationType=DryRun returns a body shorter than its Content-Length.
AWS: a DryRun invoke validates the request and answers 204 with no payload (and never runs the handler). Emulator: the response declares 2 bytes and delivers 0, so botocore raises IncompleteReadError('0 read, but total bytes expected is 2'). Why: the SDK raises a transport error instead of returning, so a client cannot use DryRun to validate permissions at all. Fix: the Invoke handler's DryRun response framing (status/Content-Length/body agreement). Traces: lambda.dry-run-does-not-invoke-handler.

Cause 9 of 10 — the duplicate-ESM conflict message does not name the event source or the function.
AWS: the message is built from the request — 'An event source mapping with SQS arn (" <arn> ") and function (" <name> ") already exists. Please update or delete the existing mapping with UUID <uuid>' — so naming the source ARN and function is the rule, not the wording. Emulator: 'An event source mapping of this event source and function already exists. Update or delete the existing mapping with UUID <uuid>'. Why: with many mappings a client cannot tell which source/function pair collided. Fix: the CreateEventSourceMapping conflict-error renderer. Traces: lambda.create-event-source-mapping-duplicate-conflict.

Cause 10 of 10 — a layer version with no resource policy is diagnosed as a missing layer.
AWS: GetLayerVersionPolicy/RemoveLayerVersionPermission on an existing layer version that has no policy says the layer version 'does not have any resource policy' (naming the layer version ARN); 'The resource you requested does not exist' is reserved for a layer version that is absent. The distinction is derived from state, so it is the rule. Emulator: both conditions answer the resource-does-not-exist message. Why: a client cannot tell 'no policy yet' (add one) from 'wrong ARN' (fix the ARN), and a duplicate-statement refusal is reported as a missing layer. Fix: the layer-version policy handlers' not-found vs no-policy branching. Traces: lambda.layer-permission-lifecycle, lambda.layer-version-permission-duplicate-statement-rejected, lambda.layer-version-permission-lifecycle.

## Evidence
Cause 1: "GetFunction(qualifier='$LATEST') must omit Concurrency even though reserved concurrency 2 is set, got {'ReservedConcurrentExecutions': 2}".
Cause 2: "published version reused $LATEST's revision id".
Cause 3: KeyError: 'Description'.
Cause 4: "ResolvedImageUri should pin the SAME repository (...) to a digest, got '...:v1'".
Cause 5: "publish: an omitted Description must default to '' and be present; got '<absent>'".
Cause 6: KeyError: 'RetentionPeriodInDays'.
Cause 7: "DATAPLANE_UNIMPLEMENTED: the presigned Code.Location returned HTTP 404: '{\"__type\":\"UnknownService\",\"message\":\"no emulated service is registered for 'awslambda-us-west-2-tasks'\"}'".
Cause 8: botocore.exceptions.IncompleteReadError: 0 read, but total bytes expected is 2.
Cause 9: got 'An event source mapping of this event source and function already exists. Update or delete the existing mapping with UUID ...', expected the message naming the SQS arn and the function.
Cause 10: "expected message 'Layer version <arn> does not have any resource policy.', got 'The resource you requested does not exist. Resource: <arn>'".

## Steps to Reproduce
1. PutFunctionConcurrency then GetFunction(Qualifier='$LATEST').
2. PublishVersion and compare the version's RevisionId with $LATEST's; read Description from the response.
3. CreateFunction with PackageType=Image and a tagged URI, then GetFunction and read ResolvedImageUri.
4. PublishLayerVersion without Description, then GetLayerVersion and ListLayerVersions.
5. Read the durable-execution configuration without setting retention.
6. GetFunction and fetch Code.Location.
7. Invoke with InvocationType='DryRun'.
8. CreateEventSourceMapping twice for the same queue+function and read the conflict message.
9. GetLayerVersionPolicy on a layer version that exists but has no policy.
