Issue-Source: trace-sweep-divergence
Issue-Trace: appstream-asvpce-traj-duplicate-streaming-entries-rejected
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appstream-asvpce-traj-duplicate-streaming-entries-rejected appstream-asvpce-traj-stack-lookup-and-delete-lag appstream-asvpce-traj-update-stack-attributes-to-delete-enum-rejected appstream-asvpce-traj-invented-vpce-id-rejected appstream-asvpce-traj-rerun-create-stack
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream|CreateStack|2xx->2xx|$.Stack.ApplicationSettings
Issue-Key: appstream|CreateStack|2xx->2xx|$.Stack.State
Issue-Key: appstream|CreateStack|2xx->2xx|$.Stack.StreamingExperienceSettings
Issue-Key: appstream|CreateStack|2xx->2xx|$.Stack.UserSettings
Issue-Key: appstream|DescribeStacks|2xx->2xx|$.Stacks[*].ApplicationSettings
Issue-Key: appstream|DescribeStacks|2xx->2xx|$.Stacks[*].State
Issue-Key: appstream|DescribeStacks|2xx->2xx|$.Stacks[*].StreamingExperienceSettings
Issue-Key: appstream|DescribeStacks|2xx->2xx|$.Stacks[*].UserSettings
Issue-Key: appstream|DescribeStacks|2xx->2xx|$.Stacks[*].AccessEndpoints
Issue-Key: appstream|DescribeStacks|2xx->2xx|$.Stacks[*].AccessEndpoints[*].VpceId
Issue-Commit: 798fb32aa8eb1e75369deb4b48c94746eb1ca248

# appstream Stack projection omits server-side defaults and misreports AccessEndpoints

> _Note: this issue was identified against the code base at commit `798fb32aa8eb1e75369deb4b48c94746eb1ca248`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Four net-new causes sharing one fault: the Stack structure the emulator serializes is built from the request only. Members AWS fills in server-side are omitted, and the stored AccessEndpoints are either dropped or replaced by a constant. The same projection is used by CreateStack and DescribeStacks, so the defect shows on both.

Cause 1 of 4: CreateStack's returned Stack omits the server-side defaults State, ApplicationSettings, StreamingExperienceSettings and UserSettings.
AWS: Smithy: CreateStack -> CreateStackResult { Stack }. The recording shows AWS materializes defaults for members the request did not set: ApplicationSettings {Enabled: false} (Smithy shape ApplicationSettingsResponse on Stack.ApplicationSettings), StreamingExperienceSettings {PreferredProtocol: 'TCP'} (Smithy: Stack.StreamingExperienceSettings -> StreamingExperienceSettings.PreferredProtocol enum), and the full default UserSettings list (Smithy: Stack.UserSettings -> UserSettingList of UserSetting{Action, Permission}) — CLIPBOARD_COPY_FROM_LOCAL_DEVICE, CLIPBOARD_COPY_TO_LOCAL_DEVICE, FILE_UPLOAD, FILE_DOWNLOAD, PRINTING_TO_LOCAL_DEVICE and DOMAIN_PASSWORD_SIGNIN ENABLED, DOMAIN_SMART_CARD_SIGNIN and AUTO_TIME_ZONE_REDIRECTION DISABLED (the model documents these actions as enabled by default). A lifecycle `State` of 'ACTIVE' is also present on the response: the on-disk Stack shape declares no State member, so for that one member this is recorded behavior only.
Emulator: returns a Stack with all four members absent (replay detail: "$.Stack.ApplicationSettings: missing (expected {'Enabled': False}); $.Stack.State: missing (expected 'ACTIVE'); $.Stack.StreamingExperienceSettings: missing (expected {'PreferredProtocol': 'TCP'}); $.Stack.UserSettings: missing").
Why: a client that reads back the effective configuration it did not specify — which clipboard/file-transfer actions are permitted, which streaming protocol the stack prefers, whether application settings persist — gets nothing, and code that branches on `State == 'ACTIVE'` before using the stack never proceeds.
Fix: the CreateStack handler's stack-record construction (apply the service defaults at create time) and the Stack serializer that projects the record onto the wire.
Traces: appstream-asvpce-traj-duplicate-streaming-entries-rejected#task#6 (cluster also covers appstream-asvpce-traj-stack-lookup-and-delete-lag#task#1 and appstream-asvpce-traj-update-stack-attributes-to-delete-enum-rejected#task#1).

Cause 2 of 4: DescribeStacks' Stacks[0] omits the same four server-side members.
AWS: Smithy: DescribeStacks -> {Stacks: StackList of Stack}. The stored defaults are projected on every read, identically to the create response.
Emulator: `$.Stacks[0].ApplicationSettings`, `.State`, `.StreamingExperienceSettings`, `.UserSettings` all missing on an otherwise HTTP 200 read.
Why: polling DescribeStacks never observes the stack reaching ACTIVE, and the effective user-settings/protocol configuration is invisible to readers that did not perform the create.
Fix: the same Stack projection, reached from the DescribeStacks handler.
Traces: appstream-asvpce-traj-duplicate-streaming-entries-rejected#task#8 (cluster also covers appstream-asvpce-traj-stack-lookup-and-delete-lag#task#6 and appstream-asvpce-traj-update-stack-attributes-to-delete-enum-rejected#task#4).

