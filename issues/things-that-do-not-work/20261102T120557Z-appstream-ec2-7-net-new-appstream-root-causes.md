Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key appstream__ec2.asvpce-stack-tags-survive-access-endpoint-detach
Issue-Key: appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected|_error_code#13|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected|_error_tuple#12|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected|_error_code#15|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected|_error#12|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected|run#11|AssertionError|assert got == ("InvalidParameterCombinationException", expected_message), (
Issue-Key: appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected|run#18|AssertionError|assert normalized == expected_message, (
Issue-Key: appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key|run#15|AssertionError|assert {k: perms.get(k) for k in sorted(enabled)} == {k: "ENABLED" for k in sorted(enabled)}, (
Issue-Key: appstream__ec2|ListTagsForResource|NotImplemented
Issue-Commit: ef6e0f5e66c899ede0df1e04050c78d39daac904

# appstream__ec2: 7 net-new AppStream root causes — CreateStack/UpdateStack never validate AccessEndpoints (duplicate type, unknown and wrong-service VpceId all accepted 200), two refusal messages are emulator-authored, the Stack projection omits server-side defaults, and ListTagsForResource answers 501

> _Note: this issue was identified against the code base at commit `ef6e0f5e66c899ede0df1e04050c78d39daac904`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Seven net-new root causes, all in the AppStream stack write/read paths exercised by the access-endpoint (asvpce) checks. Four are requests AWS REFUSES that the emulator accepts with HTTP 200, two are refusal messages the emulator authors itself, one is a read projection missing AWS's server-side defaults, and one operation is a 501 stub. Prior art: the open wire issues 20261102T092115Z-appstream-createstack-updatestack-request-validation-diverges-in-class.md and 20261102T092115Z-appstream-stack-projection-omits-server-side-defaults-and.md describe neighbouring causes on the same two code paths (an unknown VpceId refused as ResourceAlreadyExistsException at CreateStack, the '0 items' list rendering, and the missing Stack defaults). Neither covers the causes below, which are grounded in behavioral checks and in several cases show a DIFFERENT outcome (plain HTTP 200, no refusal at all) and a different operation (UpdateStack); a fixer should expect the fixes to meet in the same validation layer and Stack serializer.

Cause 1 of 7: CreateStack accepts an AccessEndpoints list containing two entries of the same EndpointType.
AWS: Smithy: CreateStack.AccessEndpoints -> AccessEndpointList (@length min 1 max 4) of AccessEndpoint{EndpointType required enum AccessEndpointType, VpceId String @length min 1}; CreateStack's modeled errors include InvalidParameterCombinationException (httpError 400). Each endpoint type may appear at most once, and AWS refuses a repeat rather than de-duplicating it or storing both: HTTP 400, code InvalidParameterCombinationException, message "You specified duplicate VPC endpoint types in the request. Specify <EndpointType> once, and try again." — the two sentences are fixed text; the only variable part is the repeated EndpointType value taken from the request (STREAMING in the recording). Nothing is created: DescribeStacks on the name then refuses ResourceNotFoundException with "The stack <name> for account <account> does not exist." (fixed text; <name> from the request, <account> from the caller's identity).
Emulator: the CreateStack call SUCCEEDS (replay detail: "the call SUCCEEDED; real AWS rejects it, so there is no error code"), so no duplicate-type check exists on the create path.
Why: a client that accidentally emits two STREAMING entries (a common templating/merge bug) gets a stack AWS would never have created, holding an endpoint list that is invalid for the service; the failure surfaces only when the configuration is promoted to real AWS, and any reconcile loop comparing desired to actual will keep re-applying a shape the real API rejects.
Fix: CreateStack's request validation in services/service-com-amazonaws-appstream-photonadminproxyservice — reject a repeated AccessEndpoints EndpointType with InvalidParameterCombinationException before any state is written.
Traces: appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected.

