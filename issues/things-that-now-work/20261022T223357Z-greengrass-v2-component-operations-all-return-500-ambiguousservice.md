Issue-Source: trace-sweep-divergence
Issue-Trace: greengrassv2-component-error-paths
Issue-Step: setup#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: greengrassv2-component-error-paths greengrassv2-component-resolution-and-artifact greengrassv2-component-version-lifecycle greengrassv2-list-component-versions-nonexistent-component
Issue-Shape-Id: com.amazonaws.greengrassv2#GreengrassV2
Issue-Key: greengrass|CreateComponentVersion|2xx->5xx|$.arn
Issue-Key: greengrass|CreateComponentVersion|2xx->5xx|$.componentName
Issue-Key: greengrass|CreateComponentVersion|2xx->5xx|$.componentVersion
Issue-Key: greengrass|CreateComponentVersion|2xx->5xx|$.creationTimestamp
Issue-Key: greengrass|CreateComponentVersion|2xx->5xx|$.status
Issue-Key: greengrass|CreateComponentVersion|2xx->5xx|$headers.content-type
Issue-Key: greengrass|CreateComponentVersion|2xx->5xx|status
Issue-Key: greengrass|CreateComponentVersion|4xx->5xx|$.message
Issue-Key: greengrass|CreateComponentVersion|4xx->5xx|$headers.content-type
Issue-Key: greengrass|CreateComponentVersion|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|CreateComponentVersion|4xx->5xx|status
Issue-Key: greengrass|DescribeComponent|4xx->5xx|$.message
Issue-Key: greengrass|DescribeComponent|4xx->5xx|$headers.content-type
Issue-Key: greengrass|DescribeComponent|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|DescribeComponent|4xx->5xx|status
Issue-Key: greengrass|GetComponent|4xx->5xx|$.message
Issue-Key: greengrass|GetComponent|4xx->5xx|$headers.content-type
Issue-Key: greengrass|GetComponent|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|GetComponent|4xx->5xx|status
Issue-Key: greengrass|DeleteComponent|4xx->5xx|$.message
Issue-Key: greengrass|DeleteComponent|4xx->5xx|$headers.content-type
Issue-Key: greengrass|DeleteComponent|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|DeleteComponent|4xx->5xx|status
Issue-Key: greengrass|ResolveComponentCandidates|4xx->5xx|$.Message
Issue-Key: greengrass|ResolveComponentCandidates|4xx->5xx|$headers.content-type
Issue-Key: greengrass|ResolveComponentCandidates|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: greengrass|ResolveComponentCandidates|4xx->5xx|status
Issue-Key: greengrass|ListComponentVersions|2xx->5xx|$.componentVersions
Issue-Key: greengrass|ListComponentVersions|2xx->5xx|$headers.content-type
Issue-Key: greengrass|ListComponentVersions|2xx->5xx|status
Issue-Key: greengrass|ListComponents|2xx->5xx|$.components
Issue-Key: greengrass|ListComponents|2xx->5xx|$headers.content-type
Issue-Key: greengrass|ListComponents|2xx->5xx|status
Issue-Commit: 6e0b8b1107b57545eaa8864803a2ad1310139146

# Greengrass V2 component operations all return 500 AmbiguousService instead of their modeled restJson1 responses

> _Note: this issue was identified against the code base at commit `6e0b8b1107b57545eaa8864803a2ad1310139146`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComponentVersion

