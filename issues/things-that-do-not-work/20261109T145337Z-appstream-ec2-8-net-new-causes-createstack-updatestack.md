Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected appstream__ec2.asvpce-deleted-vpce-lingers-on-stack-and-reattach-rejected appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key
Issue-Key: appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected|_error_code#12|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected|_error_tuple#11|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected|_error_code#14|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error code")
Issue-Key: appstream__ec2.asvpce-deleted-vpce-lingers-on-stack-and-reattach-rejected|_error#15|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected|_error#11|AssertionError|raise AssertionError("the call SUCCEEDED; real AWS rejects it, so there is no error")
Issue-Key: appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected|run#10|AssertionError|assert got == ("InvalidParameterCombinationException", expected_message), (
Issue-Key: appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected|run#17|AssertionError|assert normalized == expected_message, (
Issue-Key: appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key|run#14|AssertionError|assert {k: perms.get(k) for k in sorted(enabled)} == {k: "ENABLED" for k in sorted(enabled)}, (
Issue-Commit: c9a86881ef96f093104e2802f866de6afbf03a24

# appstream__ec2: 8 net-new causes — CreateStack/UpdateStack never validate AccessEndpoints (duplicate type, unknown, deleted and wrong-service VpceId all accepted HTTP 200), two refusal messages are emulator-authored, and the Stack projection omits AWS's server-side defaults

> _Note: this issue was identified against the code base at commit `c9a86881ef96f093104e2802f866de6afbf03a24`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Eight net-new root causes in the AppStream stack write/read paths exercised by the access-endpoint (asvpce) checks. Four are requests AWS REFUSES that the emulator accepts with HTTP 200, two are refusal messages the emulator authors itself, and one is a read projection missing AWS's server-side defaults. Overlap note: the open issues 20261102T120557Z-appstream-ec2-7-net-new-appstream-root-causes.md, 20261105T083215Z-appstream-ec2-appstream-createstack-accepts-a-streaming-accessendpoint-naming-a.md, 20261102T092115Z-appstream-createstack-updatestack-request-validation-diverges-in-class.md, 20261102T092115Z-appstream-stack-projection-omits-server-side-defaults-and.md and 20261108T212007Z-appstream-stack-write-path-diverges-a-retried-createstack.md describe neighbouring causes on these same code paths; the replay report marked NONE of the checks below as OWNED by them, so each cause is restated here with the behavior the handler must carry and the replay key a fixer must re-run. All eight fixes meet in the same AccessEndpoints validation layer and Stack serializer.

