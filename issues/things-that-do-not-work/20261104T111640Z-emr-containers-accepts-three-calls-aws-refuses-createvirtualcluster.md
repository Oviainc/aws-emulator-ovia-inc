Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-containers.create-virtual-cluster-nonexistent-eks-cluster-rejected emr-containers.create-managed-endpoint-invalid-type-rejected emr-containers.delete-virtual-cluster-with-active-job-run-rejected
Issue-Shape-Id: com.amazonaws.emrcontainers#AwsChicagoWebService
Issue-Key: emr-containers.create-virtual-cluster-nonexistent-eks-cluster-rejected|_client_error#8|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Key: emr-containers.create-managed-endpoint-invalid-type-rejected|_client_error#11|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Key: emr-containers.delete-virtual-cluster-with-active-job-run-rejected|_client_error#13|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 6e4639cd3b0eb321e74af4094a3461ae2de27150

# emr-containers accepts three calls AWS refuses: CreateVirtualCluster on a nonexistent EKS cluster, CreateManagedEndpoint with an unsupported type, DeleteVirtualCluster with an active job run

> _Note: this issue was identified against the code base at commit `6e4639cd3b0eb321e74af4094a3461ae2de27150`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVirtualCluster

## Description
Three net-new root causes in the emr-containers emulator, all the same family of defect: the operation answers HTTP 200 and MUTATES state where AWS refuses the call with HTTP 400 / ValidationException and changes nothing. Every one is an "overly permissive" divergence — the emulator never applies the precondition at all, so the refusal branch does not exist. All three causes must be fixed and all three named checks must replay clean before this issue is closed.

Cause 1 of 3 — CreateVirtualCluster registers a virtual cluster against an EKS cluster that does not exist.
AWS: Smithy: CreateVirtualCluster, POST /virtualclusters, http code 200; containerProvider is a required ContainerProvider whose `type` is the ContainerProviderType enum (EKS only) and whose `id` is ClusterId (@length 1..100, @pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$) — the id of a real EKS cluster; modeled errors are EKSRequestThrottledException(429), InternalServerException(500), ResourceNotFoundException(400) and ValidationException(400). Recorded behavior: naming an EKS cluster that does not exist is refused with HTTP 400, error code ValidationException and message `Cluster <containerProvider.id> not found` — fixed text `Cluster ` / ` not found` with the variable part taken verbatim from the request's containerProvider.id. Nothing is registered: ListVirtualClusters(containerProviderId=<that id>) is empty afterwards.
Emulator: HTTP 200 with a freshly minted id/name/arn (e.g. `{'id': '…', 'name': 'sim2real-vc-…', 'arn': 'arn:aws:emr-containers:…:/virtualclusters/…'}`), i.e. the container provider is never resolved against the EKS service state, so a virtual cluster is created pointing at a provider that does not exist and shows up in ListVirtualClusters.
Why: a client that relies on AWS to validate the container provider (the common "did I pass the right cluster id / namespace" check) gets a healthy-looking virtual cluster that can never schedule anything, and its error handling for the typo case is dead code. The dangling resource also survives into teardown and list output.
Fix: the containerProvider resolution/validation of CreateVirtualCluster's handler in the emr-containers service crate — resolve containerProvider.id against the EKS cluster state before minting anything and refuse with ValidationException(400) and AWS's fixed sentence when it names no cluster; register nothing on that path. Related but NOT covering: open issue 20261022T001417Z-emr-containers-error-paths-missing-input-validation-on.md is the same family on other operations' identifiers, and resolved issue 20261024T021827Z-emr-containers-createvirtualcluster-validation.md concerned securityConfigurationId/namespace resolution — neither makes the container-provider cluster id resolve.
Traces: check emr-containers.create-virtual-cluster-nonexistent-eks-cluster-rejected.