Cause 3 of 4: DescribeStacks drops AccessEndpoints entirely from the read projection.
AWS: Smithy: Stack.AccessEndpoints -> AccessEndpointList, @length min 1 max 4, member AccessEndpoint{EndpointType (required enum), VpceId}. An endpoint supplied at create time is returned on every subsequent read as [{EndpointType: 'STREAMING', VpceId: <the endpoint's id>}].
Emulator: `$.Stacks[0].AccessEndpoints: missing` — alongside the same four default members — so the stack's private-connectivity configuration is not reported at all.
Why: AccessEndpoints is the whole point of a VPC-endpoint-scoped stack; a client cannot verify which interface endpoint the stack is restricted to, and a reconcile loop comparing desired to actual sees the endpoint as unset and will try to re-apply it forever.
Fix: the same Stack projection — persist and serialize the AccessEndpoints the create accepted.
Traces: appstream-asvpce-traj-duplicate-streaming-entries-rejected#task#10.

Cause 4 of 4: DescribeStacks reports AccessEndpoints[0].VpceId as a fixed placeholder id instead of the endpoint the stack was created with.
AWS: Smithy: AccessEndpoint.VpceId (String, @length min 1) is the identifier of the interface endpoint named in the request; AWS echoes the stored value, which is the id the EC2 CreateVpcEndpoint in the same trajectory returned.
Emulator: serializes a hard-coded-looking constant VpceId that is unrelated to the endpoint the request named (replay detail: "$.Stacks[0].AccessEndpoints[0].VpceId: value_mismatch (expected '{{vpce_id}}', got 'vpce-0123456789abcdef0')") — the fixture's `{{vpce_id}}` placeholder resolves to the endpoint created in that run, so the requirement is "the VpceId the request supplied", not any particular literal.
Why: the reported endpoint id is wrong, so a client cross-referencing the stack against its EC2 VPC endpoint (to confirm the association, to tear the endpoint down, or to authorize traffic) matches against an id that exists nowhere in the account.
Fix: the Stack projection's AccessEndpoints serialization — emit the stored per-stack VpceId rather than a synthesized/default value.
Traces: appstream-asvpce-traj-invented-vpce-id-rejected#task#10 (cluster also covers appstream-asvpce-traj-rerun-create-stack#task#9).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the booted emulator.

Cause 1 — CreateStack, fixture appstream-asvpce-traj-duplicate-streaming-entries-rejected step task#6 (status 200 both sides):
  $.Stack.ApplicationSettings: missing (expected {'Enabled': False})
  $.Stack.State: missing (expected 'ACTIVE')
  $.Stack.StreamingExperienceSettings: missing (expected {'PreferredProtocol': 'TCP'})
  $.Stack.UserSettings: missing (expected [CLIPBOARD_COPY_FROM_LOCAL_DEVICE/ENABLED, CLIPBOARD_COPY_TO_LOCAL_DEVICE/ENABLED, FILE_UPLOAD/ENABLED, FILE_DOWNLOAD/ENABLED, PRINTING_TO_LOCAL_DEVICE/ENABLED, DOMAIN_PASSWORD_SIGNIN/ENABLED, DOMAIN_SMART_CARD_SIGNIN/DISABLED, AUTO_TIME_ZONE_REDIRECTION/DISABLED])
  divergence_key service=appstream|CreateStack|status=200->200|keys=$.Stack.ApplicationSettings,$.Stack.State,$.Stack.StreamingExperienceSettings,$.Stack.UserSettings; affected_count 3.

Cause 2 — DescribeStacks, same fixture step task#8 (status 200 both sides): $.Stacks[0].ApplicationSettings, .State, .StreamingExperienceSettings, .UserSettings all missing, same expected values. affected_count 3.

Cause 3 — DescribeStacks, same fixture step task#10 (status 200 both sides): $.Stacks[0].AccessEndpoints missing (expected [{'EndpointType': 'STREAMING', 'VpceId': <the run's vpce id>}]) plus the same four missing default members. affected_count 1.

Cause 4 — DescribeStacks, fixture appstream-asvpce-traj-invented-vpce-id-rejected step task#10 (status 200 both sides):
  $.Stacks[0].AccessEndpoints[0].VpceId: value_mismatch (expected '{{vpce_id}}' = the endpoint created in that run, got 'vpce-0123456789abcdef0'), plus the same four missing default members. affected_count 2.

Smithy evidence (services/service-com-amazonaws-appstream-photonadminproxyservice/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json): Stack members include ApplicationSettings -> ApplicationSettingsResponse, UserSettings -> UserSettingList @length min 1, AccessEndpoints -> AccessEndpointList @length min 1 max 4, StreamingExperienceSettings -> StreamingExperienceSettings{PreferredProtocol}; AccessEndpoint = {EndpointType required enum, VpceId String @length min 1}; UserSetting = {Action required enum, Permission required enum, MaximumLength}. Stack declares no State member — that member is grounded in the recording only.

## Steps to Reproduce
1. git clone the project repo and boot the emulator built from it.
2. Replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
3. Cause 1: replay fixture appstream-asvpce-traj-duplicate-streaming-entries-rejected to step task#6 — a CreateStack that does not specify UserSettings/ApplicationSettings/StreamingExperienceSettings; inspect the returned Stack and observe those members plus State are absent.
4. Cause 2: continue to step task#8 — DescribeStacks on that stack; observe the same four members absent from Stacks[0].
5. Cause 3: continue to step task#10 — DescribeStacks on the stack created with an AccessEndpoints entry; observe Stacks[0].AccessEndpoints is absent.
6. Cause 4: replay fixture appstream-asvpce-traj-invented-vpce-id-rejected to step task#10 — DescribeStacks on a stack created with the endpoint from that run; observe AccessEndpoints[0].VpceId is the constant 'vpce-0123456789abcdef0' rather than the endpoint id the request supplied.