## Description
This bundle carries 8 causes. Every one of them is a Greengrass V2 component-surface operation that the emulator refuses before dispatch: the router keys emulated services by endpoint prefix alone, and both com.amazonaws.greengrass#Greengrass (2017-06-07) and com.amazonaws.greengrassv2#GreengrassV2 (2020-11-30) declare awsService.endpointPrefix = 'greengrass' / arnNamespace 'greengrass' / sigv4Name 'greengrass' (Smithy: service_info on both models). The emulator answers 500 with content-type application/x-amz-json-1.1 and body {"__type": "AmbiguousService", "message": "2 emulated services all claim requests for 'greengrass': ... the request is rejected rather than served by an arbitrary one"}. AWS serves both API versions on greengrass.<region>.amazonaws.com and disambiguates them by the restJson1 @http method+uri binding (V2 lives under /greengrass/v2/...), never by refusing. There is an open issue filed from the BEHAVIORAL corpus with the same root cause, 'GreengrassV2: every request rejected with AmbiguousService because Greengrass v1 and v2 share endpointPrefix greengrass' (20261022T092619Z-greengrassv2-every-request-rejected-with-ambiguousservice-because-greengrass.md); it carries only check ids and no wire replay key, so these wire fixture bindings are net-new and this issue exists to gate them.

Cause 1 of 8 - CreateComponentVersion happy path.
AWS: Smithy GreengrassV2.CreateComponentVersion, http POST /greengrass/v2/createComponentVersion, success code 201, content-type application/json; output members componentName, componentVersion, creationTimestamp and status (CloudComponentStatus) are @required, arn matches ^arn:[^:]*:greengrass:[^:]*:(aws|[0-9]+):components:[^:]+:versions:[^:]+$. Recorded response: 201 with arn/componentName/componentVersion/creationTimestamp/status{componentState REQUESTED, vendorGuidance ACTIVE}.
Emulator: 500, content-type application/x-amz-json-1.1, none of those members present.
Why: a client cannot publish a component version at all, and gets no arn to reference in any later call, so the whole component lifecycle is unreachable.
Fix: the request router / service-registry dispatch layer (no CreateComponentVersion handler is reached), plus the restJson1 error serializer that emits x-amz-json-1.1 for a restJson1 service.
Traces: greengrassv2-component-error-paths#setup#1 (also greengrassv2-component-resolution-and-artifact#setup#1, greengrassv2-component-version-lifecycle#setup#1).

Cause 2 of 8 - CreateComponentVersion duplicate version.
AWS: ConflictException is a modeled error of CreateComponentVersion with httpError 409 (Smithy). Recorded response: 409, x-amzn-errortype ConflictException, message (example) 'Component [<name> : <version>] for account <acct> already exists with state: [DEPLOYABLE]'.
Emulator: 500 with x-amzn-errortype AmbiguousService.
Why: a client re-publishing an existing component version cannot distinguish 'already exists' from a server fault, and idempotent-publish logic breaks.
Fix: same dispatch layer; then CreateComponentVersion's conflict check in its handler/state.
Traces: greengrassv2-component-error-paths#task#2.

Cause 3 of 8 - DescribeComponent on a nonexistent component version.
AWS: ResourceNotFoundException is modeled with httpError 404. Recorded response: 404, x-amzn-errortype ResourceNotFoundException, message (example) 'Component (<name>:<version>) does not exist in account <acct>.'
Emulator: 500 / AmbiguousService.
Why: a client probing for existence sees a retryable server error and burns its retry budget instead of learning the component is absent.
Fix: same dispatch layer; then DescribeComponent's lookup/validation.
Traces: greengrassv2-component-error-paths#task#3.

Cause 4 of 8 - GetComponent on a nonexistent component version.
AWS: Smithy GreengrassV2.GetComponent, GET /greengrass/v2/components/{arn}, errors include ResourceNotFoundException (404) and ValidationException (400). Recorded response: 404, x-amzn-errortype ResourceNotFoundException, message (example) 'PRIVATE component (<name>:<version>) under account <acct> does not exist'.
Emulator: 500 / AmbiguousService.
Why: recipe fetches cannot be distinguished from transient failures, so a client retries forever on a permanent condition.
Fix: same dispatch layer; then GetComponent's lookup.
Traces: greengrassv2-component-error-paths#task#4.

