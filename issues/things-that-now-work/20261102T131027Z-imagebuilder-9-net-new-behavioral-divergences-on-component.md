Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.ibp-create-component-rejects-a-two-part-semantic-version imagebuilder.ibp-create-image-recipe-rejects-a-volume-above-16000-gib imagebuilder.ibp-create-component-rejects-a-document-with-no-phases imagebuilder.ibp-create-component-rejects-data-uri-and-platform-mistakes imagebuilder.ibp-create-component-from-an-s3-uri-stores-the-fetched-document imagebuilder.ibp-create-image-recipe-rejects-a-component-that-does-not-exist imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected imagebuilder.ibp-create-image-recipe-rejects-a-duplicate-semantic-version imagebuilder.concurrent-create-component-same-name-version-contention imagebuilder.ibp-component-document-round-trips-verbatim-and-defaults-to-an-encrypted-build imagebuilder.component-policy-put-get-roundtrip
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder.ibp-create-component-rejects-a-two-part-semantic-version|run#4|AssertionError|assert code == "InvalidParameterValueException", f"semanticVersion '1.0' raised {code!r}"
Issue-Key: imagebuilder.ibp-create-image-recipe-rejects-a-volume-above-16000-gib|run#6|AssertionError|assert code == "InvalidParameterValueException", f"volumeSize 16001 raised {code!r}"
Issue-Key: imagebuilder.ibp-create-component-rejects-a-document-with-no-phases|_error_code#4|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Response: {resp!r}")
Issue-Key: imagebuilder.ibp-create-component-rejects-data-uri-and-platform-mistakes|_error#7|AssertionError|raise AssertionError(f"the call succeeded; real AWS rejects it. Response: {resp!r}")
Issue-Key: imagebuilder.ibp-create-component-from-an-s3-uri-stores-the-fetched-document|run#10|KeyError|assert comp["data"] == _DOC, f"uri-sourced component stored {comp['data']!r}"
Issue-Key: imagebuilder.ibp-create-image-recipe-rejects-a-component-that-does-not-exist|run#7|AssertionError|assert (code, msg) == ("ResourceNotFoundException", expected), \
Issue-Key: imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected|run#8|AssertionError|assert sibling_gone == "ResourceNotFoundException", (
Issue-Key: imagebuilder.ibp-create-image-recipe-rejects-a-duplicate-semantic-version|run#9|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: imagebuilder.concurrent-create-component-same-name-version-contention|run#11|AssertionError|assert (code == "ResourceAlreadyExistsException" and message in exists) or (
Issue-Key: imagebuilder.ibp-component-document-round-trips-verbatim-and-defaults-to-an-encrypted-build|run#8|KeyError|refs = resp["latestVersionReferences"]
Issue-Key: imagebuilder.component-policy-put-get-roundtrip|run#12|AssertionError|assert got["Statement"][0]["Principal"]["AWS"] == principal_arn, f"expected {principal_arn!r}, got {got['Statement'][0]['Principal']['AWS']!r}"
Issue-Commit: c588c92daf57694e44caee6ddcc17c51be576a2d

# imagebuilder: 9 net-new behavioral divergences on component/image-recipe create, read, refusal and policy paths

