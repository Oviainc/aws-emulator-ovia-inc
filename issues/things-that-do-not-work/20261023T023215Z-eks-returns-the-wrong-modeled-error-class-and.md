Issue-Source: trace-sweep-divergence
Issue-Trace: eks-compute-resource-creation-on-missing-cluster
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: eks-compute-resource-creation-on-missing-cluster eks-nodegroup-and-fargate-profile-errors-unknown-cluster eks-nodegroup-create-nonexistent-cluster-error eks-tag-operations-invalid-arn-errors eks-tag-operations-on-missing-and-malformed-resource
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks|CreateNodegroup|4xx->4xx|$.nodegroupName
Issue-Key: eks|CreateNodegroup|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|CreateNodegroup|4xx->4xx|status
Issue-Key: eks|TagResource|4xx->4xx|$.message
Issue-Key: eks|TagResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|TagResource|4xx->4xx|status
Issue-Key: eks|ListTagsForResource|4xx->4xx|$.message
Issue-Key: eks|ListTagsForResource|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: eks|ListTagsForResource|4xx->4xx|status
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# EKS returns the wrong modeled error class and status for a missing cluster and a malformed resource ARN

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNodegroup

## Description
Three paths answer a DIFFERENT modeled exception - and a different HTTP status - than AWS, so SDK error branching takes the wrong arm. THREE causes; the issue is not complete until all three are closed. The open behavioral issue 20261022T154613Z-eks-control-plane-operations-answer-501-notimplemented-or.md discusses CreateNodegroup as a 501 stub; here CreateNodegroup DOES answer, but with the wrong error class, and that issue carries none of the replay keys below.

Cause 1 of 3 - CreateNodegroup on a cluster that does not exist returns 400 ClientException instead of 404 ResourceNotFoundException.
AWS: Smithy (com-amazonaws-eks-awswesleyfrontend): CreateNodegroup (POST /clusters/{clusterName}/node-groups) models ClientException (400), InvalidParameterException (400), InvalidRequestException (400), ResourceInUseException (409), ResourceLimitExceededException (400), ServerException (500), ServiceUnavailableException (503); ResourceNotFoundException is the service's 404 for an unresolvable cluster and the recording shows status 404 with x-amzn-errortype ResourceNotFoundException and $.nodegroupName NOT set.
Emulator: HTTP 400 with x-amzn-errortype ClientException and $.nodegroupName populated from the request.
Why: a client that catches ResourceNotFoundException to create the parent cluster and retry never enters that arm; it sees a 400 ClientException, which SDK users treat as a non-retryable bad request, so create-parent-then-child orchestration breaks. The 400-vs-404 difference also defeats status-based retry/alarm rules.
Fix: the CreateNodegroup handler's cluster-resolution step in the eks crate - resolve clusterName first and raise the 404 not-found error class, without populating nodegroupName.
Traces: eks-compute-resource-creation-on-missing-cluster#task#2 (also eks-nodegroup-and-fargate-profile-errors-unknown-cluster#task#1, eks-nodegroup-create-nonexistent-cluster-error#task#1).

Cause 2 of 3 - TagResource with a syntactically invalid resourceArn returns 404 NotFoundException instead of 400 BadRequestException.
AWS: Smithy: TagResource (POST /tags/{resourceArn}) models EXACTLY two client errors - BadRequestException (httpError 400, documented 'thrown if the request contains a semantic error') and NotFoundException (404). A resourceArn that is not an ARN at all is a syntax failure, and the recording shows status 400 with x-amzn-errortype BadRequestException (message example: 'Invalid input resource arn: bad syntax'), i.e. the ARN is parsed BEFORE any lookup.
Emulator: HTTP 404 with x-amzn-errortype NotFoundException - it skips ARN parsing and reports the unparseable string as a missing resource.
Why: a caller cannot distinguish 'you sent garbage' (fix the code) from 'the resource is gone' (stop retrying / treat as deleted); tagging helpers that swallow NotFoundException as benign will silently ignore a malformed ARN bug.
Fix: the TagResource handler's input validation - parse/validate resourceArn and raise the 400 bad-request error class before resource lookup.
Traces: eks-tag-operations-invalid-arn-errors#task#1 (also eks-tag-operations-on-missing-and-malformed-resource#task#2).

Cause 3 of 3 - ListTagsForResource with a syntactically invalid resourceArn returns 404 NotFoundException instead of 400 BadRequestException.
AWS: Smithy: ListTagsForResource (GET /tags/{resourceArn}) models BadRequestException (400) and NotFoundException (404); the recording shows 400 BadRequestException for a non-ARN resourceArn (message example: 'Invalid input resource arn: bad syntax').
Emulator: HTTP 404 with x-amzn-errortype NotFoundException.
Why: same as cause 2 - malformed input is misreported as absence, on the read path this time.
Fix: the ListTagsForResource handler's input validation, sharing the ARN parser with TagResource.
Traces: eks-tag-operations-invalid-arn-errors#task#2 (also eks-tag-operations-on-missing-and-malformed-resource#task#1).

## Evidence
Corpus simparity-wire-aws-v1, 35/216 steps passed.

Cause 1: eks-compute-resource-creation-on-missing-cluster#task#2 CreateNodegroup - status: expected 404, got 400; $headers.x-amzn-errortype: value_mismatch (expected 'ResourceNotFoundException', got 'ClientException'); $.nodegroupName: type_mismatch (expected None, got 'sim2real-probe-eks-nodegroup-d33198a9'). affected_count 3.
Cause 2: eks-tag-operations-invalid-arn-errors#task#1 TagResource - status: expected 400, got 404; $headers.x-amzn-errortype: value_mismatch (expected 'BadRequestException', got 'NotFoundException'); message example expected 'Invalid input resource arn: bad syntax', got 'The resource not-a-valid-eks-resource-arn could not be found'. affected_count 2.
Cause 3: eks-tag-operations-invalid-arn-errors#task#2 ListTagsForResource - status: expected 400, got 404; $headers.x-amzn-errortype: value_mismatch (expected 'BadRequestException', got 'NotFoundException'). affected_count 2.

## Steps to Reproduce
1. Boot the emulator and point an EKS client at it.
2. Cause 1: create_nodegroup(clusterName=<never created>, nodegroupName='ng', subnets=['subnet-0'], nodeRole='arn:aws:iam::000000000000:role/r') -> expect HTTP 404, x-amzn-errortype ResourceNotFoundException, and no nodegroupName member in the body; observe HTTP 400 ClientException.
3. Cause 2: tag_resource(resourceArn='not-a-valid-eks-resource-arn', tags={'k':'v'}) -> expect HTTP 400 with x-amzn-errortype BadRequestException; observe 404 NotFoundException.
4. Cause 3: list_tags_for_resource(resourceArn='not-a-valid-eks-resource-arn') -> expect HTTP 400 BadRequestException; observe 404 NotFoundException.
5. Control for causes 2/3: use a well-formed EKS cluster ARN for a cluster that does not exist - that one legitimately stays 404 NotFoundException.
