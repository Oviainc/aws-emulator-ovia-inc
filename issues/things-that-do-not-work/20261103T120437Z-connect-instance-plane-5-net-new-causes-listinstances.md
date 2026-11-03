Issue-Source: trace-sweep-divergence
Issue-Trace: connect-ccp-traj-connect-instance-detour
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: connect-ccp-traj-connect-instance-detour
Issue-Shape-Id: com.amazonaws.connect#AmazonConnectService
Issue-Key: connect|ListInstances|2xx->4xx|$.InstanceSummaryList
Issue-Key: connect|ListInstances|2xx->4xx|status
Issue-Key: connect|ListInstances|4xx->4xx|$.Message
Issue-Key: connect|ListInstances|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|DescribeInstance|4xx->4xx|$.Message
Issue-Key: connect|DescribeInstance|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: connect|DescribeInstance|4xx->4xx|status
Issue-Commit: 8b46b389ba873b3a08c5fc9877724606a6318c44

# connect instance plane: 5 net-new causes — ListInstances refuses a valid read, and instance refusals use an unmodeled ValidationException envelope with wrong status and wording

> _Note: this issue was identified against the code base at commit `8b46b389ba873b3a08c5fc9877724606a6318c44`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeInstance

## Description
Replay of the connect instance plane (ListInstances / DescribeInstance) diverges on FIVE distinct root causes. They share one fault family: the emulator's instance read path refuses a request AWS serves, and its refusal layer emits a generic framework error envelope (HTTP class, error code, and Message) instead of Amazon Connect's modeled InvalidRequestException / ResourceNotFoundException bodies. All five must be closed before this issue is resubmitted.

Cause 1 of 5: ListInstances refuses a valid enumeration with HTTP 400.
AWS: Smithy: ListInstances http GET /instance, success code 200; output ListInstancesResponse with InstanceSummaryList (list) and NextToken; modeled errors only InternalServiceException (500) and InvalidRequestException (400). Input members NextToken (httpQuery nextToken) and MaxResults (httpQuery maxResults, target MaxResult10, @range min 1 max 10) are both optional. A ListInstances call whose pagination inputs are inside those bounds is a success: AWS answers 200 and always serializes InstanceSummaryList, serializing it as an empty list when the account holds no instances.
Emulator: answers HTTP 400 and the body carries no InstanceSummaryList at all (expected [], got None).
Why: a client enumerating Connect instances — the first call of every console/CCP bootstrap flow — cannot list at all; it sees a client-fault refusal where AWS reports an empty account, so it cannot distinguish "no instances" from "bad request" and pagination never starts.
Fix: the ListInstances handler and its input validation in the connect service crate — the in-range pagination inputs must be accepted and the success path must always emit InstanceSummaryList (empty list when there is nothing to return).

Cause 2 of 5: an out-of-range MaxResults is refused with an unmodeled ValidationException carrying no Message.
AWS: Smithy: ListInstances.MaxResults targets MaxResult10 with @range min 1, max 10; the only modeled 400 is InvalidRequestException, whose Message member documents "The message about the request". Recorded response: HTTP 400 with Message "Invalid MaxResults. MaxResults should have minimum value of 1." The fixed text is "Invalid MaxResults. MaxResults should have minimum value of 1."; its variable parts are the member name (MaxResults) and the limit (the violated @range bound, 1), both taken from the request and the model — never from one recording's numbers.
Emulator: HTTP 400 but the body has no Message member at all (expected the sentence above, got None), and the response header x-amzn-errortype is "ValidationException" — an error shape Amazon Connect does not model on this operation.
Why: an SDK deserializes the error by x-amzn-errortype, so a client never receives InvalidRequestException and cannot match the modeled error; error-handling branches keyed on InvalidRequestException fall through to a generic failure, and with no Message the operator-facing text is empty, so the client cannot tell which parameter was rejected.
Fix: the connect error serializer plus the ListInstances pagination validation — raise the service's modeled InvalidRequestException with its Message populated and the matching x-amzn-errortype, not the framework-wide ValidationException.

Cause 3 of 5: the DescribeInstance not-found Message is emulator-authored and echoes the requested id.
AWS: Smithy: DescribeInstance http GET /instance/{InstanceId}, errors include ResourceNotFoundException (httpError 404) whose Message member is "The message about the resource". Recorded response for an instance id that resolves to nothing: HTTP 404 with Message "Resource not found" — that sentence is entirely fixed text with NO variable parts; it does not name the resource or quote the id.
Emulator: HTTP 404 (correct status) but Message "Instance <requested-instance-id> not found", a sentence the emulator composes from the request's InstanceId.
Why: the message is a requirement of the error class, so a client that asserts on, logs or routes by AWS's refusal text sees a message AWS never emits, and the id echo additionally leaks request input into a reply AWS keeps opaque.
Fix: the DescribeInstance handler's not-found path / the connect error message templates — emit the fixed "Resource not found" sentence with no interpolation.

