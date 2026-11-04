Issue-Source: trace-sweep-divergence
Issue-Trace: supplychain-datalake-namespace-dataset
Issue-Step: teardown#12
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: supplychain-datalake-namespace-dataset
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: scn|GetDataLakeDataset|4xx->2xx|$.message
Issue-Key: scn|GetDataLakeDataset|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: scn|GetDataLakeDataset|4xx->2xx|status
Issue-Key: scn|GetDataLakeNamespace|4xx->2xx|$.message
Issue-Key: scn|GetDataLakeNamespace|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: scn|GetDataLakeNamespace|4xx->2xx|status
Issue-Commit: 0d4134cd620ad031167c04704bdc955be5cde6a3

# scn deleted data-lake resources stay readable: GetDataLakeDataset and GetDataLakeNamespace answer 200 after the delete

> _Note: this issue was identified against the code base at commit `0d4134cd620ad031167c04704bdc955be5cde6a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDataLakeDataset

## Description
TWO (2) net-new divergence clusters on AWS Supply Chain (wire service token `scn`; sdkId `SupplyChain`, arnNamespace/endpointPrefix/sigv4Name `scn`, so the bootstrapped crate `service-com-amazonaws-supplychain-galaxypublicapigateway` IS the service answering here — these are fidelity findings, not a bootstrap gap). They share ONE fault: a data-lake resource that has been DELETED is still served as present. The delete operations (DeleteDataLakeDataset, DeleteDataLakeNamespace) remove nothing from the service store, so the read path finds the record and answers HTTP 200 with the full resource projection where AWS answers HTTP 404 ResourceNotFoundException with an `x-amzn-errortype` header and a `message`. A fixer MUST close BOTH causes before submitting; each is listed with its own fixture#step so both get re-replayed.

Cause 1 of 2: GetDataLakeDataset answers 200 with the dataset after that dataset has been deleted (fixture `supplychain-datalake-namespace-dataset`, step `teardown#12`). AWS: GetDataLakeDataset is GET /api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name}, @readonly, SUCCESS_STATUS 200 with a @required `dataset` member; it declares ResourceNotFoundException @httpError 404, documented "Request references a resource which does not exist" (Smithy: GetDataLakeDataset input {instanceId UUID @httpLabel @required, namespace DataLakeNamespaceName @httpLabel @required @pattern ^[a-z0-9_]+$, name DataLakeDatasetName @httpLabel @required @pattern ^[a-z0-9_]+$}; errors ResourceNotFoundException 404, AccessDeniedException 403, ValidationException 400, ConflictException 409, ServiceQuotaExceededException 402, ThrottlingException 429, InternalServerException 500). A dataset that DeleteDataLakeDataset (DELETE on the same URI, 200) has removed is no longer readable: the recorded response for this step is 404 with the restJson1 error-type header `x-amzn-errortype` present and `message` = "Dataset <datasetName> not exist in namespace <namespaceName>" — fixed text "Dataset " + " not exist in namespace " (AWS's capital D and its non-standard "not exist", NOT "does not exist"), variable parts ONLY the dataset name and the namespace name taken from the request's @httpLabel members; the instanceId does NOT appear in it. Emulator: HTTP 200 with the dataset body, `$.message` absent and `$headers.x-amzn-errortype` absent. Why: a client cannot tell a deleted dataset from a live one — delete appears to succeed (or is refused 501) and the resource keeps answering reads forever, so cleanup verification, re-create-after-delete and any "wait until gone" poll loop never terminate correctly, and the client gets a dataset projection for a resource that must no longer exist. Fix: the delete must mutate state — DeleteDataLakeDataset's handler (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/delete_data_lake_dataset.rs) must remove the (instanceId, namespace, name) entry from the service store's `datasets` map so GetDataLakeDataset's existing lookup misses; and that handler's ResourceNotFoundException message must render AWS's template above (today it formats "dataset {} does not exist in namespace {} of instance {}", i.e. lowercase noun, "does not exist", and an extra instance segment AWS never emits). Hypothesis: the 200 appears here because the preceding delete is still a scaffolded 501 stub, so nothing is ever removed.

Cause 2 of 2: GetDataLakeNamespace answers 200 with the namespace after that namespace has been deleted (fixture `supplychain-datalake-namespace-dataset`, step `teardown#14`). AWS: GetDataLakeNamespace is GET /api/datalake/instance/{instanceId}/namespaces/{name}, @readonly, 200 with a @required `namespace`, and declares ResourceNotFoundException @httpError 404 ("Request references a resource which does not exist"); DeleteDataLakeNamespace (DELETE on the same URI, 200, documented as deleting the namespace "and its underling datasets") makes the namespace non-existent, after which the read is a 404 carrying `x-amzn-errortype` and `message` = "Namespace <namespaceName> not exist" — fixed text "Namespace " + " not exist", the only variable part being the namespace name from the request's `name` @httpLabel. Emulator: HTTP 200 with the namespace body, `$.message` absent and `$headers.x-amzn-errortype` absent. Separately, the 404 sentence the emulator would emit if it ever took that branch is also wrong: the read handler returns the fixed constant "Namespace not found." (crate::state::NAMESPACE_NOT_FOUND_MESSAGE), which echoes no name, so fixing only the status would leave a message AWS never produces. Why: a deleted namespace keeps answering reads AND keeps appearing to hold its datasets, so a client cannot verify teardown, cannot re-create the namespace cleanly, and sees a resource the service has told it is gone. Fix: DeleteDataLakeNamespace's handler (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/delete_data_lake_namespace.rs) must remove the namespace record — and the datasets scoped to it — from the service state (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/state.rs), and GetDataLakeNamespace's not-found message (same state module) must be the name-bearing template above instead of a fixed sentence.

