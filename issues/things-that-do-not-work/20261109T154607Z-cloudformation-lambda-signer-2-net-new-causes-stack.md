Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__signer.deploy-lambda-codesigningconfig-round-trip cloudformation__lambda__signer.deploy-lambda-codesigningconfig-signingprofilearn-malformed-rejected
Issue-Key: cloudformation__lambda__signer.deploy-lambda-codesigningconfig-round-trip|stack_outputs#9|KeyError|return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: cloudformation__lambda__signer.deploy-lambda-codesigningconfig-signingprofilearn-malformed-rejected|run#11|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# cloudformation__lambda__signer: 2 net-new causes — stack Outputs for Signer/Lambda code-signing resources come back without OutputValue, and a CodeSigningConfig whose SigningProfileVersionArn is not an ARN reaches CREATE_COMPLETE instead of rolling back

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
Two independent net-new root causes on CloudFormation stacks that deploy AWS::Signer::SigningProfile + AWS::Lambda::CodeSigningConfig. BOTH must be closed before this issue is submitted.

Cause 1 of 2: DescribeStacks returns Outputs entries with no OutputValue, so Ref/Fn::GetAtt results of these resources are unobtainable.
AWS: DescribeStacks returns, for each declared template Output, an Output structure carrying both OutputKey and OutputValue (Smithy, services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json: com.amazonaws.cloudformation#Output has members OutputKey, OutputValue, Description, ExportName). The template under test declares three Outputs — {"Fn::GetAtt": ["Profile", "ProfileVersionArn"]}, {"Ref": "Csc"} and {"Fn::GetAtt": ["Csc", "CodeSigningConfigId"]} — and AWS resolves each to the resource's real identifier: the signing profile's profileVersionArn as Signer reports it, the code signing config's CodeSigningConfigArn, and its CodeSigningConfigId. The status is 200 on a CREATE_COMPLETE stack; there is no error code involved, the defect is a missing response member.
Emulator: the stack reaches CREATE_COMPLETE and DescribeStacks lists the Outputs, but at least one entry has no OutputValue member at all, so reading it raises KeyError: 'OutputValue' in the check's stack_outputs helper ({o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}).
Why: a client cannot learn the physical identifier of anything the stack created — the whole purpose of an Output. Every consumer (a nested stack's parameter, an Fn::ImportValue export, a deploy script reading the config ARN to attach to a function) reads None or faults, and the round trip through Lambda (list_code_signing_configs / get_code_signing_config by the ARN the stack reported) cannot be performed at all.
Fix: CloudFormation's Outputs resolution and the DescribeStacks serializer — Ref and Fn::GetAtt against AWS::Signer::SigningProfile (ProfileVersionArn) and AWS::Lambda::CodeSigningConfig (Ref = CodeSigningConfigArn, CodeSigningConfigId) must resolve to the values the owning service holds for the created resource, and every Output row must carry OutputValue. Note the same missing-OutputValue family is already filed for other resource sets (20261109T141301Z-cloudformation-s3tables-cloudformation-stack-outputs-for-aws-s3tables-resources-are.md, 20261109T143402Z-cloudformation-mq-stack-outputs-for-aws-amazonmq-configuration.md, 20261109T144605Z-cloudformation-elasticbeanstalk-stack-outputs-omit-outputvalue-a-stack.md); none of them covers the Signer/Lambda code-signing resource types, which is what this cause is grounded in.
Failing check ids: cloudformation__lambda__signer.deploy-lambda-codesigningconfig-round-trip.

Cause 2 of 2: an AWS::Lambda::CodeSigningConfig whose AllowedPublishers.SigningProfileVersionArns[0] is not an ARN is provisioned successfully instead of failing resource-schema validation and rolling the stack back.
AWS: the resource handler refuses the create, the stack ends ROLLBACK_COMPLETE, and the Csc resource's CREATE_FAILED event carries ResourceStatusReason 'Resource handler returned message: "Model validation failed (#/AllowedPublishers/SigningProfileVersionArns/0: failed validation constraint for keyword [pattern])" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'. Fixed text: the handler-reason envelope 'Resource handler returned message: "…" (RequestToken: …, HandlerErrorCode: InvalidRequest)' and the message 'Model validation failed (#/AllowedPublishers/SigningProfileVersionArns/0: failed validation constraint for keyword [pattern])'; the variable parts are the RequestToken (per-run) and the JSON pointer naming the offending property instance. Recorded behavior for the rollback's visible effect: Lambda lists NO code signing config under the stack's description. The resource type's schema pattern on SigningProfileVersionArns members is what rejects a non-ARN value; the Lambda API itself models the same refusal.
Emulator: the stack with the malformed publisher ARN ends CREATE_COMPLETE ('AssertionError: rejected stack ended \'CREATE_COMPLETE\''), so no CREATE_FAILED row and no handler reason exist, and a code signing config with a non-ARN allowed publisher is left behind.
Why: the emulator validates a CloudFormation deployment AWS refuses. A caller whose template or parameter wiring produced a non-ARN publisher gets a green stack and a code-signing config that can never match a real signing profile, so the Enforce policy it deployed silently protects nothing; the negative path a client tests (template rejected, nothing created) is unreachable.
Fix: the AWS::Lambda::CodeSigningConfig resource handler in CloudFormation — run the resource type's property-schema validation (the SigningProfileVersionArns member pattern) before provisioning, fail the resource with HandlerErrorCode InvalidRequest and the model-validation message above, and let the stack roll back so no config is created.
Failing check ids: cloudformation__lambda__signer.deploy-lambda-codesigningconfig-signingprofilearn-malformed-rejected.

## Evidence
Behavioral suite replay against the emulator built from this checkout: service cloudformation__lambda__signer 0/2 (0%), 2 failed.

Cause 1 — [error] cloudformation__lambda__signer.deploy-lambda-codesigningconfig-round-trip. The stack (AWS::Signer::SigningProfile 'Profile' on PlatformId AWSLambda-SHA384-ECDSA + AWS::Lambda::CodeSigningConfig 'Csc' with AllowedPublishers.SigningProfileVersionArns=[{Fn::GetAtt:[Profile,ProfileVersionArn]}] and CodeSigningPolicies.UntrustedArtifactOnDeployment=Enforce; Outputs ProfileVersionArnOut, CscArnOut, CscIdOut) reached CREATE_COMPLETE, then:
  File check_cloudformation__lambda__signer_deploy_lambda_codesigningconfig_round_trip.py, line 205, in run -> out = stack_outputs(cfn, stack_id)
  line 111: return {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
  KeyError: 'OutputValue'
Expected instead: HTTP 200 DescribeStacks whose Stacks[0].Outputs rows each carry OutputKey AND OutputValue, with CscArnOut/CscIdOut equal to the (CodeSigningConfigArn, CodeSigningConfigId) Lambda lists for the stack's description and ProfileVersionArnOut equal to signer.get_signing_profile(profileName=...)['profileVersionArn'].

Cause 2 — [mismatch] cloudformation__lambda__signer.deploy-lambda-codesigningconfig-signingprofilearn-malformed-rejected. The same template with publisher_arn='not-an-arn-<profile name>' substituted for the Fn::GetAtt:
  File run.py line 354 -> assert status == "ROLLBACK_COMPLETE", f"rejected stack ended {status!r}"
  AssertionError: rejected stack ended 'CREATE_COMPLETE'
Expected instead: wait_stack -> 'ROLLBACK_COMPLETE', the Csc resource carrying exactly one CREATE_FAILED event whose ResourceStatusReason parses as ('Model validation failed (#/AllowedPublishers/SigningProfileVersionArns/0: failed validation constraint for keyword [pattern])', 'InvalidRequest') under the check's handler-reason regex 'Resource handler returned message: "(?P<message>.*)" \\(RequestToken: [0-9a-f-]+, HandlerErrorCode: (?P<code>[A-Za-z]+)\\)', and lambda.list_code_signing_configs listing nothing under the stack's description. The check's positive control (the identical template with the valid Fn::GetAtt publisher) is asserted to reach CREATE_COMPLETE, so the requirement is selective refusal, not refusing the type.

Model facts used: smithy_query(query="shape", shape="Output") on services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json — com.amazonaws.cloudformation#Output members OutputKey, OutputValue, Description, ExportName. The resource-type property schema and the model-validation message of cause 2 are recorded behavior only (CloudFormation resource schemas are not part of the service's Smithy model).

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from this branch.
2. Replay the simparity-behavioral-aws-v1 suite and filter to service cloudformation__lambda__signer.
3. Cause 1: run cloudformation__lambda__signer.deploy-lambda-codesigningconfig-round-trip — CreateStack with an AWS::Signer::SigningProfile (PlatformId AWSLambda-SHA384-ECDSA) and an AWS::Lambda::CodeSigningConfig whose AllowedPublishers.SigningProfileVersionArns is [{Fn::GetAtt: [Profile, ProfileVersionArn]}], Outputs ProfileVersionArnOut/CscArnOut/CscIdOut; wait for CREATE_COMPLETE and call DescribeStacks(StackName=<StackId>); observe Outputs rows without an OutputValue member instead of resolved identifiers.
4. Cause 2: run cloudformation__lambda__signer.deploy-lambda-codesigningconfig-signingprofilearn-malformed-rejected — CreateStack with the same template but AllowedPublishers.SigningProfileVersionArns=['not-an-arn-<profile name>']; observe the stack settling CREATE_COMPLETE instead of ROLLBACK_COMPLETE, no CREATE_FAILED row for Csc, and lambda.list_code_signing_configs listing a config for the stack's description. Then re-run the same template with the valid Fn::GetAtt publisher and confirm it still reaches CREATE_COMPLETE.
