Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig.session-against-deleted-environment-rejected
Issue-Shape-Id: com.amazonaws.appconfig#AmazonAppConfig
Issue-Key: appconfig.session-against-deleted-environment-rejected|run#32|Timeout|time.sleep(5.0)
Issue-Commit: 95a9631b0cf16d01c2457a2264362dbc85f1bd52

# AppConfigData keeps serving a deleted environment: a pre-deletion configuration token polls successfully forever instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `95a9631b0cf16d01c2457a2264362dbc85f1bd52`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLatestConfiguration

## Description
One net-new root cause, grounded in the replayed behavioral check appconfig.session-against-deleted-environment-rejected (the only failing check in this run that the report did not mark ALREADY FILED or OWNED). Close it and re-replay the check id in trace_ids.

=== Cause 1 of 1: deleting a deployed environment never withdraws it from the AppConfigData data plane, so a token minted before the deletion keeps being served ===

AWS: AppConfigData sessions are over LIVE control-plane state. Once the environment a session polls is deleted (appconfig DeleteEnvironment, after which GetEnvironment answers ResourceNotFoundException and the environment is gone from ListEnvironments), the data plane withdraws it: GetLatestConfiguration presented with a token minted BEFORE the deletion is refused with ResourceNotFoundException, HTTP 404, and StartConfigurationSession for that same (application, environment, configuration profile) triple is refused the same way. Smithy (services/service-com-amazonaws-appconfigdata-appconfigdata/smithy-model/com-amazonaws-appconfigdata-appconfigdata.json): GetLatestConfiguration is GET /configuration, success 200, input ConfigurationToken (@httpQuery configuration_token, @required, pattern ^\S{1,8192}$), and models ResourceNotFoundException with httpError 404 and members Message, ResourceType -> ResourceType enum {Application, ConfigurationProfile, Deployment, Environment, Configuration}, ReferencedBy -> StringMap ("a map indicating which parameters in the request reference the resource that was not found"). For a deleted environment the refusal is ResourceType 'Environment'. The model bounds nothing else here; the fact that the withdrawal is not instantaneous is recorded behavior only — the check polls for the refusal rather than assuming it, and tolerates lag (its own budget is 300s per refusal). The fixed text of the message is the service's own not-found sentence; its variable parts are the identifier values taken from the session/request, never one run's literal ids.

Emulator: the refusal never arrives, so a bounded poll for it never converges and the check exceeds the harness limit — reported verdict: 'Error: Check timed out after 60.0s'. This is by construction in the checkout, not an accident: services/service-com-amazonaws-appconfigdata-appconfigdata/src/ops/get_latest_configuration.rs STEP 2 (around lines 340-392, and stated again in the module doc at lines 8-11) re-reads the control plane only to pick up a NEWER deployment and, when the triple no longer resolves, falls back to session.bound_configuration — 'A triple the control plane can no longer resolve leaves the session served from the configuration it is bound to'. Each poll re-binds the successor token to the same configuration (next_session.bound_configuration = Some(Arc::clone(&deployed))), so the poll chain is immortal: a token minted before DeleteEnvironment goes on returning HTTP 200 with the deleted environment's bytes indefinitely, and ResourceNotFoundException is reachable only for a session that was bound to no configuration at all.

Why: a client relying on AWS's behavior gets a deleted resource's configuration served to it forever. The deletion is observable on the control plane (GetEnvironment 404, absent from ListEnvironments) but invisible on the data plane, so (a) a long-polling client never learns its environment was removed and keeps applying withdrawn configuration, (b) a token is effectively a permanent grant on deleted state — a capability AWS revokes — and (c) any negative test asserting 'a deleted environment stops serving' passes against AWS and hangs/fails here. Because the hang is a never-terminating poll rather than a wrong value, it also costs the whole check: the positive controls after it (the sibling environment still serving its exact bytes) are never reached.

Fix: the appconfigdata data plane's session resolution — GetLatestConfiguration's handler (services/service-com-amazonaws-appconfigdata-appconfigdata/src/ops/get_latest_configuration.rs, STEP 2) and the state helper it calls (crate::state::served_configuration / resolve_deployed_triple): when the control plane no longer resolves the session's application, environment or configuration profile, the poll must be refused with the modeled ResourceNotFoundException (404) carrying ResourceType for the level that is gone ('Environment' here) and ReferencedBy naming the request/session member that referenced it, instead of falling back to Session::bound_configuration. The bound-configuration fallback should remain only for a control-plane store that cannot be READ (the InternalServerException path), not for a resource that has been deleted. StartConfigurationSession already resolves the triple and refuses, so only the poll path diverges.

Related open issues, neither of which covers this behavior: 20261024T112615Z-appconfigdata-data-plane-deployed-configuration-never-served-sessions.md (its Cause 1 is the opposite failure — the data plane serving nothing at all — and its Cause 2 is StartConfigurationSession minting sessions for unknown resources) and 20261103T032004Z-appconfigdata-startconfigurationsession-misreports-identifier-refusals-wrong-unresolved-tier.md (the tier/status of a StartConfigurationSession identifier refusal). Neither names a token minted before a deletion continuing to serve.

Traces: appconfig.session-against-deleted-environment-rejected.