Cause 2 of 3 — CreateManagedEndpoint accepts an endpoint `type` the service does not support.
AWS: Smithy: CreateManagedEndpoint, POST /virtualclusters/{virtualClusterId}/endpoints, http code 200; `type` is required, target EndpointType (a plain string, @length 1..64, @pattern \S — the model does NOT enumerate the accepted values, so the value set is service-side), errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400). Recorded behavior: a type outside the supported set (the recording sends a syntactically valid but unsupported value) is refused with HTTP 400, code ValidationException and the message `Invalid value for argument type` — entirely FIXED text: it names neither the member's offending value nor the endpoint name. No endpoint is created (ListManagedEndpoints for the cluster stays empty), while the supported value `JUPYTER_ENTERPRISE_GATEWAY` on the same virtual cluster succeeds.
Emulator: HTTP 200 — the unsupported type is stored and an endpoint is provisioned, so the call that AWS rejects creates a resource.
Why: EndpointType is a free-form string on the wire precisely because the service validates it, so a client that mistypes or passes a forward-looking type gets a 200 and an endpoint that can never serve, instead of the 400 it codes against; follow-on assertions that no endpoint exists after the refusal also diverge.
Fix: the input validation of CreateManagedEndpoint in the emr-containers service crate — check `type` against the service's supported endpoint types before any state is created and refuse with ValidationException(400) carrying AWS's fixed sentence `Invalid value for argument type`; keep JUPYTER_ENTERPRISE_GATEWAY accepted.
Traces: check emr-containers.create-managed-endpoint-invalid-type-rejected.

Cause 3 of 3 — DeleteVirtualCluster tears down a virtual cluster that still holds a non-terminal job run.
AWS: Smithy: DeleteVirtualCluster, DELETE /virtualclusters/{id}, http code 200; `id` is ResourceIdString @required @httpLabel @length 1..64 @pattern ^[0-9a-z]+$; modeled errors are exactly InternalServerException(500) and ValidationException(400). Recorded behavior: while a job run on that cluster is in a non-terminal state (PENDING/SUBMITTED/RUNNING), the delete is refused with HTTP 400, code ValidationException and the message `VirtualCluster has active resources` — entirely FIXED text, naming neither the cluster nor the job run. The cluster is unchanged (DescribeVirtualCluster still reports state RUNNING, per VirtualClusterState), and after the job run is cancelled the SAME delete succeeds and the cluster reaches TERMINATED.
Emulator: HTTP 200 with the cluster id echoed (`{'id': '…'}`) — the active-resource precondition is never evaluated, so the cluster is torn down underneath a live job run.
Why: AWS's refusal is what guarantees a caller cannot delete a cluster with work in flight; against the emulator the guard never fires, so cleanup code deletes clusters that still own running job runs and the client's "cancel first, then delete" sequencing is never exercised. The resolved issue 20261025T184048Z-emr-containers-deletevirtualcluster-wrong-behavior.md made teardown reach the resources the cluster holds, but AWS does not cascade here at all for a NON-terminal job run — it refuses the delete; that issue does not add this precondition.
Fix: the precondition check of DeleteVirtualCluster's handler in the emr-containers service crate — before transitioning the cluster, refuse with ValidationException(400) and the fixed sentence when the cluster owns any job run (or endpoint) in a non-terminal state, leaving the cluster's state untouched; allow the delete once all such resources are terminal.
Traces: check emr-containers.delete-virtual-cluster-with-active-job-run-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout: emr-containers 14/20 passed, 6 mismatched, 0 errored.

Cause 1 — [mismatch] emr-containers.create-virtual-cluster-nonexistent-eks-cluster-rejected:
  expected: ClientError, code ValidationException, message 'Cluster <bogus-eks-cluster-id> not found'
  actual:   the call SUCCEEDED — AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'id': '000000000843r9oesshn7kshs', 'name': 'sim2real-vc-0e2dab514a47', 'arn': 'arn:aws:emr-containers:us-west-2:000000000000:/virtualclusters/000000000843r9oesshn7kshs'}
  (the id/name/arn and the probe's cluster id are per-run values; the defect is the 200 where AWS answers 400, and the missing fixed sentence)
  The check's positive control — CreateVirtualCluster against a REAL EKS cluster, then DescribeVirtualCluster reporting RUNNING — passed, so the accept path works and only the refusal is missing.

Cause 2 — [mismatch] emr-containers.create-managed-endpoint-invalid-type-rejected:
  request: CreateManagedEndpoint(virtualClusterId=<vc>, type='SIM2REAL_NOT_A_REAL_ENDPOINT_TYPE', releaseLabel='emr-6.15.0-latest', executionRoleArn=<role>, clientToken=<tok>)
  expected: (code, status) == ('ValidationException', 400) and message exactly 'Invalid value for argument type'
  actual:   AssertionError: expected the call to be refused, but it succeeded (HTTP 200, endpoint created)

Cause 3 — [mismatch] emr-containers.delete-virtual-cluster-with-active-job-run-rejected:
  sequence: CreateVirtualCluster -> StartJobRun (DescribeJobRun state asserted in PENDING/SUBMITTED/RUNNING, assertion passed) -> DeleteVirtualCluster
  expected: ClientError, code ValidationException, message exactly 'VirtualCluster has active resources'; DescribeVirtualCluster still RUNNING
  actual:   AssertionError: expected a ClientError, but the call SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, ...}, 'id': '000000000843r9iwq5egmwvg6'}

