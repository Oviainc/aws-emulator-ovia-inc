Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-tagging-api-bpt-traj-wrong-region-sweep
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-tagging-api-bpt-traj-wrong-region-sweep
Issue-Shape-Id: com.amazonaws.resourcegroupstaggingapi#ResourceGroupsTaggingAPI_20170126
Issue-Key: lambda|ListTags|4xx->2xx|$.Message
Issue-Key: lambda|ListTags|4xx->2xx|$.Type
Issue-Key: lambda|ListTags|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|ListTags|4xx->2xx|status
Issue-Key: lambda|GetFunction|4xx->2xx|$.Message
Issue-Key: lambda|GetFunction|4xx->2xx|$.Type
Issue-Key: lambda|GetFunction|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: lambda|GetFunction|4xx->2xx|status
Issue-Commit: 46f65a015adce40c0d19696cf4b820bd06724ad6

# Lambda reads are not region-scoped: a function absent from the request's region answers 200 instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `46f65a015adce40c0d19696cf4b820bd06724ad6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTags

## Description
Two divergences, one root cause: the emulator's Lambda function state is not partitioned by region, so a read addressed to a region in which the function does not exist succeeds instead of being refused 404 ResourceNotFoundException. Both must be closed before this issue is submitted.

Cause 1 of 2 — ListTags on a function ARN whose region differs from the endpoint's region answers 200 instead of 404 (fixture resource-groups-tagging-api-bpt-traj-wrong-region-sweep, step task#5).
AWS: Lambda is a regional service and the ARN passed to ListTags must name a resource in the region the request was sent to. A cross-region ARN is refused with HTTP 404, error shape ResourceNotFoundException (x-amzn-errortype header set to that code), body members Type = "User" and a Message of the fixed form "Functions from '<arn-region>' are not reachable in this region ('<endpoint-region>')" — the fixed text is "Functions from '", "' are not reachable in this region ('" and "')", and the two region tokens come from the request's ARN and the endpoint it was sent to. Smithy: ListTags.Resource -> TaggableResource (required, @httpLabel, length 1..10000, pattern anchoring arn:<partition>:lambda:<region>:<account>:function|layer|...); operation errors InvalidParameterValueException(400), ResourceNotFoundException(404, members Type and Message, doc "The resource specified in the request does not exist."), ServiceException(500), TooManyRequestsException(429). Recorded response: 404 with Type 'User', the Message above, and an x-amzn-errortype header.
Emulator: answers HTTP 200 (a tags map) with no Type, no Message and no x-amzn-errortype header — it resolves the ARN against a single global function table and ignores the ARN's region segment entirely.
Why: a client that mistakenly points a regional client at the wrong region, or copies an ARN between regions, is told the resource exists and receives tags for a function that is not in that region. Region isolation — the basis for multi-region deployment, failover and data-residency testing — cannot be exercised at all, and the modeled 404 never fires.
Fix: Lambda's ARN resolution in the ListTags handler and the function store's keying — resolve a function by (region, account, name) and refuse an ARN whose region is not the request's region with ResourceNotFoundException(404) carrying Type 'User' and the cross-region message.
Traces: resource-groups-tagging-api-bpt-traj-wrong-region-sweep#task#5.

