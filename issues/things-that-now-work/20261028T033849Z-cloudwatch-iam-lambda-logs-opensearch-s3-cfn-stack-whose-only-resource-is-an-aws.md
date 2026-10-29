Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow
Issue-Key: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow|_deploy_cfn#10|AssertionError|raise AssertionError(f"Stack {name} ended in {stack['StackStatus']}")
Issue-Commit: c7f7d31100e47fd281228aa100b2a51711aa0f4d

# CFN stack whose only resource is an AWS::OpenSearchService::Domain ends in ROLLBACK_COMPLETE instead of CREATE_COMPLETE, so the search-indexing pipeline workflow never gets a domain

> _Note: this issue was identified against the code base at commit `c7f7d31100e47fd281228aa100b2a51711aa0f4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One root cause.

Cause 1 of 1: A CreateStack of a valid single-resource template (AWS::OpenSearchService::Domain with ClusterConfig, EBSOptions, NodeToNodeEncryptionOptions, EncryptionAtRestOptions, DomainEndpointOptions.EnforceHTTPS and an AccessPolicies document delegating to the account root) is driven to ROLLBACK_COMPLETE by the emulator, so the stack never reaches CREATE_COMPLETE and the whole deployment-ordered workflow is stopped at its first (hard) leg.

AWS: CreateStack for a template whose resources are all valid and whose parameters satisfy the resource's own constraints provisions the resources and the stack settles in CREATE_COMPLETE; DescribeStacks then reports Stacks[0].StackStatus = CREATE_COMPLETE and the template's Outputs with resolved values. ROLLBACK_COMPLETE is reached only after a resource-level create failure, and in that case the stack carries a failed resource event whose ResourceStatusReason states the refusal. Smithy: cloudformation#StackStatus enum = {CREATE_IN_PROGRESS, CREATE_FAILED, CREATE_COMPLETE, ROLLBACK_IN_PROGRESS, ROLLBACK_FAILED, ROLLBACK_COMPLETE, ...}; cloudformation#Output = {OutputKey, OutputValue, Description, ExportName}. For the resource itself, opensearch CreateDomain accepts this shape of request (single-node t3.small.search, EBS gp2 10 GiB, encryption at rest with the service-owned key, node-to-node encryption, EnforceHTTPS) and returns a DomainStatus that settles with Processing=false and a DomainEndpoint. The domain name used here is within the modeled 3-28 character bound. Recorded behavior only for the rollback reason: the replay shows the terminal status and no usable resource.

Emulator: the replayed check polls DescribeStacks until a terminal create status and observes ROLLBACK_COMPLETE: `AssertionError: Stack sim2real-xsvc-stk-<id> ended in ROLLBACK_COMPLETE` (the fixed text is "Stack <name> ended in <status>"; <name> is the generated stack name from the request and <status> is the StackStatus the emulator reported, here ROLLBACK_COMPLETE where CREATE_COMPLETE is required). The failure is at the check's first hard leg ("domain"), so the 11-leg workflow records LEGS 0/11 and nothing downstream (S3 notification, Lambda indexing, OpenSearch writes, log group, metric filter, alarm) is exercised at all.

Why: a client that deploys an OpenSearch domain by CloudFormation — the normal way this estate is created — gets a rolled-back, resource-less stack for a template real AWS accepts. There is no way to obtain the domain endpoint or ARN, so every consumer of the stack (and every dependent stack parameterised from its outputs) is dead on arrival, and the client cannot distinguish "my template is wrong" from "the provider refused it".

Note: this is NOT the divergence carried by the open issue 20261024T055421Z-cloudwatch-iam-lambda-logs-opensearch-s3-cfn-deployed-search-indexing-pipeline-fails-immediately-stack.md, which records the same check failing on Outputs entries with no OutputValue after the stack had reached CREATE_COMPLETE. The stack now does not reach CREATE_COMPLETE at all, which is a different cause at the same leg; the open issue above should not be closed on the strength of this one.

Fix: the CloudFormation stack-create orchestration and its AWS::OpenSearchService::Domain resource provider (the resource-type handler that calls opensearch CreateDomain and waits for the domain to settle) — the create must succeed for a template real AWS accepts and leave the stack in CREATE_COMPLETE; if a resource genuinely cannot be created, the stack's resource events must carry a ResourceStatusReason naming the refusal rather than rolling back silently.

Traces: cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow (deployments/search-domain.yaml, leg "domain").

## Evidence
Expected: DescribeStacks(StackName=<stack>) reports Stacks[0].StackStatus == "CREATE_COMPLETE" after CreateStack of deployments/search-domain.yaml, with Outputs DomainEndpoint / DomainArn / DomainNameOut resolved.
Actual: the stack settles in "ROLLBACK_COMPLETE" and the check raises `AssertionError: Stack sim2real-xsvc-stk-554e3e967c20 ended in ROLLBACK_COMPLETE` (raised from _deploy_cfn, the check's first hard leg "domain"). The stack name is per-run; the requirement is the status, not that literal name. No failed-resource reason is surfaced that explains the rollback. Consequence: LEGS 0/11 PASS — the entire S3 -> Lambda -> OpenSearch pipeline, its log group, metric filter and alarm are never exercised.

## Steps to Reproduce
1. Boot the emulator.
2. cloudformation.create_stack with the check's deployments/search-domain.yaml (single AWS::OpenSearchService::Domain; Parameters DomainName=<=28-char lowercase name, EngineVersion=OpenSearch_2.11; Capabilities CAPABILITY_IAM, CAPABILITY_NAMED_IAM).
3. Poll describe_stacks until a terminal create status: the status reported is ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
4. describe_stack_events for the stack: no ResourceStatusReason explains why the AWS::OpenSearchService::Domain create failed.
5. Replay cloudwatch__iam__lambda__logs__opensearch__s3.cfn-search-indexing-pipeline-workflow.
