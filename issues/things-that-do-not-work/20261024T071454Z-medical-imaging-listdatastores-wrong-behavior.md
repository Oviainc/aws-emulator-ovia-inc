Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7805cfa6f36158072c58d31caff72f5961880244

# Data stores are not scoped to the request's Region, so ListDatastores/GetDatastore leak data stores created in another Region

_Filed: 2026-10-24T07:14:54Z_
_Operation: ListDatastores | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7805cfa6f36158072c58d31caff72f5961880244`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The service state keeps one global `datastores` map keyed only by datastore id (src/state.rs). The request's Region is read ONLY to build ARNs at create time (`crate::state::request_region` in create_datastore.rs line 148) and is never stored on the `Datastore` record nor used when listing or reading.

Expected (AWS HealthImaging is a regional service): a data store created with us-east-1 credentials is invisible to a request signed for eu-west-1 — `ListDatastores` returns an empty `datastoreSummaries` for that Region, and `GetDatastore`/`DeleteDatastore`/`StartDICOMImportJob` against its id answer ResourceNotFoundException (404) because the resource does not exist in the Region the request addressed.
Actual: the same data store is returned by `ListDatastores` and `GetDatastore` under ANY signing Region, and its reported `datastoreArn` still carries the creation Region — so a client sees a us-east-1 ARN in an eu-west-1 listing, and the per-Region `MAX_DATASTORES`/quota behavior is global instead of regional. Every id-keyed operation (GetDatastore, DeleteDatastore, StartDICOMImportJob, GetDICOMImportJob, GetImageSet, GetImageFrame, CopyImageSet) is reachable cross-Region for the same reason.

Note the tagging routes already get this right by comparing the whole ARN (`ComAmazonawsMedicalimagingAhigatewayserviceState::resource_tags`, src/state.rs lines 1124-1133 — a held store named by its ARN in another Region is a 404), which shows the intended semantics; the id-keyed read/list paths simply do not apply it.

## Evidence

Spec/semantics: `smithy_query(query="service_info")` gives endpointPrefix `medical-imaging` and the ARN pattern `^arn:aws...:medical-imaging:[a-z0-9-]+:[0-9]{12}:datastore/[0-9a-z]{32}...$` — a data store is a regional resource whose ARN embeds its Region. Well-established AWS behavior: control-plane resources are not visible across Regions.
Handler: src/ops/list_datastores.rs lines 199-237 collects `store.datastores.values()` filtered only by `datastoreStatus`; no Region predicate. src/ops/get_datastore.rs lines 130-136 looks a store up by id alone.
State: src/state.rs `Datastore` (lines 1004-1022) has no region field — `datastore_arn` is the only trace of the creating Region. Contrast `resource_tags`/`resource_tags_mut` (lines 1124-1145), which refuse an ARN whose Region differs.
Tests: every suite signs with the same `us-east-1` credential (tests/all/datastore_lifecycle.rs line 31); no test drives two Regions, so nothing asserts the isolation. Repo precedent for this gap class: `20261024T064607Z-dlm-createlifecyclepolicy-wrong-behavior` and `20261024T070424Z-mwaa-serverless-listworkflows-wrong-behavior`.

## Steps to Reproduce

1. POST /datastore signed with a SigV4 credential scope naming us-east-1 (…/20260101/us-east-1/medical-imaging/aws4_request) and note the datastoreId
2. GET /datastore signed with a credential scope naming eu-west-1 — expected an empty datastoreSummaries, actual the us-east-1 data store is listed with its us-east-1 ARN
3. GET /datastore/{datastoreId} signed for eu-west-1 — expected 404 ResourceNotFoundException, actual 200 with datastoreProperties whose datastoreArn names us-east-1
