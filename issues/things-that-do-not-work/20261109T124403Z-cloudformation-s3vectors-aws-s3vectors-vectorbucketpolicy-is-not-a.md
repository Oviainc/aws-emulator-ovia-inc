Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__s3vectors.deploy-s3vectors-vector-bucket-policy-provisioned-and-removed cloudformation__s3vectors.cfn-s3vectors-vector-bucket-policy-invalid-action-namespace-rejected
Issue-Key: cloudformation__s3vectors|CreateStack|ValidationError
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# cloudformation__s3vectors: AWS::S3Vectors::VectorBucketPolicy is not a registered resource type — CreateStack refuses every vector-bucket-policy template

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's resource-type registry in the emulator does not know AWS::S3Vectors::VectorBucketPolicy, so any template that declares one is rejected at template-validation time and no vector-bucket policy can ever be provisioned, updated away, or rolled back by a stack.

AWS: CloudFormation supports the AWS::S3Vectors::VectorBucketPolicy resource type alongside AWS::S3Vectors::VectorBucket (which this emulator does accept). CreateStack with a template declaring it returns 200 and the stack proceeds; the resource's handler calls S3 Vectors PutVectorBucketPolicy (Smithy: PutVectorBucketPolicy, input vectorBucketName @length 3..63 / vectorBucketArn pattern ^arn:aws[-a-z0-9]*:s3vectors:[a-z0-9-]+:[0-9]{12}:bucket/..., policy required; GetVectorBucketPolicy returns member `policy`). Its Ref / physical resource id is the vector bucket's ARN (its primary identifier), the stored policy reads back through GetVectorBucketPolicy with the template's intrinsic functions resolved (Fn::Sub principal, Fn::GetAtt VectorBucketArn as Resource), removing just the policy resource in an UpdateStack leaves the bucket untouched and GetVectorBucketPolicy then refuses with NotFoundException, and deleting the stack removes the bucket. When the policy document names an action outside the s3vectors namespace (e.g. "s3:GetObject"), S3 Vectors refuses the PutVectorBucketPolicy call and CloudFormation records the resource CREATE_FAILED and rolls the stack back to ROLLBACK_COMPLETE with the registry-handler reason template `Resource handler returned message: "The provided policy is malformed or invalid (Service: S3Vectors, Status Code: 400, Request ID: <request id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — the fixed text is "The provided policy is malformed or invalid", HTTP status 400, service name S3Vectors and HandlerErrorCode InvalidRequest; the request id, request token and attempt count are per-run values, not part of the requirement.

Emulator: CreateStack is refused outright: ClientError (ValidationError) when calling the CreateStack operation: `Template format error: Unrecognized resource types: [AWS::S3Vectors::VectorBucketPolicy]`. No stack is created, so neither the happy-path lifecycle nor the malformed-policy rollback path is reachable at all.

Why: a client cannot express an S3 Vectors bucket policy in infrastructure-as-code. Every CDK/CloudFormation template that attaches a resource policy to a vector bucket fails at submit time with a template-format error, which a client reads as "my template is wrong" rather than "the service does not support this". Stacks that mix the bucket and its policy cannot be deployed, updated or rolled back, and the policy-malformed rejection contract (ROLLBACK_COMPLETE with HandlerErrorCode InvalidRequest and both resources torn down) is never exercised.

Fix: CloudFormation's resource-type registry/template validation (where AWS::S3Vectors::VectorBucket is already registered) must also register AWS::S3Vectors::VectorBucketPolicy, plus a provider/handler for it that: on Create/Update calls S3 Vectors PutVectorBucketPolicy with the resolved Policy document and the VectorBucketArn property, on Delete calls DeleteVectorBucketPolicy, reports the bucket ARN as the physical resource id (so Ref yields the ARN), and surfaces the S3 Vectors 400 refusal as a CREATE_FAILED ResourceStatusReason in the registry-handler format with HandlerErrorCode InvalidRequest so the stack rolls back.

