Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-component-resolution-and-artifact
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-component-resolution-and-artifact greengrassv2-deployment-error-paths greengrassv2-resource-tagging-error-paths
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|ResolveComponentCandidates|4xx->4xx|$.Message
Issue-Key: greengrass|ResolveComponentCandidates|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: greengrass|ResolveComponentCandidates|4xx->4xx|status
Issue-Key: greengrass|CreateDeployment|4xx->2xx|$.message
Issue-Key: greengrass|CreateDeployment|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: greengrass|CreateDeployment|4xx->2xx|status
Issue-Key: greengrass|CreateDeployment|4xx->4xx|$.message
Issue-Key: greengrass|GetDeployment|4xx->4xx|$.message
Issue-Key: greengrass|ListTagsForResource|4xx->4xx|$.message
Issue-Key: greengrass|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 66471a963a64e2b8a9116c3903f472052675d9f3

# greengrassv2 wire refusal paths diverge: wrong status, error code and message text

> _Note: this issue was identified against the code base at commit `66471a963a64e2b8a9116c3903f472052675d9f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ResolveComponentCandidates

## Description
Five implemented greengrassv2 refusal paths answer the wrong HTTP status, the wrong error code, or emulator-authored message text when the wire corpus is replayed. The status, the error code and the message are requirements: the fixed text exactly as AWS words it, the variable parts filled from the request and from emulator state. 5 causes to close.

Related-but-not-covering open issues (none of them carries these wire replay keys; all are behavioral-corpus findings): 20261103T065241Z-greengrassv2-refusal-responses-diverge-from-aws-10-net.md (its Cause 7 is the same GetDeployment not-found sentence as Cause 4 here), 20261103T065241Z-greengrassv2-createdeployment-accepts-nonexistent-targets-and-never-supersedes.md (its Cause 1 is the same missing target check as Cause 2 here), 20261103T091859Z-greengrassv2-3-net-new-causes-resolvecomponentcandidates-serves-200.md (its Cause 1 described ResolveComponentCandidates answering 200; it now answers 404, which is Cause 1 here), 20261103T065241Z-greengrassv2-tagresource-stub.md (TagResource is still a stub; Cause 5 here is on the IMPLEMENTED ListTagsForResource path).

Cause 1 of 5: ResolveComponentCandidates refuses an ordinary IAM SigV4 caller as not-found instead of access-denied. AWS: Smithy: ResolveComponentCandidates POST /greengrass/v2/resolveComponentCandidates, success 200 with resolvedComponentVersions; modeled errors AccessDeniedException (httpError 403, "You don't have permission to perform the action."), ResourceNotFoundException (404), ValidationException (400). Recorded behavior: this is a core-device data-plane operation, so a request signed with ordinary IAM control-plane credentials is refused HTTP 403 with x-amzn-errortype AccessDeniedException and message `Unable to determine service/operation name to be authorized` — entirely fixed text, no variable part — and the refusal is returned BEFORE any component lookup, so the candidate's existence is irrelevant. Emulator: HTTP 404 with x-amzn-errortype `ResourceNotFoundException` and no `Message` member at all. Why: a client branching on 403/AccessDeniedException (the production answer for IAM credentials) instead takes a not-found branch and concludes the component is missing, so permission handling written against the emulator is wrong in production; the absent message member also breaks any error-message logging. Fix: the credential-class check at the head of ResolveComponentCandidates' handler, before component resolution, and the AccessDeniedException it must raise (including the Message member its serializer emits).

Cause 2 of 5: CreateDeployment accepts a targetArn whose IoT target does not exist. AWS: Smithy: CreateDeployment POST /greengrass/v2/deployments, success code 201, `targetArn` required (TargetARN, @pattern ^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$), modeled errors include ValidationException (httpError 400, "The request isn't valid."). Recorded behavior: a syntactically valid targetArn naming an IoT thing or thing group that was never created is refused HTTP 400 with x-amzn-errortype `ValidationException` and message `Invalid Input: Target not found` — entirely fixed text, no variable part — and no deployment is recorded. Emulator: HTTP 201 with a deploymentId/iotJobId body, i.e. it creates a live deployment against a target that does not exist. Why: a typo'd or deleted target silently yields a deployment and an IoT job; the caller never learns the target is missing and the deployment history contains records AWS would never have. Fix: target-existence validation in CreateDeployment's handler (resolve the thing/thinggroup before recording the deployment) and the ValidationException it raises.

Cause 3 of 5: CreateDeployment's targetArn pattern-validation message leaks the model's regex anchors. AWS: Smithy: the same `targetArn` @pattern above; ValidationException (400). Recorded behavior: a targetArn that does not match is refused 400 ValidationException with message `1 validation error detected: Value '<targetArn>' at 'targetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+` — fixed text `1 validation error detected: Value '`, `' at 'targetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: `, then the pattern as AWS renders it, WITHOUT the leading `^` and trailing `$`; the one variable is the offending value copied from the request. Emulator: the same sentence but ending `pattern: ^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$` — the raw anchored model pattern. Why: the message is a requirement and clients (and test suites) that match AWS's rendered pattern text do not match the emulator's; the anchors are an artifact of how the emulator stores the trait, not something AWS emits. Fix: the shared constraint-message renderer used by CreateDeployment's request validation must strip the anchors when rendering a @pattern into the validation sentence.

Cause 4 of 5: GetDeployment's not-found message is emulator-authored and echoes the id AWS withholds. AWS: Smithy: GetDeployment GET /greengrass/v2/deployments/{deploymentId}, modeled ResourceNotFoundException (httpError 404). Recorded behavior: an unknown deploymentId is refused 404 with message `Deployment not found` — entirely fixed text, no variable part; AWS does not echo the requested deployment id. Emulator: 404 with message `The deployment <deploymentId> does not exist.` Why: the sentence differs from AWS's, so message-matching clients break, and it leaks the requested id where AWS deliberately does not. Fix: the not-found refusal in GetDeployment's handler and the shared deployment-not-found text (CancelDeployment/DeleteDeployment must use the same sentence).

Cause 5 of 5: ListTagsForResource's not-found refusal uses the component error code and wording instead of the tagging surface's. AWS: Smithy: ListTagsForResource GET /tags/{resourceArn}, `resourceArn` required (GenericV2ARN, @pattern ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):(components|deployments|coreDevices):), modeled errors ResourceNotFoundException(404)/ValidationException(400)/InternalServerException(500). Recorded behavior: a well-formed greengrass ARN naming a resource that does not exist is refused HTTP 404 with x-amzn-errortype `NotFoundException` — note this differs from the modeled ResourceNotFoundException and from the code DescribeComponent/GetComponent use for the same missing component — and message `Resource was not found`, entirely fixed text with no variable part (recorded behavior only for the code: NotFoundException is not in this operation's modeled error list, but it is what the service answers). Emulator: 404 with x-amzn-errortype `ResourceNotFoundException` and the component-flavoured sentence `Component [<componentName> : <componentVersion>] in the accountId [<accountId>] does not exist`. Why: code that branches on the tagging surface's own NotFoundException never takes that branch, and the leaked component wording makes the tagging path indistinguishable from the component path. Fix: the existence check in ListTagsForResource's handler (shared with TagResource/UntagResource) must raise the tagging surface's NotFoundException with that fixed sentence rather than reusing the component not-found error.

## Evidence
Per-cause expected (AWS, from the recorded response) vs actual (emulator), quoted from the replay output. Placeholders stand for values taken from the request/state; the per-run literals are examples, not requirements.

Cause 1 — greengrassv2-component-resolution-and-artifact#task#2, ResolveComponentCandidates:
  status: expected 403, got 404
  $.Message: expected 'Unable to determine service/operation name to be authorized', got None (member absent)
  $headers.x-amzn-errortype: expected 'AccessDeniedException', got 'ResourceNotFoundException'

Cause 2 — greengrassv2-deployment-error-paths#task#1, CreateDeployment:
  status: expected 400, got 201
  $.message: expected 'Invalid Input: Target not found', got None (member absent)
  $headers.x-amzn-errortype: expected 'ValidationException', got None (success body)

Cause 3 — greengrassv2-deployment-error-paths#task#2, CreateDeployment (status 400 on both sides):
  $.message expected "1 validation error detected: Value 'not-an-arn' at 'targetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+"
  $.message actual   "1 validation error detected: Value 'not-an-arn' at 'targetArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+$"
  (only difference: the leading ^ and trailing $; 'not-an-arn' is the value from the request)

Cause 4 — greengrassv2-deployment-error-paths#task#3, GetDeployment (status 404 on both sides):
  $.message expected 'Deployment not found'
  $.message actual   'The deployment <deploymentId> does not exist.' (recorded run: 2f709761-a52f-4189-9ffa-b60912cca380)

Cause 5 — greengrassv2-resource-tagging-error-paths#task#3, ListTagsForResource (status 404 on both sides):
  $headers.x-amzn-errortype expected 'NotFoundException', actual 'ResourceNotFoundException'
  $.message expected 'Resource was not found'
  $.message actual   'Component [<componentName> : <componentVersion>] in the accountId [<accountId>] does not exist' (recorded run: sim2real-probe-greengrassv2-ghost-tag-comp-5123e716 : 1.0.0, account 000000000000)

## Steps to Reproduce
Boot the emulator and point a greengrassv2 client (ordinary IAM SigV4 credentials) at it. <acct> is the caller's account id, <region> its region.
1. Cause 1 (fixture greengrassv2-component-resolution-and-artifact, step task#2): create a component version and then call resolve_component_candidates(platform={'name':'linux','attributes':{'os':'all'}}, componentCandidates=[{'componentName': <name>, 'componentVersion': '1.0.0', 'versionRequirements': {}}]) — expect HTTP 403, x-amzn-errortype AccessDeniedException, Message 'Unable to determine service/operation name to be authorized'; observe HTTP 404 ResourceNotFoundException with no Message.
2. Cause 2 (greengrassv2-deployment-error-paths, task#1): create_deployment(targetArn='arn:aws:iot:<region>:<acct>:thing/<never-created-thing>') — expect HTTP 400 ValidationException 'Invalid Input: Target not found' and no deployment recorded; observe HTTP 201 with a deploymentId.
3. Cause 3 (greengrassv2-deployment-error-paths, task#2): create_deployment(targetArn='not-an-arn') — expect the validation sentence ending 'pattern: arn:[^:]*:iot:[^:]*:[0-9]+:(thing|thinggroup)/.+'; observe the same sentence with the ^...$ anchors present.
4. Cause 4 (greengrassv2-deployment-error-paths, task#3): get_deployment(deploymentId='<never-created-uuid>') — expect 404 message 'Deployment not found'; observe 'The deployment <id> does not exist.'
5. Cause 5 (greengrassv2-resource-tagging-error-paths, task#3): list_tags_for_resource(resourceArn='arn:aws:greengrass:<region>:<acct>:components:<never-created-name>:versions:1.0.0') — expect 404 with x-amzn-errortype NotFoundException and message 'Resource was not found'; observe ResourceNotFoundException with the 'Component [...] in the accountId [...] does not exist' sentence.
