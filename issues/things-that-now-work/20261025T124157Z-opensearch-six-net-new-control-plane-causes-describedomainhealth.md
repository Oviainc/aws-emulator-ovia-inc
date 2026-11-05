Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.domain-health-reporting opensearch.domain-upgrade-invalid-target-version-rejected opensearch.overly-permissive-scheduled-action-invalid-type-rejected opensearch.vpc-endpoint-access-lifecycle opensearch.rollback-service-software-disabled-without-active-update opensearch.server-side-service-software-rollback-window-enforced-rejected opensearch.update-vpc-endpoint-out-of-range-subnet-rejected
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.domain-health-reporting|run#17|AssertionError|assert code == "BaseException", f"expected BaseException, got {code}"
Issue-Key: opensearch.domain-upgrade-invalid-target-version-rejected|run#11|AssertionError|assert code == "BaseException", (
Issue-Key: opensearch.overly-permissive-scheduled-action-invalid-type-rejected|run#7|AssertionError|assert invalid_message == expected, f"expected message {expected!r}, got {invalid_message!r}"
Issue-Key: opensearch.vpc-endpoint-access-lifecycle|run#12|AssertionError|assert authorized.get("PrincipalType") == "AWS Service", (
Issue-Key: opensearch.rollback-service-software-disabled-without-active-update|_wait_service_software_settled#3|Timeout|time.sleep(20)
Issue-Key: opensearch.server-side-service-software-rollback-window-enforced-rejected|_wait_service_software_settled#5|Timeout|time.sleep(20)
Issue-Key: opensearch.update-vpc-endpoint-out-of-range-subnet-rejected|_wait_domain_active#13|Timeout|time.sleep(interval_s)
Issue-Commit: df4ff7c41878e3b93504408794fd7c6fb8d98df7

# opensearch: six net-new control-plane causes — DescribeDomainHealth not-found uses the wrong error class, UpgradeDomain refuses nothing, two refusal messages diverge, and the service-software rollback / VPC-endpoint-update paths never become reachable

