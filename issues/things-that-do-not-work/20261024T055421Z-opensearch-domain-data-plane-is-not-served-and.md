Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index opensearch.ingest-pipeline-rewrites-stored-source-and-simulate-predicts-it opensearch.ism-policy-transition-sets-read-only-and-unmet-condition-stays-writable opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching opensearch.nodes-apis-redact-service-internals-while-reporting-the-configured-topology opensearch.osq-deep-opensearch-2-highlight-wraps-matched-term-on-text-but-not-keyword-sibling opensearch.restore-over-open-index-rejected opensearch.snapshot-restore-index-round-trip opensearch.deploy-domain-cfn-access-policy-enforced opensearch.deploy-domain-cfn-index-operations-behave opensearch.deploy-opensearch-domain-cfn-behavioral opensearch.deploy-vpc-endpoint-cfn-policy-enforced opensearch.vpc-endpoint-list-and-delete-lifecycle-observed opensearch.deploy-opensearch-3-fgac-secret-master-basic-auth opensearch.dry-run-progress-ttl-expiration-requires-retrigger opensearch.dry-run-progress-ttl-expiry-after-completion opensearch.visibility-timeout-ttl-dry-run-progress-expiry opensearch.rollback-service-software-disabled-without-active-update opensearch.domain-lifecycle-create-active-delete opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index|_set_auto_create#4|AssertionError|assert status == 200, (
Issue-Key: opensearch.ingest-pipeline-rewrites-stored-source-and-simulate-predicts-it|run#7|AssertionError|assert status == 200, f"creating pipeline {pipeline!r} returned HTTP {status!r}: {put!r}"
Issue-Key: opensearch.ism-policy-transition-sets-read-only-and-unmet-condition-stays-writable|run#9|AssertionError|assert status == 201, f"PUT ISM policy {policy_id!r} returned HTTP {status!r}: {stored!r}"
Issue-Key: opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching|run#6|AssertionError|assert status == 200, f"creating the {field_type} index failed with HTTP {status}: {created}"
Issue-Key: opensearch.nodes-apis-redact-service-internals-while-reporting-the-configured-topology|run#6|AssertionError|assert status == 200, f"GET _nodes returned HTTP {status!r}, expected 200: {nodes_payload!r}"
Issue-Key: opensearch.osq-deep-opensearch-2-highlight-wraps-matched-term-on-text-but-not-keyword-sibling|_wait_data_plane_ready#4|AssertionError|assert status == 200, f"signed GET / on the domain endpoint returned {status!r}: {body!r}"
Issue-Key: opensearch.restore-over-open-index-rejected|run#8|AssertionError|assert status == 200, f"creating index {index!r} returned HTTP {status!r}: {created!r}"
Issue-Key: opensearch.snapshot-restore-index-round-trip|run#7|AssertionError|assert status == 200, f"creating index {index!r} returned HTTP {status!r}: {created!r}"
Issue-Key: opensearch|DescribeDomain|ResourceNotFoundException
Issue-Key: opensearch.deploy-opensearch-3-fgac-secret-master-basic-auth|run#16|KeyError|outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}
Issue-Key: opensearch.dry-run-progress-ttl-expiration-requires-retrigger|run#6|KeyError|dry_run_id = dry_run["DryRunProgressStatus"]["DryRunId"]
Issue-Key: opensearch.dry-run-progress-ttl-expiry-after-completion|run#9|AssertionError|assert "DryRunProgressStatus" in resp, (
Issue-Key: opensearch.visibility-timeout-ttl-dry-run-progress-expiry|run#7|KeyError|dry_run_id = dry_run_resp["DryRunProgressStatus"]["DryRunId"]
Issue-Key: opensearch.rollback-service-software-disabled-without-active-update|run#3|KeyError|software_before = status_before["ServiceSoftwareOptions"]
Issue-Key: opensearch.domain-lifecycle-create-active-delete|run#8|AssertionError|assert not created.get('Endpoint'), 'Endpoint must not be published while still Creating'
Issue-Key: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password|run#6|AssertionError|assert cc == {"InstanceType": "t3.small.search", "InstanceCount": 1, "ZoneAwarenessEnabled": False,
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# OpenSearch domain data plane is not served and CFN-deployed domains do not exist; DomainStatus misreports endpoint, dry runs and service software

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Seven distinct root causes in the opensearch service. All seven must be closed before this issue is resubmitted.

