Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected appstream-asvpce-traj-invented-vpce-id-rejected appstream-asvpce-traj-duplicate-streaming-entries-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|CreateStack|2xx->4xx|$.Stack
Issue-Key: appstream|CreateStack|2xx->4xx|status
Issue-Key: appstream|UpdateStack|4xx->2xx|$.Message
Issue-Key: appstream|UpdateStack|4xx->2xx|$.__type
Issue-Key: appstream|UpdateStack|4xx->2xx|status
Issue-Key: appstream|UpdateStack|2xx->2xx|$.Stack.ApplicationSettings
Issue-Key: appstream|UpdateStack|2xx->2xx|$.Stack.State
Issue-Key: appstream|UpdateStack|2xx->2xx|$.Stack.StreamingExperienceSettings
Issue-Key: appstream|UpdateStack|2xx->2xx|$.Stack.UserSettings
Issue-Commit: 2233db37534078619f28c37e0e1db9dccbe7de65

# appstream stack write path diverges: a retried CreateStack is refused, and UpdateStack skips duplicate-AccessEndpoint validation and omits Stack defaults

> _Note: this issue was identified against the code base at commit `2233db37534078619f28c37e0e1db9dccbe7de65`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Three net-new causes on the AppStream stack WRITE path (CreateStack and UpdateStack), all exercised by the access-endpoint (asvpce) wire trajectories. A fixer must close all THREE before submitting.

Prior art that does NOT cover them: 20261102T092115Z-appstream-createstack-updatestack-request-validation-diverges-in-class.md covers how the emulator misclassifies/mis-words the CreateStack refusals themselves (cause 1 below is the opposite outcome — a create AWS ACCEPTS being refused); 20261102T092115Z-appstream-stack-projection-omits-server-side-defaults-and.md covers the identical missing members on the CreateStack and DescribeStacks projections ONLY (cause 3 below is the UpdateStack response, a third code path that issue never names); 20261102T120557Z-appstream-ec2-7-net-new-appstream-root-causes.md covers the duplicate-EndpointType check on CREATE and the unknown-VpceId check on UPDATE, but not the duplicate-EndpointType check on UPDATE (cause 2 below).