> _Note: this issue was identified against the code base at commit `df4ff7c41878e3b93504408794fd7c6fb8d98df7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomainHealth

## Description
Six net-new root causes in the OpenSearch Service control plane, all grounded in surviving behavioral checks of the `opensearch` service (every other opensearch failure in this run is already marked ALREADY FILED or OWNED and was skipped).

=== Cause 1 of 6: DescribeDomainHealth answers a missing domain with ResourceNotFoundException instead of the BaseException envelope ===
AWS: Smithy: `DescribeDomainHealth` models BaseException (HTTP 400, "An error occurred while processing the request") and ResourceNotFoundException (HTTP 409) among its errors; which one a missing domain gets is not stated by the model, so this is recorded behavior only — the recording shows real AWS answering this operation for a domain that does not exist with error code `BaseException` and message `Domain not found: <DomainName>`, where `Domain not found: ` is fixed text and `<DomainName>` is the name taken verbatim from the request.
Emulator: answers error code `ResourceNotFoundException` (the handler's `domain_not_found(&input.domain_name)` path). The check fails with `expected BaseException, got ResourceNotFoundException`.
Why: a client that distinguishes OpenSearch's generic 400 envelope from its 409 not-found envelope — e.g. retry/classification logic, or a caller that treats 409 as a conflict worth retrying — classifies this refusal wrongly, and the HTTP status is 409 where AWS sends 400.
Fix: the refusal branch of the DescribeDomainHealth handler (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/describe_domain_health.rs), which must raise the modeled BaseException with the `Domain not found: <name>` message instead of ResourceNotFoundException. Note the open issue 20261025T020249Z-opensearch-control-plane-startdomainmaintenance-and-canceldomainconfigchange-answer-notimplemented.md mentions DescribeDomainHealth reads but does not carry this check's replay key or this refusal class.
Traces: opensearch.domain-health-reporting.

=== Cause 2 of 6: UpgradeDomain answers 501 NotImplemented, so a downgrade TargetVersion is never refused ===
AWS: Smithy: `UpgradeDomain` takes `DomainName`, `TargetVersion` (VersionString) and `PerformCheckOnly`, and models BaseException (HTTP 400) among its errors. An upgrade only ever moves forward, so a TargetVersion earlier than the domain's EngineVersion is not a valid path; recorded behavior shows AWS refusing it with error code `BaseException` and the message `Failed to submit upgrade. Upgrade from <EngineVersion> to <TargetVersion> not supported.` — `Failed to submit upgrade. Upgrade from `, ` to ` and ` not supported.` are fixed text, while `<EngineVersion>` is the domain's current engine version read from state and `<TargetVersion>` is the value from the request. The refused call leaves the domain's EngineVersion untouched.
Emulator: answers `NotImplemented` (501) for UpgradeDomain, so no refusal is produced at all. The check fails with `expected UpgradeDomain to reject a downgrade TargetVersion with BaseException, got 'NotImplemented'`.
Why: a client cannot tell a rejected downgrade from an unavailable API; nothing validates the upgrade path, and the positive control (a check-only run against a compatible target, then GetUpgradeStatus/GetUpgradeHistory) is equally unreachable.
Fix: implement the UpgradeDomain handler and its TargetVersion path validation (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/upgrade_domain.rs), using the existing `compatible_target_versions`/`upgrade_rank` relation in src/state.rs to decide the refusal. The open issue 20261025T024958Z-opensearch-upgradedomain-and-getupgradestatus-answer-501-notimplemented-so.md covers the upgrade lifecycle but does not carry this check's replay key or this refusal's message rule.
Traces: opensearch.domain-upgrade-invalid-target-version-rejected.

=== Cause 3 of 6: UpdateScheduledAction's enum ValidationException lists the ActionType values in the wrong order ===
AWS: Smithy: `ActionType` is an enum whose declared values are SERVICE_SOFTWARE_UPDATE, JVM_HEAP_SIZE_TUNING, JVM_YOUNG_GEN_TUNING; the wording of the framework validation message is recorded behavior only. The recording shows AWS answering a value outside the enum with ValidationException and the message `1 validation error detected: Value '<value>' at 'actionType' failed to satisfy constraint: Member must satisfy enum value set: [JVM_YOUNG_GEN_TUNING, SERVICE_SOFTWARE_UPDATE, JVM_HEAP_SIZE_TUNING]` — the whole sentence is fixed text except `<value>` (the rejected value from the request) and the member path `actionType`; the bracketed value set, INCLUDING its order, is fixed text.
Emulator: produces the same sentence but enumerates the set in the model's declaration order: `... enum value set: [SERVICE_SOFTWARE_UPDATE, JVM_HEAP_SIZE_TUNING, JVM_YOUNG_GEN_TUNING]`.
Why: the error message is part of the contract; a client or test that matches AWS's refusal sentence (or a human reading a diff of two runs) sees a mismatch, and a message that differs from AWS's wording is a defect even when the code and status agree.
Fix: the enum-value-set rendering used by the request validation layer for this service (the validation path behind services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_scheduled_action.rs), which must emit the value set in the order AWS publishes rather than the model's member order.
Traces: opensearch.overly-permissive-scheduled-action-invalid-type-rejected.

=== Cause 4 of 6: AuthorizedPrincipal.PrincipalType reports the enum NAME for a service grantee instead of the value AWS returns ===
AWS: Smithy: `AuthorizedPrincipal$PrincipalType` targets the `PrincipalType` enum, whose declared values are AWS_ACCOUNT and AWS_SERVICE. For the service-grantee case the recording diverges from the model: real AWS answers `AWS Service` (two words, space-separated) in `AuthorizeVpcEndpointAccess`/`ListVpcEndpointAccess` responses, so this is recorded behavior only.
Emulator: returns `AWS_SERVICE`. The check fails with `expected PrincipalType 'AWS Service' for a Service grantee, got 'AWS_SERVICE'`.
Why: a client that switches on the returned PrincipalType string, or that round-trips it into a grant list, never matches the service-grantee case; the whole VPC-endpoint access lifecycle (authorize, list, revoke) reads back a value AWS never sends.
Fix: the AuthorizedPrincipal projection for a service grantee — the state projection shared by AuthorizeVpcEndpointAccess and ListVpcEndpointAccess (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/authorize_vpc_endpoint_access.rs and src/state.rs's authorized-principal mapping).
Traces: opensearch.vpc-endpoint-access-lifecycle.

=== Cause 5 of 6: the service-software rollback decline is never observable, and its Description text diverges ===
AWS: Smithy: `RollbackServiceSoftwareUpdate` answers 200 with `RollbackServiceSoftwareOptions` (`RollbackAvailable` — "whether a service software rollback is available for the domain" — and `Description` — "a description of the rollback status"). Recorded behavior: on a settled domain with no deployment in flight AWS answers 200 with `RollbackAvailable` false and the Description `No previous software version available for rollback. Please contact AWS Support for assistance.` (entirely fixed text), leaves ServiceSoftwareOptions.UpdateStatus and CurrentVersion untouched, and answers ResourceNotFoundException with `Domain not found: <DomainName>` for a domain that was never created.
Emulator: both checks never reach the assertion — each hangs until the harness 60 s cap, because a domain created moments earlier is still reported Processing/Creating on the DescribeDomain that immediately follows CreateDomain (src/state.rs `PROVISIONING`), so each readiness poll costs the caller a whole poll interval and the "three identical ServiceSoftwareOptions reads" settling step then runs past the cap. Independently, the decline payload the handler would answer with is wrong: `ServiceSoftware::rollback_unavailable` (src/state.rs) builds `Description` as `No service software rollback is available for this domain.` / `The rollback window of this domain's last service software update has closed.` / `A service software deployment is already in progress on this domain.`, none of which is AWS's sentence.
Why: a caller probing whether a rollback is available gets a 200 whose Description no client or operator sentence-matching on AWS's text recognises, and the path cannot be exercised at all within the suite's budget, so the whole out-of-window rollback contract is unverified.
Fix: the decline branch of the RollbackServiceSoftwareUpdate handler and the Description text it reads (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/rollback_service_software_update.rs, src/state.rs `ServiceSoftware::rollback_unavailable`), plus making a just-created domain reach its settled state promptly enough that a poll-then-read caller converges (src/state.rs `PROVISIONING` / `Domain::settle`).
Traces: opensearch.rollback-service-software-disabled-without-active-update, opensearch.server-side-service-software-rollback-window-enforced-rejected.

