Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__elbv2__s3.cfn-elbv2-truststorerevocation-resource-lifecycle cloudformation__elbv2__s3.cfn-truststorerevocation-missing-s3-object-rejected
Issue-Key: cloudformation__elbv2__s3|CreateStack|ValidationError
Issue-Commit: 6bab9504f4457a639e729a9dc2b08c46f536304f

# cloudformation__elbv2__s3: CloudFormation rejects AWS::ElasticLoadBalancingV2::TrustStoreRevocation as an unrecognized resource type

> _Note: this issue was identified against the code base at commit `6bab9504f4457a639e729a9dc2b08c46f536304f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause (Cause 1 of 1). Related open filings that do NOT carry these replay keys: 20261023T205437Z-elbv2-ten-control-plane-operation-families-answer-501.md covers the ELBv2 trust-store control plane answering 501, but these two checks never reach ELBv2 at all — CreateStack refuses the template, and the suite marks neither id OWNED.

Cause 1 of 1: CreateStack refuses a template containing AWS::ElasticLoadBalancingV2::TrustStoreRevocation with ValidationError "Template format error: Unrecognized resource types", so the resource type is not registered with the emulator's CloudFormation.
AWS: Smithy (cloudformation) CreateStack declares no error for a supported resource type; AWS reserves ValidationError "Template format error: Unrecognized resource types: [<Type>]" (HTTP 400) for types CloudFormation does not know. AWS::ElasticLoadBalancingV2::TrustStoreRevocation is a supported CloudFormation resource type, so AWS accepts the template, returns a StackId and provisions the revocation list against the referenced trust store and S3 object (and fails that resource — rolling the stack back — when the S3 object is missing). The fixed text of the message is AWS's; the bracketed type list is taken from the request.
Emulator: CreateStack answers HTTP 400 ValidationError "Template format error: Unrecognized resource types: [AWS::ElasticLoadBalancingV2::TrustStoreRevocation]" for both the positive lifecycle template and the negative missing-S3-object template.
Why: a client cannot model mTLS revocation lists in a stack at all. Worse, the refusal is indistinguishable from the one AWS gives for a genuinely invalid template, so a correct template reads as a user error; and the negative case — a revocation pointing at a nonexistent S3 object, which must fail the resource and roll back — can never be distinguished from this blanket template rejection.
Fix: the CloudFormation resource-type registry and its ELBv2 resource providers must register AWS::ElasticLoadBalancingV2::TrustStoreRevocation and create/delete it against the ELBv2 trust-store state, failing the resource when the referenced S3 CRL object does not exist.
Traces: cloudformation__elbv2__s3.cfn-elbv2-truststorerevocation-resource-lifecycle; cloudformation__elbv2__s3.cfn-truststorerevocation-missing-s3-object-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1: cloudformation__elbv2__s3 0/2 (0%), 2 failed; neither OWNED.

[error] cloudformation__elbv2__s3.cfn-elbv2-truststorerevocation-resource-lifecycle
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::ElasticLoadBalancingV2::TrustStoreRevocation]

[error] cloudformation__elbv2__s3.cfn-truststorerevocation-missing-s3-object-rejected
  identical ValidationError on CreateStack

Expected: CreateStack returns a StackId; the positive stack reaches CREATE_COMPLETE with the revocation list attached to its trust store, and the negative stack ends ROLLBACK_COMPLETE with the revocation resource failed for the missing S3 object. Actual: both templates are refused with HTTP 400 ValidationError for an unrecognized resource type.

Smithy consulted: cloudformation CreateStack (input TemplateBody, output StackId).

## Steps to Reproduce
1. Clone the repo and replay the behavioral suite; both ids in trace_ids reproduce.
2. Create an S3 bucket and put a CRL object in it.
3. cloudformation.create_stack with a template declaring AWS::ElasticLoadBalancingV2::TrustStore and AWS::ElasticLoadBalancingV2::TrustStoreRevocation referencing that bucket/key. Expect a StackId and CREATE_COMPLETE; the emulator answers 400 ValidationError "Template format error: Unrecognized resource types: [AWS::ElasticLoadBalancingV2::TrustStoreRevocation]".
4. Repeat with the revocation pointing at a key that does not exist. Expect the stack to roll back with that resource failed; the emulator refuses the template with the same ValidationError.
5. Re-replay the ids in trace_ids to confirm the fix.