Cause 1 of 7: the OpenSearch domain REST data plane is not served at all.
AWS: a domain publishes DomainStatus.Endpoint (search-<domain>-<suffix>.<region>.es.amazonaws.com); SigV4-signed HTTP requests to it are answered by the engine — GET / returns the cluster banner (200), PUT /<index> creates an index (200), PUT /_cluster/settings applies flat-form settings (200), PUT /_ingest/pipeline/<id> and /_plugins/_ism/policies/<id> create pipeline and ISM policy (200/201), GET /_nodes reports topology (200), and the _snapshot/_restore and _search families work. Recorded behavior only for the individual bodies; the requirement is that the endpoint the control plane published is served as an OpenSearch engine.
Emulator: every signed request to the published endpoint returns HTTP 404 with the body {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}. The endpoint is advertised by DescribeDomain/CreateDomain but no service answers on it.
Why: the entire data plane — indexing, search, mappings, cluster settings, ingest pipelines, ISM, snapshot/restore, node introspection — is unreachable, and the published endpoint is a dangling advertisement. Any client that stores documents or runs a query against an emulated domain fails with an error that is not an OpenSearch error at all.
Fix: register and serve the 'es' endpoint prefix / published domain endpoint host in the emulator's request router, backed by a domain-scoped index+search engine.
Traces: opensearch.cluster-settings-flat-form-only-and-auto-create-index-false-refuses-implicit-index, opensearch.ingest-pipeline-rewrites-stored-source-and-simulate-predicts-it, opensearch.ism-policy-transition-sets-read-only-and-unmet-condition-stays-writable, opensearch.nested-mapping-isolates-sub-documents-from-flattened-object-matching, opensearch.nodes-apis-redact-service-internals-while-reporting-the-configured-topology, opensearch.osq-deep-opensearch-2-highlight-wraps-matched-term-on-text-but-not-keyword-sibling, opensearch.restore-over-open-index-rejected, opensearch.snapshot-restore-index-round-trip.

Cause 2 of 7: a CloudFormation stack containing AWS::OpenSearchService::Domain reaches stack-create-complete but no domain exists.
AWS: when a stack with an AWS::OpenSearchService::Domain resource reaches CREATE_COMPLETE, the domain exists and opensearch:DescribeDomain on its DomainName returns 200 with a DomainStatus. Smithy: opensearch#DescribeDomain returns a required DomainStatus and models ResourceNotFoundException only for a domain that does not exist.
Emulator: the stack waiter succeeds, then DescribeDomain on the template's DomainName raises ResourceNotFoundException 'Domain not found: <name>' — the CFN resource provider does not create the underlying domain.
Why: a client that provisions OpenSearch through infrastructure-as-code gets a stack that claims success over resources that were never made. Every subsequent call against the deployed domain fails as not-found, and CFN reports a false green.
Fix: the CloudFormation resource provider for AWS::OpenSearchService::Domain (and the VPC-endpoint template path) — create the real domain through the OpenSearch state on resource create, and fail the stack if it cannot.
Traces: opensearch.deploy-domain-cfn-access-policy-enforced, opensearch.deploy-domain-cfn-index-operations-behave, opensearch.deploy-opensearch-domain-cfn-behavioral, opensearch.deploy-vpc-endpoint-cfn-policy-enforced, opensearch.vpc-endpoint-list-and-delete-lifecycle-observed.

Cause 3 of 7: CFN stack Outputs for an OpenSearch deployment are returned without OutputValue.
AWS: DescribeStacks on a completed stack returns each template Output with OutputKey and the resolved OutputValue. Smithy: cloudformation#Output = {OutputKey, OutputValue, Description, ExportName}.
Emulator: Outputs entries omit OutputValue, so reading the output map raises KeyError: 'OutputValue'.
Why: the FGAC/secret-master deployment cannot learn the deployed domain's endpoint or the secret's ARN, so nothing downstream of the deploy can run.
Fix: CloudFormation Output resolution and serialization (resolve !GetAtt/!Ref/!Sub at stack completion; emit OutputValue). Listed here because the failing check belongs to the opensearch service grouping; the same component is named by the cloudformation__iam__opensearch__sts finding filed in this sweep.
Traces: opensearch.deploy-opensearch-3-fgac-secret-master-basic-auth.