Traces: cloudformation__s3vectors.deploy-s3vectors-vector-bucket-policy-provisioned-and-removed (happy-path lifecycle: create, Ref=bucket ARN, policy read-back, policy-only update removal, stack delete) and cloudformation__s3vectors.cfn-s3vectors-vector-bucket-policy-invalid-action-namespace-rejected (rejection path: s3:GetObject action rolls the stack back). Both die on the same CreateStack ValidationError.

## Evidence
Both failing checks error identically at the first CreateStack:

  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::S3Vectors::VectorBucketPolicy]

Template submitted (abridged):
  Resources:
    VecBucket:  Type AWS::S3Vectors::VectorBucket, Properties {VectorBucketName: <name>}      <- accepted type
    VecPolicy:  Type AWS::S3Vectors::VectorBucketPolicy, Properties {VectorBucketArn: {Ref: VecBucket}, Policy: {...}}  <- refused type

Expected (AWS):
  - CreateStack accepted; stack reaches CREATE_COMPLETE.
  - Ref of the VectorBucketPolicy resource == the vector bucket's ARN (same value as Ref of the bucket / GetVectorBucket's vectorBucketArn).
  - GetVectorBucketPolicy(vectorBucketName=<name>) returns the template's policy with intrinsics resolved (Principal arn:<partition>:iam::<account>:root, Resource the bucket ARN).
  - UpdateStack to a bucket-only template -> UPDATE_COMPLETE, bucket identity (name, ARN, creationTime) unchanged, GetVectorBucketPolicy then NotFoundException.
  - DeleteStack -> GetVectorBucket refuses with NotFoundException "The specified vector bucket could not be found".
  - With Action "s3:GetObject": stack reaches ROLLBACK_COMPLETE, the VecPolicy CREATE_FAILED reason is `Resource handler returned message: "The provided policy is malformed or invalid (Service: S3Vectors, Status Code: 400, Request ID: <id>) (SDK Attempt Count: <n>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`, and both AWS::S3Vectors::VectorBucket and AWS::S3Vectors::VectorBucketPolicy end DELETE_COMPLETE.

Actual (emulator): CreateStack returns HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::S3Vectors::VectorBucketPolicy]"; no stack, no resources, none of the above observable.

Note: no open issue covers this type. The closest open issues cover other unregistered types (e.g. 20261109T113214Z-cloudformation-mediatailor-..., 20261109T113448Z-cloudformation-network-security-manager-...) and the S3Vectors/S3Tables CFN physical-id defects (20261102T101202Z-cloudformation-s3-s3tables-s3vectors-2-net-new-root.md); none registers AWS::S3Vectors::VectorBucketPolicy.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. cloudformation.CreateStack with TemplateBody:
   {"AWSTemplateFormatVersion":"2010-09-09","Resources":{"VecBucket":{"Type":"AWS::S3Vectors::VectorBucket","Properties":{"VectorBucketName":"<lowercase-unique-name>"}},"VecPolicy":{"Type":"AWS::S3Vectors::VectorBucketPolicy","Properties":{"VectorBucketArn":{"Ref":"VecBucket"},"Policy":{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":{"AWS":{"Fn::Sub":"arn:${AWS::Partition}:iam::${AWS::AccountId}:root"}},"Action":"s3vectors:ListVectors","Resource":{"Fn::GetAtt":["VecBucket","VectorBucketArn"]}}]}}}},"Outputs":{"BucketRef":{"Value":{"Ref":"VecBucket"}},"PolicyRef":{"Value":{"Ref":"VecPolicy"}}}}
   -> observe ValidationError "Template format error: Unrecognized resource types: [AWS::S3Vectors::VectorBucketPolicy]" instead of a StackId.
3. Repeat with the statement's "Action" set to "s3:GetObject" -> same CreateStack refusal, so the expected ROLLBACK_COMPLETE / HandlerErrorCode InvalidRequest path is unreachable.
4. Replay: cloudformation__s3vectors.deploy-s3vectors-vector-bucket-policy-provisioned-and-removed and cloudformation__s3vectors.cfn-s3vectors-vector-bucket-policy-invalid-action-namespace-rejected.