Related open issues, neither of which owns these two clusters (the replay reports empty `owned_by` for both): `20261104T120356Z-scn-data-lake-dataset-plane-is-unusable-in.md` covers the CREATE side of the data-lake plane and the dataset 404->404 message wording, not deletion visibility; `20261029T180412Z-scn-refusal-messages-are-emulator-authored-prose-instead.md` covers GetInstance/CreateDataLakeNamespace refusal prose; `20261029T180522Z-scn-instance-projection-omits-its-domain-members-and.md` covers the Instance projection clusters.

Traces: supplychain-datalake-namespace-dataset#teardown#12 (cause 1), supplychain-datalake-namespace-dataset#teardown#14 (cause 2).

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=101, passed=30, mismatched=70, blocked=1, errored=0, divergence_count=13, score_pct=29.7, not_implemented=41. Both clusters below carry stub_path="" and owned_by=[] (every other cluster in this replay is owned by an existing open issue).

Cause 1 — fixture supplychain-datalake-namespace-dataset, step teardown#12, operation GetDataLakeDataset, service scn, verdict mismatch, expected_status 404 / actual_status 200, affected_count 1:
  detail:
    status: expected 404, got 200
    $.message: missing (expected 'Dataset <datasetName> not exist in namespace <namespaceName>', got None)
    $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=scn|GetDataLakeDataset|status=404->200|keys=$.message,$headers.x-amzn-errortype,status
  Message template: fixed text "Dataset " + " not exist in namespace "; variable parts = the dataset name and the namespace name, both from the request's @httpLabel members. (The recorded run's literal values sim2real_probe_supplychain_ds_<suffix> / sim2real_probe_supplychain_ns_<suffix> are per-run fixture names, not part of the requirement.)

Cause 2 — fixture supplychain-datalake-namespace-dataset, step teardown#14, operation GetDataLakeNamespace, service scn, verdict mismatch, expected_status 404 / actual_status 200, affected_count 1:
  detail:
    status: expected 404, got 200
    $.message: missing (expected 'Namespace <namespaceName> not exist', got None)
    $headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=scn|GetDataLakeNamespace|status=404->200|keys=$.message,$headers.x-amzn-errortype,status
  Message template: fixed text "Namespace " + " not exist"; the only variable part is the namespace name from the request's `name` @httpLabel.

Code evidence (checkout):
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/delete_data_lake_dataset.rs:162-180 — handle() is still the scaffold body: `Err(crate::scaffolded::Scaffolded::not_implemented("DeleteDataLakeDataset"))`. It touches no state, so the dataset entry survives the delete.
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/delete_data_lake_namespace.rs:154-172 — same scaffold body for DeleteDataLakeNamespace; the namespace record and its datasets survive the delete.
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/get_data_lake_dataset.rs:173-187 — the read looks up `store.datasets.get(&dataset_key(..))` and answers 200 whenever the key is present; its None branch formats "dataset {} does not exist in namespace {} of instance {}", which is not AWS's template.
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/get_data_lake_namespace.rs:184-194 — the read answers 200 from `store.namespace(&instance_id, &name)` and its None branch returns the fixed constant `crate::state::NAMESPACE_NOT_FOUND_MESSAGE`.
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/state.rs:221-227 — `NAMESPACE_NOT_FOUND_MESSAGE = "Namespace not found."`, a fixed sentence echoing no name.

Model evidence (services/service-com-amazonaws-supplychain-galaxypublicapigateway/smithy-model/com-amazonaws-supplychain-galaxypublicapigateway.json, service com.amazonaws.supplychain#GalaxyPublicAPIGateway, restJson1, version 2024-01-01):
- GetDataLakeDataset: GET /api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name}, @readonly, code 200, output {dataset: DataLakeDataset @required}; errors include ResourceNotFoundException (client, httpError 404).
- GetDataLakeNamespace: GET /api/datalake/instance/{instanceId}/namespaces/{name}, @readonly, code 200, output {namespace: DataLakeNamespace @required}; errors include ResourceNotFoundException (client, httpError 404).
- DeleteDataLakeDataset: DELETE same dataset URI, @idempotent, code 200, output {instanceId, namespace, name all @required}.
- DeleteDataLakeNamespace: DELETE same namespace URI, @idempotent, code 200, output {instanceId, name @required}; doc: "delete an Amazon Web Services Supply Chain data lake namespace and its underling datasets".
- Service CORS additionalExposedHeaders includes x-amzn-errortype, the restJson1 error-type header the 404 responses must carry.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo and boot the emulator built from that checkout (the scn/SupplyChain crate service-com-amazonaws-supplychain-galaxypublicapigateway is bootstrapped and answering).
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture `supplychain-datalake-namespace-dataset` up to step `teardown#12` — CreateInstance, CreateDataLakeNamespace with a custom name matching ^[a-z0-9_]+$, CreateDataLakeDataset in it, then DELETE /api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name}, then GET that same dataset URI. Observe HTTP 200 with the dataset body; AWS answers 404 ResourceNotFoundException with the x-amzn-errortype header and message "Dataset <name> not exist in namespace <namespace>".
4. Cause 2: continue the same fixture to step `teardown#14` — DELETE /api/datalake/instance/{instanceId}/namespaces/{name}, then GET that namespace URI. Observe HTTP 200 with the namespace body; AWS answers 404 ResourceNotFoundException with x-amzn-errortype and message "Namespace <name> not exist".
5. For each step diff the response status, the x-amzn-errortype header and the body's `message` against the fixture's expected response.
6. Re-replay BOTH bindings — supplychain-datalake-namespace-dataset#teardown#12 and supplychain-datalake-namespace-dataset#teardown#14 — to confirm a fix; a delete that removes the record fixes the status, and the message templates must be fixed too or both steps still diverge on $.message.
