Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.streaming-url-rejected-until-fleet-running appstream.appblockbuilder-invalid-role-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.streaming-url-rejected-until-fleet-running|run#14|AssertionError|assert code == "OperationNotPermittedException", (
Issue-Key: appstream.appblockbuilder-invalid-role-rejected|_error#6|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# appstream: 2 net-new causes — CreateStreamingURL is a 501 scaffold stub so the not-RUNNING-fleet refusal is unreachable, and CreateAppBlockBuilder accepts a non-assumable IamRoleArn with HTTP 200 instead of InvalidRoleException

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStreamingURL

## Description
Two net-new root causes in the AppStream control plane, both grounded in failing behavioral checks of this suite (check ids under each cause). Overlap note: the open issues 20261029T194603Z-appstream-answers-501-notimplemented-where-aws-performs-a.md (its cause 2/cause 3) and 20261106T134718Z-appstream-app-block-builder-divergences-createappblockbuilder-accepts-a.md (its cause 1) describe neighbouring causes on these same two handlers; the replay report did NOT mark either of the checks below as OWNED by them, so they are restated here with the behavior the handlers must carry and the replay keys a fixer must re-run. A fixer should expect the fixes to meet in the same two handler files.

Cause 1 of 2: CreateStreamingURL answers HTTP 501 NotImplemented for every request, so neither the not-RUNNING-fleet refusal nor the successful URL issuance exists.
AWS: Smithy (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): CreateStreamingURL{StackName required (len 1..), FleetName required (len 1..), UserId required (StreamingUrlUserId len 2..32, pattern ^[\w+=,.@-]*$), ApplicationId, Validity, SessionContext} -> {StreamingURL (len 1..), Expires (Timestamp)}; modeled errors InvalidParameterCombinationException (400), OperationNotPermittedException (400, documented "The attempted operation is not permitted."), ResourceNotAvailableException (400), ResourceNotFoundException (404). Recorded behavior for the state gate: when the fleet associated with the stack is STOPPED, the call is refused with HTTP 400, error code OperationNotPermittedException and message "The fleet <FleetName> is not running. The stack must have a running fleet to get a streaming URL" — fixed text is "The fleet " + " is not running. The stack must have a running fleet to get a streaming URL" (no trailing period); the only variable part is <FleetName>, echoed from the request. The refusal is effect-free: DescribeSessions(StackName,FleetName,UserId,AuthenticationType=API) then returns an empty Sessions list. Positive control after StartFleet reaches RUNNING: HTTP 200 with a non-empty StreamingURL and an Expires timestamp strictly in the future.
Emulator: every call fails with error code NotImplemented, HTTP 501, message "operation CreateStreamingURL is scaffolded but not implemented in this emulator". 501/NotImplemented is not one of the operation's modeled errors, so no client's error handling can match it.
Why: a client cannot obtain a streaming URL at all, and a client that distinguishes "fleet not yet running, wait and retry" (400 OperationNotPermittedException) from a server fault reads the 501 as the emulator being broken or retries forever. The state gate that keeps a URL from being minted against a stopped fleet is absent, so once the stub is filled in without it a caller would receive a URL AWS would never issue.
Fix: the CreateStreamingURL handler (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/create_streaming_url.rs) — resolve the stack and fleet, require the fleet/stack association, gate on the fleet's state returning OperationNotPermittedException with AWS's sentence while it is not RUNNING, and on the RUNNING path return StreamingURL plus an Expires derived from Validity (default 60s) while registering the session so DescribeSessions can see it (and nothing at all on the refused path). Delete the SCAFFOLDED marker line once implemented.
Traces: appstream.streaming-url-rejected-until-fleet-running.