## Evidence
Behavioral replay of suite simparity-behavioral-aws-v1 against the booted emulator: 197/240 passed, 33 mismatched, 10 errored, 0 skipped, 0 unloadable — the emulator answered requests throughout, so this is not a bootstrap failure. Every other failing check in the run was marked OWNED by an open issue and skipped; this was the only survivor.

Failing check (reported under '### appconfig'):
  [mismatch] appconfig.session-against-deleted-environment-rejected
  Error: Check timed out after 60.0s

What the check does and where it cannot converge (from the check source in the report): it creates an application, TWO environments (a 'doomed' one and a sibling), a hosted configuration profile and a 0-minute/100%-growth/0-bake deployment strategy; deploys the same hosted version to BOTH environments and confirms both serve the exact bytes b'{"shared":"between-both-environments"}' through fresh StartConfigurationSession + GetLatestConfiguration pairs (the barrier the refusal is measured against). It then deletes only the doomed environment with DeletionProtectionCheck='BYPASS' and polls, each with a 300s budget:
  1. appconfig GetEnvironment on the deleted environment -> expects ResourceNotFoundException;
  2. ListEnvironments -> the deleted id absent, the sibling present;
  3. appconfigdata StartConfigurationSession on the deleted environment -> expects ResourceNotFoundException ('an emulator that keeps serving a deleted environment is over-permissive here');
  4. appconfigdata GetLatestConfiguration with pre_token, the token minted BEFORE the deletion -> expects ResourceNotFoundException ('a token is not a permanent grant on withdrawn state');
  5. the sibling still serving the exact bytes on a brand-new session.
Step 4 cannot ever be satisfied by this emulator, so its 300s poll (5s sleeps) runs past the harness's 60s cap and the check is killed with no assertion value to report.

Confirmed in the checkout (why the refusal is unreachable rather than merely late), services/service-com-amazonaws-appconfigdata-appconfigdata/src/ops/get_latest_configuration.rs:
  lines 8-11 (module doc): "A triple the control plane can no longer resolve leaves the session served from the configuration it is bound to, because resolving the triple is what STARTING a session does."
  lines 344-352 (STEP 2 comment): "a control-plane edit made after the session was bound cannot retroactively unmake it. Only a session bound to no configuration at all has nothing to serve, and that is what this operation reports as not found."
  lines 379-391: on Err(unresolved) the handler returns the bound configuration when session.bound_configuration is Some(..), and the modeled ResourceNotFoundException only when it is None.
  lines 423-425: every poll re-binds the successor token to the configuration just served, so the chain never degrades to the None case.
By contrast services/service-com-amazonaws-appconfigdata-appconfigdata/src/ops/start_configuration_session.rs lines 212-250 DOES resolve the triple against the control plane and refuse with ResourceNotFoundException + ResourceType + ReferencedBy, so step 3 of the check is served correctly and only the poll path diverges.

Smithy consulted: GetLatestConfiguration (GET /configuration, 200; ConfigurationToken @httpQuery configuration_token @required pattern ^\S{1,8192}$; errors BadRequestException 400, InternalServerException 500, ResourceNotFoundException 404, ThrottlingException 429) and ResourceNotFoundException {Message, ResourceType -> ResourceType {Application, ConfigurationProfile, Deployment, Environment, Configuration}, ReferencedBy -> StringMap}.

## Steps to Reproduce
Boot the emulator and point an appconfig client and an appconfigdata client at its data listener (region/credentials irrelevant).

1. appconfig: create_application -> create_environment (call it DOOMED) -> create_environment (call it SIBLING) -> create_configuration_profile(LocationUri='hosted') -> create_deployment_strategy(DeploymentDurationInMinutes=0, GrowthFactor=100, FinalBakeTimeInMinutes=0, ReplicateTo='NONE') -> create_hosted_configuration_version(Content=b'{"shared":"between-both-environments"}', ContentType='application/json').
2. start_deployment of that version into DOOMED and into SIBLING; poll get_deployment until each reports State COMPLETE.
3. appconfigdata: start_configuration_session(ApplicationIdentifier=app, EnvironmentIdentifier=DOOMED, ConfigurationProfileIdentifier=profile); keep the InitialConfigurationToken as PRE_TOKEN; get_latest_configuration(ConfigurationToken=PRE_TOKEN) and confirm the deployed bytes are served (the barrier). Do the same for SIBLING.
4. appconfig: delete_environment(ApplicationId=app, EnvironmentId=DOOMED, DeletionProtectionCheck='BYPASS'). Confirm get_environment on DOOMED now raises ResourceNotFoundException and DOOMED is absent from the paginated list_environments while SIBLING is present.
5. appconfigdata: get_latest_configuration(ConfigurationToken=PRE_TOKEN), retrying for a few minutes. Expected: ResourceNotFoundException / HTTP 404 with ResourceType 'Environment' and ReferencedBy naming the identifier that referenced the deleted environment. Actual: HTTP 200 on every attempt, served from the session's bound configuration, indefinitely — the refusal never appears, which is what makes the check run until the harness kills it at 60s.
6. Control: start_configuration_session on the same deleted triple IS refused with ResourceNotFoundException today, so the divergence is confined to the poll path; and a fresh session on SIBLING must still serve the exact bytes after the fix (the refusal must be scoped to the deleted environment, not an outage of the application, profile or endpoint).

Re-replay: appconfig.session-against-deleted-environment-rejected.
