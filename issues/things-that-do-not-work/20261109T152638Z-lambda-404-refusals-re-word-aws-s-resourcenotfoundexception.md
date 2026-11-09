Issue-Source: trace-sweep-divergence
Issue-Trace: function-event-invoke-config-qualifier-error-paths
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: function-event-invoke-config-qualifier-error-paths list-function-url-configs-error-paths lambda-snx-traj-cross-region-function-arn
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|ListFunctionEventInvokeConfigs|4xx->4xx|$.Message
Issue-Key: lambda|ListFunctionUrlConfigs|4xx->4xx|$.Message
Issue-Key: lambda|GetPolicy|4xx->4xx|$.Message
Issue-Commit: 8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6

# Lambda 404 refusals re-word AWS's ResourceNotFoundException message for missing and cross-region functions

> _Note: this issue was identified against the code base at commit `8d55b39ab66a28bf4265bc0ae130d6078a9d6cb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListFunctionEventInvokeConfigs

## Description
Shared fault: the status (404) and the error code (ResourceNotFoundException) are right, but the Message is emulator-authored: one generic template is substituted for the per-operation text AWS returns, including the distinct text AWS uses when the request names a function in ANOTHER region. Three net-new clusters. A related open issue, 20261107T030408Z-lambda-nonexistent-resource-refusals-re-worded-resourcenotfoundexception-messages.md, carries other operations' clusters of the same family; it does not carry these three replay keys, so they are filed here.

AWS: Smithy models ResourceNotFoundException {Type, Message} with httpError 404 for all three operations below. Per AWS the Message is part of the contract: the fixed text differs per operation and the only variable parts come from the request.
Why: a client that branches on the message (or surfaces it to an operator) is told the wrong thing — most importantly, a request mistakenly aimed at another region's function reads as "this resource does not exist", so the caller deletes/recreates state instead of fixing the endpoint region.
Fix: the ResourceNotFoundException construction in each of these handlers — use the operation's own message template instead of the shared "Function not found: <arn>" / "The resource you requested does not exist." fallbacks, and detect a request-region/endpoint-region mismatch before the lookup fails.

Cause 1 of 3 — ListFunctionEventInvokeConfigs on a function that does not exist.
AWS: 404 ResourceNotFoundException with Message exactly `The function doesn't exist.` — entirely fixed text, with no ARN or name interpolated.
Emulator: 404 with `Function not found: <resolved function ARN>`.
Traces: function-event-invoke-config-qualifier-error-paths#task#5.

Cause 2 of 3 — ListFunctionUrlConfigs on a function that does not exist.
AWS: 404 ResourceNotFoundException with Message exactly `Function does not exist` — fixed text, no interpolation.
Emulator: 404 with `Function not found: <resolved function ARN>`.
Traces: list-function-url-configs-error-paths#task#5.

Cause 3 of 3 — GetPolicy for a function ARN in a different region.
AWS: 404 ResourceNotFoundException whose Message names both regions: fixed text `Functions from '<region in the request ARN>' are not reachable in this region ('<region of the endpoint>')`, the two region tokens being the only variable parts. Smithy: GetPolicy FunctionName targets NamespacedFunctionName, which admits a full ARN including its region, so the mismatch is detectable from the request alone.
Emulator: 404 with the generic `The resource you requested does not exist.`, which names neither region.
Traces: lambda-snx-traj-cross-region-function-arn#task#8.

## Evidence
Replay cluster details, expected status == actual status (404) in all three:
1. function-event-invoke-config-qualifier-error-paths#task#5 ListFunctionEventInvokeConfigs 404: `$.Message: value_mismatch (expected "The function doesn't exist.", got 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-probe-lfec-ghost-1c5eecba')`.
2. list-function-url-configs-error-paths#task#5 ListFunctionUrlConfigs 404: `$.Message: value_mismatch (expected 'Function does not exist', got 'Function not found: arn:aws:lambda:us-west-2:000000000000:function:sim2real-probe-lfuc-ghost-961ae725')`.
3. lambda-snx-traj-cross-region-function-arn#task#8 GetPolicy 404: `$.Message: value_mismatch (expected "Functions from 'us-east-1' are not reachable in this region ('us-west-2')", got 'The resource you requested does not exist.')`. The recorded names/ARNs above are that run's values filling the template; only the quoted fixed text and the region tokens are requirements.
Smithy: ResourceNotFoundException {Type, Message}, httpError 404, modelled on ListFunctionEventInvokeConfigs, ListFunctionUrlConfigs and GetPolicy.

## Steps to Reproduce
1. ListFunctionEventInvokeConfigs on a function name that was never created — read the 404 body's Message (expect "The function doesn't exist.").
2. ListFunctionUrlConfigs on a function name that was never created — read Message (expect "Function does not exist").
3. Create a function in us-west-2, then GetPolicy with FunctionName="arn:aws:lambda:us-east-1:<acct>:function:<name>" against the us-west-2 endpoint — read Message (expect "Functions from 'us-east-1' are not reachable in this region ('us-west-2')").