Model evidence (services/service-com-amazonaws-emrcontainers-awschicagowebservice/smithy-model/com-amazonaws-emrcontainers-awschicagowebservice.json), via smithy_query:
  CreateVirtualCluster: POST /virtualclusters code 200; containerProvider required (ContainerProvider{type: ContainerProviderType enum, id: ClusterId length{1,100} pattern ^[0-9A-Za-z][A-Za-z0-9\-_]*$, info: ContainerInfo}); errors EKSRequestThrottledException(429), InternalServerException(500), ResourceNotFoundException(400), ValidationException(400).
  CreateManagedEndpoint: POST /virtualclusters/{virtualClusterId}/endpoints code 200; type required -> EndpointType (string, length{1,64}, pattern \S — no enumeration in the model); errors InternalServerException(500), ResourceNotFoundException(400), ValidationException(400).
  DeleteVirtualCluster: DELETE /virtualclusters/{id} code 200; id -> ResourceIdString required httpLabel length{1,64} pattern ^[0-9a-z]+$; errors InternalServerException(500), ValidationException(400) only.
  VirtualClusterState enum = RUNNING, TERMINATING, TERMINATED, ARRESTED; JobRunState enum = PENDING, SUBMITTED, RUNNING, FAILED, CANCELLED, CANCEL_PENDING, COMPLETED.

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay suite simparity-behavioral-aws-v1, checks emr-containers.create-virtual-cluster-nonexistent-eks-cluster-rejected, emr-containers.create-managed-endpoint-invalid-type-rejected and emr-containers.delete-virtual-cluster-with-active-job-run-rejected.
3. Cause 1: call CreateVirtualCluster with containerProvider={type:'EKS', id:<an id satisfying ClusterId that names no EKS cluster>, info:{eksInfo:{namespace:<any>}}} and a fresh clientToken. Expect HTTP 400 ValidationException with message 'Cluster <that id> not found' and no virtual cluster registered (ListVirtualClusters(containerProviderId=<that id>) empty). Observed HTTP 200 and a registered virtual cluster. Confirm the positive control still works: the same call against a real EKS cluster must succeed and DescribeVirtualCluster must report RUNNING.
4. Cause 2: on a RUNNING virtual cluster call CreateManagedEndpoint(type='SIM2REAL_NOT_A_REAL_ENDPOINT_TYPE', releaseLabel, executionRoleArn, clientToken). Expect HTTP 400 ValidationException with message exactly 'Invalid value for argument type' and ListManagedEndpoints(virtualClusterId) still empty. Observed HTTP 200 with an endpoint created. Confirm type='JUPYTER_ENTERPRISE_GATEWAY' on the same cluster still succeeds and is the only listed endpoint.
5. Cause 3: CreateVirtualCluster -> StartJobRun; with DescribeJobRun reporting a non-terminal state, call DeleteVirtualCluster(id=<vc>). Expect HTTP 400 ValidationException with message exactly 'VirtualCluster has active resources' and DescribeVirtualCluster still state RUNNING. Observed HTTP 200 and the cluster torn down. Then CancelJobRun, wait for CANCELLED, and confirm DeleteVirtualCluster now succeeds and the cluster reaches TERMINATED.
6. Re-replay all three checks to confirm the fix; all three must pass.
