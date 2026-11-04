Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-network-insights-path-analysis-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-network-insights-path-analysis-lifecycle ec2-dry-run-delete-validation-across-network-resources
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateNetworkInsightsPath|2xx->2xx|$.CreateNetworkInsightsPathResponse.networkInsightsPath.destinationArn
Issue-Key: ec2|CreateNetworkInsightsPath|2xx->2xx|$.CreateNetworkInsightsPathResponse.networkInsightsPath.sourceArn
Issue-Key: ec2|DescribeNetworkInsightsPaths|2xx->2xx|$.DescribeNetworkInsightsPathsResponse.networkInsightsPathSet.item.destinationArn
Issue-Key: ec2|DescribeNetworkInsightsPaths|2xx->2xx|$.DescribeNetworkInsightsPathsResponse.networkInsightsPathSet.item.sourceArn
Issue-Key: ec2|DescribeNetworkInsightsAnalyses|4xx->2xx|$
Issue-Key: ec2|DescribeNetworkInsightsAnalyses|4xx->2xx|$.Response
Issue-Key: ec2|DescribeNetworkInsightsAnalyses|4xx->2xx|status
Issue-Key: ec2|StartNetworkInsightsAnalysis|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DescribeNetworkInsightsPaths|4xx->4xx|$.Response.Errors.Error.Message
Issue-Commit: 30059ba7efbea1c9406793a7f2ccf8806d86986b

# EC2 network-insights path and analysis responses diverge from AWS

