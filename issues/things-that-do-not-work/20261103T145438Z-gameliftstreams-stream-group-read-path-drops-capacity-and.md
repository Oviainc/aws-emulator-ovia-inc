Issue-Source: trace-sweep-divergence
Issue-Trace: gameliftstreams-stream-group-application-association-lifecycle
Issue-Step: task#8
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: gameliftstreams-stream-group-application-association-lifecycle
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.ActiveCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.AssociatedApplications[*]
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.DefaultApplication
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.DesiredCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.IdleCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].AlwaysOnCapacity
Issue-Key: gameliftstreams|GetStreamGroup|2xx->2xx|$.LocationStates[*].RequestedCapacity
Issue-Key: gameliftstreams|TagResource|2xx->2xx|$headers.content-length
Issue-Commit: 66471a963a64e2b8a9116c3903f472052675d9f3

# gameliftstreams stream-group read path drops capacity and association state, and empty TagResource response omits Content-Length

> _Note: this issue was identified against the code base at commit `66471a963a64e2b8a9116c3903f472052675d9f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStreamGroup

## Description
Three net-new root causes in the gameliftstreams stream-group control plane, all found by replaying wire fixture gameliftstreams-stream-group-application-association-lifecycle against the emulator built from this checkout. ALL THREE must be closed before this issue is submitted; each cause names its own fixture step. No existing open gameliftstreams issue covers any of them (the open issues cover application-source validation/normalization/ReplicationStatuses/DeleteApplication wording, the DisassociateApplications stub, UpdateStreamGroup's DefaultApplicationIdentifier misclassification, StartStreamSession placement, and the stream-URL/stream-session not-found message templates), and the replay report marked none of these three as stubbed or owned.

=== Cause 1 of 3: GetStreamGroup never reflects the stream group's configured capacity — per-location AlwaysOnCapacity/RequestedCapacity stay 0 and the top-level capacity summary members are absent (steps task#8 and task#16) ===
AWS: Smithy (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json): GetStreamGroup is GET /streamgroups/{Identifier}, success 200, output GetStreamGroupOutput with LocationStates (list of LocationState). LocationState declares AlwaysOnCapacity, OnDemandCapacity, TargetIdleCapacity, MaximumCapacity, RequestedCapacity, AllocatedCapacity and IdleCapacity, each an integer @range min 0; RequestedCapacity is documented as "the always-on capacity that you most recently requested for a stream group" (requested per location). CreateStreamGroup / UpdateStreamGroup take LocationConfigurations whose LocationConfiguration.AlwaysOnCapacity is the requested always-on capacity for that location. So the requirement is: whatever always-on capacity a location was created or updated with (call it N) must come back in that location's LocationState as AlwaysOnCapacity = N and RequestedCapacity = N; capacity the group has not provisioned yet reads 0 in AllocatedCapacity/IdleCapacity. The recorded AWS response additionally carries a group-level capacity summary at the top level of the body — ActiveCapacity, DesiredCapacity and IdleCapacity (recorded behavior only: the on-disk model's GetStreamGroupOutput declares no top-level capacity members, so the recording is the only authority here) — where DesiredCapacity equals the sum of the requested always-on capacity across locations (N) and ActiveCapacity/IdleCapacity are 0 while the capacity is not yet streaming/idle-held.
Emulator: GetStreamGroup answers 200 with a body that omits ActiveCapacity, DesiredCapacity and IdleCapacity entirely (replay reports them as `missing`, got None) and reports LocationStates[0].AlwaysOnCapacity = 0 and LocationStates[0].RequestedCapacity = 0 where the group's location was configured with always-on capacity N (the fixture configured N=2, so expected 2 and 2, DesiredCapacity 2, ActiveCapacity 0, IdleCapacity 0). The same omission/zeroing is reported twice in one fixture — immediately after the group is configured (task#8) and again later in the lifecycle (task#16) — so it is not a transient provisioning window: the configured capacity is never carried into the read path at all.
Why: capacity is the one thing a client scales and then polls for. A caller that sets always-on capacity N and reads the group back sees 0 requested and no DesiredCapacity member, so it concludes the scaling request was lost and either retries forever or treats the group as having no capacity; code that waits for RequestedCapacity/DesiredCapacity to reach its target never converges, and code that computes headroom from DesiredCapacity - ActiveCapacity hits a missing key (KeyError/nil) instead of a number.
Fix: the stream-group state and the GetStreamGroup read projection — the capacity requested by CreateStreamGroup/UpdateStreamGroup/AddStreamGroupLocations must be stored per location and surfaced as that LocationState's AlwaysOnCapacity and RequestedCapacity, and the group-level ActiveCapacity/DesiredCapacity/IdleCapacity summary members must be serialized (present, aggregated from the locations) rather than left unset. The same projection backs CreateStreamGroup/UpdateStreamGroup responses, so fix it once in the shared projection rather than in one handler.
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#8 (representative) and #task#16.

=== Cause 2 of 3: GetStreamGroup omits AssociatedApplications and DefaultApplication, so an application association is invisible on read (step task#8) ===
AWS: Smithy: GetStreamGroupOutput declares DefaultApplication (structure DefaultApplication {Id, Arn}, cfnMutability create-and-read, documented "The default Amazon GameLift Streams application that is associated with this stream group") and AssociatedApplications (ArnList, documented "A set of applications that this stream group is associated to"). AssociateApplications is POST /streamgroups/{Identifier}/associations and returns the group Arn plus ApplicationArns; CreateStreamGroup/UpdateStreamGroup take DefaultApplicationIdentifier. So after an application is associated with a group, GetStreamGroup must list that application's ARN in AssociatedApplications, and a group whose default application is set must report DefaultApplication as {Arn: <that application's ARN>, Id: <that application's id>}.
Emulator: GetStreamGroup answers 200 with AssociatedApplications empty/absent (replay: $.AssociatedApplications[0] missing, got None) and DefaultApplication absent (replay: missing, expected the {Arn, Id} pair of the application the fixture had just associated and made default). The association itself is accepted by the association call, but the read path does not project it.
Why: AssociatedApplications/DefaultApplication are how a client confirms an association landed and how it discovers which application a group can stream. With both unset, an associate-then-verify flow fails, DeleteApplication's association conflict and StartStreamSession's application resolution cannot be reasoned about from the group's own state, and SDK consumers that dereference DefaultApplication.Arn get a null dereference instead of an ARN.
Fix: the stream-group state/read projection for GetStreamGroup — the association recorded by AssociateApplications must be stored on the group and serialized into AssociatedApplications, and the default application (from CreateStreamGroup/UpdateStreamGroup DefaultApplicationIdentifier) must be serialized as the DefaultApplication {Arn, Id} structure.
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#8 (same step as cause 1; the two causes are independent projections of the same response).

=== Cause 3 of 3: TagResource's empty success response omits the Content-Length header (step task#11) ===
AWS: Smithy: TagResource is POST /tags/{ResourceArn} with output shape TagResourceResponse, a structure with NO members, and success code 200 (defaulted in the model). AWS always frames such an empty REST-JSON response with a Content-Length header describing the body it did send (0 for a truly bodiless response, 2 for the `{}` document), and the recording asserts only that the header is PRESENT and shaped (`<shaped-not-asserted>`), not any particular numeric value — so the requirement is the presence of Content-Length on this empty-body success, not a specific length.
Emulator: the status (204) matched the recording, but the response carries no Content-Length header at all (replay: $headers.content-length missing, got None).
Why: an HTTP client that frames a response by Content-Length and does not get one must fall back to reading until close (or, with no chunked framing either, treat the message as unterminated). On keep-alive connections this stalls or desynchronizes the connection, and strict clients/proxies reject the response, so a tagging call that the emulator considers successful can surface as a transport error.
Fix: the response serializer for operations whose output shape has no members — emit Content-Length for the empty body it writes (this is shared framing, not TagResource-specific, so every empty-output operation in this service benefits).
Traces: gameliftstreams-stream-group-application-association-lifecycle#task#11.

## Evidence
Replay of wire trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, /__admin/health 200, 48 steps, 24 passed, score 50%). All three causes come from fixture gameliftstreams-stream-group-application-association-lifecycle; none was flagged stub_path or owned_by.

Cause 1 of 3 — step task#8, GetStreamGroup, verdict mismatch, expected status 200 / actual 200:
  $.ActiveCapacity: missing (expected 0, got None)
  $.DesiredCapacity: missing (expected 2, got None)
  $.IdleCapacity: missing (expected 0, got None)
  $.LocationStates[0].AlwaysOnCapacity: value_mismatch (expected 2, got 0)
  $.LocationStates[0].RequestedCapacity: value_mismatch (expected 2, got 0)
Cause 1 of 3 — step task#16, GetStreamGroup, verdict mismatch, expected 200 / actual 200: the identical five divergences (ActiveCapacity/DesiredCapacity/IdleCapacity missing; LocationStates[0].AlwaysOnCapacity and RequestedCapacity 0 instead of 2), i.e. still zero later in the lifecycle.
  The value 2 is the always-on capacity this fixture's location was configured with; the requirement is that the CONFIGURED capacity is echoed, not the literal 2.

Cause 2 of 3 — step task#8, same GetStreamGroup response:
  $.AssociatedApplications[0]: missing (expected the associated application's ARN, got None)
  $.DefaultApplication: missing (expected {'Arn': <application ARN>, 'Id': <application id>}, got None)
  The fixture's expected ARN/Id are per-run generated values; the requirement is that the associated/default application is reported at all.

Cause 3 of 3 — step task#11, TagResource, verdict mismatch, expected status 204 / actual 204:
  $headers.content-length: missing (expected '<shaped-not-asserted>', got None)
  Status and (empty) body matched; only the Content-Length framing header is absent.

Model evidence (services/service-com-amazonaws-gameliftstreams-gameliftstreams/smithy-model/com-amazonaws-gameliftstreams-gameliftstreams.json): GetStreamGroup GET /streamgroups/{Identifier} code 200, output members include LocationStates, DefaultApplication, AssociatedApplications; LocationState members AlwaysOnCapacity/RequestedCapacity/AllocatedCapacity/IdleCapacity are integers @range min 0; CreateStreamGroup/UpdateStreamGroup accept LocationConfigurations[].AlwaysOnCapacity and DefaultApplicationIdentifier; AssociateApplications POST /streamgroups/{Identifier}/associations returns {Arn, ApplicationArns}; TagResource POST /tags/{ResourceArn} with empty-member output shape TagResourceResponse. The top-level ActiveCapacity/DesiredCapacity/IdleCapacity members asserted by the recording are NOT declared in this on-disk GetStreamGroupOutput — for those three members the recorded response is the only authority (recorded behavior only).

## Steps to Reproduce
Boot the emulator built from this checkout and replay fixture gameliftstreams-stream-group-application-association-lifecycle (steps task#8, task#11, task#16), or by hand:
1. CreateApplication over real S3 content and poll GetApplication until Status == READY.
2. CreateStreamGroup(Description=<d>, StreamClass=<class>, LocationConfigurations=[{LocationName=<region>, AlwaysOnCapacity=2}]) and poll GetStreamGroup until the group/location is ACTIVE.
3. AssociateApplications(Identifier=<group ARN>, ApplicationIdentifiers=[<application ARN>]) and set that application as the group's default (CreateStreamGroup DefaultApplicationIdentifier or UpdateStreamGroup).
4. Cause 1: GetStreamGroup(Identifier=<group ARN>) — observe LocationStates[0].AlwaysOnCapacity == 0 and LocationStates[0].RequestedCapacity == 0 instead of the configured 2, and no ActiveCapacity / DesiredCapacity / IdleCapacity members in the body at all (AWS returns DesiredCapacity == the requested always-on capacity, ActiveCapacity == 0, IdleCapacity == 0). Repeat the same GetStreamGroup later in the lifecycle (fixture step task#16) and observe the values are still 0/absent.
5. Cause 2: in the same GetStreamGroup response, observe AssociatedApplications is empty/absent and DefaultApplication is absent, instead of AssociatedApplications containing the associated application's ARN and DefaultApplication == {Arn: <application ARN>, Id: <application id>}.
6. Cause 3: TagResource(ResourceArn=<group ARN>, Tags={<k>: <v>}) and inspect the raw response headers — observe no Content-Length header on the empty-body success (status 204), where AWS frames the empty response with Content-Length.
