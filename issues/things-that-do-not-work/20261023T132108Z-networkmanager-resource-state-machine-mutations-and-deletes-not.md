Issue-Source: trace-sweep-divergence
Issue-Trace: networkmanager-connection-workflow
Issue-Step: teardown#20
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: networkmanager-connection-workflow networkmanager-link-lifecycle-and-association networkmanager-core-network-lifecycle networkmanager-global-network-lifecycle networkmanager-site-lifecycle-with-tags
Issue-Shape-Id: com.amazonaws.networkmanager#NetworkManager
Issue-Key: networkmanager|DeleteSite|2xx->4xx|$.Site
Issue-Key: networkmanager|DeleteSite|2xx->4xx|status
Issue-Key: networkmanager|DeleteGlobalNetwork|2xx->4xx|$.GlobalNetwork
Issue-Key: networkmanager|DeleteGlobalNetwork|2xx->4xx|status
Issue-Key: networkmanager|CreateCoreNetwork|2xx->2xx|$.CoreNetwork.State
Issue-Key: networkmanager|GetCoreNetwork|2xx->2xx|$.CoreNetwork.Description
Issue-Key: networkmanager|GetCoreNetwork|4xx->2xx|$
Issue-Key: networkmanager|GetCoreNetwork|4xx->2xx|$.Message
Issue-Key: networkmanager|GetCoreNetwork|4xx->2xx|$.ResourceId
Issue-Key: networkmanager|GetCoreNetwork|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: networkmanager|GetCoreNetwork|4xx->2xx|status
Issue-Key: networkmanager|UpdateGlobalNetwork|2xx->2xx|$.GlobalNetwork.State
Issue-Key: networkmanager|GetSites|2xx->2xx|$.Sites[*]
Issue-Key: networkmanager|DeleteSite|2xx->2xx|$.Site.Description
Issue-Commit: 88a6b3d80dac2fa3529d7971147a50fe7f0d0905

# networkmanager resource state machine: mutations and deletes not applied to stored state

> _Note: this issue was identified against the code base at commit `88a6b3d80dac2fa3529d7971147a50fe7f0d0905`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteSite

## Description
The networkmanager emulator does not maintain the lifecycle state of its resources: Update* calls do not persist onto the stored resource, Delete* calls either refuse with a conflict or leave the resource readable forever, and the State enum returned by create/update does not reflect the transition that was just requested. This bundle carries EIGHT distinct causes; all eight must be closed.

Cause 1 of 8 — DeleteSite refuses with 409 instead of returning the site in DELETING.
AWS: Smithy: DeleteSite -> DELETE /global-networks/{GlobalNetworkId}/sites/{SiteId}, http code 200, output DeleteSiteResponse{Site: Site}; Site.State targets SiteState enum [PENDING, AVAILABLE, DELETING, UPDATING]. ConflictException (409) is modeled but the recording shows a plain teardown delete of a site whose dependent connection/link were already deleted succeeding with 200 and Site.State = 'DELETING'.
Emulator: answers 409 and emits no $.Site member at all.
Why: a client (or any teardown/CloudFormation-style unwind) that deletes children and then the site cannot ever delete it; the 409 is read as a real dependency conflict, so the caller retries or aborts with leaked resources.
Fix: the DeleteSite handler plus the site state store in the networkmanager crate — the conflict predicate must only fire for still-existing dependents, and the success path must mark the site DELETING and echo the Site body.
Traces: networkmanager-connection-workflow#teardown#20 (representative), networkmanager-link-lifecycle-and-association#teardown#14.

Cause 2 of 8 — DeleteGlobalNetwork refuses with 409 instead of returning the global network in DELETING.
AWS: Smithy: DeleteGlobalNetwork -> DELETE /global-networks/{GlobalNetworkId}, http code 200, output {GlobalNetwork: GlobalNetwork}; GlobalNetworkState enum [PENDING, AVAILABLE, DELETING, UPDATING]. Recording: 200 with GlobalNetwork.State = 'DELETING' after its sites/links/core network were deleted.
Emulator: 409, no $.GlobalNetwork member.
Why: the top-level container can never be torn down, so every fixture/customer flow that ends in DeleteGlobalNetwork fails and the account accumulates undeletable global networks.
Fix: the DeleteGlobalNetwork handler and its dependency check in the networkmanager crate's state module.
Traces: networkmanager-connection-workflow#teardown#21 (representative), networkmanager-core-network-lifecycle#teardown#9, networkmanager-link-lifecycle-and-association#teardown#15.