Cause 2 of 7: CreateStack accepts an AccessEndpoints VpceId that resolves to no VPC endpoint, and one that resolves to an endpoint of ANOTHER service.
AWS: recorded behavior only for the error class (Smithy lists InvalidParameterCombinationException/InvalidRoleException/ResourceAlreadyExistsException/ResourceNotFoundException for CreateStack, not InvalidParameterValueException). AppStream resolves AccessEndpoint.VpceId against the caller's EC2 VPC endpoints at create time and refuses two distinct cases with HTTP 400 code InvalidParameterValueException: (a) an id that exists nowhere — message "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again.", fixed text with the rejected id interpolated from the request; (b) an id that exists but whose ServiceName is not the AppStream streaming service — message "You specified a VPC endpoint that is not valid for AppStream 2.0.  Check that the endpoint you specify corresponds to the AppStream 2.0 service, and try again." (note the two spaces after the first sentence), entirely fixed text. In both cases nothing is created and DescribeStacks on the name refuses ResourceNotFoundException with "The stack <name> for account <account> does not exist."
Emulator: both CreateStack calls SUCCEED (replay detail on each: "the call SUCCEEDED; real AWS rejects it, so there is no error code") — the VpceId is stored unchecked, neither resolved for existence nor checked for its owning service.
Why: the stack's whole purpose is to pin streaming to one interface endpoint. Accepting an unresolvable or wrong-service id yields a stack that can never serve a session, and the client gets a success it will act on (hand the stack to users, mark the deployment healthy) instead of the immediate, actionable parameter refusal AWS gives. A typo'd or stale endpoint id is silently promoted into state.
Fix: CreateStack's AccessEndpoints validation — resolve each VpceId against EC2's VPC endpoints for the account/region, and require the endpoint's service to be the AppStream streaming service, raising InvalidParameterValueException with the respective sentence before writing state.
Traces: appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected, appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected.

Cause 3 of 7: UpdateStack accepts an AccessEndpoints VpceId that resolves to no VPC endpoint, and overwrites the stack's valid endpoint with it.
AWS: recorded behavior only for the class, as in cause 2; UpdateStack validates the VpceId on its own code path and refuses an unknown id with HTTP 400 InvalidParameterValueException and the identical sentence "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again." (fixed text, rejected id from the request). The refusal is also effect-free: a subsequent DescribeStacks still reports the previously attached, valid endpoint.
Emulator: the UpdateStack call SUCCEEDS (replay detail: "the call SUCCEEDED; real AWS rejects it, so there is no error"), so the update path performs no endpoint resolution at all.
Why: this is the destructive form of cause 2 — an existing, working stack has its valid access endpoint replaced by a non-existent one and the caller is told the update succeeded. A client has no signal that private connectivity was just broken, and the stored endpoint is unrecoverable from the API response.
Fix: UpdateStack's AccessEndpoints validation (the separate update code path) — same resolution and refusal as CreateStack, applied before the stack record is mutated so a refused update leaves the stored endpoints untouched.
Traces: appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected.

Cause 4 of 7: the UpdateStack set-and-delete conflict is refused with an emulator-authored message.
AWS: Smithy: UpdateStack takes both AccessEndpoints (AccessEndpointList) and AttributesToDelete (StackAttributes, list of the StackAttribute enum, which includes ACCESS_ENDPOINTS), and lists InvalidParameterCombinationException (httpError 400) among its errors. Setting AccessEndpoints while AttributesToDelete contains ACCESS_ENDPOINTS in one call is refused HTTP 400 InvalidParameterCombinationException with the message "You can’t update and delete vpc endpoints at the same time." — the entire sentence is fixed text, with a U+2019 right single quotation mark in "can’t" and no variable part at all. The refusal leaves the stored endpoints exactly as they were, and the delete on its own is accepted.
Emulator: the correct status and code, but the message is its own prose: "AccessEndpoints cannot be specified while AttributesToDelete contains ACCESS_ENDPOINTS" (replay detail: got ('InvalidParameterCombinationException', 'AccessEndpoints cannot be specified while AttributesToDelete contains ACCESS_ENDPOINTS')).
Why: the message is part of the contract. Clients, test suites and runbooks that match AWS's sentence (or surface it to operators) see text AWS never emits, so error-text assertions and log-based alerting that pass against AWS fail here.
Fix: the UpdateStack handler's parameter-combination refusal — emit AWS's sentence verbatim, including the U+2019 apostrophe.
Traces: appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected.