Cause 4 of 7: UpdateDomainConfig with a dry run returns no DryRunProgressStatus.
AWS: UpdateDomainConfig with DryRun=true (and DryRunMode='Verbose') returns a trackable DryRunProgressStatus carrying a DryRunId whose progress can be polled, alongside DryRunResults. Smithy: opensearch#UpdateDomainConfig output members are DomainConfig (required), DryRunResults and DryRunProgressStatus; DryRunMode selects the Verbose, trackable form.
Emulator: the response contains only DomainConfig — 'expected a trackable DryRunProgressStatus for a Verbose dry run, got {...DomainConfig...}' — and indexing the member raises KeyError: 'DryRunProgressStatus'. A verbose dry run is indistinguishable from a real config update in the response.
Why: a client that validates a configuration change before applying it has no id to poll and no progress to read, so it cannot tell whether the change was validated or actually applied, and the dry-run TTL/expiry semantics are unobservable.
Fix: UpdateDomainConfig's dry-run path in the OpenSearch service crate — mint and store a DryRunId with progress state for DryRunMode=Verbose and serialize DryRunProgressStatus (and DryRunResults) in the response.
Traces: opensearch.dry-run-progress-ttl-expiration-requires-retrigger, opensearch.dry-run-progress-ttl-expiry-after-completion, opensearch.visibility-timeout-ttl-dry-run-progress-expiry.

Cause 5 of 7: DomainStatus omits ServiceSoftwareOptions.
AWS: DescribeDomain's DomainStatus always carries ServiceSoftwareOptions (CurrentVersion, NewVersion, UpdateAvailable, Cancellable, UpdateStatus, Description, AutomatedUpdateDate, OptionalDeployment) — it is how a client learns whether a service-software update or rollback is possible. Smithy: opensearch#DomainStatus has member ServiceSoftwareOptions; StartServiceSoftwareUpdate returns the same shape.
Emulator: the member is absent from DomainStatus, so reading it raises KeyError: 'ServiceSoftwareOptions'.
Why: a client cannot determine update availability or rollback eligibility, and the documented 'decline with RollbackAvailable=False' path is unreachable because the state it is derived from is never reported.
Fix: the DomainStatus projection/serializer and the domain state that backs it — populate ServiceSoftwareOptions on create and keep it current through service-software operations.
Traces: opensearch.rollback-service-software-disabled-without-active-update.

Cause 6 of 7: Endpoint is published while the domain is still Creating.
AWS: a domain that has not finished creating reports Processing=true and does NOT yet carry an Endpoint; the endpoint appears only once the domain is available. Recorded behavior only; the requirement is that Endpoint is absent while the domain is still being created.
Emulator: CreateDomain's DomainStatus already contains 'Endpoint': 'search-<domain>-<suffix>.<region>.es.amazonaws.com' together with Created=true/Processing=true.
Why: a client that waits for an endpoint by polling for its presence proceeds immediately to a domain that is not ready and gets failures from the first data-plane call; the standard readiness signal is destroyed.
Fix: the DomainStatus serializer / domain lifecycle state — withhold Endpoint (and Endpoints) until the domain leaves the creating state.
Traces: opensearch.domain-lifecycle-create-active-delete.

