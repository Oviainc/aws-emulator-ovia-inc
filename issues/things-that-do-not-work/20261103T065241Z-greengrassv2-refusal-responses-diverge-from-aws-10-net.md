Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: greengrassv2.describe-component-nonexistent-not-found greengrassv2.delete-component-then-redelete-rejected greengrassv2.delete-component-in-use-by-active-deployment-lenient greengrassv2.list-components-scope-private-isolation greengrassv2.delete-component-nonexistent-not-found greengrassv2.get-component-nonexistent-not-found greengrassv2.get-component-malformed-arn-rejected greengrassv2.describe-component-malformed-arn-validation-rejected greengrassv2.get-component-version-artifact-malformed-arn-rejected greengrassv2.list-component-versions-malformed-arn-rejected greengrassv2.list-component-versions-pagination-next-token greengrassv2.get-deployment-nonexistent-not-found greengrassv2.component-version-duplicate-conflict-rejected greengrassv2.concurrent-duplicate-component-version-creation-race-single-winner greengrassv2.create-component-version-invalid-recipe-rejected greengrassv2.create-component-version-invalid-recipe-validation-rejected greengrassv2.tag-resource-non-greengrass-arn-rejected
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrassv2.describe-component-nonexistent-not-found|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2.delete-component-then-redelete-rejected|run#12|AssertionError|assert message_after_delete == describe_expected, (
Issue-Key: greengrassv2.delete-component-in-use-by-active-deployment-lenient|run#21|AssertionError|assert deleted_message == expected, f"expected message {expected!r}, got {deleted_message!r}"
Issue-Key: greengrassv2.list-components-scope-private-isolation|run#16|AssertionError|assert gone_message == expected, f"expected message {expected!r}, got {gone_message!r}"
Issue-Key: greengrassv2.delete-component-nonexistent-not-found|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2.get-component-nonexistent-not-found|run#14|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2.get-component-malformed-arn-rejected|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2.describe-component-malformed-arn-validation-rejected|run#5|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2.get-component-version-artifact-malformed-arn-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2.list-component-versions-malformed-arn-rejected|run#10|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2|ListComponentVersions|ValidationException
Issue-Key: greengrassv2.get-deployment-nonexistent-not-found|run#18|AssertionError|assert message == "Deployment not found", f"got message={message!r}"
Issue-Key: greengrassv2.component-version-duplicate-conflict-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: greengrassv2.concurrent-duplicate-component-version-creation-race-single-winner|run#17|AssertionError|assert message == expected, f"loser message {message!r}, expected {expected!r}"
Issue-Key: greengrassv2.create-component-version-invalid-recipe-rejected|run#9|AssertionError|assert code == "AccessDeniedException", f"expected AccessDeniedException, got code={code!r} msg={msg!r}"
Issue-Key: greengrassv2.create-component-version-invalid-recipe-validation-rejected|run#9|AssertionError|assert code == "AccessDeniedException", f"got code={code!r} msg={msg!r}"
Issue-Key: greengrassv2.tag-resource-non-greengrass-arn-rejected|run#9|AssertionError|assert code == "BadRequestException", f"expected BadRequestException, got code={code!r}"
Issue-Commit: 3ecb0d6b353755c874093f3eab62bbd62a85b9a2

# greengrassv2 refusal responses diverge from AWS: 10 net-new causes across component/deployment not-found wording, ARN-validation wording, duplicate-conflict wording, recipe refusal taxonomy, TagResource error class and a valid ListComponentVersions ARN refused

