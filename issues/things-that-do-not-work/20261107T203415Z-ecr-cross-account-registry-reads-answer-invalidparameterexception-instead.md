Issue-Source: trace-sweep-divergence
Issue-Trace: ecr-ecracc-cross-account-registry-id-reads
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ecr-ecracc-cross-account-registry-id-reads
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|DescribeRepositories|4xx->4xx|$.Message
Issue-Key: ecr|DescribeRepositories|4xx->4xx|$.__type
Issue-Key: ecr|GetRepositoryPolicy|4xx->4xx|$.Message
Issue-Key: ecr|GetRepositoryPolicy|4xx->4xx|$.__type
Issue-Key: ecr|BatchGetImage|4xx->4xx|$.Message
Issue-Key: ecr|BatchGetImage|4xx->4xx|$.__type
Issue-Key: ecr|DescribePullThroughCacheRules|4xx->4xx|$.Message
Issue-Key: ecr|DescribePullThroughCacheRules|4xx->4xx|$.__type
Issue-Commit: ae442358c61414b2bbb5a7e8dd80b7eec77accf8

# ECR cross-account registry reads answer InvalidParameterException instead of AccessDeniedException

> _Note: this issue was identified against the code base at commit `ae442358c61414b2bbb5a7e8dd80b7eec77accf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRepositories

## Description
4 causes, one shared fault surfacing on four operations: a read addressed to a registryId that is NOT the caller's own account is refused as a malformed-parameter error instead of an authorization failure. The registryId is well-formed (12 digits) in every case — it simply belongs to another account whose resource policy grants the caller nothing, which is an authorization outcome, not a validation outcome.

Shared AWS behavior: a cross-account ECR read whose target registry has no resource-based policy allowing the caller fails with error code AccessDeniedException at HTTP 400, and that error's body carries the message under the capitalised member `Message` (not the lowercase `message` ECR's modelled InvalidParameterException/ImageNotFoundException use). Message, fixed text exactly: "User: <callerArn> is not authorized to perform: <action> on resource: <resourceArn> because no resource-based policy allows the <action> action". Variable parts = the caller's principal ARN, the ecr:<Operation> action name, and the resource ARN — arn:aws:ecr:<region>:<targetRegistryId>:repository/<repositoryName> for a repository-scoped read, and the literal `*` for a registry-scoped read that names no repository. Smithy grounding: DescribeRepositories/GetRepositoryPolicy/BatchGetImage/DescribePullThroughCacheRules all model InvalidParameterException (httpError 400) for bad input; none of them models the cross-account denial, so AccessDeniedException here is recorded behavior only (the common authorization error AWS returns across ECR), and the request-level detail (which action, which resource) is taken from the request, never from one run.

Shared emulator behavior: all four answer HTTP 400 with $.__type = "InvalidParameterException" and NO `Message` member at all (the denial sentence is absent entirely).

Shared why: a client distinguishes "I sent a bad value, fail fast" from "I am not allowed, retry under another role / fall back to my own registry" purely by the error code. Mapping a denial onto InvalidParameterException makes every cross-account probe look like a client bug; and because the `Message` member is missing, nothing tells the caller which action on which resource was denied, so an operator cannot author the missing resource policy. SDK retry/credential-refresh logic keyed on AccessDeniedException never fires.

Shared fix: the cross-account registryId path shared by ECR's read handlers — resolve a well-formed foreign registryId, evaluate the target registry/repository resource policy, and on no-allow serialize AccessDeniedException with the `Message` member; it belongs in the authorization step ahead of each handler, not in input validation (which is where the refusal is coming from today).

Cause 1 of 4: DescribeRepositories against a foreign registryId. Action ecr:DescribeRepositories, resource the repository ARN in the target registry. Trace: ecr-ecracc-cross-account-registry-id-reads#task#3.
Cause 2 of 4: GetRepositoryPolicy against a foreign registryId. Action ecr:GetRepositoryPolicy, resource the repository ARN in the target registry. Trace: ecr-ecracc-cross-account-registry-id-reads#task#4.
Cause 3 of 4: BatchGetImage against a foreign registryId. Action ecr:BatchGetImage, resource the repository ARN in the target registry. Trace: ecr-ecracc-cross-account-registry-id-reads#task#5.
Cause 4 of 4: DescribePullThroughCacheRules against a foreign registryId. Registry-scoped, so the resource rendered in the message is the literal `*`, not a repository ARN. Trace: ecr-ecracc-cross-account-registry-id-reads#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture ecr-ecracc-cross-account-registry-id-reads. Every step below: expected status 400, actual status 400 — only the error class and body diverge.

task#3 DescribeRepositories:
  $.__type expected 'AccessDeniedException', got 'InvalidParameterException'
  $.Message expected 'User: arn:aws:iam::000000000000:user/emulator is not authorized to perform: ecr:DescribeRepositories on resource: arn:aws:ecr:us-west-2:111111111111:repository/sim2real-probe-ecracc-xacct-6b20d8f1 because no resource-based policy allows the ecr:DescribeRepositories action', got None (member absent)

task#4 GetRepositoryPolicy:
  $.__type expected 'AccessDeniedException', got 'InvalidParameterException'
  $.Message expected the same sentence with action ecr:GetRepositoryPolicy, got None

task#5 BatchGetImage:
  $.__type expected 'AccessDeniedException', got 'InvalidParameterException'
  $.Message expected the same sentence with action ecr:BatchGetImage, got None

task#6 DescribePullThroughCacheRules:
  $.__type expected 'AccessDeniedException', got 'InvalidParameterException'
  $.Message expected 'User: <callerArn> is not authorized to perform: ecr:DescribePullThroughCacheRules on resource: * because no resource-based policy allows the ecr:DescribePullThroughCacheRules action', got None

The principal ARN, the two account ids, the region and the repository name above are this environment's values and fill the template; the fixed sentence, the error code AccessDeniedException, the capitalised `Message` member and the `*` resource for the registry-scoped read are the requirements.

## Steps to Reproduce
1. Replay the wire corpus against the emulator (trace set simparity-wire-aws-v1), fixture ecr-ecracc-cross-account-registry-id-reads.
2. As the emulator's default principal, call DescribeRepositories with registryId set to a well-formed 12-digit account id that is NOT the caller's (e.g. a repositoryNames entry plus registryId="111111111111").
3. Observe HTTP 400 with __type InvalidParameterException and no Message member, where AWS answers 400 AccessDeniedException with the 'User: ... is not authorized to perform: ecr:DescribeRepositories on resource: <repo arn> because no resource-based policy allows the ecr:DescribeRepositories action' sentence under Message.
4. Repeat with GetRepositoryPolicy, BatchGetImage and DescribePullThroughCacheRules against the same foreign registryId; the last is registry-scoped and must render the resource as `*`.