Cause 7 of 7: DomainStatus.ClusterConfig drops DedicatedMasterEnabled and injects MultiAZWithStandbyEnabled.
AWS: ClusterConfig echoed back by CreateDomain/DescribeDomain reflects the requested topology and reports DedicatedMasterEnabled; for a single-node non-zone-aware domain the recorded response is {'InstanceType': 't3.small.search', 'InstanceCount': 1, 'ZoneAwarenessEnabled': False, 'DedicatedMasterEnabled': False}. Smithy: opensearch#ClusterConfig declares both DedicatedMasterEnabled and MultiAZWithStandbyEnabled; DedicatedMasterEnabled is the member that round-trips the request's dedicated-master choice.
Emulator: the echoed ClusterConfig reads {'InstanceType': 't3.small.search', 'InstanceCount': 1, 'ZoneAwarenessEnabled': False, 'MultiAZWithStandbyEnabled': False} — DedicatedMasterEnabled is missing and MultiAZWithStandbyEnabled is reported in its place.
Why: the request does not round-trip. A client that reads back its cluster topology cannot tell whether a dedicated master was configured, and sees a member it never set, so configuration drift detection and idempotent reconciliation both break.
Fix: the ClusterConfig projection in the OpenSearch domain state/serializer — echo DedicatedMasterEnabled as requested and only emit MultiAZWithStandbyEnabled when it is part of the domain's configuration.
Traces: opensearch.albos-fgac-domain-round-trips-security-and-encryption-and-never-returns-the-master-password.

## Evidence
Cause 1: AssertionError: creating index 'sim2real-behav-snap' returned HTTP 404: {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}; also 'signed GET / on the domain endpoint returned 404: ...', "PUT _cluster/settings action.auto_create_index='true' in the flat form returned HTTP 404, expected 200", "creating pipeline 'sim2real-behav-pipe' returned HTTP 404", "PUT ISM policy '...' returned HTTP 404", 'GET _nodes returned HTTP 404, expected 200'.
Cause 2: botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DescribeDomain operation: Domain not found: aosdxa436bce0e3f21 / aosdxcd0037fa4ff4f / cfndom-44e711397bf9 / sim2real-behav-1ee6c972d2d8 / sim2osdlc-664b080a60 — each after the stack's create waiter returned successfully.
Cause 3: KeyError: 'OutputValue' at `outputs = {o["OutputKey"]: o["OutputValue"] for o in stack.get("Outputs", [])}`.
Cause 4: AssertionError: expected a trackable DryRunProgressStatus for a Verbose dry run, got {'ResponseMetadata': {...}, 'DomainConfig': {...}} (no DryRunProgressStatus); and KeyError: 'DryRunProgressStatus'.
Cause 5: KeyError: 'ServiceSoftwareOptions' at `status_before["ServiceSoftwareOptions"]`.
Cause 6: AssertionError: Endpoint must not be published while still Creating — CreateDomain returned 'Endpoint': 'search-sim2real-...-8sh3tqaaaaaa.us-west-2.es.amazonaws.com' with Created=True, Processing=True.
Cause 7: AssertionError: ClusterConfig reads {'InstanceType': 't3.small.search', 'InstanceCount': 1, 'ZoneAwarenessEnabled': False, 'MultiAZWithStandbyEnabled': False} (expected DedicatedMasterEnabled: False, no MultiAZWithStandbyEnabled).

## Steps to Reproduce
Cause 1: create a domain, wait for it, then issue a SigV4-signed GET / (and PUT /<index>) against DomainStatus['Endpoint'] -> HTTP 404 UnknownService 'es'.
Cause 2: cfn.create_stack with an AWS::OpenSearchService::Domain resource, wait for stack_create_complete, then opensearch.describe_domain(DomainName=<template name>) -> ResourceNotFoundException.
Cause 3: same stack, then cfn.describe_stacks and read Stacks[0]['Outputs'] -> no OutputValue member.
Cause 4: opensearch.update_domain_config(DomainName=<d>, ClusterConfig={...}, DryRun=True, DryRunMode='Verbose') -> response has no DryRunProgressStatus.
Cause 5: opensearch.describe_domain(DomainName=<d>)['DomainStatus'] -> no ServiceSoftwareOptions key.
Cause 6: opensearch.create_domain(...) -> response DomainStatus already contains Endpoint while Processing is True.
Cause 7: opensearch.create_domain(ClusterConfig={'InstanceType':'t3.small.search','InstanceCount':1,'DedicatedMasterEnabled':False,'ZoneAwarenessEnabled':False}, ...) then describe_domain -> ClusterConfig lacks DedicatedMasterEnabled and adds MultiAZWithStandbyEnabled.

Replay the check ids listed in trace_ids.
