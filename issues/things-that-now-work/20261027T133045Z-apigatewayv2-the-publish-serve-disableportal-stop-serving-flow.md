Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2.disable-portal-revokes-access-server-side
Issue-Shape-Id: com.amazonaws.apigatewayv2#ApiGatewayV2
Issue-Key: apigatewayv2.disable-portal-revokes-access-server-side|run#15|Timeout|time.sleep(10)
Issue-Commit: ac318820ce0cc5f19770024ed132bf63c4007170

# apigatewayv2: the publish → serve → DisablePortal → stop-serving flow for a portal's default domain never completes (1 cause)

> _Note: this issue was identified against the code base at commit `ac318820ce0cc5f19770024ed132bf63c4007170`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisablePortal

## Description
Cause 1 of 1: a portal's website lifecycle — PublishPortal putting content on the generated default domain, and DisablePortal taking it back offline — cannot be observed end to end. Failing check: apigatewayv2.disable-portal-revokes-access-server-side.

AWS: PublishPortal (Smithy: POST /v2/portals/{PortalId}/publish, success 202, no body; errors AccessDeniedException 403, BadRequestException 400, ConflictException 409, NotFoundException 404, TooManyRequestsException 429) publishes the portal — it builds the portal's content and puts it on the host GetPortal reports in EndpointConfigurationResponse$PortalDefaultDomainName ("This domain name is generated and managed by API Gateway"). While GetPortal$PublishStatus reads PUBLISHED, a plain unauthenticated GET of that host's root answers 200 with the portal's site. DisablePortal (Smithy: DELETE /v2/portals/{PortalId}/publish, success 204, no body; same five modeled errors) takes the publication down, not merely the status member: once PublishStatus settles on DISABLED the host must stop answering 200, and must keep refusing on every later probe (the revocation must hold, not blip). Smithy: PublishStatus = PUBLISHED | PUBLISH_IN_PROGRESS | PUBLISH_FAILED | DISABLE_IN_PROGRESS | DISABLE_FAILED | DISABLED — DISABLED is its one terminal not-serving value. Reference the check cites: https://docs.aws.amazon.com/apigateway/latest/developerguide/apigateway-portals-disable-portal.html

Emulator: the sequence never completes. The replay is killed by the harness budget — 'Check timed out after 60.0s' — with no phase of the check converging: CreatePortal/PublishPortal/DisablePortal answer, but at least one of the check's polls (GetPortal reaching PUBLISHED, the reported PortalDefaultDomainName answering 200 while published, GetPortal reaching DISABLED, or that domain ceasing to answer 200 once DISABLED has settled) does not reach its predicate inside the budget. No AWS error code or status is produced for the diverging step: the run is terminated, so the publish-then-revoke behavior is simply unobservable against this emulator.

Hypothesis (neither the model nor the recording states the mechanism): the service settles portal builds off the clock with a non-zero span (crate::state::PORTAL_BUILD, 500 ms), so a GetPortal issued immediately after PublishPortal/DisablePortal reports PUBLISH_IN_PROGRESS/DISABLE_IN_PROGRESS and each of the check's two status polls burns a full 15s poll interval; added to the check's mandatory ~30s 'revocation holds' window this exceeds the 60s budget even when every status and every probe is otherwise correct. The alternative mechanism is addressing: crate::state::announced_portal_default_domain_name announces the AWS-style host {portal-id}.portal.{region}.amazonaws.com whenever the CreatePortal request was not addressed at the emulator's own local endpoint host, and that host resolves to nothing on the client, so every probe of the announced domain fails to connect and the 'serves 200 while published' poll can never converge.

Why a client is misled: a caller cannot observe the thing publishing a portal exists to do (its site being served on the domain the service itself reported), and — the security-relevant half — cannot verify that DisablePortal actually revoked public access rather than only flipping PublishStatus. Code that gates a release on 'the portal stopped serving after DisablePortal' cannot be exercised at all.

Fix: belongs in the portal publish/disable settle timing in this service's state (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state.rs — the portal build span and Portal::settle/live_site) and in the portal data plane's addressing and serving (services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/service/portal.rs — claims/resolve and the status it answers for a live vs. a disabled build), together with the PublishPortal/DisablePortal handlers (src/ops/publish_portal.rs, src/ops/delete-side disable_portal.rs). A settle span short enough that a caller's immediate read-back sees the terminal status, and an announced default domain that is the address the portal data plane is actually reachable at, are both needed for this behavior to be observable.

Not covered by any open issue: 20261024T120951Z-apigatewayv2-the-execute-api-data-plane-never-serves.md concerns the execute-api invoke/WebSocket data planes, a different plane from the portal website domain, and the portal checks in this suite that are marked ALREADY FILED name unimplemented portal-product/product-page operations (DeletePortalProduct, ListPortalProducts, UpdateProductPage), none of which this check calls.

Traces: apigatewayv2.disable-portal-revokes-access-server-side.

