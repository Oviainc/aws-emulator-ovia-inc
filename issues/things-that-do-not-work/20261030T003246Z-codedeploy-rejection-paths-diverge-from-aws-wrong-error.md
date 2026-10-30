Issue-Source: trace-sweep-divergence
Issue-Trace: codedeploy-resource-tagging-via-arn-for-application-and-deployment-config
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codedeploy-resource-tagging-via-arn-for-application-and-deployment-config codedeploy-custom-deployment-config-lifecycle codedeploy-on-premises-instance-registration-and-tagging
Issue-Shape-Id: com.amazonaws.codedeploy#CodeDeploy_20141006
Issue-Key: codedeploy|TagResource|2xx->4xx|$
Issue-Key: codedeploy|TagResource|2xx->4xx|status
Issue-Key: codedeploy|TagResource|4xx->4xx|$.message
Issue-Key: codedeploy|CreateDeploymentConfig|4xx->4xx|$.message
Issue-Key: codedeploy|DeleteDeploymentConfig|4xx->4xx|$.message
Issue-Key: codedeploy|RegisterOnPremisesInstance|4xx->4xx|$.message
Issue-Commit: a6cd77094d82d955b2186a9b9b1faf688af70a8f

# CodeDeploy rejection paths diverge from AWS: wrong error messages and a TagResource request AWS accepts is refused (5 causes)