> _Note: this issue was identified against the code base at commit `c588c92daf57694e44caee6ddcc17c51be576a2d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComponent

## Description
NINE distinct root causes in the imagebuilder emulator, all surfaced by the behavioral suite and none of them covered by an already-filed issue (the suite marked no owner for any of the checks below). All NINE must be closed before this issue is submitted.

Related open issues that look similar but do NOT carry these replay keys and do not cover these checks: 20261102T085510Z-imagebuilder-image-builder-parameter-validation-refuses-with-validationexception-instead.md (wire corpus; its causes are other operations/parameters), 20261101T001020Z-imagebuilder-image-builder-not-found-lookups-refuse-400-invalidrequestexception.md, 20261102T085510Z-imagebuilder-image-builder-not-found-and-already-exists-refusals.md, 20261031T232326Z-imagebuilder-image-builder-component-and-image-recipe-responses-omit.md, 20261102T085510Z-imagebuilder-image-builder-accepts-creates-and-deletes-that-aws.md.

--- Cause 1 of 9: constraint refusals answered as a generic ValidationException instead of InvalidParameterValueException (CreateComponent.semanticVersion, CreateImageRecipe.blockDeviceMappings[].ebs.volumeSize).
AWS: Smithy: CreateComponentRequest.semanticVersion -> VersionNumber @pattern ^[0-9]+\.[0-9]+\.[0-9]+$; EbsInstanceBlockDeviceSpecification.volumeSize -> EbsVolumeSizeInteger @range min 1 max 16000. Smithy com.amazonaws.imagebuilder#InvalidParameterValueException is a client error with @httpError 400, doc "The value that you provided for the specified parameter is invalid.", one `message` member. Image Builder answers a parameter refusal with HTTP 400, error type InvalidParameterValueException (in $.__type and the x-amzn-errortype header) and a single-sentence message whose FIXED text is "The value supplied for parameter '" + "' is not valid. " followed by a constraint clause; the VARIABLE parts are the API parameter path taken from the request and the limit/pattern taken from the model. For a pattern violation the clause is "<param> must match pattern <pattern>"; for an upper-bound violation it is "<param> must be smaller than or equal to <max>". List members are addressed with zero-based bracket indices built from the request (blockDeviceMappings[0].ebs.volumeSize), never Smithy's internal member path.
Emulator: the generic Smithy constraint layer refuses first, so semanticVersion='1.0' comes back as error code 'ValidationException' and volumeSize=16001 likewise as 'ValidationException', with the generic "1 validation error detected: Value '<v>' at '<smithy path>' failed to satisfy constraint: ..." sentence instead of Image Builder's template.
Why: SDK/CLI callers discriminate on the error code. Code that catches InvalidParameterValueException (retry, fall-back, user-facing field reporting) never runs because an unmodelled ValidationException is seen as an unknown client error; code that parses the message to name the offending field gets a Smithy member path that does not exist in the API.
Fix: the imagebuilder request-validation layer for create_component and create_image_recipe — translate constraint violations, per operation handler, into InvalidParameterValueException (400, type in $.__type and x-amzn-errortype) with AWS's message template and the API-level, bracket-indexed parameter path.
Traces: imagebuilder.ibp-create-component-rejects-a-two-part-semantic-version, imagebuilder.ibp-create-image-recipe-rejects-a-volume-above-16000-gib.

--- Cause 2 of 9: CreateComponent accepts a component document that declares no phases.
AWS: recorded behavior only (the model constrains `data` only by InlineComponentData @length 1..16000 and @pattern ^[^\x00]+$; the document grammar is enforced server-side). Image Builder parses the submitted YAML at create time and refuses a document with no `phases` key with HTTP 400 InvalidParameterValueException, message "The value supplied for parameter 'data' is not valid. Missing phases." — the whole sentence is fixed text for this error class (the parameter name 'data' is the member that carried the document). Nothing is minted: the same name created afterwards with a valid document receives build version 1.
Emulator: the call succeeds with HTTP 200 and mints a build-version ARN .../component/<name>/1.0.0/1, so no refusal is raised at all.
Why: a caller that relies on the service to validate component documents gets an ACTIVE component whose document can never build, and the broken component consumes the name/version, so the error is discovered only at image-build time instead of at create time.
Fix: create_component's validation in the imagebuilder handler — parse `data` and refuse documents lacking phases before any state is written.
Traces: imagebuilder.ibp-create-component-rejects-a-document-with-no-phases.

--- Cause 3 of 9: CreateComponent's `uri` alternative to `data` is never fetched — a uri naming a missing object is accepted, and a component created from a uri stores no document.
AWS: Smithy: CreateComponentRequest has mutually-exclusive `data` (inline) and `uri` members; Component.data (ComponentData) carries the YAML document on GetComponent. Recorded behavior: Image Builder fetches the S3 object itself at create time. If the object is absent the create is refused HTTP 400 InvalidParameterValueException with message "The value supplied for parameter 'uri' is not valid. Not Found" (entirely fixed text for this class; 'uri' is the member name) and nothing is minted. When the object exists, the fetched bytes are stored verbatim as a one-time snapshot: GetComponent returns component.data byte-for-byte equal to the object's contents, and overwriting the S3 object afterwards does not change what GetComponent returns.
Emulator: a `uri` pointing at an object that does not exist is ACCEPTED with HTTP 200 and a build-version ARN; and when the object does exist, GetComponent on the resulting component has no `data` member at all (KeyError 'data'). The uri is accepted and then ignored.
Why: callers that stage component documents in S3 (the documented alternative to inline data) get components that silently contain no document, and typo'd/not-yet-uploaded uris are never reported, so the failure surfaces much later at build time.
Fix: create_component in the imagebuilder handler plus the component record in state — resolve the s3:// uri at create time, refuse with InvalidParameterValueException when the object cannot be fetched, and persist the fetched bytes as the component's immutable `data` for GetComponent to serialize.
Traces: imagebuilder.ibp-create-component-rejects-data-uri-and-platform-mistakes, imagebuilder.ibp-create-component-from-an-s3-uri-stores-the-fetched-document.

--- Cause 4 of 9: CreateImageRecipe referencing a component that does not exist refuses InvalidRequestException with an emulator-authored sentence instead of 404 ResourceNotFoundException with AWS's template.
AWS: Smithy: com.amazonaws.imagebuilder#ResourceNotFoundException is a client error with @httpError 404, doc "At least one of the resources referenced by your request does not exist.", one `message` member; it is the modeled not-found error for referenced resources. Image Builder refuses the create with HTTP 404 ResourceNotFoundException and the message template whose FIXED text is "The following required resource '" + "' cannot be found: '" + "'." — the VARIABLE parts are the resource type word (here Component) and the ARN taken from the request's components[].componentArn.
Emulator: HTTP 400 with error code 'InvalidRequestException' and its own sentence "The componentArn <arn> does not name a component in this Amazon Web Services Region."
Why: both the status class and the error code are wrong, so a caller branching on ResourceNotFoundException (or on 404) to create the missing component and retry instead sees a generic bad-request and aborts; message-parsing callers get wording AWS never emits.
Fix: create_image_recipe's component-reference resolution in the imagebuilder handler — raise ResourceNotFoundException (404) with AWS's "The following required resource 'Component' cannot be found: '<arn>'." template.
Traces: imagebuilder.ibp-create-image-recipe-rejects-a-component-that-does-not-exist.

--- Cause 5 of 9: GetComponent on a deleted component build refuses InvalidRequestException instead of 404 ResourceNotFoundException.
AWS: Smithy: ResourceNotFoundException @httpError 404. After DeleteComponent, a GetComponent on that build-version ARN is refused HTTP 404 ResourceNotFoundException with message "The following required resource 'Component' cannot be found: '<componentBuildVersionArn>'." — fixed text as in cause 4 with the ARN taken from the request. This holds for any build of a component, including a build that was never referenced by a recipe.
Emulator: the lookup after delete answers error code 'InvalidRequestException' (HTTP 400), so the check polling for the resource to disappear never observes a not-found.
Why: the standard "delete then confirm gone" idiom cannot distinguish "deleted" from "malformed request"; waiters and cleanup code that treat ResourceNotFoundException as success loop or fail.
Fix: the imagebuilder state lookup shared by get_component/delete_component — a миссing resource must map to ResourceNotFoundException (404) with the template above rather than to InvalidRequestException. Note the related open issue 20261101T001020Z-imagebuilder-image-builder-not-found-lookups-refuse-400-invalidrequestexception.md, which does not carry this check.
Traces: imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected.

--- Cause 6 of 9: CreateImageRecipe duplicate-version refusal carries an emulator-authored message.
AWS: Smithy: com.amazonaws.imagebuilder#ResourceAlreadyExistsException is a client error with @httpError 400, doc "The resource that you are trying to create already exists.", one `message` member, and it is in CreateImageRecipe's modeled error list. The message template's FIXED text is "The following resource '" + "' already exists: '" + "'."; the VARIABLE parts are the resource type word (ImageRecipe) and the ARN of the existing recipe from the emulator's own state. The refused create must leave the existing recipe untouched, and a different semanticVersion of the same name must still be accepted.
Emulator: the code ResourceAlreadyExistsException is right but the message is its own sentence, "An image recipe named <name> at version <version> already exists in this Amazon Web Services Region."
Why: callers that surface or match on the service message (and the AWS CLI's error rendering) get wording AWS never produces, and lose the existing resource's ARN that AWS's message hands back.
Fix: create_image_recipe's duplicate-detection branch in the imagebuilder handler — emit AWS's "The following resource 'ImageRecipe' already exists: '<arn>'." message.
Traces: imagebuilder.ibp-create-image-recipe-rejects-a-duplicate-semantic-version.

--- Cause 7 of 9: concurrent CreateComponent on the same name+version refuses losers with ResourceInUseException instead of ResourceAlreadyExistsException/BadRequestException.
AWS: Smithy: ResourceInUseException @httpError 400 is documented "The resource that you are trying to operate on is currently in use" — a different class from the already-exists case; ResourceAlreadyExistsException @httpError 400 is the create-collision error. Recorded behavior: with N racers creating the same component name and version under distinct clientTokens, at least one wins and every loser is refused either ResourceAlreadyExistsException with the message "The following resource 'Component' already exists: '<winning build-version ARN>'." (fixed text plus the existing ARN from state) or, when the loss happens inside build-number allocation, BadRequestException whose message begins with the fixed text "Superfluous identifier provided for ".
Emulator: every loser is refused 'ResourceInUseException' with the message "The component <arn> already exists with identical content." — neither the AWS error class nor the AWS wording.
Why: ResourceInUseException is the code AWS uses for a resource busy elsewhere, and clients commonly treat it as retryable; a create-collision reported under it makes callers retry forever instead of taking the already-exists path.
Fix: create_component's name+version collision handling in the imagebuilder handler/state — classify a collision as ResourceAlreadyExistsException with AWS's message (or BadRequestException "Superfluous identifier provided for ..." when build-number allocation loses).
Traces: imagebuilder.concurrent-create-component-same-name-version-contention.

--- Cause 8 of 9: CreateComponent's 200 response omits latestVersionReferences.
AWS: Smithy: CreateComponentResponse members requestId, clientToken, componentBuildVersionArn and latestVersionReferences -> LatestVersionReferences{latestVersionArn, latestMajorVersionArn, latestMinorVersionArn, latestPatchVersionArn}. AWS returns the four wildcard-version ARNs derived from the created build: the build-version ARN with its trailing /<version>/<build> replaced by /x.x.x, /<major>.x.x, /<major>.<minor>.x and /<major>.<minor>.<patch>. Account, region, component name and version come from the request and the created resource.
Emulator: the whole latestVersionReferences object is absent from the 200 body (the check raises KeyError 'latestVersionReferences').
Why: a client that resolves the documented "latest" wildcard ARNs straight from the create response — the normal way to reference the newest major/minor/patch of a component in a recipe — has nothing to read and cannot build the follow-up request.
Fix: create_component's response serializer in the imagebuilder handler, deriving the four ARNs from the created resource's ARN and semantic version. Related open issue 20261031T232326Z-imagebuilder-image-builder-component-and-image-recipe-responses-omit.md does not carry this check.
Traces: imagebuilder.ibp-component-document-round-trips-verbatim-and-defaults-to-an-encrypted-build.

--- Cause 9 of 9: GetComponentPolicy returns a single-principal Principal.AWS as a one-element array instead of the scalar string AWS stores.
AWS: Smithy: PutComponentPolicy takes `policy` (ResourcePolicyDocument, @length 1..30000) and GetComponentPolicyResponse returns `policy`. Recorded behavior only: AWS normalizes the attached IAM policy document the way every IAM-policy store does — a Principal "AWS" value given as a one-element list is returned as the scalar string, so Statement[0].Principal.AWS reads back as "<principal arn>" (the ARN value comes from the request) and not as a list; Action, given as a list, keeps its list form.
Emulator: the submitted document is echoed verbatim, so Statement[0].Principal.AWS reads back as a one-element array ["<principal arn>"].
Why: callers that read the policy back and index Principal.AWS as a string (the shape AWS returns, and the shape IAM policy tooling assumes for a single principal) get a list and either mis-compare or crash; round-trip diff/idempotency logic that compares a locally normalized document with the service's reports a spurious change on every poll.
Fix: PutComponentPolicy/GetComponentPolicy in the imagebuilder handler (or the resource-policy record in state) must normalize the stored policy document's single-element Principal values to scalars as AWS does, and GetComponentPolicy must return that normalized form.
Traces: imagebuilder.component-policy-put-get-roundtrip.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator (11/77 checks passed). All eleven failing checks below are listed under the suite's `### imagebuilder` heading and none was marked ALREADY FILED or OWNED.