=== Cause 6 of 6: the cross-VPC UpdateVpcEndpoint refusal is never reached — the check hangs to the cap in setup ===
AWS: Smithy: `UpdateVpcEndpoint` takes `VpcEndpointId` and `VpcOptions` and models ValidationException (HTTP 400); an endpoint's `VpcOptions.VPCId` is fixed at creation, so recorded behavior is that a subnet belonging to a different VPC than the endpoint's is refused with ValidationException, the endpoint's SubnetIds/SecurityGroupIds are left unchanged, and a subnet from the SAME VPC is accepted.
Emulator: the check never issues the UpdateVpcEndpoint call. It hangs until the 60 s cap while standing the fixture up: each just-created resource is reported not-yet-ready on the read that immediately follows its create (EC2 CreateVpc then the `vpc_available` waiter, twice; then CreateDomain then DomainProcessingStatus), so every readiness probe costs a whole waiter interval and the budget is gone before the endpoint exists. Downstream, `CreateVpcEndpoint` is still an unimplemented scaffold (src/ops/create_vpc_endpoint.rs), so the endpoint could not be created even with budget.
Why: the invariant that an endpoint's VPC is immutable is entirely unenforced and unobservable; a client can believe it moved an endpoint into another VPC.
Fix: UpdateVpcEndpoint's VpcOptions validation against the endpoint's stored VPCId (services/service-com-amazonaws-opensearch-amazonopensearchservice/src/ops/update_vpc_endpoint.rs), on top of a CreateVpcEndpoint that mints a record; and the promptness with which a freshly created VPC and domain are reported ready (src/state.rs `PROVISIONING` here, and the EC2 crate's VPC state transition). The open issues 20261025T034451Z-opensearch-associatepackage-associatepackages-createvpcendpoint-answer-notimplemented-instead-of.md and 20261024T055421Z-opensearch-createvpcendpoint-stub.md cover CreateVpcEndpoint itself but neither carries this check's replay key nor the cross-VPC update rule.
Traces: opensearch.update-vpc-endpoint-out-of-range-subnet-rejected.

## Evidence
Replay output, per cause:

Cause 1 — opensearch.domain-health-reporting [mismatch]:
  AssertionError: expected BaseException, got ResourceNotFoundException
  Expected: code `BaseException`, message `Domain not found: <missing-domain-name>`.
  Actual:   code `ResourceNotFoundException` (HTTP 409).
  Source: src/ops/describe_domain_health.rs `handle` -> `DescribeDomainHealthErrorModeled::ResourceNotFoundException(crate::state::domain_not_found(...))`.

Cause 2 — opensearch.domain-upgrade-invalid-target-version-rejected [mismatch]:
  AssertionError: expected UpgradeDomain to reject a downgrade TargetVersion with BaseException, got 'NotImplemented'
  Expected: code `BaseException`, message `Failed to submit upgrade. Upgrade from OpenSearch_2.11 to OpenSearch_1.0 not supported.` (versions are request/state values filling the template), and DescribeDomain.EngineVersion unchanged.
  Actual:   501 `NotImplemented`.

Cause 3 — opensearch.overly-permissive-scheduled-action-invalid-type-rejected [mismatch]:
  AssertionError: expected message "1 validation error detected: Value 'NOT_A_REAL_ACTION_TYPE' at 'actionType' failed to satisfy constraint: Member must satisfy enum value set: [JVM_YOUNG_GEN_TUNING, SERVICE_SOFTWARE_UPDATE, JVM_HEAP_SIZE_TUNING]", got "1 validation error detected: Value 'NOT_A_REAL_ACTION_TYPE' at 'actionType' failed to satisfy constraint: Member must satisfy enum value set: [SERVICE_SOFTWARE_UPDATE, JVM_HEAP_SIZE_TUNING, JVM_YOUNG_GEN_TUNING]"
  Code and status agree (ValidationException); only the bracketed value set's order diverges.

Cause 4 — opensearch.vpc-endpoint-access-lifecycle [mismatch]:
  AssertionError: expected PrincipalType 'AWS Service' for a Service grantee, got 'AWS_SERVICE'
  Smithy PrincipalType declares [AWS_ACCOUNT, AWS_SERVICE]; the recording shows AWS answering `AWS Service`.

Cause 5 — opensearch.rollback-service-software-disabled-without-active-update and opensearch.server-side-service-software-rollback-window-enforced-rejected [both mismatch]:
  Error: Check timed out after 60.0s (both checks, in the create-domain + ServiceSoftwareOptions settling phase).
  Expected on reaching the call: HTTP 200, RollbackAvailable=false, Description `No previous software version available for rollback. Please contact AWS Support for assistance.`; ResourceNotFoundException `Domain not found: <name>` for an uncreated domain; ServiceSoftwareOptions.UpdateStatus/CurrentVersion unchanged.
  Actual decline text in state: `No service software rollback is available for this domain.` (src/state.rs ServiceSoftware::rollback_unavailable).

Cause 6 — opensearch.update-vpc-endpoint-out-of-range-subnet-rejected [mismatch]:
  Error: Check timed out after 60.0s, before UpdateVpcEndpoint was ever called.
  Expected: ValidationException for a subnet in another VPC, endpoint's SubnetIds unchanged, same-VPC update accepted.
  Actual: no response observed; CreateVpcEndpoint is an unimplemented scaffold.

## Steps to Reproduce
Boot the emulator and replay the six checks named in trace_ids. Per cause:

1. CreateDomain a single-node OpenSearch_2.11 domain, wait for Created && !Processing, then call DescribeDomainHealth with a DomainName no domain exists for. Observe the error code and HTTP status: AWS answers BaseException/400 with `Domain not found: <name>`, the emulator answers ResourceNotFoundException/409.
2. On an active domain running OpenSearch_2.11 call UpgradeDomain(TargetVersion="OpenSearch_1.0", PerformCheckOnly=true). AWS refuses with BaseException `Failed to submit upgrade. Upgrade from OpenSearch_2.11 to OpenSearch_1.0 not supported.`; the emulator answers 501 NotImplemented.
3. On an active domain call UpdateScheduledAction(ActionID=<nonexistent>, ActionType="NOT_A_REAL_ACTION_TYPE", ScheduleAt="NOW") and compare the ValidationException message's bracketed enum value set with AWS's order.
4. Create a VPC/subnet/security group and a VPC domain, CreateVpcEndpoint, then AuthorizeVpcEndpointAccess for a SERVICE grantee and read the AuthorizedPrincipal back (and via ListVpcEndpointAccess). Compare PrincipalType with `AWS Service`.
5. CreateDomain a minimal domain, poll DescribeDomain until settled, read ServiceSoftwareOptions until three consecutive reads agree, then call RollbackServiceSoftwareUpdate. Observe that the checks exhaust the 60 s cap during that settling, and that the decline Description the handler would return is not AWS's sentence; also call RollbackServiceSoftwareUpdate for a never-created domain and compare with `Domain not found: <name>`.
6. Create two VPCs with subnets in the same AZ plus a security group in VPC A, CreateDomain with VPCOptions in VPC A, wait for DomainProcessingStatus==Active, CreateVpcEndpoint in VPC A, then UpdateVpcEndpoint with a subnet from VPC B. Observe the check never gets past setup within the cap, and that CreateVpcEndpoint is a scaffold stub.