Cause 2 of 2: CreateAppBlockBuilder does not gate IamRoleArn on assumability — a role that does not exist is accepted with HTTP 200.
AWS: Smithy: CreateAppBlockBuilder.IamRoleArn targets Arn (pattern ^arn:aws(?:-cn|-iso-b|-iso|-us-gov)?:...$) and the operation models InvalidRoleException (httpError 400). A syntactically valid ARN satisfies the modeled pattern, so assumability is a handler gate. Recorded behavior: an IamRoleArn naming a role AppStream cannot assume is refused with HTTP 400, error code InvalidRoleException, message "WorkSpaces Applications encountered an error because your IAM role '<IamRoleArn>' is invalid" — fixed text "WorkSpaces Applications encountered an error because your IAM role '" + "' is invalid" (ARN in single quotes, no trailing period); the only variable part is <IamRoleArn> echoed from the request. The refusal creates nothing, so DescribeAppBlockBuilders(Names=[<Name>]) then answers HTTP 404 ResourceNotFoundException with "The appblockbuilder <Name> for account <accountId> does not exist." (fixed text; <Name> from the request, <accountId> the caller's account). Positive control: the same request without the bogus role succeeds.
Emulator: the call SUCCEEDS with HTTP 200 and returns the AppBlockBuilder (State STOPPED) echoing the non-existent IamRoleArn, and the builder is persisted and describable — the handler performs no role resolution at all.
Why: a client that provisions a builder with a mistyped or not-yet-created role is told the create succeeded and ends up with a builder that can never stream; worse, callers (and this suite's own setup loops) retry CreateAppBlockBuilder on InvalidRoleException while an IAM role propagates, and an unconditional 200 turns a misconfiguration into a silently broken resource with no negative-class signal.
Fix: the CreateAppBlockBuilder handler (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/create_app_block_builder.rs) — add the IamRoleArn assumability gate before any state is written, returning InvalidRoleException with AWS's sentence, and ensure a refused request persists nothing so the following DescribeAppBlockBuilders answers ResourceNotFoundException.
Traces: appstream.appblockbuilder-invalid-role-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator; per-service rate appstream 9/24 (38%), 15 failed.

Cause 1 — [mismatch] appstream.streaming-url-rejected-until-fleet-running:
  setup: CreateStack(S); CreateFleet(F, FleetType=ELASTIC, Platform=WINDOWS_SERVER_2019, InstanceType=stream.standard.small, MaxConcurrentSessions=5, VpcConfig, IamRoleArn); AssociateFleet(F,S); F still STOPPED.
  call: create_streaming_url(StackName=S, FleetName=F, UserId='sim2real-streamuser1')
  expected: HTTP 400 ('OperationNotPermittedException', "The fleet <F> is not running. The stack must have a running fleet to get a streaming URL"), then DescribeSessions -> Sessions == []; after StartFleet/RUNNING, HTTP 200 with a non-empty StreamingURL and Expires > now.
  got: AssertionError: CreateStreamingURL against a STOPPED fleet must be rejected with OperationNotPermittedException; got 'NotImplemented'
  (HTTP 501, message "operation CreateStreamingURL is scaffolded but not implemented in this emulator")
  Checkout evidence: services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/create_streaming_url.rs carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (line 4) and its handle() body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("CreateStreamingURL"))` (line 173). The report did not mark this check ALREADY FILED for that stub.

Cause 2 — [mismatch] appstream.appblockbuilder-invalid-role-rejected:
  call: create_app_block_builder(Name=<new>, Platform='WINDOWS_SERVER_2019', InstanceType='stream.standard.small', VpcConfig={SubnetIds:[s1,s2]}, IamRoleArn='arn:aws:iam::<account>:role/<nonexistent-role>')
  expected: HTTP 400 ('InvalidRoleException', "WorkSpaces Applications encountered an error because your IAM role '<arn>' is invalid"), then DescribeAppBlockBuilders(Names=[<new>]) -> HTTP 404 ResourceNotFoundException "The appblockbuilder <new> for account <accountId> does not exist."
  got: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned {'AppBlockBuilder': {'Arn': 'arn:aws:appstream:<region>:<account>:app-block-builder/<name>', 'Name': '<name>', 'Platform': 'WINDOWS_SERVER_2019', 'InstanceType': 'stream.standard.small', 'IamRoleArn': 'arn:aws:iam::<account>:role/<nonexistent-role>', 'VpcConfig': {...}, 'State': 'STOPPED', 'CreatedTime': ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Smithy evidence (same model file): CreateStreamingURL errors {InvalidParameterCombinationException 400, OperationNotPermittedException 400, ResourceNotAvailableException 400, ResourceNotFoundException 404}, output {StreamingURL, Expires}; CreateAppBlockBuilder.IamRoleArn -> Arn with the arn pattern, errors include InvalidRoleException (400) and ResourceNotFoundException (404).

ARNs, account ids, region, builder/fleet/stack names and timestamps above are per-run values, not requirements; the requirements are the HTTP status, the error code and the message templates.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Cause 1: appstream CreateStack(Name=S); CreateFleet(Name=F, FleetType=ELASTIC, Platform=WINDOWS_SERVER_2019, InstanceType=stream.standard.small, MaxConcurrentSessions=5, VpcConfig={SubnetIds:[two subnets]}, IamRoleArn=<AppStreamServiceAccess role>); AssociateFleet(FleetName=F, StackName=S). With F STOPPED, call CreateStreamingURL(StackName=S, FleetName=F, UserId='sim2real-streamuser1'): observe HTTP 501 NotImplemented instead of HTTP 400 OperationNotPermittedException "The fleet F is not running. The stack must have a running fleet to get a streaming URL". Then StartFleet(F), wait for RUNNING, and call CreateStreamingURL again: observe 501 instead of HTTP 200 with a non-empty StreamingURL and a future Expires.
3. Cause 2: create a VPC with two subnets, then CreateAppBlockBuilder(Name=B, Platform=WINDOWS_SERVER_2019, InstanceType=stream.standard.small, VpcConfig={SubnetIds:[s1,s2]}, IamRoleArn=arn:aws:iam::<account>:role/does-not-exist): observe HTTP 200 with the builder persisted, instead of HTTP 400 InvalidRoleException "WorkSpaces Applications encountered an error because your IAM role '<arn>' is invalid". Then DescribeAppBlockBuilders(Names=[B]): the emulator describes the builder, where AWS answers 404 ResourceNotFoundException "The appblockbuilder B for account <accountId> does not exist."
4. Or replay the two checks named in trace_ids.