Cause 1 of 8: CreateStack accepts an AccessEndpoints list containing two entries of the same EndpointType.
AWS: Smithy: CreateStack.AccessEndpoints -> AccessEndpointList (@length min 1 max 4) of AccessEndpoint{EndpointType required enum AccessEndpointType, VpceId String}; CreateStack models InvalidParameterCombinationException (httpError 400). Each endpoint type may appear at most once and AWS refuses a repeat rather than de-duplicating it: HTTP 400, code InvalidParameterCombinationException, message "You specified duplicate VPC endpoint types in the request. Specify <EndpointType> once, and try again." — both sentences are fixed text; the only variable part is the repeated EndpointType value taken from the request. Nothing is created: DescribeStacks on the name then refuses ResourceNotFoundException "The stack <name> for account <account> does not exist." (fixed text; <name> from the request, <account> the caller's account).
Emulator: the CreateStack call SUCCEEDS (replay detail "the call SUCCEEDED; real AWS rejects it, so there is no error code") — no duplicate-type check exists on the create path.
Why: a client that emits two STREAMING entries (a common templating/merge bug) gets a stack AWS would never have created, holding an endpoint list invalid for the service; the failure surfaces only on promotion to real AWS, and any reconcile loop keeps re-applying a shape the real API rejects.
Fix: CreateStack's request validation in services/service-com-amazonaws-appstream-photonadminproxyservice — reject a repeated AccessEndpoints EndpointType with InvalidParameterCombinationException before any state is written.
Traces: appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected.

Cause 2 of 8: CreateStack accepts an AccessEndpoints VpceId that resolves to no VPC endpoint.
AWS: recorded behavior only for the class (CreateStack's modeled errors are ConcurrentModificationException/InvalidAccountStatusException/InvalidParameterCombinationException/InvalidRoleException/LimitExceededException/OperationNotPermittedException/ResourceAlreadyExistsException/ResourceNotFoundException — InvalidParameterValueException is not modeled for it). AppStream resolves AccessEndpoint.VpceId against the caller's EC2 VPC endpoints at create time and refuses an id that resolves nowhere with HTTP 400, code InvalidParameterValueException, message "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again." — fixed text with the rejected id interpolated from the request. Nothing is created; DescribeStacks then refuses ResourceNotFoundException with "The stack <name> for account <account> does not exist."
Emulator: the call SUCCEEDS (replay detail "the call SUCCEEDED; real AWS rejects it, so there is no error code") — the VpceId is stored unresolved.
Why: the stack's purpose is to pin streaming to one interface endpoint. Accepting an unresolvable id yields a stack that can never serve a session while the client is told it succeeded, so a typo'd or stale endpoint id is silently promoted into state instead of drawing an immediate parameter refusal.
Fix: CreateStack's AccessEndpoints validation — resolve each VpceId against EC2's VPC endpoints for the account/region and raise InvalidParameterValueException with that sentence before writing state.
Traces: appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected.

Cause 3 of 8: CreateStack accepts an AccessEndpoints VpceId naming an endpoint of ANOTHER service.
AWS: recorded behavior only, as in cause 2. An id that exists but whose ServiceName is not the AppStream streaming service is refused HTTP 400, code InvalidParameterValueException, message "You specified a VPC endpoint that is not valid for AppStream 2.0.  Check that the endpoint you specify corresponds to the AppStream 2.0 service, and try again." — entirely fixed text, note the TWO spaces after the first sentence. Nothing is created.
Emulator: the call SUCCEEDS (replay detail "the call SUCCEEDED; real AWS rejects it, so there is no error code") — the endpoint's owning service is never checked.
Why: a stack pinned to, say, an STS interface endpoint can never stream; the client gets a success it acts on (hands the stack to users, marks the deployment healthy) rather than the actionable refusal AWS gives.
Fix: CreateStack's AccessEndpoints validation — require the resolved endpoint's service to be the AppStream streaming service, refusing otherwise with that sentence.
Traces: appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected.

Cause 4 of 8: CreateStack accepts an AccessEndpoints VpceId naming a DELETED VPC endpoint.
AWS: recorded behavior only, as in cause 2. Deleting the endpoint does NOT detach it from the stack that already references it (DescribeStacks keeps reporting the stale entry), but a NEW CreateStack naming that now-unresolvable id is refused HTTP 400, code InvalidParameterValueException with the same cause-2 sentence "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again." (fixed text, id from the request); the refused stack is not created, so DescribeStacks on it answers ResourceNotFoundException "The stack <name> for account <account> does not exist." The repair path — UpdateStack with a freshly created live endpoint — then succeeds and replaces the stale entry.
Emulator: the CreateStack naming the deleted endpoint SUCCEEDS (replay detail "the call SUCCEEDED; real AWS rejects it, so there is no error"), i.e. liveness is re-validated nowhere: a well-formed id is enough.
Why: this is the lifecycle form of cause 2 — an endpoint id that was valid yesterday is accepted today, so a redeploy from a stored template silently produces unusable stacks after the endpoint has been torn down, and the caller has no signal that private connectivity is dead.
Fix: the same AccessEndpoints resolution on CreateStack — resolve against the CURRENT EC2 endpoint state (an id-targeted lookup, not merely a syntactic check) so a deleted endpoint is refused.
Traces: appstream__ec2.asvpce-deleted-vpce-lingers-on-stack-and-reattach-rejected.

Cause 5 of 8: UpdateStack accepts an AccessEndpoints VpceId that resolves to no VPC endpoint, and overwrites the stack's valid endpoint with it.
AWS: recorded behavior only for the class; UpdateStack (which takes the same AccessEndpoints -> AccessEndpointList @length 1..4 and models InvalidParameterCombinationException/ResourceNotFoundException among others) validates the VpceId on its own code path and refuses an unknown id with HTTP 400 InvalidParameterValueException and the identical sentence "The VPC endpoint <vpce-id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again." The refusal is effect-free: a subsequent DescribeStacks still reports the previously attached, valid endpoint.
Emulator: the UpdateStack call SUCCEEDS (replay detail "the call SUCCEEDED; real AWS rejects it, so there is no error") — the update path performs no endpoint resolution at all.
Why: the destructive form of cause 2 — an existing, working stack has its valid access endpoint replaced by a non-existent one and the caller is told the update succeeded; private connectivity is broken with no signal and the previous endpoint is unrecoverable from the API response.
Fix: UpdateStack's AccessEndpoints validation (the separate update code path) — same resolution and refusal as CreateStack, applied before the stack record is mutated so a refused update leaves the stored endpoints untouched.
Traces: appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected.

Cause 6 of 8: the UpdateStack set-and-delete conflict is refused with an emulator-authored message.
AWS: Smithy: UpdateStack takes both AccessEndpoints (AccessEndpointList) and AttributesToDelete (StackAttributes, whose enum includes ACCESS_ENDPOINTS) and models InvalidParameterCombinationException (httpError 400). Setting AccessEndpoints while AttributesToDelete contains ACCESS_ENDPOINTS in one call is refused HTTP 400 InvalidParameterCombinationException with the message "You can’t update and delete vpc endpoints at the same time." — the entire sentence is fixed text, with a U+2019 right single quotation mark in "can’t" and no variable part. The refusal leaves the stored endpoints exactly as they were, and the delete on its own is accepted.
Emulator: correct status and code, but its own prose: "AccessEndpoints cannot be specified while AttributesToDelete contains ACCESS_ENDPOINTS".
Why: the message is part of the contract. Clients, test suites and runbooks that match AWS's sentence (or surface it to operators) see text AWS never emits, so error-text assertions and log-based alerting that pass against AWS fail here.
Fix: the UpdateStack handler's parameter-combination refusal — emit AWS's sentence verbatim, including the U+2019 apostrophe.
Traces: appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected.

Cause 7 of 8: the list-length ValidationException message renders the rejected list as an item count instead of the list value.
AWS: Smithy: CreateStack.AccessEndpoints -> AccessEndpointList @length min 1 max 4. A five-entry list is refused before any semantic check, HTTP 400, code ValidationException, message "1 validation error detected: Value '[<entry>, <entry>, <entry>, <entry>, <entry>]' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4" — fixed text apart from (a) the rendered rejected VALUE, a bracketed comma-separated list with one rendering per submitted entry (each entry renders as com.amazonaws.photon.AccessEndpoint@<object-hash>; the hash is per-run and NOT a requirement, the per-entry rendering and the entry count are), (b) the member path 'accessEndpoints' from the request, and (c) the limit 4 from the model's @length max. The refused call creates nothing: DescribeStacks then refuses ResourceNotFoundException "The stack <name> for account <account> does not exist."
Emulator: same status and class, but the value is rendered by cardinality: "1 validation error detected: Value '5 items' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4".
Why: AWS never produces a "<n> items" value rendering, so text-matching clients and operators see a form that does not exist on AWS; because the renderer is shared, the divergence reappears on every list constraint in the service.
Fix: the shared validation-message renderer for list-length violations in the service's validation layer — render the rejected collection value, not its size.
Traces: appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected.

Cause 8 of 8: a created stack carries none of AWS's server-side defaults, so DescribeStacks reports an empty UserSettings.
AWS: Smithy: Stack.UserSettings -> UserSettingList (@length min 1) of UserSetting{Action required enum, Permission required enum, MaximumLength}, documented "The actions that are enabled or disabled for users during their streaming sessions. By default these actions are enabled."; Stack.ApplicationSettings -> ApplicationSettingsResponse; Stack.StreamingExperienceSettings{PreferredProtocol}; Stack.StackErrors. A stack created without specifying them is read back with the defaults materialized: UserSettings has CLIPBOARD_COPY_FROM_LOCAL_DEVICE, CLIPBOARD_COPY_TO_LOCAL_DEVICE, FILE_UPLOAD, FILE_DOWNLOAD and PRINTING_TO_LOCAL_DEVICE = ENABLED and DOMAIN_SMART_CARD_SIGNIN = DISABLED; ApplicationSettings = {Enabled: false}; StreamingExperienceSettings = {PreferredProtocol: 'TCP'}; StackErrors = []. These are identical whether or not an access endpoint is attached.
Emulator: DescribeStacks reports no UserSettings entries at all (replay detail "the bare stack must default the clipboard/file/print actions to ENABLED; UserSettings reports {}") — the Stack record is built from the request only and unspecified members are left empty.
Why: a client reading back the EFFECTIVE configuration it did not specify — which clipboard/file-transfer/print actions users may perform, whether application settings persist, which streaming protocol is preferred — gets nothing and cannot distinguish "default" from "disabled"; policy auditors asserting DOMAIN_SMART_CARD_SIGNIN is DISABLED or that clipboard is ENABLED find no data to assert on.
Fix: the CreateStack handler's stack-record construction (apply the service defaults at create time) and the shared Stack projection used by CreateStack/UpdateStack/DescribeStacks.
Traces: appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 replayed against the booted emulator; per-service rate appstream__ec2 8/17 (47%), 9 failed.

Cause 1 — [mismatch] appstream__ec2.asvpce-create-stack-duplicate-streaming-endpoints-rejected:
  call: create_stack(Name=<new>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:<vpce>}, {EndpointType:'STREAMING',VpceId:<same vpce>}])
  expected: HTTP 400 ('InvalidParameterCombinationException', "You specified duplicate VPC endpoint types in the request. Specify STREAMING once, and try again.") and the stack NOT created
  got: AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code

