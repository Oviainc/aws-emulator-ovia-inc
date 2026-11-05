Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cfode-distribution-lookup-identifier-mistakes
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cfode-distribution-lookup-identifier-mistakes cloudfront-cfode-function-discovery-stage-and-identifier-mistakes cloudfront-icc-distribution-identifier-mistakes cloudfront-cfoac-oac-and-distribution-id-crossover
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetDistributionConfig|4xx->5xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|GetDistributionConfig|4xx->5xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetDistributionConfig|4xx->5xx|status
Issue-Key: cloudfront|DescribeFunction|4xx->5xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|DescribeFunction|4xx->5xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DescribeFunction|4xx->5xx|status
Issue-Key: cloudfront|CreateInvalidation|4xx->5xx|$.ErrorResponse.Error.Code
Issue-Key: cloudfront|CreateInvalidation|4xx->5xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|CreateInvalidation|4xx->5xx|status
Issue-Key: cloudfront|GetOriginAccessControl|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|GetDistributionConfig|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Related: 20261105T231732Z-cloudfront-refusals-carry-the-wrong-http-status-and.md
Issue-Commit: 1a915a323f43835e1873a1493fb7da2b1abdc334

# CloudFront path identifiers are not format-validated before resource lookup

> _Note: this issue was identified against the code base at commit `1a915a323f43835e1873a1493fb7da2b1abdc334`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDistributionConfig

## Description
Six net-new wire divergences with one shared root cause: CloudFront's REST routing layer never validates the SHAPE of the identifier carried in the request path. AWS rejects a syntactically impossible identifier as such; the emulator either fails to route the request at all (answering 501 NotImplemented) or routes it and reports a plain not-found. This is not covered by the open issue 20261102T050405Z-cloudfront-answers-404-entitynotfound-where-aws-refuses-the.md, whose replay keys are different.

Cause 1 of 6: GetDistributionConfig with an ARN in the {Id} path label answers 501 NotImplemented instead of 404 InvalidAction.
AWS: Smithy: GetDistributionConfig http {method GET, uri /2020-05-31/distribution/{Id}/config}; Id is an httpLabel. An ARN contains '/' and ':' so it cannot occupy a single path segment, and AWS's REST front end therefore fails to match any action and answers (recorded behavior only) 404 with Code `InvalidAction` and no Message member.
Emulator: answers 501 with Code `NotImplemented` and Message "operation GET /2020-05-31/distribution/<the ARN>/config is not implemented in this emulator".
Why: 501 is a server-fault class. A client is told the emulator lacks the operation — which is false, GetDistributionConfig works — so SDK retry logic and feature probes draw the wrong conclusion, and the real defect (the caller passed an ARN where an Id belongs) is never reported. A per-request path string also leaks into the message, which AWS never does.
Fix: the CloudFront REST router — an unmatched path under the /2020-05-31 prefix must produce the service's InvalidAction refusal rather than the emulator's generic NotImplemented fallback.
Traces: cloudfront-cfode-distribution-lookup-identifier-mistakes#task#6.

Cause 2 of 6: DescribeFunction with an ARN in the {Name} path label answers 501 NotImplemented instead of 404 InvalidAction.
AWS: Smithy: DescribeFunction http {method GET, uri /2020-05-31/function/{Name}/describe}; Name is an httpLabel on FunctionName with @length 1..64 and @pattern ^[a-zA-Z0-9-_]{1,64}$ — an ARN cannot satisfy it. Recorded behavior: 404 with Code `InvalidAction`.
Emulator: 501 / `NotImplemented` / "operation GET /2020-05-31/function/<ARN>/describe is not implemented in this emulator".
Why: same as Cause 1 — a working operation is reported as unimplemented, and the modelled @pattern on Name is never enforced.
Fix: the CloudFront REST router plus httpLabel constraint enforcement for FunctionName.
Traces: cloudfront-cfode-function-discovery-stage-and-identifier-mistakes#task#4.

Cause 3 of 6: CreateInvalidation with an ARN in the {DistributionId} path label answers 501 NotImplemented instead of 404 InvalidAction.
AWS: Smithy: CreateInvalidation http {method POST, uri /2020-05-31/distribution/{DistributionId}/invalidation, code 201}; DistributionId is an httpLabel. Recorded behavior: 404 with Code `InvalidAction`.
Emulator: 501 / `NotImplemented` / "operation POST /2020-05-31/distribution/<ARN>/invalidation is not implemented in this emulator".
Why: same as Cause 1, on a mutating operation — a caller that treats 501 as "feature absent" disables invalidation entirely.
Fix: the CloudFront REST router.
Traces: cloudfront-icc-distribution-identifier-mistakes#task#4.

