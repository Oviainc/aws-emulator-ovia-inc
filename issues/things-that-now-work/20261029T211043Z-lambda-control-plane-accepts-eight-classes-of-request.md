Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.blueprint-tag-tag-resource-empty-tag-map-rejected lambda.blueprint-tag-tag-resource-oversize-value-rejected lambda.blueprint-tag-tag-resource-system-key-rejected lambda.create-function-code-validation-errors lambda.create-function-non-zip-and-empty-zip-rejected-creating-nothing lambda.publish-layer-version-corrupt-zip-rejected-no-version-minted lambda.docdb-esm-without-database-name-rejected-before-any-access-check lambda.mq-esm-role-without-describe-broker-rejected-before-the-broker-is-looked-up lambda.msk-esm-role-without-describe-cluster-rejected-before-the-cluster-is-looked-up lambda.capacity-provider-invalid-params-rejected lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected lambda.weighted-alias-across-execution-roles-rejected lambda.weighted-alias-on-latest-primary-rejected lambda.weighted-alias-with-two-additional-versions-rejected lambda.delete-version-referenced-by-routing-weight-rejected lambda.snapstart-on-a-runtime-without-snapstart-rejected lambda.provisioned-concurrency-on-snapstart-version-rejected lambda.snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version lambda.env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create lambda.delete-latest-qualifier-rejected lambda.delete-layer-version lambda.invoke-qualifier-conflicting-with-name-qualifier-rejected lambda.put-resource-policy-length-exceeded lambda.layer-permission-revisionid-precondition lambda.remove-layer-permission-precondition-failed lambda.blueprint-tag-version-and-alias-arns-rejected lambda.async-payload-limit
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.blueprint-tag-tag-resource-empty-tag-map-rejected|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.blueprint-tag-tag-resource-oversize-value-rejected|run#7|AssertionError|assert msg == expected, f"expected the exact ValidationException message, got {msg[-200:]!r}"
Issue-Key: lambda.blueprint-tag-tag-resource-system-key-rejected|_error#4|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error code")
Issue-Key: lambda.create-function-code-validation-errors|_error_code#18|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: lambda.create-function-non-zip-and-empty-zip-rejected-creating-nothing|_error#6|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Key: lambda.publish-layer-version-corrupt-zip-rejected-no-version-minted|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Key: lambda.docdb-esm-without-database-name-rejected-before-any-access-check|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded: {out!r}")
Issue-Key: lambda.mq-esm-role-without-describe-broker-rejected-before-the-broker-is-looked-up|_error#9|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded: {out!r}")
Issue-Key: lambda.msk-esm-role-without-describe-cluster-rejected-before-the-cluster-is-looked-up|_error#10|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded: {out!r}")
Issue-Key: lambda.capacity-provider-invalid-params-rejected|_client_error#8|AssertionError|raise AssertionError(f"{what} succeeded instead of being rejected")
Issue-Key: lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded with {result!r}")
Issue-Key: lambda.weighted-alias-across-execution-roles-rejected|_error#17|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: lambda.weighted-alias-on-latest-primary-rejected|run#10|AssertionError|assert (code, status, message) == ("InvalidParameterValueException", 400, expected), (
Issue-Key: lambda.weighted-alias-with-two-additional-versions-rejected|run#12|AssertionError|assert message == "Number of items in AdditionalVersionWeights cannot be greater than 1", message
Issue-Key: lambda.delete-version-referenced-by-routing-weight-rejected|_error#11|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: lambda.snapstart-on-a-runtime-without-snapstart-rejected|_error#4|AssertionError|raise AssertionError(f"expected a ClientError, the call succeeded: {out!r}")
Issue-Key: lambda.provisioned-concurrency-on-snapstart-version-rejected|_error#8|AssertionError|raise AssertionError(
Issue-Key: lambda.snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version|run#6|AssertionError|assert (conf["State"], conf.get("StateReasonCode"), conf.get("StateReason")) == (
Issue-Key: lambda.env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {str(result)[:400]}")
Issue-Key: lambda.delete-latest-qualifier-rejected|run#10|AssertionError|assert message == "$LATEST version cannot be deleted without deleting the function.", message
Issue-Key: lambda.delete-layer-version|run#14|AssertionError|assert mutate_code_msg == expected_mutate_code_msg, (
Issue-Key: lambda.invoke-qualifier-conflicting-with-name-qualifier-rejected|run#9|AssertionError|assert message == expected, f"expected the message {expected!r}, got {message!r}"
Issue-Key: lambda.put-resource-policy-length-exceeded|run#15|AssertionError|assert error_msg == (
Issue-Key: lambda.layer-permission-revisionid-precondition|run#13|AssertionError|assert stale_add_msg == STALE_REVISION_MESSAGE, f"unexpected message: {stale_add_msg!r}"
Issue-Key: lambda.remove-layer-permission-precondition-failed|run#14|AssertionError|assert code_msg == (
Issue-Key: lambda.blueprint-tag-version-and-alias-arns-rejected|run#8|AssertionError|assert msg == expected, f"ListTags: expected the exact message {expected!r}, got {msg!r}"
Issue-Key: lambda.async-payload-limit|run#10|AssertionError|assert code == "RequestEntityTooLargeException", (
Issue-Commit: 221dd3602e5d0e66d71e70da1fb8509b07c948c5