Cause 2 — [mismatch] appstream__ec2.asvpce-create-stack-with-nonexistent-vpce-rejected:
  call: create_stack(Name=<new>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:'<bogus vpce id>'}])
  expected: HTTP 400 ('InvalidParameterValueException', "The VPC endpoint <bogus vpce id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again.")
  got: AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code

Cause 3 — [mismatch] appstream__ec2.asvpce-create-stack-with-non-appstream-vpce-rejected:
  call: create_stack(Name=<new>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:<a real Interface endpoint for com.amazonaws.<region>.sts>}])
  expected: HTTP 400 ('InvalidParameterValueException', "You specified a VPC endpoint that is not valid for AppStream 2.0.  Check that the endpoint you specify corresponds to the AppStream 2.0 service, and try again.")
  got: AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error code

Cause 4 — [mismatch] appstream__ec2.asvpce-deleted-vpce-lingers-on-stack-and-reattach-rejected:
  setup: a stack referencing a live STREAMING endpoint; ec2 DeleteVpcEndpoints on it, waited until an id-targeted describe no longer resolves it. The 'the stack still lists the stale entry' assertion PASSED.
  call: create_stack(Name=<other>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:<deleted vpce>}])
  expected: HTTP 400 ('InvalidParameterValueException', "The VPC endpoint <deleted vpce> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again."), then DescribeStacks(Names=[<other>]) -> ResourceNotFoundException "The stack <other> for account <account> does not exist."
  got: AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error

