Issue-Source: trace-sweep-divergence
Issue-Trace: supplychain-data-integration-flow-dataset-target
Issue-Step: setup#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: supplychain-data-integration-flow-dataset-target supplychain-data-integration-flow-s3 supplychain-instance-error-paths supplychain-datalake-namespace-dataset
Issue-Shape-Id: com.amazonaws.supplychain#GalaxyPublicAPIGateway
Issue-Key: scn|CreateDataLakeDataset|2xx->4xx|status
Issue-Key: scn|CreateDataLakeDataset|2xx->4xx|$.dataset
Issue-Key: scn|GetDataLakeDataset|2xx->4xx|$.dataset
Issue-Key: scn|GetDataLakeDataset|2xx->4xx|status
Issue-Key: scn|GetDataLakeDataset|4xx->4xx|$.message
Issue-Commit: 91fa93186158a34b967cb70818834efe40b65b11

# scn data-lake dataset plane is unusable in a created namespace, and its not-found message is emulator-authored

> _Note: this issue was identified against the code base at commit `91fa93186158a34b967cb70818834efe40b65b11`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDataLakeDataset

## Description
FOUR (4) net-new divergence clusters on AWS Supply Chain (service token `scn`, sdkId `SupplyChain`, endpointPrefix/sigv4Name `scn`; the bootstrap crate `service-com-amazonaws-supplychain-galaxypublicapigateway` is stood up and answering, so these are fidelity findings, not a bootstrap gap). They share one fault: the emulator keeps NO data-lake namespace store, so a namespace a caller created with CreateDataLakeNamespace does not exist as far as the dataset operations are concerned — every dataset addressed to it is refused 404, every read of that dataset is then 404 too, and the one not-found refusal that IS correctly classified carries the emulator's own prose. A fixer MUST close all four causes before submitting.

Cause 1 of 4: CreateDataLakeDataset refuses a dataset in a caller-created namespace with 404 instead of creating it (fixture `supplychain-data-integration-flow-dataset-target`, step `setup#4`, 6 steps affected). AWS: CreateDataLakeDataset is PUT /api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name} with SUCCESS_STATUS 200 and a @required `dataset` output member (Smithy: CreateDataLakeDataset input {instanceId UUID @httpLabel @required, namespace DataLakeNamespaceName @httpLabel @required @length 1..50 @pattern ^[a-z0-9_]+$, name DataLakeDatasetName @httpLabel @required @length 1..75 @pattern ^[a-z0-9_]+$, schema DataLakeDatasetSchema, description, partitionSpec, tags}; errors AccessDeniedException 403, ConflictException 409, ResourceNotFoundException 404, ServiceQuotaExceededException 402, ThrottlingException 429, ValidationException 400, InternalServerException 500). The `namespace` member's own documentation states that BESIDES the custom defined namespace, every instance comes with the pre-defined namespaces `asc` and `default` — i.e. a namespace created through CreateDataLakeNamespace (PUT /api/datalake/instance/{instanceId}/namespaces/{name}, 200 with a @required `namespace`) is a perfectly valid dataset namespace. The recording shows the service answering 200 for a dataset created under the namespace the same fixture had just created. Emulator: 404 ResourceNotFoundException with no `dataset` in the body. Why: a client cannot create any dataset outside the two pre-defined namespaces, so the whole custom-schema data-lake workflow (create namespace -> create dataset -> ingest/flow) is impossible; worse, the refusal is a not-found on a namespace the caller demonstrably just created, so the caller has no way to make the call succeed. Fix: CreateDataLakeDataset's namespace existence check in the operation's handler (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/create_data_lake_dataset.rs, the `is_predefined_namespace` guard) must accept a namespace the store holds as well as a pre-defined one, which requires the service state (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/state.rs, `ComAmazonawsSupplychainGalaxypublicapigatewayState` holds only `instances`, `idempotency` and `datasets`) to actually persist the namespaces CreateDataLakeNamespace creates, and CreateDataLakeNamespace to write them there.

Cause 2 of 4: the same refusal on the datalake-namespace fixture's own create, where the 404 also costs the entire `dataset` payload (fixture `supplychain-datalake-namespace-dataset`, step `task#6`). AWS: as cause 1 — 200 with `dataset` carrying arn (arn:aws:scn:<region>:<account>:instance/<instanceId>/namespaces/<namespace>/datasets/<name>), instanceId, namespace, name, description, schema (fields + primaryKeys as sent) and created/lastModified times. Emulator: 404 and `$.dataset` absent. Why: a caller that creates a namespace and then a dataset in it never receives the dataset projection at all, so nothing downstream (schema echo, ARN, partitionSpec) can be read. Fix: same component as cause 1; once the namespace is resolvable the handler's existing projection path (`DataLakeDatasetRecord::to_wire`) already answers the modelled shape.