Cause 3 of 8 — CreateCoreNetwork reports the wrong State.
AWS: Smithy: CreateCoreNetwork -> POST /core-networks, output {CoreNetwork: CoreNetwork}, CoreNetwork.State targets CoreNetworkState enum [CREATING, UPDATING, AVAILABLE, DELETING]. Recorded behavior only: the recording's CreateCoreNetwork response carries CoreNetwork.State = 'AVAILABLE'.
Emulator: returns CoreNetwork.State = 'CREATING'.
Why: callers that gate on the create response (or poll until AVAILABLE) see a core network stuck in a transitional state the emulator never advances, so the next step in the workflow never starts.
Fix: the CreateCoreNetwork handler's state assignment in the networkmanager crate.
Traces: networkmanager-core-network-lifecycle#setup#2.

Cause 4 of 8 — UpdateCoreNetwork is not persisted; GetCoreNetwork returns the pre-update description.
AWS: Smithy: UpdateCoreNetwork -> PATCH /core-networks/{CoreNetworkId} with Description (ConstrainedString, @length max 256), output {CoreNetwork: CoreNetwork}; a subsequent GetCoreNetwork must observe the updated Description.
Emulator: after UpdateCoreNetwork, GetCoreNetwork still reports the original description ($.CoreNetwork.Description value_mismatch — expected the updated value, got the value supplied at create time).
Why: the write is silently dropped. A client that updates and then reads back believes its change was rejected, or worse keeps serving the stale configuration.
Fix: the UpdateCoreNetwork handler must write the mutated record back into the networkmanager state store rather than only shaping a response.
Traces: networkmanager-core-network-lifecycle#task#5.

Cause 5 of 8 — DeleteCoreNetwork does not remove the core network; GetCoreNetwork keeps returning 200.
AWS: Smithy: DeleteCoreNetwork -> DELETE /core-networks/{CoreNetworkId}; GetCoreNetwork models ResourceNotFoundException (httpError 404). Recording: after the delete, GetCoreNetwork answers 404 with x-amzn-errortype ResourceNotFoundException and a body carrying ResourceId = 'coreNetworkId' (example message: 'Resource not found.').
Emulator: GetCoreNetwork answers 200 with the full CoreNetwork, State still 'AVAILABLE', for the whole 60s/7-attempt poll window; no error code is ever produced.
Why: deletion is unobservable. Any client that polls for ResourceNotFoundException to confirm a delete loops until its own timeout, and resource counts/quotas never drop.
Fix: the DeleteCoreNetwork handler plus the networkmanager state store — the delete must transition to DELETING and then remove the record so GetCoreNetwork raises ResourceNotFoundException (404).
Traces: networkmanager-core-network-lifecycle#teardown#8.

Cause 6 of 8 — UpdateGlobalNetwork reports the wrong State.
AWS: Smithy: UpdateGlobalNetwork -> PATCH /global-networks/{GlobalNetworkId}, output {GlobalNetwork: GlobalNetwork}, State targets GlobalNetworkState enum which includes 'UPDATING'. Recorded behavior only: the update response carries GlobalNetwork.State = 'UPDATING'.
Emulator: returns 'AVAILABLE' from the update call.
Why: the UPDATING transition is never visible, so clients that assert on or wait for the transitional state (and anything that distinguishes an in-flight update from a settled one) behave differently against the emulator than against AWS.
Fix: the UpdateGlobalNetwork handler's state transition in the networkmanager crate.
Traces: networkmanager-global-network-lifecycle#task#3.

Cause 7 of 8 — UpdateSite is not persisted; GetSites returns the pre-update description.
AWS: Smithy: UpdateSite -> PATCH /global-networks/{GlobalNetworkId}/sites/{SiteId} with Description, output {Site: Site}; GetSites -> GET /global-networks/{GlobalNetworkId}/sites must list the updated Site.
Emulator: $.Sites[0] still carries the description supplied at CreateSite instead of the description set by the intervening UpdateSite (every other member of the listed Site matches).
Why: same lost-write as cause 4, on the site resource: read-after-update returns stale data, so a client cannot confirm or trust its own mutation.
Fix: the UpdateSite handler must commit the mutated site into the networkmanager state store.
Traces: networkmanager-site-lifecycle-with-tags#task#5.