Cause 5 — [mismatch] appstream__ec2.asvpce-update-stack-with-nonexistent-vpce-rejected:
  setup: a stack whose AccessEndpoints is [{STREAMING, <real vpce>}] (the control update attached it successfully)
  call: update_stack(Name=<stack>, AccessEndpoints=[{EndpointType:'STREAMING',VpceId:'<bogus vpce id>'}])
  expected: HTTP 400 ('InvalidParameterValueException', "The VPC endpoint <bogus vpce id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again."), stack still lists the real endpoint
  got: AssertionError: the call SUCCEEDED; real AWS rejects it, so there is no error

Cause 6 — [mismatch] appstream__ec2.asvpce-update-stack-set-and-delete-access-endpoints-rejected:
  call: update_stack(Name=<stack>, AccessEndpoints=[{STREAMING,<vpce>}], AttributesToDelete=['ACCESS_ENDPOINTS'])
  expected: ('InvalidParameterCombinationException', 'You can’t update and delete vpc endpoints at the same time.')
  got:      ('InvalidParameterCombinationException', 'AccessEndpoints cannot be specified while AttributesToDelete contains ACCESS_ENDPOINTS')
  status and code match; only the message diverges.

Cause 7 — [mismatch] appstream__ec2.asvpce-create-stack-five-access-endpoints-rejected:
  call: create_stack(Name=<new>, AccessEndpoints=[five {STREAMING,<vpce>} entries])
  code: ValidationException on both sides (that assertion passed)
  expected message (per-entry object hashes masked to <entry>): "1 validation error detected: Value '[<entry>, <entry>, <entry>, <entry>, <entry>]' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4"
  got:                                                          "1 validation error detected: Value '5 items' at 'accessEndpoints' failed to satisfy constraint: Member must have length less than or equal to 4"

