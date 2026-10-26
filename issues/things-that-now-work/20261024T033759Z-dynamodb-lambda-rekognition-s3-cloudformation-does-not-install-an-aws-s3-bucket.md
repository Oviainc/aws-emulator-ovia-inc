Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle
Issue-Key: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle|phase#7|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: b7068a3aaca27128a9512527e96d46285251fa2e

# CloudFormation does not install an AWS::S3::Bucket NotificationConfiguration: the deployed bucket reports no LambdaFunctionConfigurations

> _Note: this issue was identified against the code base at commit `b7068a3aaca27128a9512527e96d46285251fa2e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: a CloudFormation stack whose AWS::S3::Bucket declares NotificationConfiguration.LambdaConfigurations reaches CREATE_COMPLETE, resolves its Outputs and creates the bucket, but the notification configuration is never installed into S3 bucket state, so the event-driven leg of the stack is dead. Failing check: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle (phase 1/6 'deploy').

AWS: an AWS::S3::Bucket resource's NotificationConfiguration.LambdaConfigurations are applied to the bucket as part of resource creation (equivalent to PutBucketNotificationConfiguration), and a subsequent s3:GetBucketNotificationConfiguration on that bucket returns one LambdaFunctionConfiguration per declared LambdaConfiguration. Smithy (services/service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json): NotificationConfiguration.LambdaFunctionConfigurations -> LambdaFunctionConfigurationList; LambdaFunctionConfiguration.LambdaFunctionArn (required) and .Events (required), with optional Filter -> NotificationConfigurationFilter.Key -> S3KeyFilter.FilterRules (Name/Value, e.g. Name=suffix). The template under replay declares three suffix-filtered configurations (.jpg, .jpeg, .png) all targeting the stack's Lambda, with the bucket DependsOn the AWS::Lambda::Permission that grants s3.amazonaws.com invoke rights. So after CREATE_COMPLETE, GetBucketNotificationConfiguration must report three LambdaFunctionConfigurations whose suffix filter values are .jpeg, .jpg and .png, and uploads matching those suffixes must invoke the function.

Emulator: the stack is created, the bucket exists (GetBucketLocation on it returns the request region and the stack Outputs carry bucket/table/function names), but GetBucketNotificationConfiguration on the stack's bucket returns no LambdaFunctionConfigurations at all — the check's sorted list of installed suffix filter values is `[]` instead of `['.jpeg', '.jpg', '.png']`, and the deploy phase asserts: "CloudFormation must install the three suffix-filtered configurations; the bucket reports []".

Why: every client that models S3-triggered compute in CloudFormation (the canonical upload-to-Lambda reference architecture) gets a stack that claims success while nothing is wired. No object-created event is ever delivered, so the Lambda never runs, nothing is written to DynamoDB, no log group appears, and any later diagnostic read of the notification configuration (the first operation an operator issues when a trigger does not fire) shows an empty configuration — which misdirects the diagnosis towards the template rather than the deploy. It also means `destroy` releases a configuration that was never installed, so the notification state is untestable end to end.

Fix: the CloudFormation resource provider for AWS::S3::Bucket — the property path that translates NotificationConfiguration.LambdaConfigurations (Event/Function/Filter.S3Key.Rules) into the S3 bucket's stored notification configuration, i.e. the same state GetBucketNotificationConfiguration/PutBucketNotificationConfiguration read and write. The Fn::GetAtt-resolved function ARN must be stored as LambdaFunctionArn, the single Event string expanded into the Events list, and the S3Key suffix/prefix rules preserved as FilterRules. Related but not covering: open issue 20261023T000000Z-cloudformation-stacks-report-create-complete-but-provision-no-resources.md (stacks reporting CREATE_COMPLETE while provisioning nothing) — here the bucket IS created and addressable, only the notification sub-resource is dropped.

Traces: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle, phase 1/6 'deploy'.

## Evidence
Check: dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle (verdict: mismatch).

AssertionError: [phase 1/6 deploy] CloudFormation must install the three suffix-filtered configurations; the bucket reports []

Expected (real AWS): after the stack reaches CREATE_COMPLETE,
  s3.get_bucket_notification_configuration(Bucket=<stack bucket>)
returns LambdaFunctionConfigurations with three entries; sorted(c['Filter']['Key']['FilterRules'][0]['Value']) == ['.jpeg', '.jpg', '.png'], each with LambdaFunctionArn = the stack's handler ARN and Events = ['s3:ObjectCreated:*'].

Actual (emulator): the same call returns no LambdaFunctionConfigurations (empty/absent list) -> sorted suffix values == [].

Earlier assertions in the same phase passed, so this is not a stack-creation failure: the stack's Outputs resolved (bucket, table and function names all non-empty) and s3.get_bucket_location on the bucket returned the request region.

Template resource under replay (carved CDK RekognitionS3TriggerStack), abbreviated:
  "ImageBucket": {"Type": "AWS::S3::Bucket", "DependsOn": ["InvokePermission"], "Properties": {
    "BucketName": {"Ref": "BucketName"},
    "NotificationConfiguration": {"LambdaConfigurations": [
      {"Event": "s3:ObjectCreated:*", "Function": {"Fn::GetAtt": ["Handler", "Arn"]},
       "Filter": {"S3Key": {"Rules": [{"Name": "suffix", "Value": ".jpg"}]}}},
      {... ".jpeg" ...}, {... ".png" ...}]}}}

## Steps to Reproduce
1. Create a CloudFormation stack from a template containing an AWS::S3::Bucket whose Properties include a NotificationConfiguration with three LambdaConfigurations, each Event="s3:ObjectCreated:*", Function={"Fn::GetAtt": ["Handler", "Arn"]} and Filter.S3Key.Rules=[{Name: suffix, Value: ".jpg"|".jpeg"|".png"}], plus the AWS::Lambda::Function, AWS::IAM::Role and AWS::Lambda::Permission (Principal s3.amazonaws.com) the bucket DependsOn.
2. Wait for the stack to reach CREATE_COMPLETE and read the BucketName output.
3. Call s3 GetBucketNotificationConfiguration on that bucket.
4. Observe: no LambdaFunctionConfigurations are returned. Expected: three, with suffix FilterRules .jpeg/.jpg/.png and LambdaFunctionArn equal to the stack's handler ARN.
5. Equivalent replay: run behavioral check dynamodb__lambda__rekognition__s3.e2e-diagnose-rekognition-reupload-stale-lifecycle; it fails in phase 1/6 'deploy'.