Cause 8 of 8 — DeleteSite echoes the pre-update description (same lost UpdateSite write, observed on the delete response).
AWS: Smithy: DeleteSite output {Site: Site} returns the site as last configured, i.e. with the description set by the preceding UpdateSite.
Emulator: $.Site.Description is the original create-time description.
Why: the delete response is the last chance a caller has to record what it deleted; returning superseded attributes misreports it.
Fix: follows from cause 7 — once UpdateSite persists, the DeleteSite handler's echo of the stored site is correct.
Traces: networkmanager-site-lifecycle-with-tags#teardown#11.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from mainline (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):

Cause 1 DeleteSite — expected status 200, got 409; '$.Site: missing (expected {... 'State': 'DELETING' ...}, got None)'. Cluster also covers networkmanager-link-lifecycle-and-association#teardown#14.
Cause 2 DeleteGlobalNetwork — expected 200, got 409; '$.GlobalNetwork: missing (expected {... 'State': 'DELETING' ...}, got None)'. Cluster also covers networkmanager-core-network-lifecycle#teardown#9 and networkmanager-link-lifecycle-and-association#teardown#15.
Cause 3 CreateCoreNetwork — '$.CoreNetwork.State: value_mismatch (expected 'AVAILABLE', got 'CREATING')' (status 200 both sides).
Cause 4 GetCoreNetwork — '$.CoreNetwork.Description: value_mismatch (expected 'updated core network description', got 'initial core network description')'.
Cause 5 GetCoreNetwork after delete — expected 404, got 200; '$: poll capped at 60s'; '$: poll: error code None not in ['ResourceNotFoundException'] after 7 attempt(s)'; '$.Message: missing (expected 'Resource not found.', got None)'; '$.ResourceId: missing (expected 'coreNetworkId', got None)'; '$headers.x-amzn-errortype: missing'. Actual body still 'State': 'AVAILABLE'.
Cause 6 UpdateGlobalNetwork — '$.GlobalNetwork.State: value_mismatch (expected 'UPDATING', got 'AVAILABLE')'.
Cause 7 GetSites — '$.Sites[0]: missing (expected {... 'Description': 'updated-site' ...}, got {... 'Description': 'sim2real-probe-networkmanager-site-desc-c5f39812' ...})'.
Cause 8 DeleteSite — '$.Site.Description: value_mismatch (expected 'updated-site', got 'sim2real-probe-networkmanager-site-desc-c5f39812')'.

Model basis: services/service-com-amazonaws-networkmanager-networkmanager/smithy-model/com-amazonaws-networkmanager-networkmanager.json — SiteState/GlobalNetworkState = [PENDING, AVAILABLE, DELETING, UPDATING], CoreNetworkState = [CREATING, UPDATING, AVAILABLE, DELETING]; GetCoreNetwork models ResourceNotFoundException @httpError 404. Handlers live under services/service-com-amazonaws-networkmanager-networkmanager/src/ops/ with the store in src/state.rs (crate layout per src/lib.rs lines 10-15).

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from mainline.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: fixture networkmanager-connection-workflow, step teardown#20 (DeleteSite) -> 409 instead of 200 + Site{State: DELETING}.
4. Cause 2: networkmanager-connection-workflow#teardown#21 (DeleteGlobalNetwork) -> 409 instead of 200 + GlobalNetwork{State: DELETING}.
5. Cause 3: networkmanager-core-network-lifecycle#setup#2 (CreateCoreNetwork) -> CoreNetwork.State 'CREATING' instead of 'AVAILABLE'.
6. Cause 4: networkmanager-core-network-lifecycle#task#5 (GetCoreNetwork after UpdateCoreNetwork) -> stale Description.
7. Cause 5: networkmanager-core-network-lifecycle#teardown#8 (GetCoreNetwork after DeleteCoreNetwork) -> 200 AVAILABLE instead of 404 ResourceNotFoundException.
8. Cause 6: networkmanager-global-network-lifecycle#task#3 (UpdateGlobalNetwork) -> State 'AVAILABLE' instead of 'UPDATING'.
9. Cause 7: networkmanager-site-lifecycle-with-tags#task#5 (GetSites after UpdateSite) -> stale Sites[0].Description.
10. Cause 8: networkmanager-site-lifecycle-with-tags#teardown#11 (DeleteSite) -> stale Site.Description.
