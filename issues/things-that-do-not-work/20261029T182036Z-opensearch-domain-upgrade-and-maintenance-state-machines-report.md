Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.dry-run-progress-reporting opensearch.domain-maintenance-lifecycle opensearch.upgrade-domain-incompatible-version-rejected opensearch.domain-upgrade-lifecycle opensearch.dissociate-packages-not-associated-rejected opensearch.vpc-endpoint-access-ops-on-missing-domain opensearch.update-vpc-endpoint-concurrent-config-race-conflict
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.dry-run-progress-reporting|run#8|AssertionError|assert dry_run_id, f"expected a DryRunId in the dry-run response, got {dry_run_resp}"
Issue-Key: opensearch.domain-maintenance-lifecycle|run#12|AssertionError|assert final_status == "SUCCESS", f"expected the maintenance action to SUCCEED, got {final_status!r}"
Issue-Key: opensearch.upgrade-domain-incompatible-version-rejected|run#15|AssertionError|assert code_bogus == "BaseException", (
Issue-Key: opensearch.domain-upgrade-lifecycle|run#22|AssertionError|assert code == "BaseException", f"expected BaseException, got {code}"
Issue-Key: opensearch.dissociate-packages-not-associated-rejected|run#16|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}: {message}"
Issue-Key: opensearch.vpc-endpoint-access-ops-on-missing-domain|run#7|AssertionError|assert code == 'ResourceNotFoundException', f'RevokeVpcEndpointAccess: expected ResourceNotFoundException, got {code}'
Issue-Key: opensearch.update-vpc-endpoint-concurrent-config-race-conflict|_wait_endpoint_status#6|Timeout|time.sleep(interval_s)
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# opensearch: domain-upgrade and maintenance state machines report the wrong values, DryRun returns no dry-run id, DissociatePackages mis-classifies its refusal, and RevokeVpcEndpointAccess is still NotImplemented (6 causes)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateDomainConfig

## Description
This issue bundles 6 root causes, all in the opensearch service. The open issue 20261027T004336Z-opensearch-ten-net-new-control-plane-causes-getmigration.md and 20261029T072536Z-opensearch-two-net-new-causes-updatevpcendpoint-accepts-a.md own other checks in the VPC-endpoint and migration surface; none of the checks below was marked owned by them. Each cause must be closed before submitting.

Cause 1 of 6: UpdateDomainConfig with DryRun returns neither DryRunResults nor DryRunProgressStatus.
AWS: Smithy: UpdateDomainConfig takes DryRun (boolean) and DryRunMode (enum) and its output has three members — DomainConfig (required), DryRunResults and DryRunProgressStatus. DryRunProgressStatus is a structure whose DryRunId (GUID, @length 36, @pattern "^\\p{XDigit}{8}-..."), DryRunStatus, CreationDate and UpdateDate are all REQUIRED. A Verbose dry run therefore answers with a trackable DryRunProgressStatus carrying a DryRunId, and the dry run can be polled by that id.
Emulator: the response contains only DomainConfig — no DryRunResults and no DryRunProgressStatus member at all, so a client reading response["DryRunProgressStatus"] raises KeyError: 'DryRunProgressStatus'.
Why: a dry run is indistinguishable from a real update: there is no id to track, no validation-failure list, and no status — the entire pre-change validation feature is unusable, and worse, the caller cannot tell whether the configuration change was actually applied.
Fix: UpdateDomainConfig's handler and response serializer must mint and return a DryRunProgressStatus (with a GUID DryRunId) and DryRunResults when DryRun is set, and must not apply the change.
Traces: opensearch.dry-run-progress-reporting.

Cause 2 of 6: GetDomainMaintenanceStatus reports a successfully completed action as COMPLETED instead of SUCCESS.
AWS: Smithy: GetDomainMaintenanceStatus.Status targets the MaintenanceStatus enum whose values are PENDING|IN_PROGRESS|COMPLETED|FAILED|TIMED_OUT. Recorded behavior (measured on real AWS): a successfully finished maintenance action is reported with Status="SUCCESS" — a value the enum does not list — not the COMPLETED the model documents.
Emulator: it reports Status='COMPLETED'.
Why: client code written against observed AWS behaviour waits for SUCCESS and never sees it, so a completed maintenance action is treated as still running; this is exactly the kind of wire quirk an emulator must reproduce rather than normalise to the model.
Fix: the terminal status the StartDomainMaintenance/GetDomainMaintenanceStatus handlers write into maintenance state.
Traces: opensearch.domain-maintenance-lifecycle.

Cause 3 of 6: UpgradeDomain refuses with the wrong error class for an unadvertised or incompatible target version.
AWS: Smithy: UpgradeDomain(DomainName, TargetVersion required) binds BaseException (400, defaulted), ValidationException (400), ResourceAlreadyExistsException (409), ResourceNotFoundException (409) and DisabledOperationException (409). Recorded behavior: a TargetVersion that GetCompatibleVersions never advertised for the domain's current engine version is refused with the unmodelled-generic BaseException envelope and a "not supported" message; the domain's EngineVersion does not move.
Emulator: the same call answers ResourceAlreadyExistsException in one path and ValidationException in another — two different wrong classes for the same rule.
Why: SDK error-type matching selects the wrong exception class, so a caller's upgrade-eligibility branch (retry with another target vs fail) takes the wrong path; ResourceAlreadyExistsException in particular tells the caller a resource conflict that does not exist.
Fix: the target-version eligibility check in the UpgradeDomain handler must emit the BaseException envelope AWS emits.
Traces: opensearch.upgrade-domain-incompatible-version-rejected, opensearch.domain-upgrade-lifecycle.