Cause 3 of 4: GetDataLakeDataset answers 404 for the dataset that create was refused, instead of 200 with the stored dataset (fixture `supplychain-datalake-namespace-dataset`, step `task#7`). AWS: GetDataLakeDataset is GET /api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name}, @readonly, 200 with a @required `dataset`; a dataset that was created IS readable back unchanged. Emulator: 404 ResourceNotFoundException, `$.dataset` absent, because nothing was ever persisted. Why: the read path reports a resource the caller was told to expect as missing; a client polling for its dataset after a create loops forever on a 404. Fix: the read handler itself is correct (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/get_data_lake_dataset.rs looks the dataset up by (instanceId, namespace, name)); this cause is closed by the create/namespace-state fix of causes 1-2, and this cluster is listed separately so the fixer re-replays it rather than assuming it.

Cause 4 of 4: the ResourceNotFoundException message for a genuinely absent dataset is emulator-authored prose (fixture `supplychain-datalake-namespace-dataset`, step `teardown#12`, operation GetDataLakeDataset). AWS: GetDataLakeDataset declares ResourceNotFoundException @httpError 404 with a single `message` member documented as "Request references a resource which does not exist"; the status and error class the emulator returns are already correct, only `message` diverges. The recorded message template is `Dataset <name> not exist in namespace <namespace>` — its FIXED TEXT is "Dataset ", " not exist in namespace " (note AWS's capital D and its non-standard "not exist", not "does not exist"), and its only VARIABLE parts are the dataset name and the namespace name taken from the request's URI labels; the instanceId does NOT appear. Emulator: 404 with `dataset <name> does not exist in namespace <namespace> of instance <instanceId>` — lowercased "dataset", "does not exist" instead of "not exist", and an extra " of instance <instanceId>" segment AWS never emits. Why: a client that matches or surfaces the service's not-found text gets wording AWS never produces, and the extra interpolated instance id makes the message unstable for a fixed error class, so no exact-match assertion on it can hold. Fix: the ResourceNotFoundException constructor in GetDataLakeDataset's handler (services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/get_data_lake_dataset.rs) must render AWS's template with only the dataset name and namespace. The open issue `20261029T180412Z-scn-refusal-messages-are-emulator-authored-prose-instead.md` describes the same class of fault for GetInstance and CreateDataLakeNamespace but does NOT own this dataset cluster (the replay reports no `owned_by` for it), so it is filed here; `20261029T180522Z-scn-instance-projection-omits-its-domain-members-and.md` covers the Instance-shape clusters and none of these.

Traces: supplychain-data-integration-flow-dataset-target#setup#4 (and #setup#5, supplychain-data-integration-flow-s3#setup#4/#setup#5, supplychain-instance-error-paths#setup#4/#setup#5), supplychain-datalake-namespace-dataset#task#6, supplychain-datalake-namespace-dataset#task#7, supplychain-datalake-namespace-dataset#teardown#12.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=101, passed=16, mismatched=80, blocked=5, divergence_count=15, score_pct=15.8. All four clusters below carry empty `stub_path` and empty `owned_by`.

Cause 1 - supplychain-data-integration-flow-dataset-target#setup#4, CreateDataLakeDataset, expected 200 / actual 404:
  detail: status: expected 200, got 404
  affected (6): supplychain-data-integration-flow-dataset-target#setup#4, #setup#5, supplychain-data-integration-flow-s3#setup#4, #setup#5, supplychain-instance-error-paths#setup#4, #setup#5
  divergence_key: service=scn|CreateDataLakeDataset|status=200->404|keys=status

Cause 2 - supplychain-datalake-namespace-dataset#task#6, CreateDataLakeDataset, expected 200 / actual 404:
  detail: status: expected 200, got 404; $.dataset: missing (expected a DataLakeDataset with arn arn:aws:scn:<region>:<account>:instance/<instanceId>/namespaces/<namespace>/datasets/<name>, instanceId, namespace, name, description, schema {fields [{name id, type STRING, isRequired true}, {name qty, type INT, isRequired false}], name ProbeSchema, primaryKeys [{name id}]}, createdTime/lastModifiedTime shaped-not-asserted; got None)
  divergence_key: service=scn|CreateDataLakeDataset|status=200->404|keys=$.dataset,status

Cause 3 - supplychain-datalake-namespace-dataset#task#7, GetDataLakeDataset, expected 200 / actual 404:
  detail: status: expected 200, got 404; $.dataset: missing (same DataLakeDataset as cause 2; got None)
  divergence_key: service=scn|GetDataLakeDataset|status=200->404|keys=$.dataset,status

Cause 4 - supplychain-datalake-namespace-dataset#teardown#12, GetDataLakeDataset, expected 404 / actual 404 (status and error class MATCH):
  $.message: value_mismatch
    expected: "Dataset <datasetName> not exist in namespace <namespaceName>"
    actual:   "dataset <datasetName> does not exist in namespace <namespaceName> of instance <instanceId>"
  Fixed text: "Dataset " + " not exist in namespace ". Variable parts: the dataset name and the namespace name, both from the request's @httpLabel members. The emulator's three deviations: lowercase "dataset", "does not exist" for "not exist", and an appended " of instance <instanceId>" segment with a value AWS's message does not carry.
  divergence_key: service=scn|GetDataLakeDataset|status=404->404|keys=$.message

Code evidence (checkout):
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/create_data_lake_dataset.rs:224-234 — the namespace guard is `if !crate::state::is_predefined_namespace(&input.namespace) { ResourceNotFoundException }`, so ONLY `asc`/`default` are accepted and a created namespace is refused 404.
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/state.rs:209-214 — `PREDEFINED_NAMESPACES = ["asc", "default"]` / `is_predefined_namespace`.
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/state.rs:283-296 — the service state holds `instances`, `idempotency` and `datasets` and NO namespace map at all, so CreateDataLakeNamespace persists nothing a dataset create could resolve.
- services/service-com-amazonaws-supplychain-galaxypublicapigateway/src/ops/get_data_lake_dataset.rs:178-186 — the not-found message is formatted as "dataset {} does not exist in namespace {} of instance {}".

Model evidence (services/service-com-amazonaws-supplychain-galaxypublicapigateway/smithy-model/com-amazonaws-supplychain-galaxypublicapigateway.json):
- CreateDataLakeDataset: PUT /api/datalake/instance/{instanceId}/namespaces/{namespace}/datasets/{name}, code 200, @idempotent, output {dataset: DataLakeDataset @required}; errors include ResourceNotFoundException(404), ConflictException(409), ValidationException(400).
- GetDataLakeDataset: GET same URI, @readonly, code 200, output {dataset: DataLakeDataset @required}; errors include ResourceNotFoundException(404).
- CreateDataLakeNamespace: PUT /api/datalake/instance/{instanceId}/namespaces/{name}, code 200, output {namespace: DataLakeNamespace @required} — the operation that makes a custom namespace exist.
- `namespace` member doc: "besides the custom defined namespace, every instance comes with below pre-defined namespaces: asc ... default ..." — custom namespaces are expected to hold datasets.

## Steps to Reproduce
1. git clone the mockforge-aws-emulator repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (replay_wire_corpus).
3. Cause 1: replay fixture `supplychain-data-integration-flow-dataset-target` step `setup#4` — CreateInstance, then CreateDataLakeNamespace for a custom namespace name matching ^[a-z0-9_]+$, then PUT /api/datalake/instance/{instanceId}/namespaces/{thatNamespace}/datasets/{name} with a schema. Observe 404 ResourceNotFoundException naming the namespace; AWS answers 200 with the created dataset. Same divergence on #setup#5 and on supplychain-data-integration-flow-s3#setup#4/#setup#5 and supplychain-instance-error-paths#setup#4/#setup#5.
4. Cause 2: replay fixture `supplychain-datalake-namespace-dataset` step `task#6` — same call in that fixture; observe 404 and an entirely absent `$.dataset`.
5. Cause 3: replay fixture `supplychain-datalake-namespace-dataset` step `task#7` — GET the dataset just created; observe 404 with no `$.dataset` because nothing was persisted.
6. Cause 4: replay fixture `supplychain-datalake-namespace-dataset` step `teardown#12` — GET a dataset after it has been deleted (any triple the store does not hold). Status 404 and the ResourceNotFoundException class are correct; diff `$.message` against AWS's template "Dataset <name> not exist in namespace <namespace>" and observe the emulator's lowercase "dataset", "does not exist", and extra " of instance <instanceId>" segment.
7. For each step diff the response status and body against the fixture's expected response; re-replay all four fixture#step bindings to confirm a fix.