> _Note: this issue was identified against the code base at commit `3ecb0d6b353755c874093f3eab62bbd62a85b9a2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeComponent

## Description
Every implemented greengrassv2 refusal path that the suite exercises answers emulator-authored wording, the wrong error class, or refuses input AWS accepts. The messages below are requirements: fixed text exactly as AWS words it, variable parts filled from the request and from emulator state. 10 causes to close.

Cause 1 of 10: DescribeComponent not-found message is emulator-authored.
AWS: Smithy: DescribeComponent GET /greengrass/v2/components/{arn}/metadata, error ResourceNotFoundException (HTTP 404) — "The requested resource can't be found." Recorded behavior: the message is `Component (<componentName>:<componentVersion>) does not exist in account <accountId>.` Fixed text: `Component (`, `) does not exist in account `, `.`; variables: the component name and version resolved from the requested ARN, and the caller's account id.
Emulator: 404 ResourceNotFoundException but message `The component version <full component-version ARN> does not exist.` — a different sentence that names the ARN instead of name:version and account.
Why: clients and CLIs that match on AWS's sentence (and humans reading it) do not recognise the emulator's; the account id, which AWS includes, is absent.
Fix: the not-found refusal in DescribeComponent's handler/serializer for this service.
Traces: greengrassv2.describe-component-nonexistent-not-found, greengrassv2.delete-component-then-redelete-rejected, greengrassv2.delete-component-in-use-by-active-deployment-lenient, greengrassv2.list-components-scope-private-isolation.

Cause 2 of 10: DeleteComponent not-found message is emulator-authored, and differs from DescribeComponent's on AWS.
AWS: Smithy: DeleteComponent DELETE /greengrass/v2/components/{arn}, 204 on success, ResourceNotFoundException (404). Recorded behavior: deleting a component version that does not exist (including re-deleting one already deleted) is refused with message `Component [<componentName> : <componentVersion>] in the accountId [<accountId>] does not exist` — fixed text `Component [`, ` : `, `] in the accountId [`, `] does not exist`; variables as above. Note the bracket-and-spaces form is DeleteComponent's own wording, NOT DescribeComponent's parenthesised form.
Emulator: the same generic `The component version <arn> does not exist.`
Why: the two operations are indistinguishable to a client that keys off the message, and neither matches AWS.
Fix: the not-found refusal in DeleteComponent's handler.
Traces: greengrassv2.delete-component-nonexistent-not-found, greengrassv2.delete-component-then-redelete-rejected.

Cause 3 of 10: GetComponent not-found message is emulator-authored.
AWS: Smithy: GetComponent GET /greengrass/v2/components/{arn}, ResourceNotFoundException (404). Recorded behavior: message `PRIVATE component (<componentName>:<componentVersion>) under account <accountId> does not exist` — fixed text `PRIVATE component (`, `) under account `, ` does not exist` (no trailing period), where PRIVATE is the scope of the component looked up.
Emulator: `The component version <arn> does not exist.`
Why: the scope word and the account id AWS reports are lost, and the sentence does not match.
Fix: the not-found refusal in GetComponent's handler.
Traces: greengrassv2.get-component-nonexistent-not-found, greengrassv2.delete-component-in-use-by-active-deployment-lenient.

Cause 4 of 10: a malformed component-version ARN is refused with the generic Smithy constraint sentence instead of AWS's.
AWS: Smithy: GetComponent/DescribeComponent/GetComponentVersionArtifact take `arn` (ComponentVersionARN, pattern ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+:versions:[^:]+$) and model ValidationException (HTTP 400) — "The request isn't valid." Recorded behavior: AWS refuses 400 ValidationException with message `Invalid Input: Component version ARN <arn> is invalid` — fixed text `Invalid Input: Component version ARN `, ` is invalid`; the one variable is the offending `arn` value copied from the request.
Emulator: 400 ValidationException but message `1 validation error detected: Value '<arn>' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+:versions:[^:]+$` — the framework's generic constraint rendering, which also leaks the model regex.
Why: the message is not AWS's, so message-matching clients break, and the regex leak is not something AWS ever emits.
Fix: the request validation for these operations must emit the service's own invalid-ARN sentence rather than the generic constraint message.
Traces: greengrassv2.get-component-malformed-arn-rejected, greengrassv2.describe-component-malformed-arn-validation-rejected, greengrassv2.get-component-version-artifact-malformed-arn-rejected.

Cause 5 of 10: ListComponentVersions' malformed-ARN refusal has the same defect with its own wording.
AWS: Smithy: ListComponentVersions `arn` (ComponentARN, pattern ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+$), ValidationException (400). Recorded behavior: message `Invalid Input: Component ARN <arn> is invalid` — note `Component ARN`, not `Component version ARN`; the variable is the offending arn from the request.
Emulator: the generic `1 validation error detected: Value '<arn>' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+$`.
Why: same as cause 4, with the wrong noun phrase as well.
Fix: ListComponentVersions' request validation.
Traces: greengrassv2.list-component-versions-malformed-arn-rejected.

Cause 6 of 10: ListComponentVersions refuses a component-VERSION ARN that AWS accepts.
AWS: recorded behavior only — the modeled @pattern for `arn` matches a component ARN, but AWS does not enforce it server-side: passing a component-version ARN (as returned by CreateComponentVersion, `...:components:<name>:versions:<version>`) is accepted and the operation pages that component's versions normally (maxResults=1 + nextToken walks every version exactly once).
Emulator: the pre-handler constraint check refuses the request with 400 ValidationException `1 validation error detected: Value '<version arn>' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+$`.
Why: the natural client flow — create a version, keep the returned ARN, list the component's versions with it — is impossible; the caller gets a 400 for a request AWS serves.
Fix: ListComponentVersions' request validation must accept a component-version ARN and normalise it to the component ARN before the handler. (Complementary to, and not covered by, the already-filed stub issue for implementing ListComponentVersions: that 501 is a different cause, and this refusal happens before the handler runs.)
Traces: greengrassv2.list-component-versions-pagination-next-token.

Cause 7 of 10: GetDeployment's not-found message is emulator-authored.
AWS: Smithy: GetDeployment GET /greengrass/v2/deployments/{deploymentId}, ResourceNotFoundException (404). Recorded behavior: message `Deployment not found` — entirely fixed text, no variable part; AWS does NOT echo the deployment id.
Emulator: 404 ResourceNotFoundException with message `The deployment <deploymentId> does not exist.`
Why: the sentence differs and leaks the requested id where AWS deliberately does not; the same `Deployment not found` text is what CancelDeployment/DeleteDeployment must also use.
Fix: the not-found refusal in GetDeployment's handler (and the shared deployment-not-found text).
Traces: greengrassv2.get-deployment-nonexistent-not-found.

Cause 8 of 10: CreateComponentVersion's duplicate-version ConflictException message is emulator-authored, and AWS has two distinct sentences.
AWS: Smithy: CreateComponentVersion models ConflictException (HTTP 409) — "Your request has conflicting operations." Recorded behavior: re-creating an existing component version answers 409 with `Component [<componentName> : <componentVersion>] for account [<accountId>] already exists with state: [<componentState>]` (fixed text `Component [`, ` : `, `] for account [`, `] already exists with state: [`, `]`; state observed as DEPLOYABLE for a settled version); when the losing request carries `tags`, the message is instead `Component [<componentName> : <componentVersion>] for account [<accountId>] already exists and can't be created again with tags.`
Emulator: 409 with message `The component version <name>:<version> already exists.` for both cases.
Why: clients cannot tell the plain conflict from the tags-conflict, and neither sentence matches AWS's.
Fix: the duplicate-detection refusal in CreateComponentVersion's handler, selecting the message by whether the request carried tags and reporting the stored component state.
Traces: greengrassv2.component-version-duplicate-conflict-rejected, greengrassv2.concurrent-duplicate-component-version-creation-race-single-winner.