Cause 5 of 8 - DeleteComponent on a nonexistent component version.
AWS: ResourceNotFoundException modeled, httpError 404. Recorded response: 404, x-amzn-errortype ResourceNotFoundException, message (example) 'Component [<name> : <version>] in the accountId [<acct>] does not exist'.
Emulator: 500 / AmbiguousService.
Why: cleanup code cannot treat an already-deleted component as a no-op.
Fix: same dispatch layer; then DeleteComponent's lookup.
Traces: greengrassv2-component-error-paths#task#5.

Cause 6 of 8 - ResolveComponentCandidates without device auth.
AWS: Smithy GreengrassV2.ResolveComponentCandidates, POST /greengrass/v2/resolveComponentCandidates, errors include AccessDeniedException with httpError 403. Recorded response: 403, x-amzn-errortype AccessDeniedException, body member Message (example 'Unable to determine service/operation name to be authorized').
Emulator: 500 / AmbiguousService, Message absent.
Why: this operation is the Greengrass nucleus' dependency-resolution path and is authorized by device certificate; a client cannot tell 'not authorized' from a server fault, which is the difference between fixing credentials and retrying.
Fix: same dispatch layer; then ResolveComponentCandidates' auth check and its error serializer.
Traces: greengrassv2-component-resolution-and-artifact#task#2.

Cause 7 of 8 - ListComponentVersions.
AWS: GreengrassV2.ListComponentVersions returns 200 application/json with a componentVersions list (entries carrying arn/componentName/componentVersion); a nonexistent component is reported through the operation's modeled errors, not a 500.
Emulator: 500 / AmbiguousService, componentVersions absent.
Why: version enumeration and its pagination are unusable, so clients cannot discover what versions exist.
Fix: same dispatch layer; then ListComponentVersions' handler/state.
Traces: greengrassv2-component-version-lifecycle#task#4 (also greengrassv2-list-component-versions-nonexistent-component#task#1).

Cause 8 of 8 - ListComponents.
AWS: Smithy GreengrassV2.ListComponents, GET /greengrass/v2/components, success 200 application/json, output members components + nextToken, paginated (items=components, pageSize=maxResults). Recorded response: 200 with a components entry carrying arn, componentName and latestVersion{arn, componentVersion, creationTimestamp, description, platforms, publisher}.
Emulator: 500 / AmbiguousService, components absent.
Why: a client cannot enumerate published components or read latestVersion, so scope-filtered listing and pagination are both unreachable.
Fix: same dispatch layer; then ListComponents' handler/serializer.
Traces: greengrassv2-component-version-lifecycle#task#5.

## Evidence
Expected (AWS / recorded): each component operation answers with its modeled restJson1 result and content-type application/json - CreateComponentVersion 201 with required componentName/componentVersion/creationTimestamp/status; CreateComponentVersion duplicate 409 ConflictException; DescribeComponent/GetComponent/DeleteComponent on a missing version 404 ResourceNotFoundException; ResolveComponentCandidates unauthorized 403 AccessDeniedException; ListComponentVersions 200 with componentVersions; ListComponents 200 with components.

Actual (emulator): all eight answer HTTP 500 with content-type application/x-amz-json-1.1, x-amzn-errortype AmbiguousService where an error type is present, and body {"__type":"AmbiguousService","message":"2 emulated services all claim requests for 'greengrass': greengrass, greengrass; the request is rejected rather than served by an arbitrary one"}. No response member expected by any step is present. Replay score for the corpus was 6.8% with 47 mismatched steps; no state_reset_failures, so these are genuine response divergences.

Model citation: smithy_query service_info on services/service-com-amazonaws-greengrassv2-greengrassv2 -> GreengrassV2 2020-11-30, restJson1, endpointPrefix 'greengrass', arnNamespace 'greengrass', sigv4Name 'greengrass'; same three values on services/service-com-amazonaws-greengrass-greengrass (Greengrass 2017-06-07, restJson1). AmbiguousService is not a modeled error of either service.

