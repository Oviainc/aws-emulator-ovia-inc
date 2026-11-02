Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__resourcegroupstaggingapi.blueprint-tag-applied-after-create-surfaces-in-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-untag-removes-the-function-from-the-tag-query lambda__resourcegroupstaggingapi.blueprint-tag-e2e-find-tagged-functions-lifecycle
Issue-Key: lambda__resourcegroupstaggingapi|CreateFunction|AmbiguousService
Issue-Key: lambda__resourcegroupstaggingapi.blueprint-tag-e2e-find-tagged-functions-lifecycle|phase#6|AssertionError|raise AssertionError(f"[phase {i}/{len(self.phases)} {name}] {exc}") from exc
Issue-Commit: 5ef273b5ec187d9b55db2ccfdad00816d8c5c39a

# Lambda is unreachable (router AmbiguousService across four 'lambda'-signing services), and a CloudFormation-deployed Lambda stack exposes no Outputs (2 causes)

> _Note: this issue was identified against the code base at commit `5ef273b5ec187d9b55db2ccfdad00816d8c5c39a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Two independent causes, both grounded in lambda-prefixed checks. A fixer must close BOTH before submitting.

=== Cause 1 of 2: every lambda-signed request is refused by the router with AmbiguousService ===
AWS: CreateFunction is a bound Lambda operation — Smithy: CreateFunction, POST /2015-03-31/functions, http code 201, output shape FunctionConfiguration (FunctionName, FunctionArn, State, RevisionId, ...), modeled errors InvalidParameterValueException 400 / ResourceConflictException 409 / ResourceNotFoundException 404 / CodeStorageExceededException 400 / ServiceException 500 / TooManyRequestsException 429 and others. AmbiguousService is not an AWS error code and no Lambda operation can answer it.
Emulator: "An error occurred (AmbiguousService) when calling the CreateFunction operation (reached max retries: 3): 4 emulated services all claim requests for 'lambda': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one" — an emulator-generated, retryable 5xx-class refusal. The router resolves a request to a service by its sigv4 signing name, and four bootstrapped services sign as 'lambda' (the Lambda control plane plus the lambdacore / lambdamicrovms / lambdaweb services), so resolution is ambiguous and the request is dropped before dispatch. All four claimants are printed under the same token, so the message cannot distinguish them.
Why: Lambda is completely unusable through the emulator — no function can be created, tagged, listed or deleted — and the failure arrives as a retried server error rather than any modeled client error, so every caller burns its full retry budget. Lambda is a dependency of a large share of realistic scenarios, so this blocks far more than the tagging checks that caught it.
Fix: the request router's service-resolution step — disambiguate a signing name shared by several modeled services (by endpoint prefix / host, or by matching the request's method+URI against each candidate's bound operations) instead of refusing the request. Same defect class as the already-open "Kinesis Analytics V2 is entirely unreachable: router refuses every request with AmbiguousService ..." and "Chime SDK Media Pipelines is entirely unreachable: the router resolves by sigv4 signing name ..."; neither names the 'lambda' claimants, so this instance is net-new.
Traces: lambda__resource-groups-tagging-api.blueprint-tag-applied-after-create-surfaces-in-the-tag-query, lambda__resource-groups-tagging-api.blueprint-tag-untag-removes-the-function-from-the-tag-query (both die on their first CreateFunction call).

=== Cause 2 of 2: a CloudFormation-deployed stack of Lambda functions reports no Outputs ===
AWS: recorded behavior only (no model statement governs stack Output resolution). The check deploys a CloudFormation template whose Outputs publish the created functions' names/ARNs and then reads them back from the stack; on AWS a stack that reaches CREATE_COMPLETE exposes each declared Output with its resolved value, so the deploy phase obtains the tagged function's name and its untagged sibling's ARN, and CloudFormation additionally stamps aws:cloudformation:stack-id / stack-name / logical-id onto every taggable resource it creates.
Emulator: the deploy phase fails at its first assertion — "the stack must publish the tagged function's name 'sim2real-scn-e2ebp-tagged-730b1a96e3a8'; outputs are {}" — i.e. the stack's Outputs map comes back empty, so nothing downstream (the function ARNs, the CFN system tags, the frozen GetResources trajectory, the untag transition, the destroy assertions) can be evaluated. Hypothesis: the template's Outputs are never evaluated/stored at stack creation; a contributing possibility is that the stack's AWS::Lambda::Function resources were never provisioned at all, which cause 1 would also explain if the emulator's CloudFormation provisions through the same router.
Why: a client that deploys infrastructure and consumes stack Outputs — the normal contract between a template and whatever runs after it — gets an empty map and cannot locate anything it just created, so every post-deploy step fails even when the stack reports success.
Fix: the CloudFormation stack-creation path (Outputs evaluation and their exposure through DescribeStacks), and the Lambda resource provisioning it drives. A closely related cause is already filed for a different service's stack — open issue "CloudFormation-deployed Firehose delivery streams are never provisioned and the stack exposes no Outputs (2 causes)" — it does not carry this check's replay key, so this Lambda instance is filed separately; fixing the shared Outputs evaluation may close both.
Traces: lambda__resource-groups-tagging-api.blueprint-tag-e2e-find-tagged-functions-lifecycle (phase 1/4 deploy).

## Evidence
Cause 1 — Expected: CreateFunction returns HTTP 201 with a FunctionConfiguration envelope (Smithy: CreateFunction -> FunctionConfiguration, http {method: POST, uri: /2015-03-31/functions, code: 201}).
Actual: botocore.exceptions.ClientError: An error occurred (AmbiguousService) when calling the CreateFunction operation (reached max retries: 3): 4 emulated services all claim requests for 'lambda': lambda, lambda, lambda, lambda; the request is rejected rather than served by an arbitrary one. Retry exhaustion shows a retryable 5xx-class status; AmbiguousService appears in no Lambda operation's modeled error list.

Cause 2 — Expected: after the stack reaches CREATE_COMPLETE, its Outputs publish the deployed functions' identifiers.
Actual: AssertionError: [phase 1/4 deploy] the stack must publish the tagged function's name 'sim2real-scn-e2ebp-tagged-730b1a96e3a8'; outputs are {}.

## Steps to Reproduce
Cause 1: boot the emulator, point a botocore 'lambda' client at it, call create_function(...); observe the AmbiguousService refusal after three retries instead of a 201 FunctionConfiguration. Every other lambda operation fails identically, since the refusal precedes dispatch.
Cause 2: deploy a CloudFormation template that creates two AWS::Lambda::Function resources (one with a Tags entry) and declares Outputs publishing their names and ARNs; wait for the stack, then read DescribeStacks Outputs. Observe an empty Outputs map where AWS returns each declared Output resolved.