Cause 9 of 10: CreateComponentVersion refuses a bad recipe with the wrong error class and its own wording.
AWS: Smithy: CreateComponentVersion models both AccessDeniedException (HTTP 403) and ValidationException (HTTP 400). Recorded behavior: recipe-level rejections come back as 403 `AccessDeniedException` whose message prefixes the recipe diagnosis onto an authorization sentence: for a payload that is neither JSON nor YAML, `Invalid recipe format; line: <line>, column: <column>. User: <callerArn> is not authorized to perform: null` (fixed text `Invalid recipe format; line: `, `, column: `, `. User: `, ` is not authorized to perform: null`; variables are the parse position and the caller's STS ARN); for a parseable recipe missing the name/version, `Invalid Input: The component recipe doesn't specify a component name or version. Add ComponentName and ComponentVersion parameters to the recipe and try again. User: <callerArn> is not authorized to perform: null`.
Emulator: 400 `ValidationException` with its own prose — `the recipe could not be parsed: while parsing a node, did not find expected node content at byte 0 line 1 column 1` and `The recipe must declare ComponentName.`
Why: the HTTP status (403 vs 400), the error code and the message all differ, so retry/permission handling on the client takes the wrong branch.
Fix: the recipe parsing/validation step of CreateComponentVersion's handler and the error it raises.
Traces: greengrassv2.create-component-version-invalid-recipe-rejected, greengrassv2.create-component-version-invalid-recipe-validation-rejected.

Cause 10 of 10: TagResource refuses a non-Greengrass resourceArn with the wrong error class and message.
AWS: Smithy: TagResource POST /tags/{resourceArn}, `resourceArn` is GenericV2ARN with pattern ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):(components|deployments|coreDevices): and the operation models ValidationException(400)/ResourceNotFoundException(404)/InternalServerException(500). Recorded behavior: a well-formed ARN of another service (e.g. `arn:aws:s3:::<bucket>`) is refused 400 with error code `BadRequestException` and message `Invalid input resource arn: expected vendor greengrass` — entirely fixed text. (Recorded behavior only for the code: `BadRequestException` is not in this operation's modeled error list, but it is what the service answers.)
Emulator: 400 with code `ValidationException` and the generic constraint sentence from the `resourceArn` pattern.
Why: a client branching on `BadRequestException` (and a companion path that expects `NotFoundException`/`Resource was not found` for a missing greengrass resource) never sees it.
Fix: TagResource's resourceArn vendor check — raise the service's BadRequestException with that sentence instead of letting the modeled pattern constraint reject the request.
Traces: greengrassv2.tag-resource-non-greengrass-arn-rejected.

## Evidence
Per-cause expected (AWS) vs actual (emulator), quoted from the replay output:

1. describe-component-nonexistent-not-found / delete-component-then-redelete-rejected / delete-component-in-use-by-active-deployment-lenient / list-components-scope-private-isolation —
   expected `Component (<name>:1.0.0) does not exist in account <account>.`
   actual   `The component version arn:aws:greengrass:<region>:<account>:components:<name>:versions:1.0.0 does not exist.`
2. delete-component-nonexistent-not-found —
   expected `Component [<name> : 1.0.0] in the accountId [<account>] does not exist`
   actual   `The component version arn:...:components:<name>:versions:1.0.0 does not exist.`
3. get-component-nonexistent-not-found —
   expected `PRIVATE component (<name>:1.0.0) under account <account> does not exist`
   actual   `The component version arn:...:components:<name>:versions:1.0.0 does not exist.`
4. get-component-malformed-arn-rejected / describe-component-malformed-arn-validation-rejected / get-component-version-artifact-malformed-arn-rejected —
   expected `Invalid Input: Component version ARN <arn> is invalid`
   actual   `1 validation error detected: Value '<arn>' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+:versions:[^:]+$`
5. list-component-versions-malformed-arn-rejected —
   expected `Invalid Input: Component ARN <arn> is invalid`
   actual   `1 validation error detected: Value '<arn>' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+$`
6. list-component-versions-pagination-next-token —
   expected: ListComponentVersions(arn=<component-version ARN>, maxResults=1) succeeds and pages all versions
   actual   `botocore.errorfactory.ValidationException: ... Value 'arn:aws:greengrass:<region>:<account>:components:<name>:versions:1.0.0' at 'arn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:...:components:[^:]+$`
7. get-deployment-nonexistent-not-found —
   expected message `Deployment not found`
   actual   `The deployment <deploymentId> does not exist.`
8. component-version-duplicate-conflict-rejected —
   expected `Component [<name> : 1.0.0] for account [<account>] already exists with state: [DEPLOYABLE]`
   actual   `The component version <name>:1.0.0 already exists.`
   concurrent-duplicate-component-version-creation-race-single-winner —
   expected loser message `Component [<name> : 1.0.0] for account [<account>] already exists and can't be created again with tags.`
   actual   `The component version <name>:1.0.0 already exists.`
9. create-component-version-invalid-recipe-rejected —
   expected code `AccessDeniedException` (403), message `Invalid recipe format; line: 1, column: 1. User: <callerArn> is not authorized to perform: null`
   actual   code `ValidationException`, message `the recipe could not be parsed: while parsing a node, did not find expected node content at byte 0 line 1 column 1`
   create-component-version-invalid-recipe-validation-rejected —
   expected code `AccessDeniedException`, message `Invalid Input: The component recipe doesn't specify a component name or version. Add ComponentName and ComponentVersion parameters to the recipe and try again. User: <callerArn> is not authorized to perform: null`
   actual   code `ValidationException`, message `The recipe must declare ComponentName.`
10. tag-resource-non-greengrass-arn-rejected —
   expected code `BadRequestException`, message `Invalid input resource arn: expected vendor greengrass`
   actual   code `ValidationException` (generic resourceArn pattern constraint message)

(The `<name>`, `<account>`, `<arn>`, `<deploymentId>`, `<callerArn>` placeholders stand for values taken from the request/state; the per-run literals in the replay output are examples, not requirements.)

## Steps to Reproduce
Point a greengrassv2 client at the emulator; `<acct>` is the caller's account id.
1. Causes 1-3: create a component version from an inline recipe, DeleteComponent it, then call `describe_component(arn=...)`, `get_component(arn=...)` and `delete_component(arn=...)` again on the same ARN. Compare each 404 message with `Component (<name>:<ver>) does not exist in account <acct>.`, `PRIVATE component (<name>:<ver>) under account <acct> does not exist` and `Component [<name> : <ver>] in the accountId [<acct>] does not exist`.
2. Causes 4-5: call `get_component(arn="not-a-valid-arn")`, `describe_component(arn="not-a-valid-arn")`, `get_component_version_artifact(arn="not-an-arn", artifactName="x")` and `list_component_versions(arn="not-an-arn-at-all")`; expect `Invalid Input: Component version ARN <arn> is invalid` / `Invalid Input: Component ARN <arn> is invalid`.
3. Cause 6: create versions 1.0.0/1.0.1/1.0.2 of one component, then `list_component_versions(arn=<the component-VERSION ARN returned by create>, maxResults=1)` and page with nextToken — expect success, observe a 400 ValidationException.
4. Cause 7: `get_deployment(deploymentId="<never-created>")` — expect message `Deployment not found`.
5. Cause 8: call `create_component_version` twice with the same inline recipe (once without tags, once with `tags={...}`) and compare the two 409 messages.
6. Cause 9: `create_component_version(inlineRecipe=b"]] not json nor yaml")` and `create_component_version(inlineRecipe=json.dumps({"NotARecipe": True}).encode())` — expect 403 AccessDeniedException with the two sentences above.
7. Cause 10: `tag_resource(resourceArn="arn:aws:s3:::some-bucket", tags={"env": "x"})` — expect 400 `BadRequestException` / `Invalid input resource arn: expected vendor greengrass`.
