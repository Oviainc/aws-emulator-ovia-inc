Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.deploy-acm-certificate-stack-behaves acm.deploy-stack-deletion-removes-certificate acm.deploy-acme-endpoint-via-cloudformation-behaves acm.deploy-certificate-with-tags-via-cloudformation-tags-queryable acm.delete-acme-external-account-binding-missing-idempotent-noop acm.get-eab-credentials-cross-account-denied
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm.deploy-acm-certificate-stack-behaves|_poll_until#9|Timeout|time.sleep(interval_s)
Issue-Key: acm.deploy-stack-deletion-removes-certificate|_poll_until#7|Timeout|time.sleep(interval_s)
Issue-Key: acm.deploy-acme-endpoint-via-cloudformation-behaves|run#10|KeyError|o["OutputKey"]: o["OutputValue"]
Issue-Key: acm-pca|IssueCertificate|InvalidStateException
Issue-Key: iam|PutUserPolicy|NoSuchEntity
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# ACM deploy and cross-service prerequisites broken: DescribeStackResource 501, unresolved stack Outputs, PCA issuance refused, caller identity absent from IAM (4 causes)

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStackResource

## Description
Four distinct root causes, all surfaced by ACM behavioral checks (service token 'acm') but each sitting in a prerequisite the ACM flows depend on. None is covered by an open issue (the filed CloudFormation stub issue names DescribeStackResources, the PLURAL operation, not these).

Cause 1 of 4: DescribeStackResource (singular) is an unimplemented scaffold stub, so a CloudFormation-deployed ACM certificate can never be located.
AWS: CloudFormation models DescribeStackResource(StackName, LogicalResourceId) -> StackResourceDetail{PhysicalResourceId, ResourceStatus, ...}; for an AWS::CertificateManager::Certificate resource the PhysicalResourceId is the CertificateArn and is assigned as soon as the resource is created (before CREATE_COMPLETE).
Emulator: the handler returns Err(crate::scaffolded::Scaffolded::not_implemented("DescribeStackResource")) — HTTP 501 'NotImplemented' — so the checks' poll for a PhysicalResourceId never yields a value and both checks hit their 60s deadline.
Why: any deploy-time flow that resolves a stack resource to its physical id (the standard way to find what a template created) is impossible; a client sees a 501 server fault instead of the resource detail, and stack-owned ACM certificates are unreachable.
Fix: the operation's handler — services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stack_resource.rs (file carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker).
Traces: acm.deploy-acm-certificate-stack-behaves, acm.deploy-stack-deletion-removes-certificate.

Cause 2 of 4: stack Outputs are returned without OutputValue (Fn::GetAtt is not resolved).
AWS: DescribeStacks returns Stacks[0].Outputs as a list of Output{OutputKey, OutputValue}; an output declared as {"Value": {"Fn::GetAtt": ["Endpoint", "AcmeEndpointArn"]}} is resolved to the created resource's attribute by the time the stack reports CREATE_COMPLETE.
Emulator: the stack reaches CREATE_COMPLETE (the stack_create_complete waiter returns), but the Outputs entry has an OutputKey and no OutputValue (KeyError: 'OutputValue').
Why: a client that reads a stack output — the normal hand-off from a template to the rest of a deployment — gets a key with no value, so the deployed resource's identifier is unobtainable even though CloudFormation claims success.
Fix: output/intrinsic-function resolution in the CloudFormation stack engine (Fn::GetAtt against created resource attributes) and the DescribeStacks Outputs serializer.
Traces: acm.deploy-acme-endpoint-via-cloudformation-behaves.

Cause 3 of 4: ACM-PCA refuses to issue from a CA the emulator just created and activated.
AWS: ACM Private CA models IssueCertificate against a CA in state ACTIVE; InvalidStateException ('The certificate authority is not in a valid state for issuing certificates') is reserved for a CA that is not ACTIVE (CREATING, PENDING_CERTIFICATE, DISABLED, DELETED). After CreateCertificateAuthority + GetCertificateAuthorityCsr + self-signed import, the CA is ACTIVE and IssueCertificate returns 200 with a CertificateArn.
Emulator: the check's private-CA bring-up completes, but IssueCertificate answers 400 InvalidStateException 'The certificate authority is not in a valid state for issuing certificates'.
Why: private-CA issuance is unusable, which also removes the only synchronous path to an ISSUED certificate (public DNS validation never completes — see the DNS-validation cause filed separately), so every private-CA and CFN-with-private-CA flow is blocked.
Fix: the certificate-authority state machine in the ACM-PCA service crate (the CA must become ACTIVE once its signed certificate is imported) and IssueCertificate's state gate.
Traces: acm.deploy-certificate-with-tags-via-cloudformation-tags-queryable.

