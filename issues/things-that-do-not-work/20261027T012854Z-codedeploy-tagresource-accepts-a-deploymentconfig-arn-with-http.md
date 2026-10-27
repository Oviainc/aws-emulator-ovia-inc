Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codedeploy.tag-resource-unsupported-arn-rejected
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy.tag-resource-unsupported-arn-rejected|_client_error#9|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 32af0d92e59e46f2fdf169ea1f28f8cf7b2e0f6a

# CodeDeploy TagResource accepts a deploymentconfig ARN with HTTP 200 instead of refusing it with ArnNotSupportedException

> _Note: this issue was identified against the code base at commit `32af0d92e59e46f2fdf169ea1f28f8cf7b2e0f6a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
One cause.

Cause 1 of 1: the tagging ARN resolver treats a `deploymentconfig` ARN as a taggable/readable CodeDeploy resource, so TagResource writes tags onto a deployment configuration and answers 200 where AWS refuses the ARN by resource type.

AWS: Smithy TagResource declares `ResourceArn` (Arn, @length 1..1011) @required and models ArnNotSupportedException (HTTP 400, doc: "The specified ARN is not supported. For example, it might be an ARN for a resource that is not expected.") as a class DISTINCT from InvalidArnException ("The specified ARN is not in a valid format") and from the per-kind existence errors (ApplicationDoesNotExistException, DeploymentGroupDoesNotExistException, DeploymentConfigDoesNotExistException). CodeDeploy's taggable resource types are applications and deployment groups; a well-formed ARN whose resource part is `deploymentconfig:<name>` is a CodeDeploy ARN naming a resource these tagging operations do not act on, so it is refused with HTTP 400 / ArnNotSupportedException even when the deployment configuration itself exists. The message for the write path is "The ARN: <ResourceArn> is not supported by CodeDeploy" — fixed text "The ARN: " + " is not supported by CodeDeploy", with the ARN taken verbatim from the request's ResourceArn member. ListTagsForResource models the same ArnNotSupportedException and words it differently: "Not a CodeDeploy ARN: <ResourceArn>" — fixed text "Not a CodeDeploy ARN: " with the request's ResourceArn appended. (Model documents no message text for the exception member itself — "The message that corresponds to the exception thrown by CodeDeploy" — so the status and error code are the modeled requirements and the two message templates above are the recorded behavior of the respective operations.)
Emulator: TagResource(ResourceArn=arn:aws:codedeploy:<region>:<account>:deploymentconfig:<name>, Tags=[{Key:Team,Value:probe-config}]) returns HTTP 200 with an empty body (content-length 2, `{}`) and no `__type`/`message`; the tags are applied to the deployment configuration record. The same resolver classifies the ARN as supported for the read path, so ListTagsForResource on a deploymentconfig ARN is likewise not refused with ArnNotSupportedException/"Not a CodeDeploy ARN: <arn>" — the check's very next assertion, never reached because the write leg already succeeded. The positive control in the same check passes: TagResource + ListTagsForResource against an `application:<name>` ARN in the same account round-trip correctly, so the defect is attributable to the resource-type gate alone, not to tagging being broken.
Why: a client that uses ArnNotSupportedException to discriminate "this resource type cannot carry tags" from "malformed ARN" (InvalidArnException) and from "resource missing" (…DoesNotExistException) never sees the refusal; a tag-propagation or compliance sweep that walks every CodeDeploy ARN believes it successfully tagged deployment configurations, and the emulator's state then reports tags on a resource AWS cannot hold any — a divergence that reappears on every later read. botocore's exception factory exposes ArnNotSupportedException for this operation, so the client's except-branch is dead code against the emulator.
Fix: the tagging ARN resolver shared by TagResource and ListTagsForResource — `crate::state::parse_tagged_resource_arn` must classify a well-formed CodeDeploy ARN whose resource part is `deploymentconfig` as ArnRefusal::Unsupported rather than resolving it to a taggable resource, and the two operations must emit their own ArnNotSupportedException message templates (write: "The ARN: <arn> is not supported by CodeDeploy"; read: "Not a CodeDeploy ARN: <arn>"), which today share one wording through `crate::state::resource_arn_not_supported_message`. Related open issue "CodeDeploy accepts invalid input and misclassifies rejections" covers other validation gates (CreateDeploymentGroup/RegisterApplicationRevision/ListDeployments/Stop|GetDeployment) and does NOT cover the tagging ARN resolver or carry this check's replay key.
Traces: codedeploy.tag-resource-unsupported-arn-rejected (behavioral check, group error-semantics).

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, check `codedeploy.tag-resource-unsupported-arn-rejected` [mismatch]:

  File "check_codedeploy_tag_resource_unsupported_arn_rejected.py", line 80, in run
    error = _client_error(lambda: cd.tag_resource(ResourceArn=cfg_arn, Tags=[{"Key": "Team", "Value": "probe-config"}]))
  AssertionError: expected a ClientError, but the call SUCCEEDED and returned
    {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}

Expected (AWS): HTTP 400, Error.Code = 'ArnNotSupportedException', Error.Message = 'The ARN: <cfg_arn> is not supported by CodeDeploy' where <cfg_arn> is the ResourceArn sent in the request (arn:aws:codedeploy:<region>:<account>:deploymentconfig:<deploymentConfigName>).
Actual (emulator): HTTP 200, empty JSON body, no __type/message; the tag is stored on the deployment configuration.

Same-check downstream requirement (not reached because the write leg returned 200): ListTagsForResource(ResourceArn=<same deploymentconfig ARN>) must answer HTTP 400 / ArnNotSupportedException with message 'Not a CodeDeploy ARN: <cfg_arn>'.

Positive control in the same check PASSED: TagResource then ListTagsForResource on arn:aws:codedeploy:<region>:<account>:application:<applicationName> returned Team=probe-control, so TagResource is reachable and functional for supported ARNs.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): TagResource input {ResourceArn: Arn @required @length 1..1011, Tags: TagList @required}; errors = ApplicationDoesNotExistException, ArnNotSupportedException, DeploymentConfigDoesNotExistException, DeploymentGroupDoesNotExistException, InvalidArnException, InvalidTagsToAddException, ResourceArnRequiredException, TagRequiredException (all HTTP 400). ListTagsForResource errors = ArnNotSupportedException, InvalidArnException, ResourceArnRequiredException (all HTTP 400). ArnNotSupportedException doc: "The specified ARN is not supported. For example, it might be an ARN for a resource that is not expected."

Code: services/service-com-amazonaws-codedeploy-codedeploy-20141006/src/ops/tag_resource.rs:217-219 resolves the ARN via crate::state::parse_tagged_resource_arn and only refuses on ArnRefusal::Malformed/Unsupported (helper arn_refused, lines 123-138); a deploymentconfig ARN is resolved as a taggable resource instead, so lines 240-253 write the tags and line 255 returns TagResourceOutput{} (200).

## Steps to Reproduce
1. Boot the emulator from this checkout and point a botocore codedeploy client at it.
2. CreateApplication(applicationName=<app>, computePlatform='Server'); TagResource(ResourceArn='arn:aws:codedeploy:<region>:<account>:application:<app>', Tags=[{Key:'Team',Value:'probe-control'}]); ListTagsForResource on the same ARN -> Team=probe-control (positive control, passes today).
3. CreateDeploymentConfig(deploymentConfigName=<cfg>, computePlatform='Server', minimumHealthyHosts={type:'HOST_COUNT', value:1}).
4. TagResource(ResourceArn='arn:aws:codedeploy:<region>:<account>:deploymentconfig:<cfg>', Tags=[{Key:'Team',Value:'probe-config'}]).
   Expect: HTTP 400, ArnNotSupportedException, message 'The ARN: arn:aws:codedeploy:<region>:<account>:deploymentconfig:<cfg> is not supported by CodeDeploy'.
   Observe: HTTP 200 with an empty body and the tag applied.
5. ListTagsForResource(ResourceArn=<same deploymentconfig ARN>).
   Expect: HTTP 400, ArnNotSupportedException, message 'Not a CodeDeploy ARN: arn:aws:codedeploy:<region>:<account>:deploymentconfig:<cfg>'.
6. GetDeploymentConfig(deploymentConfigName=<cfg>) must still read back normally (the refused calls must not have mutated it).
7. Re-replay the behavioral check `codedeploy.tag-resource-unsupported-arn-rejected` to confirm.