Cause 1 of 3: a CreateStack that AWS accepts is refused HTTP 400 after an earlier create of the same Name was refused — a rejected create is not effect-free.
AWS: Smithy: CreateStack -> CreateStackResult { Stack }; CreateStack.Name is @required with @pattern ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$; its modeled errors are {ConcurrentModificationException, InvalidAccountStatusException, InvalidParameterCombinationException, InvalidRoleException, LimitExceededException, OperationNotPermittedException, ResourceAlreadyExistsException (httpError 400), ResourceNotFoundException (404)}. Stack names are unique per account+region, and ResourceAlreadyExistsException is correct ONLY when a stack of that Name actually exists. A CreateStack that AWS REFUSED on request grounds creates nothing and therefore does not reserve the Name: in these trajectories the first attempt is refused (an AccessEndpoints entry whose EndpointType is the API endpoint type, and in the sibling trajectory an AccessEndpoints VpceId that resolves to no VPC endpoint), the recorded DescribeStacks in between confirms the stack does not exist, and the corrected retry of the SAME Name then succeeds with HTTP 200 and a full Stack body (Name and Arn derived from the request and the caller's account/region, CreatedTime, StackErrors [], AccessEndpoints = [{EndpointType: 'STREAMING', VpceId: the endpoint id the request supplied}], plus the server-side defaults State 'ACTIVE', ApplicationSettings {Enabled: false}, StreamingExperienceSettings {PreferredProtocol: 'TCP'} and the default UserSettings list). The requirement is the general one: a refused create leaves no state and the retry of a never-created name succeeds.
Emulator: the retry answers HTTP 400 with no Stack member at all (replay detail: "status: expected 200, got 400" / "$.Stack: missing"). Hypothesis: the emulator ACCEPTED the earlier invalid create — the same trajectories' task#7 clusters show that create answering HTTP 200 instead of the modeled refusal, and the sibling invented-VpceId trajectory shows this very path answering code ResourceAlreadyExistsException where AWS answers InvalidParameterValueException — so the name is already allocated in emulator state and the valid retry collides with a stack AWS never created. Equivalently, validation runs after (or without) the state write, so a refused create is not effect-free.
Why: this is the create-retry loop every client and every IaC provider performs — submit, read the parameter refusal, correct the request, resubmit under the same name. Against the emulator the corrected resubmission is permanently refused as an already-existing resource, so the client can never create the stack at all and cannot distinguish "your request was wrong" from "this name is taken"; worse, a reader that trusts the first 200 believes a stack exists that AWS would have refused.
Fix: the CreateStack handler in the appstream service crate — validate the whole request (including AccessEndpoints) BEFORE inserting the stack record, and make a refused create leave the name store untouched, so the subsequent valid create of that Name proceeds to 200 with the normal Stack body.
Traces: appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected#task#9 (cluster also covers appstream-asvpce-traj-invented-vpce-id-rejected#task#9).

Cause 2 of 3: UpdateStack accepts an AccessEndpoints list carrying two entries of the same EndpointType instead of refusing it.
AWS: Smithy: UpdateStack.AccessEndpoints -> AccessEndpointList (@length min 1 max 4) of AccessEndpoint{EndpointType required enum, VpceId}; UpdateStack -> UpdateStackResult { Stack }; UpdateStack's modeled errors include InvalidParameterCombinationException (httpError 400). An endpoint type may appear at most once, and AWS refuses a repeat rather than de-duplicating it or storing both: HTTP 400, code InvalidParameterCombinationException, message "You specified duplicate VPC endpoint types in the request. Specify STREAMING once, and try again." — both sentences are fixed text; the only variable part is the repeated EndpointType value taken from the request (STREAMING here). The refusal is effect-free: the stack keeps the endpoint list it already had.
Emulator: answers HTTP 200 with no error at all (replay detail: "status: expected 400, got 200; $.__type: missing (expected 'InvalidParameterCombinationException'); $.Message: missing (expected 'You specified duplicate VPC endpoint types in the request. Specify STREAMING once, and try again.')"), and it stores BOTH entries — the DescribeStacks step that follows reports AccessEndpoints with the same endpoint listed twice. So the update path performs no duplicate-type check.
Why: a client whose templating or list-merge logic emits two STREAMING entries (the common case) is told the update succeeded and ends up with a stack holding an endpoint list the real service rejects. The breakage surfaces only on promotion to real AWS, and any reconcile loop comparing desired to actual keeps re-applying a shape AWS refuses.
Fix: UpdateStack's request validation in the appstream service crate — reject a repeated AccessEndpoints EndpointType with InvalidParameterCombinationException and AWS's sentence before the stack record is mutated (the same check CreateStack needs; the two should meet in one shared validator).
Traces: appstream-asvpce-traj-duplicate-streaming-entries-rejected#task#7.

Cause 3 of 3: UpdateStack's returned Stack omits the server-side members State, ApplicationSettings, StreamingExperienceSettings and UserSettings.
AWS: Smithy: UpdateStack -> UpdateStackResult { Stack }; Stack.ApplicationSettings -> ApplicationSettingsResponse, Stack.StreamingExperienceSettings -> StreamingExperienceSettings{PreferredProtocol}, Stack.UserSettings -> UserSettingList (@length min 1) of UserSetting{Action required enum, Permission required enum}, documented as enabled by default. AWS projects the stack's EFFECTIVE state on the update response exactly as on create and read: ApplicationSettings {Enabled: false}, StreamingExperienceSettings {PreferredProtocol: 'TCP'}, the full default UserSettings list (CLIPBOARD_COPY_FROM_LOCAL_DEVICE, CLIPBOARD_COPY_TO_LOCAL_DEVICE, FILE_UPLOAD, FILE_DOWNLOAD, PRINTING_TO_LOCAL_DEVICE and DOMAIN_PASSWORD_SIGNIN ENABLED; DOMAIN_SMART_CARD_SIGNIN and AUTO_TIME_ZONE_REDIRECTION DISABLED), and State 'ACTIVE'. The on-disk Stack shape declares no State member, so for that one member this is recorded behavior only.
Emulator: HTTP 200, but all four members are absent (replay detail: "$.Stack.ApplicationSettings: missing (expected {'Enabled': False}); $.Stack.State: missing (expected 'ACTIVE'); $.Stack.StreamingExperienceSettings: missing (expected {'PreferredProtocol': 'TCP'}); $.Stack.UserSettings: missing") — the update response, like the create and read responses, is built from the request only.
Why: UpdateStack is the one response that tells a client what the stack looks like AFTER its change; with the effective configuration omitted, a client cannot confirm which clipboard/file-transfer/print actions remain permitted, which protocol the stack prefers, or that the stack is still ACTIVE, and code that gates on State == 'ACTIVE' after an update never proceeds.
Fix: the shared Stack projection/serializer, reached from the UpdateStack handler — the same projection the open issue 20261102T092115Z-appstream-stack-projection-omits-server-side-defaults-and.md fixes for CreateStack and DescribeStacks; the UpdateStack call site must use it too (and the defaults must be materialized on the stored record, not synthesized per response).
Traces: appstream-asvpce-traj-duplicate-streaming-entries-rejected#task#9.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator (emulator_up true, /__admin/health 200; 162 steps, 83 passed, score 51.2%). None of the three clusters below carries a stub_path or an owned_by, i.e. no filed issue holds their replay key.

Cause 1 — CreateStack, fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected step task#9 (verdict mismatch):
  expected_status 200, actual_status 400
  $.Stack: missing — expected {'Name': <the requested name>, 'Arn': 'arn:aws:appstream:<region>:<account>:stack/<name>', 'CreatedTime': <t>, 'StackErrors': [], 'AccessEndpoints': [{'EndpointType': 'STREAMING', 'VpceId': <the run's endpoint id>}], 'State': 'ACTIVE', 'ApplicationSettings': {'Enabled': False}, 'StreamingExperienceSettings': {'PreferredProtocol': 'TCP'}, 'UserSettings': [the eight default Action/Permission pairs]}
  divergence_key service=appstream|CreateStack|status=200->400|keys=$.Stack,status; affected_count 2 (also appstream-asvpce-traj-invented-vpce-id-rejected#task#9).
  Supporting context from the same trajectories' earlier step: task#7 of these fixtures is where AWS refuses the first create — there the emulator answered 200 (cluster service=appstream|CreateStack|status=400->200) and, in the invented-VpceId trajectory, answered code 'ResourceAlreadyExistsException' where AWS answers 'InvalidParameterValueException' (cluster service=appstream|CreateStack|status=400->400|keys=$.__type,$.message). The 400 on task#9 carries no Stack; the cluster detail does not quote its error body, so the name-collision mechanism is labelled Hypothesis above.

Cause 2 — UpdateStack, fixture appstream-asvpce-traj-duplicate-streaming-entries-rejected step task#7 (verdict mismatch):
  expected_status 400, actual_status 200
  status: expected 400, got 200
  $.__type: missing (expected 'InvalidParameterCombinationException', got None)
  $.Message: missing (expected 'You specified duplicate VPC endpoint types in the request. Specify STREAMING once, and try again.', got None)
  divergence_key service=appstream|UpdateStack|status=400->200|keys=$.Message,$.__type,status; affected_count 1.
  Corroborating effect at the next read (task#8 of the same fixture): $.Stacks[0].AccessEndpoints unexpected_present, got [{'EndpointType': 'STREAMING', 'VpceId': <id>}, {'EndpointType': 'STREAMING', 'VpceId': <same id>}] — both duplicate entries were stored.

Cause 3 — UpdateStack, fixture appstream-asvpce-traj-duplicate-streaming-entries-rejected step task#9 (verdict mismatch, status 200 on both sides):
  $.Stack.ApplicationSettings: missing (expected {'Enabled': False})
  $.Stack.State: missing (expected 'ACTIVE')
  $.Stack.StreamingExperienceSettings: missing (expected {'PreferredProtocol': 'TCP'})
  $.Stack.UserSettings: missing (expected [CLIPBOARD_COPY_FROM_LOCAL_DEVICE/ENABLED, CLIPBOARD_COPY_TO_LOCAL_DEVICE/ENABLED, FILE_UPLOAD/ENABLED, FILE_DOWNLOAD/ENABLED, PRINTING_TO_LOCAL_DEVICE/ENABLED, DOMAIN_PASSWORD_SIGNIN/ENABLED, DOMAIN_SMART_CARD_SIGNIN/DISABLED, AUTO_TIME_ZONE_REDIRECTION/DISABLED])
  divergence_key service=appstream|UpdateStack|status=200->200|keys=$.Stack.ApplicationSettings,$.Stack.State,$.Stack.StreamingExperienceSettings,$.Stack.UserSettings; affected_count 1.

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): CreateStack{Name required @pattern ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$, AccessEndpoints -> AccessEndpointList @length min 1 max 4, ...} -> CreateStackResult{Stack}, errors include ResourceAlreadyExistsException (httpError 400) and InvalidParameterCombinationException (400); UpdateStack{Name required, AccessEndpoints -> AccessEndpointList @length min 1 max 4, AttributesToDelete -> StackAttributes, ...} -> UpdateStackResult{Stack}, errors include InvalidParameterCombinationException (400), ResourceInUseException (400), ResourceNotFoundException (404); Stack{ApplicationSettings -> ApplicationSettingsResponse, UserSettings -> UserSettingList @length min 1, StreamingExperienceSettings{PreferredProtocol}, AccessEndpoints, StackErrors}. Stack declares no State member — that member is grounded in the recordings only. InvalidParameterValueException (referenced as context in cause 1) is not in CreateStack's modeled error list and is likewise recorded behavior only.

## Steps to Reproduce
1. git_clone the project repo and boot the emulator built from that checkout.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-asvpce-traj-api-endpoint-instead-of-streaming-rejected. At task#7 a CreateStack carrying an AccessEndpoints entry whose EndpointType is the API endpoint type is refused by AWS (the emulator answers 200); the intervening DescribeStacks confirms no stack exists. At task#9 the corrected CreateStack re-submits the SAME Name with EndpointType STREAMING: AWS answers 200 with the full Stack, the emulator answers 400 with no Stack. Repeat with appstream-asvpce-traj-invented-vpce-id-rejected (same shape, first attempt refused for an unresolvable VpceId) to see the identical 400 at its task#9.
4. Cause 2: replay fixture appstream-asvpce-traj-duplicate-streaming-entries-rejected to task#7 — UpdateStack on the existing stack with AccessEndpoints = two entries both EndpointType STREAMING. Observe HTTP 200 instead of HTTP 400 __type 'InvalidParameterCombinationException' with Message 'You specified duplicate VPC endpoint types in the request. Specify STREAMING once, and try again.'; then continue to task#8 and observe DescribeStacks reporting both duplicate entries, where AWS would still report the stack's prior single endpoint.
5. Cause 3: continue the same fixture to task#9 — an UpdateStack that does not specify UserSettings/ApplicationSettings/StreamingExperienceSettings. Inspect the returned Stack and observe State, ApplicationSettings, StreamingExperienceSettings and UserSettings are all absent, where AWS projects 'ACTIVE', {Enabled: false}, {PreferredProtocol: 'TCP'} and the eight default UserSettings pairs.
