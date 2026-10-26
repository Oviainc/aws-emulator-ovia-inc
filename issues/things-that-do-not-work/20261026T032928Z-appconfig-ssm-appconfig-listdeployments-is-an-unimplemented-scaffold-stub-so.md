Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__ssm.ssm-parameter-backed-profile-serves-the-deployed-parameter-version
Issue-Key: appconfig__ssm|ListDeployments|NotImplemented
Issue-Commit: 632275b31d4caea8230a511abd27a180321ca025

# AppConfig ListDeployments is an unimplemented scaffold stub, so the SSM-parameter-backed configuration profile check cannot run at all

> _Note: this issue was identified against the code base at commit `632275b31d4caea8230a511abd27a180321ca025`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDeployments

## Description
One root cause, grounded in the replayed behavioral check appconfig__ssm.ssm-parameter-backed-profile-serves-the-deployed-parameter-version (verdict: error). Close Cause 1 of 1 before submitting and re-replay the check id listed in trace_ids.

NOTE ON OVERLAP: the open issues 20261024T131534Z-appconfig-listdeployments-stub.md and 20261024T112615Z-appconfigdata-data-plane-deployed-configuration-never-served-sessions.md (its Cause 5) describe the same stub under the service tokens `appconfig` and `appconfigdata`. Neither carries THIS check id as a replay key and this check was NOT marked ALREADY FILED or OWNED, so this issue binds it. A fixer who has already implemented the operation for those issues only needs to re-replay the check id below to confirm this one.

=== Cause 1 of 1: ListDeployments answers 501 NotImplemented, which blocks the whole external-source (SSM parameter) profile scenario ===
AWS: ListDeployments (appconfig control plane, GET /applications/{ApplicationId}/environments/{EnvironmentId}/deployments, success 200) lists an environment's deployments in DESCENDING deployment-number order. Smithy (services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/com-amazonaws-appconfig-amazonappconfig.json): input ApplicationId (Name, len 1..64) @required, EnvironmentId (Name, len 1..64) @required, MaxResults (range 1..50), NextToken (len 1..2048); output Items (DeploymentList) and NextToken (len 1..2048) present only when a further page exists; errors BadRequestException 400, InternalServerException 500, ResourceNotFoundException 404. An environment with no deployment returns 200 with an empty Items list, not an error.
Emulator: HTTP 501 with error code 'NotImplemented' and message 'operation ListDeployments is scaffolded but not implemented in this emulator'. Confirmed in the checkout: services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/list_deployments.rs line 4 still carries the '//! SCAFFOLDED, NOT IMPLEMENTED:' marker and ListDeployments::handle returns Err(crate::scaffolded::Scaffolded::not_implemented("ListDeployments")) at line 165.
Why: every client that enumerates an environment's deployment history — including the paginator this check uses to tear its hierarchy down — gets a server-fault 501 instead of a list, so the SSM-parameter-source behavior it exists to pin is entirely unobservable: that a profile with LocationUri='ssm-parameter://<name>' plus a RetrievalRoleArn is accepted, that StartDeployment PINS the source version it named (overwriting the SSM parameter afterwards does not change what GetLatestConfiguration serves), that the data plane serves the parameter value as raw UTF-8 bytes with no wrapper, and that GetDeployment's ConfigurationLocationUri records the external source. A 501 is also not a modeled outcome of the operation, so a client cannot even handle it.
Fix: implement the operation in its own handler file, services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/list_deployments.rs, against the appconfig deployment state (resolve ApplicationId/EnvironmentId, refuse an unknown one with the modeled ResourceNotFoundException/404, return the environment's deployment summaries newest-first, honour MaxResults 1..50 and NextToken paging, and return an empty Items list for an environment with no deployment), then delete that file's SCAFFOLDED marker line. A 501 is correct for a stub — do not change the stub's error, implement the operation.
Traces: appconfig__ssm.ssm-parameter-backed-profile-serves-the-deployed-parameter-version.

## Evidence
Behavioral replay of suite simparity-behavioral-ga-prod-v1 against the booted emulator (97 checks, 50 passed, 25 mismatched, 22 errored, 0 skipped). bootstrap_signal was false.

Failing check: appconfig__ssm.ssm-parameter-backed-profile-serves-the-deployed-parameter-version [error]
  File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListDeployments operation: operation ListDeployments is scaffolded but not implemented in this emulator

The check reaches ListDeployments through its own paginator helper (_purge_items(ac.get_paginator("list_deployments").paginate(ApplicationId=app_id, EnvironmentId=env_id))) and through the post-BEAT-1 deployment-history snapshot it compares against, so the 501 aborts it before any SSM-source assertion is graded. The check header records it as BLESSED against real AWS (us-west-2) with H1-H8 confirmed, including H3 'a deployment PINS the source version it named' and H5 'GetDeployment's ConfigurationLocationUri records the external source'.

This check was NOT marked ALREADY FILED and NOT marked OWNED by an open issue in this run; the ALREADY FILED grouping for the same stub was attributed to the appconfig-only check appconfig.deployment-event-log-records-state-transitions.

Confirmed in the checkout — services/service-com-amazonaws-appconfig-amazonappconfig/src/ops/list_deployments.rs:
  line 4: //! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and
  line 165: Err(crate::scaffolded::Scaffolded::not_implemented("ListDeployments"))

Smithy consulted (same file, modeled contract reproduced in the handler's doc comment): ListDeployments — 'Lists the deployments for an environment in descending deployment number order.'; input ApplicationId (Name, len 1..64, required), EnvironmentId (Name, len 1..64, required), MaxResults (range 1..50), NextToken (len 1..2048); output Items (DeploymentList), NextToken (len 1..2048); errors BadRequestException 400, InternalServerException 500, ResourceNotFoundException 404.

## Steps to Reproduce
Boot the emulator and point an appconfig client at its data listener.

1. create_application(Name=<n>) -> create_environment(ApplicationId=<app>, Name=<n>).
2. appconfig.list_deployments(ApplicationId=<app>, EnvironmentId=<env>) for that environment, which has no deployment yet. EXPECTED: HTTP 200 with Items == [] and no NextToken. OBSERVED: HTTP 501, code 'NotImplemented', message 'operation ListDeployments is scaffolded but not implemented in this emulator'.
3. Then create an SSM parameter, a retrieval role, and create_configuration_profile(LocationUri='ssm-parameter://<name>', RetrievalRoleArn=<role>); start_deployment with ConfigurationVersion = the parameter's PutParameter Version; and page appconfig.get_paginator('list_deployments').paginate(ApplicationId=<app>, EnvironmentId=<env>). EXPECTED: the deployment summaries newest-first, with the deployment just started present. OBSERVED: the same 501, which aborts the scenario before the SSM-source assertions (version pinning, raw-bytes delivery, GetDeployment.ConfigurationLocationUri) are reached.
4. With more than MaxResults deployments in the environment, page with MaxResults=1 and assert NextToken round-trips and the order is descending by DeploymentNumber.

Re-replay appconfig__ssm.ssm-parameter-backed-profile-serves-the-deployed-parameter-version.