Cause 5 of 7: the list-length ValidationException message renders the rejected list as an item count instead of the list value.
AWS: Smithy: CreateStack.AccessEndpoints -> AccessEndpointList @length min 1 max 4. A five-entry list is refused BEFORE the duplicate-type check, HTTP 400, code ValidationException, message "1 validation error detected: Value '[<entry>, <entry>, <entry>, <entry>, <entry>]' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4" — fixed text apart from (a) the rendered rejected VALUE, a bracketed comma-separated list with one rendering per submitted entry (on AWS each entry renders as com.amazonaws.photon.AccessEndpoint@<object-hash>; the hash is per-run and NOT a requirement, the per-entry rendering and the entry count are), (b) the member path 'accessEndpoints' from the request, and (c) the limit 4 from the model's @length max. The refused call creates nothing: DescribeStacks then refuses ResourceNotFoundException with "The stack <name> for account <account> does not exist."
Emulator: same status and class, but the value is rendered by cardinality: "1 validation error detected: Value '5 items' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4".
Why: AWS never produces a "<n> items" value rendering, so text-matching clients and operators reading the error see a form that does not exist on AWS; the generic form shows the shared validator renders every rejected collection by size rather than by value, so the divergence reappears on every list constraint in the service. (The open wire issue ...createstack-updatestack-request-validation... records the same renderer producing '0 items' for the @length min 1 case; this is its max-length counterpart.)
Fix: the shared validation-message renderer for list-length violations in the service's validation layer — render the rejected collection value, not its size.
Traces: appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected.

Cause 6 of 7: a created stack carries none of AWS's server-side defaults, so DescribeStacks reports an empty UserSettings.
AWS: Smithy: Stack.UserSettings -> UserSettingList (@length min 1) of UserSetting{Action required enum, Permission required enum, MaximumLength}, documented "The actions that are enabled or disabled for users during their streaming sessions. By default these actions are enabled."; Stack.ApplicationSettings -> ApplicationSettingsResponse; Stack.StreamingExperienceSettings -> StreamingExperienceSettings{PreferredProtocol}; Stack.StackErrors -> StackErrors. A stack created without specifying them is read back with the defaults materialized: UserSettings has CLIPBOARD_COPY_FROM_LOCAL_DEVICE, CLIPBOARD_COPY_TO_LOCAL_DEVICE, FILE_UPLOAD, FILE_DOWNLOAD and PRINTING_TO_LOCAL_DEVICE = ENABLED and DOMAIN_SMART_CARD_SIGNIN = DISABLED; ApplicationSettings = {Enabled: false}; StreamingExperienceSettings = {PreferredProtocol: 'TCP'}; StackErrors = []. These are identical whether or not an access endpoint is attached.
Emulator: DescribeStacks reports no UserSettings entries at all (replay detail: "the bare stack must default the clipboard/file/print actions to ENABLED; UserSettings reports {}"), i.e. the Stack record is built from the request only and unspecified members are left empty.
Why: a client reading back the EFFECTIVE configuration it did not specify — which clipboard/file-transfer/print actions users may perform, whether application settings persist, which streaming protocol is preferred — gets nothing and cannot distinguish "default" from "disabled"; policy auditors that assert DOMAIN_SMART_CARD_SIGNIN is DISABLED, or that clipboard is ENABLED, find no data to assert on.
Fix: the CreateStack handler's stack-record construction (apply the service defaults at create time) and the shared Stack projection used by CreateStack/UpdateStack/DescribeStacks. Overlaps the open wire issue 20261102T092115Z-appstream-stack-projection-omits-server-side-defaults-and.md (same members, wire corpus); this cause is grounded in a behavioral read of a stack created with NO optional members.
Traces: appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key.