## Steps to Reproduce
1. Boot the emulator from this checkout with its default service set (both greengrass and greengrassv2 register the 'greengrass' endpoint prefix).
2. Replay fixture greengrassv2-component-error-paths, step setup#1 (CreateComponentVersion): expected 201 with arn/componentName/componentVersion/creationTimestamp/status, observed 500 AmbiguousService.
3. Replay the same fixture's task#2..task#5 for the ConflictException/ResourceNotFoundException paths of CreateComponentVersion, DescribeComponent, GetComponent and DeleteComponent.
4. Replay greengrassv2-component-resolution-and-artifact#task#2 (ResolveComponentCandidates, expected 403 AccessDeniedException) and greengrassv2-component-version-lifecycle#task#4 / #task#5 (ListComponentVersions / ListComponents, expected 200).
5. Observe every step returns 500 with content-type application/x-amz-json-1.1 and errortype AmbiguousService. All 8 causes must route and answer with their modeled status/code/shape before this issue is closed.

## Resolution

The dispatch layer all eight causes shared is FIXED, and so are the eight causes' own
answers. Two parts of two recorded instances are DECLINED and recorded here so a later
sweep does not have to re-derive them.

- The claim rule — a service whose signing name is shared claims only the requests its
  OWN model binds a route for, and a route BOTH models bind goes to the single claimant
  a declared table names (the account- and thing-level facts both versions publish to one
  owner, a tagging request to the version that owns the resource its ARN names) — lives in
  `services/common/src/greengrass.rs` and is read by both crates' `can_handle`. That
  module landed on mainline from the sibling connectivity-info/service-role issue while
  this fix was in review; this fix was written against its own copy of the same rule and
  was MERGED onto it, so the rule is stated once.
  `core/router/tests/all/greengrass_signing_name_routing.rs` pins exactly one claimant for
  every Greengrass route with BOTH services mounted together — the configuration
  `core/router` ships, and the only one in which a two-claimant row is observable — and
  that the claimant then answers from its own state.
- `ListComponentVersions` (cause 7) and `ResolveComponentCandidates` (cause 6) are
  implemented; `CloudComponentStatus` now carries `message`/`errors`/
  `vendorGuidanceMessage`, an untagged resource reports the empty tag map, and
  `CreateComponentVersion`/`DescribeComponent`/`GetComponent`/`DeleteComponent` word
  their conflict and not-found answers as Greengrass words them. Covered by
  `services/service-com-amazonaws-greengrassv2-greengrassv2/tests/all/component_versions_and_resolution.rs`.
- DECLINED, `greengrass|ResolveComponentCandidates|…|{status,$headers.x-amzn-errortype,$.Message}`:
  the recorded 403 carries its detail in `Message`, while this service's modelled
  `AccessDeniedException` member is `message` — so that answer came from AWS's
  authorization layer failing to name a service/operation pair for the recording
  principal, not from the modelled service. The same message was declined for the same
  reason in `20261022T192333Z-m2-returns-wrong-http-status-and-error-type.md`. The
  recorded request also names a component version the fixture never published, so the
  modelled answer for it is the 404 this emulator now gives, and the operation itself
  resolves candidates out of the published catalog.
- DECLINED, the `[state]` variable of cause 2's conflict message: its fixed text is now
  AWS's exactly — `Component [<name> : <version>] for account [<account>] already exists
  with state: [<state>]`, read off the RECORDED response body, which brackets the account
  as well as the component and the state (the prose paraphrase in cause 2 above dropped
  the brackets around the account; where the two disagree the recording governs). The
  state token is read from this emulator's state. The recording shows `DEPLOYABLE`
  because AWS's recipe validation had completed before the duplicate create arrived; the
  emulator owes that transition over a non-zero validation span, so a duplicate create
  issued immediately after the publish sees `REQUESTED`. No non-zero span matches that
  recording (a 5 ms span was tried).
- Still open, and NOT causes of this issue: `GetComponent` re-renders a stored recipe
  into the other format with its object keys sorted rather than in document order, and
  `DescribeComponent` reports `componentState REQUESTED` for a version read inside the
  validation span.