Cause 4 of 5: a syntactically invalid instance id is refused with an empty-bodied ValidationException instead of InvalidRequestException "Instance id is not valid".
AWS: Smithy: DescribeInstance.InstanceId is @required, httpLabel, target InstanceId with @length min 1 max 100; the modeled 400 is InvalidRequestException. Recorded response for a malformed id: HTTP 400 with Message "Instance id is not valid" — fixed text with no variable parts (the rejected id is NOT quoted).
Emulator: HTTP 400 with no Message member (expected "Instance id is not valid", got None) and x-amzn-errortype "ValidationException", again a shape this service does not model.
Why: the client gets an unmodeled error code and an empty message, so it cannot distinguish a malformed-id rejection from any other validation failure, and the modeled InvalidRequestException branch of its generated code never runs.
Fix: DescribeInstance input validation plus the connect error serializer — classify a malformed InstanceId as InvalidRequestException with the fixed message and the correct x-amzn-errortype.

Cause 5 of 5: a blank InstanceId is resolved against state before it is validated, so the refusal has the wrong status class and the wrong error.
AWS: Smithy: DescribeInstance.InstanceId is @required with @length min 1, so a blank/whitespace label fails input validation before any lookup; the recorded response is HTTP 400 with Message "Invalid request" — fixed text, no variable parts.
Emulator: HTTP 404 with x-amzn-errortype "ResourceNotFoundException" and Message "Instance   not found" (the blank id interpolated into the not-found template), i.e. it performs the state lookup first and reports a missing resource.
Why: the fault is reclassified from a 400 client input error to a 404 missing resource. A caller retries or treats the instance as deleted instead of fixing its request, and status-class-driven logic (retry/alarm on 404 vs fail-fast on 400) takes the wrong branch.
Fix: validation ordering in the DescribeInstance handler — enforce the @required/@length constraints on the InstanceId path label before any state resolution, and return the modeled InvalidRequestException "Invalid request".

No open issue covers this surface: the existing connect-family issues concern other services/operations (connect-contact-lens ListRealtimeContactAnalysisSegments, connectcampaigns/connectcampaignsv2, connectcases, connecthealth) and the only open `connect` issue is the UpdateAttachedFilesConfiguration stub (20261030T060829Z-connect-updateattachedfilesconfiguration-stub.md), a different operation.

## Evidence
Replay of fixture connect-ccp-traj-connect-instance-detour (trace set simparity-wire-aws-v1), 8 steps: 1 passed, 7 mismatched, 5 divergence clusters, score 12.5%. None carried a stub_path or owned_by, so all five are net-new.

1) task#2 ListInstances — expected status 200, got 400; $.InstanceSummaryList missing (expected [], got None).
2) task#3 ListInstances — status 400/400 but $.Message missing (expected 'Invalid MaxResults. MaxResults should have minimum value of 1.', got None); $headers.x-amzn-errortype shape_mismatch (got 'ValidationException', a shape connect does not model).
3) task#4 (and task#5, same cluster) DescribeInstance — status 404/404 but $.Message value_mismatch: expected 'Resource not found', got 'Instance d6f2a9c1-5e3b-4a7d-9c21-8f4e6b2d1a70 not found' (the id is this run's value; the defect is the interpolated sentence).
4) task#6 DescribeInstance — status 400/400 but $.Message missing (expected 'Instance id is not valid', got None); $headers.x-amzn-errortype got 'ValidationException'.
5) task#7 DescribeInstance — expected status 400, got 404; $.Message value_mismatch (expected 'Invalid request', got 'Instance   not found'); $headers.x-amzn-errortype got 'ResourceNotFoundException'.

Model evidence (services/service-com-amazonaws-connect-amazonconnectservice/smithy-model): ListInstances GET /instance -> 200, output member InstanceSummaryList, MaxResults target MaxResult10 @range min 1 max 10, errors InternalServiceException(500)/InvalidRequestException(400). DescribeInstance GET /instance/{InstanceId}, InstanceId @required httpLabel @length min 1 max 100, errors InternalServiceException(500)/InvalidRequestException(400)/ResourceNotFoundException(404). Neither operation models ValidationException.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay wire fixture connect-ccp-traj-connect-instance-detour from trace set simparity-wire-aws-v1.
3. Step task#2: GET /instance (ListInstances) with in-range pagination inputs -> emulator answers 400 with no InstanceSummaryList; AWS answers 200 with InstanceSummaryList (empty).
4. Step task#3: GET /instance?maxResults=<below the @range min of 1> -> emulator answers 400 with an empty body and x-amzn-errortype ValidationException; AWS answers 400 InvalidRequestException with Message 'Invalid MaxResults. MaxResults should have minimum value of 1.'.
5. Steps task#4/task#5: GET /instance/{unknown-instance-id} -> emulator answers 404 with Message 'Instance <id> not found'; AWS answers 404 ResourceNotFoundException with Message 'Resource not found'.
6. Step task#6: GET /instance/{malformed-instance-id} -> emulator answers 400 with no Message and x-amzn-errortype ValidationException; AWS answers 400 InvalidRequestException with Message 'Instance id is not valid'.
7. Step task#7: GET /instance/{blank or whitespace InstanceId} -> emulator answers 404 ResourceNotFoundException 'Instance   not found'; AWS answers 400 InvalidRequestException 'Invalid request'.