> _Note: this issue was identified against the code base at commit `a6cd77094d82d955b2186a9b9b1faf688af70a8f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Five CodeDeploy refusal sites diverge from AWS on the wire. One refuses a request AWS accepts (HTTP 200 becomes HTTP 400), and four return AWS's HTTP status and AWS's modeled error code with an emulator-authored message where AWS's text for that error class is fixed (or fixed with documented variable parts). The status, the error code and the error MESSAGE are all requirements, so each wrong message is a defect. Five independent sites, five causes.

Cause 1 of 5: TagResource refuses a request AWS accepts, with TagRequiredException.
AWS: Smithy TagResource declares ResourceArn (required, Arn, length 1..1011) and Tags (required, TagList) and models TagRequiredException, InvalidTagsToAddException, ResourceArnRequiredException, InvalidArnException, ArnNotSupportedException and the three *DoesNotExistException errors (all HTTP 400). Recorded behavior: the tagging step in this fixture — tagging an existing application/deployment-config by ARN — is ACCEPTED with HTTP 200 and an empty body {} (TagResourceOutput has no members). AWS raises TagRequiredException only when the request carries no Tags member at all, not for a Tags list the emulator judges insufficient.
Emulator: HTTP 400 with {'__type': 'TagRequiredException', 'message': 'A tag was not specified'} for that same accepted request.
Why: this is an over-rejection on the success path — a caller that successfully tags a CodeDeploy resource against AWS gets a hard 400 against the emulator, so tagging (and any provisioning flow that tags after create) fails outright rather than diverging in a detail. Hypothesis: the emulator treats a present-but-empty Tags list (or tag entries without a Value) as a missing Tags member.
Fix: TagResource's input validation in the codedeploy service crate — restrict TagRequiredException to an absent Tags member and let the modeled success path serialize the empty TagResourceOutput.
Traces: codedeploy-resource-tagging-via-arn-for-application-and-deployment-config#task#8.

Cause 2 of 5: TagResource InvalidArnException message text.
AWS: Smithy TagResource models InvalidArnException (HTTP 400). Recorded behavior: a ResourceArn that is not a parseable ARN is refused with HTTP 400, code InvalidArnException and the message 'Invalid ARN: <ResourceArn>' — fixed prefix 'Invalid ARN: ' followed by the offending ResourceArn taken verbatim from the request (the recording's 'not-a-valid-arn' is that request value filling the template, not part of it).
Emulator: same status and same code, message 'The ARN <ResourceArn> is not in a valid format' — AWS's fixed prefix is absent and the sentence is locally invented.
Why: tagging helpers and CLIs surface or match on the 'Invalid ARN: ' prefix; against the emulator the refusal is unrecognizable even though the code is right.
Fix: TagResource's ARN parse / InvalidArnException construction.
Traces: codedeploy-resource-tagging-via-arn-for-application-and-deployment-config#task#9.

Cause 3 of 5: CreateDeploymentConfig InvalidMinimumHealthyHostValueException message text.
AWS: Smithy CreateDeploymentConfig takes minimumHealthyHosts (MinimumHealthyHosts) and models InvalidMinimumHealthyHostValueException (HTTP 400). Recorded behavior: a minimumHealthyHosts whose type is FLEET_PERCENT and whose value is outside 0..100 is refused with HTTP 400 and the message 'The value for the minimum healthy hosts with type of FLEET_PERCENT should be positive and less than 100' — fixed text whose only variable part is the minimumHealthyHosts.type name from the request; the rejected VALUE never appears.
Emulator: same status and code, message 'minimumHealthyHosts value <value> is not a percentage: a FLEET_PERCENT value must be between 0 and 100', interpolating the rejected value AWS's message omits.
Why: a client or fixture matching AWS's refusal text does not recognize the rejection, and operators see wording no AWS runbook contains.
Fix: CreateDeploymentConfig's minimumHealthyHosts validation / error construction.
Traces: codedeploy-custom-deployment-config-lifecycle#task#9.

Cause 4 of 5: DeleteDeploymentConfig InvalidOperationException message for a predefined CodeDeployDefault.* config.
AWS: Smithy DeleteDeploymentConfig models InvalidOperationException (HTTP 400). Recorded behavior: deleting a predefined CodeDeployDefault.* deployment configuration is refused with HTTP 400 and the message 'Can not delete a predefined deployment configuration' — entirely fixed text with NO variable parts; the config name is not interpolated.
Emulator: same status and code, message 'The deployment configuration <deploymentConfigName> is a predefined deployment configuration and cannot be deleted', which interpolates the request's config name into invented text.
Why: callers branching on the refusal message for built-in configs misread it; none of AWS's required text is present.
Fix: DeleteDeploymentConfig's predefined-config guard / error construction.
Traces: codedeploy-custom-deployment-config-lifecycle#task#12.

Cause 5 of 5: RegisterOnPremisesInstance message when both an IAM user ARN and an IAM session ARN are supplied.
AWS: Smithy RegisterOnPremisesInstance takes instanceName (required), iamUserArn and iamSessionArn and models MultipleIamArnsProvidedException (HTTP 400) alongside IamArnRequiredException. Recorded behavior: a request carrying BOTH iamUserArn and iamSessionArn is refused with HTTP 400 and the message 'The on-premises instance could not be registered because both an IAM session ARN and an IAM user ARN were included in the request. Include only one IAM ARN in the request, and then try again.' — entirely fixed text; neither ARN nor the instance name is interpolated.
Emulator: same status and code, message 'Both an user ARN and an IAM session ARN were included in the request. Use only one ARN type' — a locally worded sentence missing AWS's leading clause and its remediation sentence.
Why: on-premises registration tooling reports the refusal verbatim and matchers written against AWS's text fail; the remediation guidance AWS gives is lost.
Fix: RegisterOnPremisesInstance's mutual-exclusion validation / MultipleIamArnsProvidedException construction.
Traces: codedeploy-on-premises-instance-registration-and-tagging#task#4.

Related open issues that do NOT cover these wire steps: 'CodeDeploy accepts invalid input and misclassifies rejections' (wire) owns the CreateDeploymentGroup/ListDeployments missing-gate and wrong-code clusters, not these sites. 'CodeDeploy rejection paths return the right error codes with the wrong error messages' is a BEHAVIORAL issue (corpus simparity-behavioral-aws-v1) whose keys are check ids; it carries no replay key for any wire fixture step, and the replay reports these clusters as unowned — causes 2, 3 and 4 above are the same emulator message sites seen from the wire corpus and must be re-replayed through these fixture steps to be confirmed fixed. Causes 1 and 5 are not described by it at all.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against this checkout (14 fixtures, 127 steps, 112 passed, 15 mismatched, score 88.2%, emulator_up=true, bootstrap_signal=false). Expected = recorded AWS response, Actual = emulator response.

1. codedeploy-resource-tagging-via-arn-for-application-and-deployment-config#task#8 — TagResource: expected HTTP 200 with body {}; got HTTP 400 with {'__type': 'TagRequiredException', 'message': 'A tag was not specified'}.
2. codedeploy-resource-tagging-via-arn-for-application-and-deployment-config#task#9 — TagResource: both HTTP 400; $.message value_mismatch — expected 'Invalid ARN: not-a-valid-arn' (prefix 'Invalid ARN: ' + the request's ResourceArn), got 'The ARN not-a-valid-arn is not in a valid format'.
3. codedeploy-custom-deployment-config-lifecycle#task#9 — CreateDeploymentConfig: both HTTP 400; $.message value_mismatch — expected 'The value for the minimum healthy hosts with type of FLEET_PERCENT should be positive and less than 100', got 'minimumHealthyHosts value -5 is not a percentage: a FLEET_PERCENT value must be between 0 and 100'.
4. codedeploy-custom-deployment-config-lifecycle#task#12 — DeleteDeploymentConfig: both HTTP 400; $.message value_mismatch — expected 'Can not delete a predefined deployment configuration', got 'The deployment configuration CodeDeployDefault.OneAtATime is a predefined deployment configuration and cannot be deleted'.
5. codedeploy-on-premises-instance-registration-and-tagging#task#4 — RegisterOnPremisesInstance: both HTTP 400; $.message value_mismatch — expected 'The on-premises instance could not be registered because both an IAM session ARN and an IAM user ARN were included in the request. Include only one IAM ARN in the request, and then try again.', got 'Both an user ARN and an IAM session ARN were included in the request. Use only one ARN type'.

Smithy (services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/com-amazonaws-codedeploy-codedeploy-20141006.json): TagResource(ResourceArn required Arn length 1..1011, Tags required TagList) -> TagResourceOutput {} with errors TagRequiredException / InvalidArnException / InvalidTagsToAddException / ArnNotSupportedException / ResourceArnRequiredException (400); CreateDeploymentConfig models InvalidMinimumHealthyHostValueException (400); DeleteDeploymentConfig models InvalidOperationException (400); RegisterOnPremisesInstance models MultipleIamArnsProvidedException and IamArnRequiredException (400). The model documents no message text for these errors, so the expected texts above are AWS's recorded wording for those error classes.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1; the five clusters reproduce.
2. Cause 1: CreateApplication(A) / CreateDeploymentConfig(C), then replay TagResource for the resource ARN exactly as fixture codedeploy-resource-tagging-via-arn-for-application-and-deployment-config task#8 sends it — AWS answers 200 with {}, the emulator answers 400 TagRequiredException.
3. Cause 2: TagResource(ResourceArn='not-a-valid-arn', Tags=[{'Key':'A','Value':'B'}]) and compare the message to 'Invalid ARN: not-a-valid-arn' (task#9 of the same fixture).
4. Cause 3: CreateDeploymentConfig(deploymentConfigName=<new>, minimumHealthyHosts={'type':'FLEET_PERCENT','value':-5}) and compare the message (fixture codedeploy-custom-deployment-config-lifecycle, task#9).
5. Cause 4: DeleteDeploymentConfig(deploymentConfigName='CodeDeployDefault.OneAtATime') and compare the message (same fixture, task#12).
6. Cause 5: RegisterOnPremisesInstance(instanceName=<new>, iamUserArn=<user arn>, iamSessionArn=<session arn>) and compare the message (fixture codedeploy-on-premises-instance-registration-and-tagging, task#4).