Cause 7 of 7: ListTagsForResource is an unimplemented scaffold stub answering 501 NotImplemented.
AWS: Smithy: ListTagsForResource{ResourceArn required (Arn, @pattern ^arn:aws...)} -> ListTagsForResourceResponse{Tags: Tags map @length 1..50}, error ResourceNotFoundException (httpError 404). For a stack ARN it answers HTTP 200 with exactly the tag set in effect: the create-time Tags round-trip unchanged, an UpdateStack that detaches ACCESS_ENDPOINTS does not touch them, and a later TagResource MERGES into that set rather than replacing it.
Emulator: every call fails with "An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator" (HTTP 501).
Why: tagging is unobservable — no client can read a resource's tags, so cost-allocation, ownership and policy tooling built on AppStream tags cannot run at all, and TagResource's merge semantics cannot be verified even once TagResource itself is implemented.
Fix: implement the ListTagsForResource handler in services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/list_tags_for_resource.rs, reading the tag store keyed on the resource ARN (separate from the resource record, so stack updates do not disturb it). Note: open issue 20261029T194603Z-appstream-tagresource-stub.md covers the sibling TagResource stub only; no open issue names ListTagsForResource.
Traces: appstream__ec2.asvpce-stack-tags-survive-access-endpoint-detach.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator; per-service rate appstream__ec2 6/17 (35%), 11 failed.

Cause 1 — appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected [mismatch]:
  call: create_stack(Name=<new>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:<vpce>}, {EndpointType:'STREAMING',VpceId:<same vpce>}])
  expected: HTTP 400 ('InvalidParameterCombinationException', "You specified duplicate VPC endpoint types in the request. Specify STREAMING once, and try again.") and the stack NOT created
  got:      HTTP 200 — AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code
  (control: a one-entry list is accepted and DescribeStacks lists exactly that entry — that part passed.)

Cause 2 — appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected [mismatch]:
  call: create_stack(Name=<new>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:'vpce-00000000000000000'}])
  expected: HTTP 400 ('InvalidParameterValueException', "The VPC endpoint vpce-00000000000000000 is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again.") — the id is the one the request supplied
  got:      HTTP 200 — AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code
  appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected [mismatch]:
  call: create_stack(Name=<new>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:<a real Interface endpoint for com.amazonaws.<region>.sts>}])
  expected: HTTP 400 ('InvalidParameterValueException', "You specified a VPC endpoint that is not valid for AppStream 2.0.  Check that the endpoint you specify corresponds to the AppStream 2.0 service, and try again.")
  got:      HTTP 200 — AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code

Cause 3 — appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected [mismatch]:
  setup: a stack whose AccessEndpoints is [{STREAMING, <real vpce>}] (the control update attached it successfully)
  call: update_stack(Name=<stack>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:'vpce-00000000000000000'}])
  expected: HTTP 400 ('InvalidParameterValueException', "The VPC endpoint vpce-00000000000000000 is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again."), stack still lists the real endpoint
  got:      HTTP 200 — AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error

Cause 4 — appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected [mismatch]:
  call: update_stack(Name=<stack>, AccessEndpoints=[{STREAMING,<vpce>}], AttributesToDelete=['ACCESS_ENDPOINTS'])
  expected: ('InvalidParameterCombinationException', 'You can’t update and delete vpc endpoints at the same time.')
  got:      ('InvalidParameterCombinationException', 'AccessEndpoints cannot be specified while AttributesToDelete contains ACCESS_ENDPOINTS')
  status and code match; only the message diverges.

Cause 5 — appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected [mismatch]:
  call: create_stack(Name=<new>, AccessEndpoints=[five identical {STREAMING,<vpce>} entries])
  code: ValidationException on both sides (that assertion passed)
  expected message (per-entry object hashes masked to <entry>): "1 validation error detected: Value '[<entry>, <entry>, <entry>, <entry>, <entry>]' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4"
  got:                                                          "1 validation error detected: Value '5 items' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4"