Cause 8 — [mismatch] appstream__ec2.asvpce-stack-without-access-endpoints-omits-the-key:
  setup: create_stack(Name=<bare>) with no optional members, beside a sibling created with an access endpoint; the 'AccessEndpoints key absent on the bare stack' assertion PASSED, the next one failed:
  expected: UserSettings contains CLIPBOARD_COPY_FROM_LOCAL_DEVICE/CLIPBOARD_COPY_TO_LOCAL_DEVICE/FILE_UPLOAD/FILE_DOWNLOAD/PRINTING_TO_LOCAL_DEVICE = ENABLED (then DOMAIN_SMART_CARD_SIGNIN = DISABLED, ApplicationSettings = {'Enabled': False}, StreamingExperienceSettings = {'PreferredProtocol': 'TCP'}, StackErrors = [])
  got: AssertionError: the bare stack must default the clipboard/file/print actions to ENABLED; UserSettings reports {}

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): CreateStack.AccessEndpoints -> AccessEndpointList @length min 1 max 4; AccessEndpoint{EndpointType required enum, VpceId}; CreateStack errors {ConcurrentModificationException 400, InvalidAccountStatusException 400, InvalidParameterCombinationException 400, InvalidRoleException 400, LimitExceededException 400, OperationNotPermittedException 400, ResourceAlreadyExistsException 400, ResourceNotFoundException 404}; UpdateStack takes AccessEndpoints + AttributesToDelete -> StackAttributes and lists InvalidParameterCombinationException 400 / ResourceNotFoundException 404; Stack{UserSettings -> UserSettingList @length min 1 (doc: "By default these actions are enabled"), ApplicationSettings, StreamingExperienceSettings, StackErrors, AccessEndpoints}. InvalidParameterValueException is not in CreateStack/UpdateStack's modeled error list — that class and its sentences are grounded in the recordings.

VPC endpoint ids, stack names, region and account numbers above are per-run values; the requirements are the HTTP status, the error code and the message templates.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Common setup: ec2 CreateVpc + CreateSubnet, an Interface VPC endpoint for com.amazonaws.<region>.appstream.streaming, and the AppStream onboarding role.
3. Cause 1: appstream CreateStack with AccessEndpoints = two entries both EndpointType=STREAMING; observe HTTP 200 instead of 400 InvalidParameterCombinationException "You specified duplicate VPC endpoint types in the request. Specify STREAMING once, and try again."
4. Cause 2: CreateStack with AccessEndpoints VpceId set to an id that exists nowhere; observe HTTP 200 instead of 400 InvalidParameterValueException "The VPC endpoint <id> is not valid. Specify a valid VPC endpoint that exists in your AWS account, and try again."
5. Cause 3: CreateStack with a VpceId naming a real Interface endpoint for com.amazonaws.<region>.sts; observe HTTP 200 instead of 400 InvalidParameterValueException "You specified a VPC endpoint that is not valid for AppStream 2.0.  Check that the endpoint you specify corresponds to the AppStream 2.0 service, and try again."
6. Cause 4: create a stack referencing the live streaming endpoint, ec2 DeleteVpcEndpoints it, wait until an id-targeted DescribeVpcEndpoints no longer resolves it, then CreateStack a NEW stack naming that id; observe HTTP 200 instead of the cause-2 InvalidParameterValueException refusal, and confirm the refused stack must not exist afterwards.
7. Cause 5: attach a real endpoint with UpdateStack, then UpdateStack the same stack with an unresolvable VpceId; observe HTTP 200 and the bogus id stored, instead of the InvalidParameterValueException refusal that leaves the previous endpoint in place.
8. Cause 6: UpdateStack with both AccessEndpoints=[...] and AttributesToDelete=['ACCESS_ENDPOINTS']; compare the returned message with 'You can’t update and delete vpc endpoints at the same time.' (U+2019 apostrophe).
9. Cause 7: CreateStack with a five-entry AccessEndpoints list; inspect the ValidationException message's rendered value ('5 items' vs the bracketed per-entry list).
10. Cause 8: CreateStack with only a Name, then DescribeStacks on it; observe UserSettings/ApplicationSettings/StreamingExperienceSettings/StackErrors are not populated with AWS's defaults.
11. Or replay the eight checks named in trace_ids.