# Lambda control plane accepts eight classes of request AWS refuses, and its refusal codes/messages diverge (8 causes)

> _Note: this issue was identified against the code base at commit `221dd3602e5d0e66d71e70da1fb8509b07c948c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Eight net-new root causes in the Lambda control plane, grounded in failing `lambda` behavioral checks that the replay report marks neither ALREADY FILED nor OWNED. Seven are missing validation (the emulator returns 2xx where AWS refuses); the eighth is a family of wrong refusal codes/messages.

Cause 1 of 8 - TagResource accepts tag maps AWS refuses.
AWS: TagResource requires a non-empty Tags map, enforces the per-value length limit, and refuses AWS-reserved (`aws:`) tag keys, answering `400 InvalidParameterValueException`. Smithy: TagResource.Tags is @required (target Tags map) and the operation models InvalidParameterValueException (httpError 400); success is 204 with an empty body.
Emulator: `tag_resource(Resource=<fn-arn>, Tags={})` and the oversize-value and system-key variants all succeed (204), so no error code or message is produced at all.
Why: a client's malformed tagging call silently "succeeds", so tag-governance code that relies on the refusal (and on reading back exactly what AWS would have stored) diverges from production.
Fix: input validation in the TagResource handler.
Traces: lambda.blueprint-tag-tag-resource-empty-tag-map-rejected, lambda.blueprint-tag-tag-resource-oversize-value-rejected, lambda.blueprint-tag-tag-resource-system-key-rejected.

Cause 2 of 8 - Code packages are never validated, so non-zip/empty/corrupt artifacts are deployed and layer versions are minted from them.
AWS: CreateFunction/UpdateFunctionCode refuse a payload that is not a valid zip (or an empty zip) with `400 InvalidParameterValueException` and create nothing; PublishLayerVersion refuses a corrupt zip and mints no version.
Emulator: CreateFunction with a non-zip body returns `201` with a full FunctionConfiguration (State=Pending, a computed CodeSha256), and PublishLayerVersion returns `201` with Content.Location/CodeSha256 for a corrupt archive - a layer version now exists.
Why: broken artifacts are accepted and only surface later (or never), so the "rejected, nothing created" contract clients depend on at deploy time is lost.
Fix: package validation shared by the CreateFunction / UpdateFunctionCode / PublishLayerVersion handlers (parse the zip before storing; leave no state behind on refusal).
Traces: lambda.create-function-code-validation-errors, lambda.create-function-non-zip-and-empty-zip-rejected-creating-nothing, lambda.publish-layer-version-corrupt-zip-rejected-no-version-minted.

Cause 3 of 8 - CreateEventSourceMapping accepts mappings whose required source configuration is missing or whose source does not exist.
AWS: a DocumentDB mapping without DocumentDBEventSourceConfig.DatabaseName is refused with `400 InvalidParameterValueException` BEFORE any access check; MQ/MSK mappings naming a broker/cluster the caller cannot describe (or that does not exist) are refused (InvalidParameterValueException / ResourceNotFoundException). Smithy: CreateEventSourceMapping models InvalidParameterValueException (400) and ResourceNotFoundException (404).
Emulator: all three succeed with `202` and a mapping in State=Creating, EventSourceArn echoing the absent source (`arn:aws:rds:...:cluster:sim2real-absent-...`, `arn:aws:mq:...:broker:sim2real-absent-...`).
Why: the mapping is "created" and then silently does nothing; a client's negative test passes in production and fails here, and provisioning code never learns its source is wrong.
Fix: request validation and source resolution in the CreateEventSourceMapping handler.
Traces: lambda.docdb-esm-without-database-name-rejected-before-any-access-check, lambda.mq-esm-role-without-describe-broker-rejected-before-the-broker-is-looked-up, lambda.msk-esm-role-without-describe-cluster-rejected-before-the-cluster-is-looked-up.

Cause 4 of 8 - Capacity-provider APIs accept invalid parameters.
AWS: CreateCapacityProvider naming a nonexistent VPC/operator role, with the same instance type both allowed and excluded, is refused (`400 InvalidParameterValueException`); ListFunctionVersionsByCapacityProvider with an invalid parameter is refused rather than answering an empty list.
Emulator: CreateCapacityProvider succeeds and a capacity provider exists; ListFunctionVersionsByCapacityProvider answers `200 {'CapacityProviderArn': ..., 'FunctionVersions': []}`.
Why: contradictory infrastructure requests are accepted, so misconfiguration is invisible until the (never-arriving) capacity is needed.
Fix: validation in the CreateCapacityProvider and ListFunctionVersionsByCapacityProvider handlers.
Traces: lambda.capacity-provider-invalid-params-rejected, lambda.list-function-versions-by-capacity-provider-invalid-parameter-rejected.

Cause 5 of 8 - Weighted alias routing is not validated and a version referenced by a routing weight can be deleted.
AWS: CreateAlias/UpdateAlias refuse a RoutingConfig whose versions do not share one execution role, whose primary FunctionVersion is $LATEST, or that names more than one additional version, with `400 InvalidParameterValueException`; DeleteFunction(Qualifier=<version>) for a version referenced by an alias' AdditionalVersionWeights is refused with `409 ResourceConflictException`. Smithy: CreateAlias models InvalidParameterValueException (400) and ResourceConflictException (409); AliasRoutingConfiguration.AdditionalVersionWeights carries the single-additional-version rule.
Emulator: CreateAlias returns `201` with `RoutingConfig: {'AdditionalVersionWeights': {'3': 0.5}}` for all three invalid shapes, and DeleteFunction of the referenced version returns `204`, leaving an alias pointing at a deleted version.
Why: a traffic-shifting deployment can be configured in a way AWS forbids, and the alias can be left dangling - a client's rollback logic then operates on state production cannot reach.
Fix: RoutingConfig validation in the CreateAlias/UpdateAlias handlers and a reference check in DeleteFunction's qualifier branch.
Traces: lambda.weighted-alias-across-execution-roles-rejected, lambda.weighted-alias-on-latest-primary-rejected, lambda.weighted-alias-with-two-additional-versions-rejected, lambda.delete-version-referenced-by-routing-weight-rejected.

Cause 6 of 8 - SnapStart constraints are not enforced.
AWS: UpdateFunctionConfiguration(SnapStart={'ApplyOn':'PublishedVersions'}) on a runtime without SnapStart support is refused (`400 InvalidParameterValueException`); PutProvisionedConcurrencyConfig on a SnapStart version is refused (the two features are mutually exclusive); a published version whose beforeCheckpoint/before-snapshot hook raises settles in State=Failed with a StateReasonCode naming the SnapStart failure, not Active.
Emulator: the nodejs22.x SnapStart update returns `200` with `SnapStart: {'ApplyOn': 'PublishedVersions', 'OptimizationStatus': 'Off'}`; PutProvisionedConcurrencyConfig succeeds and a provisioned config exists on the SnapStart version; the version with a raising hook settles `State='Active'` with StateReasonCode/StateReason absent (`None`).
Why: a client believes SnapStart is active where it is not, combines two mutually exclusive features, and sees a broken version reported healthy.
Fix: SnapStart validation in UpdateFunctionConfiguration/PublishVersion, the PutProvisionedConcurrencyConfig handler, and the version-state machine that records hook failure.
Traces: lambda.snapstart-on-a-runtime-without-snapstart-rejected, lambda.provisioned-concurrency-on-snapstart-version-rejected, lambda.snapstart-python-before-snapshot-hook-that-raises-fails-the-published-version.

Cause 7 of 8 - The 4 KB environment-variable limit is not enforced at CreateFunction.
AWS: an Environment.Variables map whose serialized size is exactly 4096 bytes is accepted and one byte over is refused with `400 InvalidParameterValueException` ("Lambda was unable to configure your environment variables because the environment variables you have provided exceeded the 4KB limit").
Emulator: the 4097-byte request returns `201` and the function exists.
Why: a client that sizes its configuration against the documented quota gets no feedback here and a hard failure in production.
Fix: Environment size validation in the CreateFunction (and UpdateFunctionConfiguration) handler.
Traces: lambda.env-vars-exactly-4kb-accepted-one-byte-over-rejected-at-create.

Cause 8 of 8 - Refusal codes and messages diverge from AWS's wording for eight refusals that the emulator does produce.
AWS (per refusal; fixed text quoted, variable parts named):
- DeleteFunction(Qualifier='$LATEST'): message `$LATEST version cannot be deleted without deleting the function.` (fixed text, trailing period included).
- GetLayerVersion/DeleteLayerVersion of an absent layer version: `Layer version <layer-version-arn> does not exist.` - the variable part is the FULL layer-version ARN, and the sentence ends with a period.
- Invoke whose function name qualifies a version conflicting with Qualifier: `The derived qualifier from the function name does not match the specified qualifier.` (fixed text, no request values).
- PutResourcePolicy with a policy over the limit: Smithy models Policy with @length max 20480 and the operation's own `PolicyLengthExceededException` (httpError 400); AWS answers that error with its policy-length message, not a generic validation report.
- AddLayerVersionPermission / RemoveLayerVersionPermission with a stale RevisionId: Smithy models PreconditionFailedException (httpError 412) on both; AWS's message names the LAYER VERSION and the GetLayerVersion API.
- ListTags/TagResource on a qualified (version or alias) ARN: `Tags on function aliases and versions are not supported. Please specify a function ARN.` (fixed text).
- Asynchronous Invoke (InvocationType=Event) above the async payload limit: recorded behavior only - the recording requires error code `RequestEntityTooLargeException`; Smithy models Invoke's 413s as RequestTooLargeException / SerializedRequestEntityTooLargeException, so the async code is not modelled and the recording is the requirement.
Emulator: `$LATEST version cannot be deleted without deleting the function` (period dropped); `Layer version sim2real-behav-layer-88fa8fb69e8b:1 does not exist` (bare `<name>:<version>` instead of the ARN, period dropped); `The function name '<name>:1' qualifies version '1', which does not match the Qualifier parameter '$LATEST'`; `1 validation error detected: Value '<policy>... [91147 bytes]' at 'policy' failed to satisfy constraint: Member must have length less than or equal to 20480` instead of PolicyLengthExceededException's message; `The Revision Id provided does not match the latest Revision Id. Call the GetFunction/GetAlias API to retrieve the latest Revision Id` on BOTH layer-permission operations (the function/alias text reused for a layer version); `Tags are not supported for a function version or alias. The resource '<qualified-arn>' qualifies the function with '1'; specify the unqualified function ARN instead.`; and error code `RequestTooLargeException` where the async path must answer `RequestEntityTooLargeException`.
Why: clients and SDK middleware branch on the error code and parse these messages; a reworded message or a swapped code makes production error handling (retry/classify/alert) behave differently against the emulator.
Fix: the message/code construction in each named handler and its validation layer (delete_function, delete_layer_version/get_layer_version, invoke's qualifier resolution, put_resource_policy, add_layer_version_permission, remove_layer_version_permission, list_tags/tag_resource).
Traces: lambda.delete-latest-qualifier-rejected, lambda.delete-layer-version, lambda.invoke-qualifier-conflicting-with-name-qualifier-rejected, lambda.put-resource-policy-length-exceeded, lambda.layer-permission-revisionid-precondition, lambda.remove-layer-permission-precondition-failed, lambda.blueprint-tag-version-and-alias-arns-rejected, lambda.async-payload-limit.

## Evidence
Cause 1: `AssertionError: the call succeeded; real AWS rejects it, so there is no error code` (TagResource(Tags={})).
Cause 2: `AssertionError: expected the call to be rejected, but it SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 201 ...}, 'State': 'Pending', 'CodeSha256': '...'}`; `expected the call to be refused, but it succeeded: {'HTTPStatusCode': 201 ..., 'Content': {'Location': 'https://awslambda-us-west-2-layers.s3...', 'CodeSha256': '...'}}`.
Cause 3: `AssertionError: expected a ClientError, the call succeeded: {'HTTPStatusCode': 202 ..., 'State': 'Creating', 'EventSourceArn': 'arn:aws:mq:us-west-2:000000000000:broker:sim2real-absent-...'}`.
Cause 4: `AssertionError: CreateCapacityProvider with a nonexistent VPC/role and contradictory instance-type filters succeeded instead of being rejected`; `expected the call to be refused, but it succeeded with {'HTTPStatusCode': 200, 'FunctionVersions': []}`.
Cause 5: `expected the call to be refused, but it succeeded: {'HTTPStatusCode': 201, 'RoutingConfig': {'AdditionalVersionWeights': {'3': 0.5}}}`; DeleteFunction of the weighted version -> `{'HTTPStatusCode': 204}`.
Cause 6: `expected a ClientError, the call succeeded: {... 'Runtime': 'nodejs22.x', 'SnapStart': {'ApplyOn': 'PublishedVersions'}}`; `PutProvisionedConcurrencyConfig was expected to be refused on a SnapStart version but it SUCCEEDED`; `version 1 with a raising hook settled 'Active' None: None`.
Cause 7: `expected the call to be refused, but it succeeded: {'HTTPStatusCode': 201, 'FunctionName': '...env4097...'}`.
Cause 8: see the quoted expected-vs-actual message pairs in the description; e.g. `AssertionError: expected message 'Layer version arn:aws:lambda:us-west-2:000000000000:layer:<name>:1 does not exist.', got 'Layer version <name>:1 does not exist'` and `AssertionError: expected RequestEntityTooLargeException for a 1048577-byte payload, got 'RequestTooLargeException'`.

## Steps to Reproduce
1. git_clone(); 2. run_behavioral_suite(); 3. replay the ids in trace_ids. Per cause: (1) TagResource with Tags={}, with an oversize value, and with an `aws:`-prefixed key; (2) CreateFunction with a non-zip/empty-zip ZipFile and PublishLayerVersion with a corrupt zip; (3) CreateEventSourceMapping for a DocumentDB cluster without DatabaseName, and for absent MQ/MSK sources; (4) CreateCapacityProvider with a nonexistent VPC/role plus contradictory instance-type filters, then ListFunctionVersionsByCapacityProvider with an invalid parameter; (5) CreateAlias with the three invalid RoutingConfigs, then DeleteFunction(Qualifier=<weighted version>); (6) UpdateFunctionConfiguration(SnapStart=PublishedVersions) on nodejs22.x, PutProvisionedConcurrencyConfig on a SnapStart version, PublishVersion with a raising before-snapshot hook; (7) CreateFunction with 4097 bytes of environment variables; (8) trigger each refusal named in cause 8 and compare code + message text.