Cause 6 — appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key [mismatch]:
  setup: create_stack(Name=<bare>) with no optional members, beside a sibling created with an access endpoint
  the 'AccessEndpoints key absent on the bare stack' assertion PASSED; the next one failed:
  expected: UserSettings contains CLIPBOARD_COPY_FROM_LOCAL_DEVICE/CLIPBOARD_COPY_TO_LOCAL_DEVICE/FILE_UPLOAD/FILE_DOWNLOAD/PRINTING_TO_LOCAL_DEVICE = ENABLED (then DOMAIN_SMART_CARD_SIGNIN = DISABLED, ApplicationSettings = {'Enabled': False}, StreamingExperienceSettings = {'PreferredProtocol': 'TCP'}, StackErrors = [])
  got:      AssertionError: the bare stack must default the clipboard/file/print actions to ENABLED; UserSettings reports {}

Cause 7 — appstream__ec2.asvpce-stack-tags-survive-access-endpoint-detach [error]:
  call: list_tags_for_resource(ResourceArn=<stack arn>)
  got:  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListTagsForResource operation: operation ListTagsForResource is scaffolded but not implemented in this emulator
  expected: HTTP 200 {'Tags': <the create-time tag map>}, unchanged across an UpdateStack(AttributesToDelete=['ACCESS_ENDPOINTS']), and merged (not replaced) by a later TagResource.

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): CreateStack.AccessEndpoints -> AccessEndpointList @length min 1 max 4; AccessEndpoint{EndpointType required enum, VpceId String @length min 1}; CreateStack errors {ConcurrentModificationException, InvalidAccountStatusException, InvalidParameterCombinationException, InvalidRoleException, LimitExceededException, OperationNotPermittedException, ResourceAlreadyExistsException, ResourceNotFoundException(404)}; UpdateStack takes AccessEndpoints + AttributesToDelete -> StackAttributes and lists InvalidParameterCombinationException; Stack{UserSettings -> UserSettingList @length min 1 (doc: "By default these actions are enabled"), ApplicationSettings -> ApplicationSettingsResponse, StreamingExperienceSettings, StackErrors, AccessEndpoints}; ListTagsForResource{ResourceArn required} -> {Tags map @length 1..50}, error ResourceNotFoundException(404). InvalidParameterValueException is not in CreateStack/UpdateStack's modeled error list — that class and its sentences are grounded in the recordings.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Run the behavioral suite (simparity-behavioral-aws-v1) and replay the seven checks named in trace_ids.
3. Setup common to all of them: create an EC2 VPC + subnet, an Interface VPC endpoint for com.amazonaws.<region>.appstream.streaming, and the AppStream service-linked onboarding role.
4. Cause 1: appstream CreateStack with AccessEndpoints = two entries both EndpointType=STREAMING; observe HTTP 200 instead of 400 InvalidParameterCombinationException with the duplicate-type sentence.
5. Cause 2: CreateStack with AccessEndpoints VpceId='vpce-00000000000000000'; then CreateStack with a VpceId naming a real endpoint for com.amazonaws.<region>.sts. Observe HTTP 200 on both instead of 400 InvalidParameterValueException with, respectively, the "The VPC endpoint <id> is not valid..." and "You specified a VPC endpoint that is not valid for AppStream 2.0.  ..." sentences.
6. Cause 3: attach a real endpoint with UpdateStack, then UpdateStack the same stack with VpceId='vpce-00000000000000000'; observe HTTP 200 and the bogus id stored, instead of the InvalidParameterValueException refusal that leaves the previous endpoint in place.
7. Cause 4: UpdateStack with both AccessEndpoints=[...] and AttributesToDelete=['ACCESS_ENDPOINTS']; compare the returned message with 'You can’t update and delete vpc endpoints at the same time.'
8. Cause 5: CreateStack with a five-entry AccessEndpoints list; inspect the ValidationException message's rendered value ('5 items' vs the bracketed per-entry list).
9. Cause 6: CreateStack with only a Name, then DescribeStacks on it; observe UserSettings/ApplicationSettings/StreamingExperienceSettings/StackErrors are not populated with AWS's defaults.
10. Cause 7: CreateStack with Tags, then ListTagsForResource on the returned Arn; observe HTTP 501 NotImplemented.