Cause 1:
- [mismatch] imagebuilder.ibp-create-component-rejects-a-two-part-semantic-version — AssertionError: semanticVersion '1.0' raised 'ValidationException'; expected code InvalidParameterValueException and message "The value supplied for parameter 'semanticVersion' is not valid. semanticVersion must match pattern ^[0-9]+\.[0-9]+\.[0-9]+$".
- [mismatch] imagebuilder.ibp-create-image-recipe-rejects-a-volume-above-16000-gib — AssertionError: volumeSize 16001 raised 'ValidationException'; expected InvalidParameterValueException with "The value supplied for parameter 'blockDeviceMappings[0].ebs.volumeSize' is not valid. blockDeviceMappings[0].ebs.volumeSize must be smaller than or equal to 16000".

Cause 2:
- [mismatch] imagebuilder.ibp-create-component-rejects-a-document-with-no-phases — AssertionError: "the call succeeded; real AWS rejects it"; response HTTPStatusCode 200 with componentBuildVersionArn .../component/<name>/1.0.0/1. Expected (InvalidParameterValueException, "The value supplied for parameter 'data' is not valid. Missing phases.").

Cause 3:
- [mismatch] imagebuilder.ibp-create-component-rejects-data-uri-and-platform-mistakes — AssertionError: "the call succeeded; real AWS rejects it"; CreateComponent(uri=s3://<bucket>/component.yaml) against a bucket with no such object returned HTTPStatusCode 200 and a build-version ARN. Expected (InvalidParameterValueException, "The value supplied for parameter 'uri' is not valid. Not Found").
- [error] imagebuilder.ibp-create-component-from-an-s3-uri-stores-the-fetched-document — KeyError: 'data' at `comp["data"] == _DOC`; GetComponent on the uri-created component returned a component object with no `data` member.

Cause 4:
- [mismatch] imagebuilder.ibp-create-image-recipe-rejects-a-component-that-does-not-exist — AssertionError: missing component raised ('InvalidRequestException', 'The componentArn <arn> does not name a component in this Amazon Web Services Region.'), expected ('ResourceNotFoundException', "The following required resource 'Component' cannot be found: '<arn>'.").

Cause 5:
- [mismatch] imagebuilder.ibp-delete-component-referenced-by-a-recipe-rejected — AssertionError: "an unreferenced sibling build of the same component: lookup after delete returned 'InvalidRequestException'"; expected ResourceNotFoundException.

Cause 6:
- [mismatch] imagebuilder.ibp-create-image-recipe-rejects-a-duplicate-semantic-version — AssertionError: expected message "The following resource 'ImageRecipe' already exists: '<recipe arn>'.", got 'An image recipe named <name> at version 1.0.0 already exists in this Amazon Web Services Region.' (code ResourceAlreadyExistsException matched).

Cause 7:
- [mismatch] imagebuilder.concurrent-create-component-same-name-version-contention — AssertionError: "a losing racer got an unexpected refusal"; 5 of 6 racers were refused ('ResourceInUseException', 'The component <arn> already exists with identical content.'). Expected each loser to be ResourceAlreadyExistsException with "The following resource 'Component' already exists: '<winner arn>'." or BadRequestException with a message starting 'Superfluous identifier provided for '.

Cause 8:
- [error] imagebuilder.ibp-component-document-round-trips-verbatim-and-defaults-to-an-encrypted-build — KeyError: 'latestVersionReferences' at `resp["latestVersionReferences"]` on the CreateComponent 200 body. Expected the four derived ARNs {latestVersionArn: <base>/x.x.x, latestMajorVersionArn: <base>/1.x.x, latestMinorVersionArn: <base>/1.0.x, latestPatchVersionArn: <base>/1.0.0}.

Cause 9:
- [mismatch] imagebuilder.component-policy-put-get-roundtrip — AssertionError: expected 'arn:aws:iam::<account>:root', got ['arn:aws:iam::<account>:root'] for Statement[0].Principal.AWS from GetComponentPolicy after PutComponentPolicy with Principal {"AWS": [<arn>]}.

Model references used: smithy_query operation CreateComponent/GetComponent/CreateImageRecipe/GetComponentPolicy/DeleteComponent and shapes InvalidParameterValueException (client, httpError 400, member message), ResourceNotFoundException (client, httpError 404), ResourceAlreadyExistsException (client, httpError 400), ResourceInUseException (client, httpError 400), Component.data (ComponentData), EbsInstanceBlockDeviceSpecification.volumeSize (@range 1..16000), CreateComponentRequest.semanticVersion (@pattern ^[0-9]+\.[0-9]+\.[0-9]+$), CreateComponentResponse.latestVersionReferences -> LatestVersionReferences.
(ARNs, account ids, bucket/component names and version numbers above are one run's values; the requirement is the status, error code and the message template with its variable parts taken from the request and state.)

## Steps to Reproduce
All steps use a bootstrapped imagebuilder emulator; <name> is any fresh resource name, <doc> is a minimal valid component YAML with build and validate phases.

1. Cause 1a: CreateComponent(name=<name>, semanticVersion='1.0', platform='Linux', data=<doc>). Expect 400 InvalidParameterValueException with "The value supplied for parameter 'semanticVersion' is not valid. semanticVersion must match pattern ^[0-9]+\.[0-9]+\.[0-9]+$"; the emulator answers ValidationException with the generic '1 validation error detected' sentence.
2. Cause 1b: CreateImageRecipe(name=<name>, semanticVersion='1.0.0', parentImage='arn:aws:imagebuilder:<region>:aws:image/amazon-linux-2-x86/x.x.x', components=[{componentArn:<valid component>}], blockDeviceMappings=[{deviceName:'/dev/xvda', ebs:{volumeSize:16001, volumeType:'gp3'}}]). Expect 400 InvalidParameterValueException naming 'blockDeviceMappings[0].ebs.volumeSize'; got ValidationException. Then repeat with volumeSize 16000 — it must be accepted and round-trip.
3. Cause 2: CreateComponent(name=<name>, semanticVersion='1.0.0', platform='Linux', data='name: x\nschemaVersion: 1.0\n') — a document with no phases. Expect 400 InvalidParameterValueException "The value supplied for parameter 'data' is not valid. Missing phases." and nothing minted; the emulator returns 200 with a build-version ARN. Confirm nothing was minted by creating the same name with a valid document and checking the build suffix is /1.
4. Cause 3a: create an S3 bucket, do NOT put the object, then CreateComponent(name=<name>, semanticVersion='1.0.0', platform='Linux', uri='s3://<bucket>/component.yaml'). Expect 400 InvalidParameterValueException "The value supplied for parameter 'uri' is not valid. Not Found"; the emulator returns 200.
5. Cause 3b: PutObject the valid <doc> at s3://<bucket>/component.yaml, CreateComponent with that uri, then GetComponent on the returned build-version ARN. Expect component.data to equal the object bytes verbatim; the emulator returns a component with no `data` member. Overwrite the S3 object and GetComponent again — data must still equal the original snapshot.
6. Cause 4: CreateImageRecipe(..., components=[{componentArn:<a well-formed component ARN that does not exist>}]). Expect 404 ResourceNotFoundException "The following required resource 'Component' cannot be found: '<arn>'."; the emulator answers 400 InvalidRequestException with its own sentence.
7. Cause 5: CreateComponent twice under one name (two builds), DeleteComponent the unreferenced build, then GetComponent on it. Expect 404 ResourceNotFoundException with the 'cannot be found' template; the emulator answers InvalidRequestException.
8. Cause 6: CreateImageRecipe(name=<name>, semanticVersion='1.0.0', ...) then repeat the identical name+version. Expect 400 ResourceAlreadyExistsException "The following resource 'ImageRecipe' already exists: '<existing recipe arn>'."; the emulator uses its own 'An image recipe named ... already exists in this Amazon Web Services Region.' wording. Verify the first recipe's components were not rewritten and that version 1.0.1 of the same name is accepted.
9. Cause 7: fire 6 concurrent CreateComponent calls with the same name and semanticVersion='1.0.0' and distinct clientTokens. Expect every loser to be ResourceAlreadyExistsException with "The following resource 'Component' already exists: '<winner arn>'." (or BadRequestException 'Superfluous identifier provided for ...'); the emulator refuses them all with ResourceInUseException.
10. Cause 8: CreateComponent(name=<name>, semanticVersion='1.0.0', platform='Linux', data=<doc>, description=<text>) and inspect the 200 body for latestVersionReferences — expect the four wildcard ARNs derived from the created build (x.x.x / 1.x.x / 1.0.x / 1.0.0); the member is absent.
11. Cause 9: CreateComponent, then PutComponentPolicy(componentArn=<arn>, policy=json with Statement[0].Principal={'AWS': ['arn:aws:iam::<account>:root']} and Action=['imagebuilder:GetComponent']), then GetComponentPolicy(componentArn=<arn>). Expect Statement[0].Principal.AWS to read back as the scalar ARN string (Action stays a list); the emulator returns the one-element array verbatim.
12. Re-replay the check ids listed in trace_ids to confirm the fix.