Cause 4 of 4: the principal STS attributes to the caller does not exist in IAM, so a caller cannot manage its own policies.
AWS: STS GetCallerIdentity returns the ARN of the calling principal, and that principal exists in IAM: for arn:aws:iam::<acct>:user/<name>, iam PutUserPolicy(UserName=<name>, ...) succeeds (200); NoSuchEntityException is reserved for a user that genuinely does not exist.
Emulator: GetCallerIdentity reports a user ARN whose user name IAM does not know — PutUserPolicy answers 404/NoSuchEntity 'The user with name emulator cannot be found.' So any check or client that reads its own identity and then grants itself a scoped inline policy (here iam:PassRole for acm-acme.amazonaws.com, the prerequisite for CreateAcmeExternalAccountBinding) cannot proceed.
Why: STS and IAM disagree about who the caller is. Self-service permission flows (grant PassRole, attach an inline policy, then call the service) fail with a not-found that is impossible on AWS, and the EAB flows that depend on them never run.
Fix: the IAM/STS identity bootstrap — materialize an IAM user (or role) record for the principal GetCallerIdentity attributes to the emulator's credentials, so PutUserPolicy/GetUser resolve it.
Traces: acm.delete-acme-external-account-binding-missing-idempotent-noop, acm.get-eab-credentials-cross-account-denied.

## Evidence
Cause 1: acm.deploy-acm-certificate-stack-behaves and acm.deploy-stack-deletion-removes-certificate — 'Check timed out after 60.0s'; both poll _physical_id(), which calls cfn.describe_stack_resource(StackName=..., LogicalResourceId='Cert') and swallows the ClientError, returning None forever. services/service-com-amazonaws-cloudformation-cloudformation/src/ops/describe_stack_resource.rs is marked '//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented'.
Cause 2: acm.deploy-acme-endpoint-via-cloudformation-behaves — KeyError: 'OutputValue' at `{o["OutputKey"]: o["OutputValue"] for o in cfn.describe_stacks(StackName=...)["Stacks"][0].get("Outputs", [])}` for the template output {"EndpointArn": {"Value": {"Fn::GetAtt": ["Endpoint", "AcmeEndpointArn"]}}}, after stack_create_complete returned.
Cause 3: acm.deploy-certificate-with-tags-via-cloudformation-tags-queryable — botocore.errorfactory.InvalidStateException: An error occurred (InvalidStateException) when calling the IssueCertificate operation: The certificate authority is not in a valid state for issuing certificates.
Cause 4: acm.delete-acme-external-account-binding-missing-idempotent-noop and acm.get-eab-credentials-cross-account-denied — botocore.errorfactory.NoSuchEntityException: An error occurred (NoSuchEntity) when calling the PutUserPolicy operation: The user with name emulator cannot be found. The user name is taken from sts.get_caller_identity()['Arn'] (':user/' branch of the checks' _ensure_pass_role_permission helper).

## Steps to Reproduce
1. Boot the emulator and replay the ACM behavioral suite.
2. Cause 1: cfn.create_stack with an AWS::CertificateManager::Certificate resource, then cfn.describe_stack_resource(StackName=<stack>, LogicalResourceId='Cert') — HTTP 501 NotImplemented, so no PhysicalResourceId is ever available.
3. Cause 2: deploy a template whose Outputs use Fn::GetAtt on a created resource, wait for CREATE_COMPLETE, then cfn.describe_stacks(StackName=<stack>)['Stacks'][0]['Outputs'] — entries carry OutputKey but no OutputValue.
4. Cause 3: create and activate an ACM-PCA private CA, then acm-pca IssueCertificate against it — 400 InvalidStateException.
5. Cause 4: arn = sts.get_caller_identity()['Arn']; iam.put_user_policy(UserName=arn.split(':user/')[1], PolicyName='x', PolicyDocument=<any>) — NoSuchEntity 'The user with name emulator cannot be found.'
6. Re-replay the six checks in trace_ids.