Cause 4 of 6: GetOriginAccessControl given an identifier of the wrong resource type reports not-found instead of an id-format refusal.
AWS: Smithy: GetOriginAccessControl http {GET /2020-05-31/origin-access-control/{Id}}, errors AccessDenied (403) and NoSuchOriginAccessControl (404). Recorded behavior: 404 whose Message is the fixed text "Id format is invalid" — AWS checks the identifier's shape before looking it up, and that message has no variable parts.
Emulator: answers 404 with Message "The specified origin access control does not exist." (status and code match).
Why: the two conditions mean different things. "Id format is invalid" tells the caller it passed the wrong kind of identifier (here a distribution id where an OAC id belongs) and that retrying or waiting will never help; "does not exist" implies a well-formed id that is simply absent, so a client polls for eventual consistency that will never arrive.
Fix: GetOriginAccessControl's input validation — validate the Id's format before the state lookup and return AWS's "Id format is invalid" message.
Traces: cloudfront-cfoac-oac-and-distribution-id-crossover#task#3.

Cause 5 of 6: GetDistributionConfig given a malformed distribution id reports not-found instead of an id-format refusal.
AWS: Smithy: GetDistributionConfig errors include NoSuchDistribution (404) / AccessDenied (403). Recorded behavior: 404 with the fixed Message "Id format is invalid" for an identifier that is not a well-formed distribution id.
Emulator: 404 with Message "The specified distribution does not exist." 2 affected steps.
Why: as Cause 4 — a permanent client error is presented as a transient absence.
Fix: GetDistributionConfig's input validation, ahead of the state lookup.
Traces: cloudfront-cfode-distribution-lookup-identifier-mistakes#task#4 (2 affected steps).

Cause 6 of 6: CreateInvalidation given a malformed distribution id reports not-found instead of an id-format refusal.
AWS: Smithy: CreateInvalidation errors include NoSuchDistribution (404). Recorded behavior: 404 with the fixed Message "DistributionId format is invalid" — note AWS names the input member (DistributionId), not a generic "Id", in this operation's message.
Emulator: 404 with Message "The specified distribution does not exist." 2 affected steps.
Why: as above, plus the message no longer names which input member was rejected, so a caller validating its own inputs cannot tell what to fix.
Fix: CreateInvalidation's input validation — validate DistributionId's format before the lookup, and word the message with the member name.
Traces: cloudfront-icc-distribution-identifier-mistakes#task#2 (2 affected steps).

## Evidence
Replay of trace set simparity-wire-aws-v1 (score 78.2%, 102 divergence clusters, all cloudfront).

Cause 1 — cloudfront-cfode-distribution-lookup-identifier-mistakes#task#6 GetDistributionConfig: expected status 404 / Code 'InvalidAction' / no Message; got 501 / Code 'NotImplemented' / Message 'operation GET /2020-05-31/distribution/arn:aws:cloudfront::000000000000:distribution/EJZ50Z92T9DP30/config is not implemented in this emulator'.
Cause 2 — cloudfront-cfode-function-discovery-stage-and-identifier-mistakes#task#4 DescribeFunction: expected 404 / 'InvalidAction'; got 501 / 'NotImplemented' / 'operation GET /2020-05-31/function/arn:aws:cloudfront::000000000000:function/<name>/describe is not implemented in this emulator'.
Cause 3 — cloudfront-icc-distribution-identifier-mistakes#task#4 CreateInvalidation: expected 404 / 'InvalidAction'; got 501 / 'NotImplemented' / 'operation POST /2020-05-31/distribution/arn:aws:cloudfront::000000000000:distribution/E7IYCGPKTANSF3/invalidation is not implemented in this emulator'.
Cause 4 — cloudfront-cfoac-oac-and-distribution-id-crossover#task#3 GetOriginAccessControl: $.ErrorResponse.Error.Message expected 'Id format is invalid', got 'The specified origin access control does not exist.' (both 404).
Cause 5 — cloudfront-cfode-distribution-lookup-identifier-mistakes#task#4 GetDistributionConfig: expected Message 'Id format is invalid', got 'The specified distribution does not exist.' (also #task#5).
Cause 6 — cloudfront-icc-distribution-identifier-mistakes#task#2 CreateInvalidation: expected Message 'DistributionId format is invalid', got 'The specified distribution does not exist.' (also #task#3).

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Replay fixture cloudfront-cfode-distribution-lookup-identifier-mistakes and inspect step task#6 (representative) and task#4.
4. Replay cloudfront-cfode-function-discovery-stage-and-identifier-mistakes#task#4, cloudfront-icc-distribution-identifier-mistakes#task#4 and #task#2, and cloudfront-cfoac-oac-and-distribution-id-crossover#task#3.
5. For each, compare status, ErrorResponse/Error/Code and Message with the recorded response: no step may answer 501 NotImplemented, and the id-format steps must carry AWS's 'Id format is invalid' / 'DistributionId format is invalid' message.