## Evidence
Expected (AWS): PublishPortal → 202; GetPortal$PublishStatus settles on PUBLISHED; GET https://{PortalDefaultDomainName}/ → 200 (portal site). DisablePortal → 204; GetPortal$PublishStatus settles on DISABLED; GET https://{PortalDefaultDomainName}/ → a non-200 refusal, and every follow-up probe on the same cadence stays non-200.

Actual (emulator replay of apigatewayv2.disable-portal-revokes-access-server-side): verdict 'mismatch', detail 'Check timed out after 60.0s'. The check's own polls allow 900s for a PublishStatus transition and 600s for a probe transition, so a timeout means some poll's predicate was never satisfied within the harness budget: either PublishStatus did not settle in time, or the announced PortalDefaultDomainName never answered 200 while PUBLISHED, or it never stopped answering 200 once DISABLED. No HTTP status or error code is observable for the diverging step because the run was terminated.

## Steps to Reproduce
1. apigatewayv2 CreatePortal with Authorization={"None":{}}, EndpointConfiguration={"None":{}}, PortalContent={DisplayName, Theme.CustomColors{...}} → note PortalId.
2. PublishPortal(PortalId) and poll GetPortal(PortalId).PublishStatus until it reads PUBLISHED.
3. Read GetPortal(PortalId).EndpointConfiguration.PortalDefaultDomainName and GET that host's root path with no AWS credential → expect 200.
4. DisablePortal(PortalId) and poll GetPortal(PortalId).PublishStatus until it reads DISABLED.
5. GET the same URL again → expect a non-200, and re-probe on a 10s cadence for 30s → every read must stay non-200.
Against this emulator the whole sequence does not complete inside the 60s replay budget (behavioral check apigatewayv2.disable-portal-revokes-access-server-side).

## Status note (resolved: the flow is served and pinned end to end; the revocation now applies when it is accepted)

**What the flow does now.** A revocation takes effect when the call asking for it is ACCEPTED: the
address the portal announced stops serving from the instant `DisablePortal` answers its `204`, and
keeps refusing (`403` `{"message":"Forbidden"}`) on every later probe — through the take-down's own
span and past the span of a publish the disable superseded, which publishes nothing. What
`PublishStatus` reports meanwhile (`DISABLE_IN_PROGRESS` until the take-down comes due) is the work
the SERVICE has left to do, not whether public access is still granted
(`services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state.rs`: `Portal::live_site` and
`PortalTransition::revokes_publication`; `src/ops/disable_portal.rs`;
`src/service/portal.rs`). Before this change the domain went on serving the revoked build for the
whole take-down window, so a caller probing straight after the 204 was served the site it had just
revoked — the "blip" this report asks not to happen — and the only evidence of the revocation was
the status member the report says not to trust. A grant is the other way round and unchanged: the
`202` of `PublishPortal` means the service has only accepted the build, so it goes live when that
build comes due and until then the domain serves whatever the last settled publish left.

**The check's remaining timeout is its own cadence against the harness cap, not an emulator wait.**
Nothing in this emulator is waited on: both builds settle in `crate::state::PORTAL_BUILD` (500 ms).
The check polls `PublishStatus` on a 15 s cadence, and a caller's immediate read-back after each of
`PublishPortal` and `DisablePortal` reports that build in progress, so each poll costs it one 15 s
interval; with its mandatory ~30 s "revocation holds" window that is ~60 s against the suite's 60 s
cap. Those two windows cannot be removed: the recorded units
`apigatewayv2.disabled-portal-publish-accepted` and `apigatewayv2.disabled-portal-preview-accepted`
assert `PublishStatus == DISABLE_IN_PROGRESS` on the read immediately after `DisablePortal` ("the
race window this check probes never opened"), and `apigatewayv2.publish-portal-server-managed-status-transition`,
`apigatewayv2.portal-publish-server-managed-build` and
`apigatewayv2.portal-concurrent-publish-preview-conflict` depend on the publish's. An attempt to
settle the take-down inside the answering call was therefore reverted after the merge gate's
fidelity-regression reviewer caught the first two of those units failing on it.

**The report's alternative hypothesis — the announced `PortalDefaultDomainName` naming an address
the client cannot reach — is not the cause.** `state::announced_portal_default_domain_name` already
announces the emulator's own endpoint for a create addressed there and the generated AWS host
otherwise, and this report's replay key sits in the check's own 30 s hold window (`run#15`,
`time.sleep(10)`), past both of its probe polls.

Pinned by `tests/all/portal_publication_revocation.rs`: the whole publish → serve → disable →
stop-serving flow over six rows (both announced addressings; never published, published, a publish
still being built, a republish still being built, published twice; one and two disables; one to four
re-probes), plus a two-row table asserting the grant/revocation asymmetry against the domain and the
status at the same instant. `tests/all/portal_publication.rs` keeps the status windows and now also
covers a disable that supersedes a publish and a publication taken down twice.