Cause 2 of 2 — GetFunction for a function that does not exist in the request's region answers 200 instead of 404 (fixture resource-groups-tagging-api-bpt-traj-wrong-region-sweep, step task#6).
AWS: GetFunction for a name or ARN that names no function in the endpoint's region is refused with HTTP 404 ResourceNotFoundException (x-amzn-errortype set to it), Type = "User", Message of the fixed form "Function not found: <the function ARN as resolved from the request>" — "Function not found: " is fixed text and the ARN is derived from the request and the endpoint's region/account. Smithy: GetFunction.FunctionName -> NamespacedFunctionName (required, @httpLabel, length 1..256); errors InvalidParameterValueException(400), ResourceNotFoundException(404), ServiceException(500), TooManyRequestsException(429).
Emulator: answers HTTP 200 with a function envelope — no Type, no Message, no x-amzn-errortype — because the function created in another region is visible in this one.
Why: existence checks are the primary control-flow signal for Lambda clients (create-if-absent, delete-if-present, cross-region replication). Returning a configuration for a function that is not in the region makes every such check take the wrong branch, and the function's reported ARN then contradicts the region it was read from.
Fix: Lambda's function lookup in the GetFunction handler and the shared region-scoped keying of the function store (same state defect as cause 1) — a lookup that finds nothing in the request's region must raise ResourceNotFoundException(404) with the 'Function not found: <arn>' message.
Traces: resource-groups-tagging-api-bpt-traj-wrong-region-sweep#task#6.

No open issue covers this: the open Lambda trace issues concern other operations' behavior, and nothing on file reports Lambda's state being region-blind. These two clusters carry neither stub_path nor owned_by in the replay report.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 11 fixtures replayed, 95 steps (66 passed, 29 mismatched), score 69.5%, emulator_up=true, bootstrap_signal=false. Two clusters belong to service=lambda; neither carries stub_path or owned_by.

Cause 1 — fixture resource-groups-tagging-api-bpt-traj-wrong-region-sweep step task#5, operation ListTags, verdict mismatch:
  status: expected 404, got 200
  $.Message: missing (expected "Functions from 'us-east-1' are not reachable in this region ('us-west-2')", got None)
  $.Type: missing (expected 'User', got None)
  $headers.x-amzn-errortype: missing (expected the error-type header, got None)
  (the two region tokens are the recording's values for the template described above)

Cause 2 — fixture resource-groups-tagging-api-bpt-traj-wrong-region-sweep step task#6, operation GetFunction, verdict mismatch:
  status: expected 404, got 200
  $.Type: missing (expected 'User', got None)
  $.Message: missing (expected 'Function not found: arn:aws:lambda:<region>:<account>:function:<probe-fn>', got None)
  $headers.x-amzn-errortype: missing (expected the error-type header, got None)

Model evidence (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json):
  ListTags: GET /2017-03-31/tags/{Resource}; Resource required @httpLabel TaggableResource len 1..10000 with an ARN pattern whose region segment is part of the match; errors InvalidParameterValueException(400), ResourceNotFoundException(404), ServiceException(500), TooManyRequestsException(429).
  GetFunction: GET /2015-03-31/functions/{FunctionName}; FunctionName required @httpLabel NamespacedFunctionName len 1..256; same error set.
  ResourceNotFoundException: client error, httpError 404, members Type (String) and Message (String), doc "The resource specified in the request does not exist."

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 against the emulator built from this checkout (replay_wire_corpus), then re-run the two bound steps.

Cause 1: resource-groups-tagging-api-bpt-traj-wrong-region-sweep#task#5 — create a tagged Lambda function in region A (e.g. us-east-1), then send GET /2017-03-31/tags/arn%3Aaws%3Alambda%3Aus-east-1%3A<account>%3Afunction%3A<fn> to the Lambda endpoint for region B (e.g. us-west-2). Expect HTTP 404 with x-amzn-errortype ResourceNotFoundException, Type 'User' and Message "Functions from 'us-east-1' are not reachable in this region ('us-west-2')"; observe HTTP 200 with a tags map.

Cause 2: resource-groups-tagging-api-bpt-traj-wrong-region-sweep#task#6 — with the function existing only in region A, send GET /2015-03-31/functions/<fn> (or its region-B ARN) to the region-B endpoint. Expect HTTP 404 with x-amzn-errortype ResourceNotFoundException, Type 'User' and Message 'Function not found: arn:aws:lambda:us-west-2:<account>:function:<fn>'; observe HTTP 200 with a function configuration envelope.