Cause 4 of 6: DissociatePackages refuses a package that is not associated with ResourceNotFoundException instead of ValidationException.
AWS: Smithy: DissociatePackages(PackageList, DomainName required) binds ValidationException (400), ConflictException (409), ResourceNotFoundException (409) and BaseException (400). Recorded behavior: dissociating a package that is not associated with the named domain is refused ValidationException (the request names a relationship that cannot be dissolved), not ResourceNotFoundException.
Emulator: ResourceNotFoundException with `Package <packageID> is not associated with domain <domainName>.`
Why: 400-vs-409 and the exception class change how clients retry; a ResourceNotFoundException invites a create-then-retry loop for a package that exists.
Fix: the not-associated refusal class in the DissociatePackages handler.
Traces: opensearch.dissociate-packages-not-associated-rejected.

Cause 5 of 6: RevokeVpcEndpointAccess answers NotImplemented.
AWS: Smithy: RevokeVpcEndpointAccess(DomainName required, Account, Service) binds BaseException (400), ValidationException (400), ResourceNotFoundException (409) and DisabledOperationException (409); its output is empty. For a syntactically valid DomainName that does not exist AWS refuses with ResourceNotFoundException — exactly as its sibling AuthorizeVpcEndpointAccess does (while ListVpcEndpointAccess is permissive and answers 200 with an empty list).
Emulator: the call answers the error code `NotImplemented` — the operation is a scaffold stub, so no revoke can ever be performed and the not-found rule is never reached.
Why: a client can grant VPC-endpoint access but never revoke it; and the error code it gets is not in the operation's modelled error set, so no SDK error handler matches it.
Fix: implement the RevokeVpcEndpointAccess handler (domain lookup -> ResourceNotFoundException for an unknown domain, otherwise remove the grant and return the empty output).
Traces: opensearch.vpc-endpoint-access-ops-on-missing-domain.

Cause 6 of 6: concurrent UpdateVpcEndpoint calls on one endpoint never produce a response.
AWS: Smithy: UpdateVpcEndpoint binds ConflictException (409) alongside ValidationException and ResourceNotFoundException; when two config updates race on the same VPC endpoint, one is accepted and the other is refused ConflictException — both calls terminate.
Emulator: the check exhausts the 60 s harness cap with no answer at all, so the race never serializes into an accept + a ConflictException.
Why: a client issuing concurrent updates hangs instead of receiving either the applied change or the conflict it must retry.
Fix: the in-flight-modification serialization in the UpdateVpcEndpoint handler / VPC-endpoint state.
Traces: opensearch.update-vpc-endpoint-concurrent-config-race-conflict.

## Evidence
Cause 1 — expected `DryRunProgressStatus` (with a 36-char GUID DryRunId) and/or DryRunResults in the UpdateDomainConfig response; actual: response contains only `{'DomainConfig': {...}}` -> `AssertionError: expected a DryRunId in the dry-run response` and `KeyError: 'DryRunProgressStatus'`.
Cause 2 — expected Status='SUCCESS'; actual `expected the maintenance action to SUCCEED, got 'COMPLETED'`.
Cause 3 — `expected BaseException for unadvertised target 'OpenSearch_1.0', got ResourceAlreadyExistsException`; `expected BaseException, got ValidationException`.
Cause 4 — `expected ValidationException, got 'ResourceNotFoundException': Package pkg-4ccf389695234ec2b60e077c0477edf9 is not associated with domain sim2real-dpa19a7ef977ee.`
Cause 5 — `RevokeVpcEndpointAccess: expected ResourceNotFoundException, got NotImplemented`.
Cause 6 — `Error: Check timed out after 60.0s` with no response to either concurrent UpdateVpcEndpoint.

## Steps to Reproduce
1. Cause 1: create a domain, then UpdateDomainConfig(DomainName=<d>, ClusterConfig={...changed...}, DryRun=true, DryRunMode='Verbose') and read DryRunProgressStatus/DryRunResults from the response.
2. Cause 2: StartDomainMaintenance on an active domain, then poll GetDomainMaintenanceStatus to a terminal value.
3. Cause 3: UpgradeDomain(DomainName=<OpenSearch_2.11 domain>, TargetVersion='OpenSearch_1.0', PerformCheckOnly=true) and inspect the error code.
4. Cause 4: DissociatePackages(PackageList=[<a package never associated>], DomainName=<existing domain>).
5. Cause 5: RevokeVpcEndpointAccess(DomainName=<syntactically valid, nonexistent>, Account=<caller account>).
6. Cause 6: create a VPC endpoint for a domain and issue two concurrent UpdateVpcEndpoint calls with different VpcOptions.