> _Note: this issue was identified against the code base at commit `30059ba7efbea1c9406793a7f2ccf8806d86986b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateNetworkInsightsPath

## Description
The Reachability Analyzer (network-insights) surface diverges from AWS in five ways: the path resource is stored and served without its source/destination ARNs, an analysis id that AWS reports as nonexistent is answered 200 with an emulator-authored record, and the not-found message for a path id is emulator-authored wording. This bundle carries 5 causes.

Cause 1 of 5: CreateNetworkInsightsPath omits sourceArn and destinationArn (fixture ec2-network-insights-path-analysis-lifecycle, step task#5).
AWS: Smithy: CreateNetworkInsightsPath output member networkInsightsPath -> shape NetworkInsightsPath, which models SourceArn (xmlName sourceArn, target ResourceArn, @length 1..1283, "The Amazon Resource Name (ARN) of the source") and DestinationArn (xmlName destinationArn, same target). AWS returns both, each being the ARN of the resource named by Source/Destination - for an ENI source/destination the arn:aws:ec2:<region>:<account>:network-interface/<eni-id> form.
Emulator: answers 200 with the path but both members absent (sourceArn: missing, destinationArn: missing).
Why: a client that keys off the path's endpoint ARNs - cross-account/cross-Region analysis, IAM policy or tag reconciliation, infra tooling that stores ARNs rather than ids - gets null and cannot reconstruct the ARN itself (it does not know the owner account or Region the service assigned).
Fix: the CreateNetworkInsightsPath handler/state in the ec2 service crate: resolve Source/Destination to the referenced resource and persist + serialize sourceArn/destinationArn.
Traces: ec2-network-insights-path-analysis-lifecycle#task#5.

Cause 2 of 5: DescribeNetworkInsightsPaths omits the same two members (fixture ec2-network-insights-path-analysis-lifecycle, step task#6).
AWS: same Smithy shape NetworkInsightsPath is used in DescribeNetworkInsightsPaths -> networkInsightsPathSet.item, so the read path returns sourceArn/destinationArn for the path created above.
Emulator: 200, networkInsightsPathSet.item.sourceArn and .destinationArn missing - the read reproduces the gap from Cause 1 because the stored path never carries them.
Why: re-reading a path cannot recover the ARNs either, so the gap is not transient and clients that list paths see a permanently incomplete resource.
Fix: the network-insights path state record plus the DescribeNetworkInsightsPaths projection.
Traces: ec2-network-insights-path-analysis-lifecycle#task#6.

Cause 3 of 5: DescribeNetworkInsightsAnalyses answers 200 for an analysis that does not exist (fixture ec2-network-insights-path-analysis-lifecycle, step task#9).
AWS: recorded behavior only (EC2 models no error shapes for this operation). The recording shows HTTP 400 with the EC2 error envelope Response/Errors/Error, Code 'InvalidNetworkInsightsAnalysisId.NotFound' and Message '<analysis-id> does not exist' - fixed text ' does not exist', the variable part being the NetworkInsightsAnalysisId echoed from the request.
Emulator: HTTP 200 with networkInsightsAnalysisSet.item for that id, status 'failed' and an emulator-authored statusMessage ('This emulator does not perform Reachability Analyzer path tracing, so no reachability result was produced for this path.'). The step polled 12 times and the error never appeared. Hypothesis: the emulator materialises an analysis record for any well-formed id (or retains one after it is gone) instead of looking it up in state.
Why: a client polling for the NotFound error as the signal that an analysis is gone never terminates, and a client reading status sees a 'failed' analysis that never ran - including emulator prose in a field it surfaces to users.
Fix: the DescribeNetworkInsightsAnalyses handler and the network-insights analysis state: unknown ids must raise 400 InvalidNetworkInsightsAnalysisId.NotFound with the AWS message template, and no emulator-authored statusMessage may be injected.
Traces: ec2-network-insights-path-analysis-lifecycle#task#9.

Cause 4 of 5: StartNetworkInsightsAnalysis returns the right code with an emulator-authored not-found message (fixture ec2-network-insights-path-analysis-lifecycle, step task#12).
AWS: recorded behavior only. The recorded 400 carries Code 'InvalidNetworkInsightsPathId.NotFound' (the emulator matches this) and Message '<path-id> does not exist'. The message's fixed text is ' does not exist'; its only variable part is the NetworkInsightsPathId taken from the request, unquoted and with no prefix.
Emulator: 400 with Message "The network insights path ID '<path-id>' does not exist" - a different sentence (added prefix, quoted id).
Why: the error message is part of the contract; clients and test suites that assert on or display the message text diverge from AWS even though the status and code are right.
Fix: the shared network-insights path lookup/validation helper that formats this message.
Traces: ec2-network-insights-path-analysis-lifecycle#task#12.

Cause 5 of 5: DescribeNetworkInsightsPaths emits the same emulator-authored not-found message (fixture ec2-dry-run-delete-validation-across-network-resources, step task#24).
AWS: recorded behavior only - 400 with Message '<path-id> does not exist' for an unknown path id, same template as Cause 4.
Emulator: 400 with "The network insights path ID '<path-id>' does not exist".
Why: same contract break on a second operation, confirming the wording comes from a shared helper rather than one handler.
Fix: same helper as Cause 4; fixing it must cover both the describe and the start path.
Traces: ec2-dry-run-delete-validation-across-network-resources#task#24.

No open issue carries these replay keys; the nearest open issues cover other network-insights work (20261023T102725Z and 20261029T200817Z are DeleteNetworkInsightsAccessScope* stubs, 20261029T230911Z covers the CreateNetworkInsightsAccessScope idempotency message).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 57.3%), service ec2, all five clusters unowned and not stub-blocked.
Cause 1 - ec2-network-insights-path-analysis-lifecycle#task#5 CreateNetworkInsightsPath, expected 200 / actual 200: $.CreateNetworkInsightsPathResponse.networkInsightsPath.sourceArn missing (expected an arn:aws:ec2:<region>:<account>:network-interface/<eni-id> value, got None); .destinationArn missing likewise.
Cause 2 - ec2-network-insights-path-analysis-lifecycle#task#6 DescribeNetworkInsightsPaths, expected 200 / actual 200: $.DescribeNetworkInsightsPathsResponse.networkInsightsPathSet.item.sourceArn missing, .destinationArn missing.
Cause 3 - ec2-network-insights-path-analysis-lifecycle#task#9 DescribeNetworkInsightsAnalyses, expected 400 / actual 200: expected $.Response.Errors.Error.Code 'InvalidNetworkInsightsAnalysisId.NotFound' with Message '<analysis-id> does not exist'; got DescribeNetworkInsightsAnalysesResponse.networkInsightsAnalysisSet.item with status 'failed' and statusMessage 'This emulator does not perform Reachability Analyzer path tracing, so no reachability result was produced for this path.' - poll: error code None not in ['InvalidNetworkInsightsAnalysisId.NotFound'] after 12 attempts.
Cause 4 - ec2-network-insights-path-analysis-lifecycle#task#12 StartNetworkInsightsAnalysis, expected 400 / actual 400: $.Response.Errors.Error.Message expected '<path-id> does not exist', got "The network insights path ID '<path-id>' does not exist".
Cause 5 - ec2-dry-run-delete-validation-across-network-resources#task#24 DescribeNetworkInsightsPaths, expected 400 / actual 400: $.Response.Errors.Error.Message expected '<path-id> does not exist', got "The network insights path ID '<path-id>' does not exist".

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Replay fixture ec2-network-insights-path-analysis-lifecycle: step task#5 (CreateNetworkInsightsPath on two ENIs) - the response has no sourceArn/destinationArn; step task#6 (DescribeNetworkInsightsPaths) - the stored path still has neither.
3. In the same fixture, step task#9 polls DescribeNetworkInsightsAnalyses for an analysis id AWS reports as nonexistent: the emulator returns 200 with a synthesized 'failed' analysis instead of 400 InvalidNetworkInsightsAnalysisId.NotFound.
4. Step task#12 calls StartNetworkInsightsAnalysis with an unknown nip- id and step task#24 of fixture ec2-dry-run-delete-validation-across-network-resources calls DescribeNetworkInsightsPaths with an unknown nip- id: both return 400 with the emulator's "The network insights path ID '...' does not exist" wording instead of AWS's '<path-id> does not exist'.
