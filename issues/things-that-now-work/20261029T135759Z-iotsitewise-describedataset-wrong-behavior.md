Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 02c390dd8e861c3d98b16a550da31d4601fa546f

# A dataset whose enrichment job has COMPLETED still reports no enrichmentStatus, although the store holds that job keyed to the dataset and derives its completion from the clock

_Filed: 2026-10-29T13:57:59Z_
_Operation: DescribeDataset | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `02c390dd8e861c3d98b16a550da31d4601fa546f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateEnrichmentJob resolves the dataset its configuration names (refusing a job for a dataset the store does not hold), persists an `EnrichmentJobRecord` for it, and the crate DERIVES that job's whole lifecycle from the clock: `EnrichmentJobRecord::status` reports PENDING -> RUNNING -> COMPLETED and `completed_at` reports the instant it completed, both read back by DescribeEnrichmentJob and ListEnrichmentJobs. The record even exposes the link to the dataset as a public helper, `EnrichmentJobRecord::dataset_id()`.

The dataset reads never consult any of it. DescribeDataset hard-codes `enrichment_status: None`, and ListDatasets' summary projection does the same, each under a comment that admits the gap ("Nothing in this emulator enriches a dataset ... so no enrichment status is reported"). So after a caller starts an enrichment job on dataset D and that job reaches COMPLETED — which this emulator's own DescribeEnrichmentJob reports — DescribeDataset(D) and ListDatasets still answer as if nothing had ever been enriched: the completion is observable on the job and invisible on the resource the enrichment was performed on. ListDatasetDataSegments reports the per-segment `enrichment` the same way, pointing at DescribeDataset as its precedent, so the whole "enrich, then see that the data is enriched" workflow dead-ends.

The comment's justification ("no video is analysed and no embedding is produced") does not cover the status: the status is not an embedding, and the crate already manufactures every other lifecycle fact of that job from the stored instant. Scope of the defect is only the dataset that HAS a terminal enrichment job naming it; a dataset no job names has nothing to report and keeps answering as it does today.

## Evidence

The read that reports absence, with the comment admitting it:
  services/service-com-amazonaws-iotsitewise-awsiotsitewise/src/ops/describe_dataset.rs:241-245 —
    "            // Nothing in this emulator enriches a dataset: `CreateEnrichmentJob`
            // records the job and the lifecycle the clock owes it, but no video is
            // analysed and no embedding is produced, so no enrichment status is
            // reported.
            enrichment_status: None,"
the same projection in the listing:
  src/ops/list_datasets.rs:277-279 — "        // Nothing in this emulator enriches a dataset, so no enrichment status is\n        // reported; `DescribeDataset` answers the same way.\n        enrichment_status: None,"
and the segment-level one that cites them:
  src/ops/list_dataset_data_segments.rs:282-286 — "        // Nothing in this emulator enriches a data segment: `CreateEnrichmentJob`\n        // ... so no enrichment is reported — the same way `DescribeDataset`\n        // and `ListDatasets` report no dataset enrichment status.\n        enrichment: None,"

What the store actually holds — the dataset->job link and the derived terminal status:
  src/state.rs:8974-8979 — "impl EnrichmentJobRecord {\n    /// The dataset this job analyses: `EventDetection`'s \"IoT SiteWise dataset\n    /// ID containing the video time-series data to analyze\".\n    pub fn dataset_id(&self) -> &str {\n        enrichment_job_target(&self.configuration).0\n    }"
  src/state.rs:9008-9017 — "    pub fn status(&self, now: SystemTime) -> &'static str {\n        if self.cancelled_at.is_some() { return ENRICHMENT_JOB_CANCELLED; }\n        match self.completed_at(now) {\n            Some(_) => ENRICHMENT_JOB_COMPLETED,\n            None if now >= self.created_at + ENRICHMENT_JOB_STEP => ENRICHMENT_JOB_RUNNING,\n            None => ENRICHMENT_JOB_PENDING,\n        }\n    }"
  src/state.rs:9023-9029 — `completed_at` returning the instant the job completed.

That the job is firmly bound to an existing dataset of this same store:
  src/ops/create_enrichment_job.rs:237-246 — "        // \"The IoT SiteWise dataset ID containing the video time-series data to\n        // analyze\": the configuration names a dataset of this service's own control\n        // plane, so one no dataset holds is the modelled not-found refusal.\n        let dataset_id = crate::state::enrichment_job_target(&input.job_configuration).0;\n        if !store.datasets.contains_key(dataset_id) { return Err(not_found(...)); }"

The member that is left empty, from the model: DatasetSummary$enrichmentStatus targets `DatasetEnrichment` (`video` -> `DatasetEnrichmentEntry { status (required), lastEnrichedAt }`), and `EnrichmentStatus` is the two-value enum `["ENRICHED", "NOT_ENRICHED"]` — i.e. a status, not an embedding payload.

Test coverage: nothing pins the enriched case. tests/all/dataset_lifecycle.rs:346-352 pins only a dataset NOTHING has enriched — "// Nothing has enriched this dataset, so no enrichment status is reported." with `described.get("enrichmentStatus") == None` — and tests/all/list_dataset_data_segments.rs:505-512 likewise asserts over a fixture with no enrichment job. No test creates an enrichment job and then reads the dataset it named.

## Steps to Reproduce

1. CreateWorkspace W, then CreateDataset in W (a SESSION dataset) and ingest/record the video time series the dataset holds, so the dataset D exists.
2. POST /workspaces/W/enrichment-jobs with a jobConfiguration whose eventDetection.datasetId = D (and a timeSeriesId/propertyAlias of that dataset) -> 2xx with a jobId.
3. Poll GET /workspaces/W/enrichment-jobs/{jobId} until it reports status COMPLETED (the crate derives this from ENRICHMENT_JOB_STEP; state.rs:9008-9029).
4. GET /datasets/{D} (DescribeDataset).
5. Observed: the response carries no `enrichmentStatus` at all — identical to the answer for a dataset no job has ever named. GET /datasets?sourceType=... (ListDatasets) reports the same, and GET /datasets/{D}/data-segments reports `enrichment` absent for every segment of the enriched time series.
6. Expected: the dataset reports the enrichment the store already knows about — derived from the COMPLETED job that names it, exactly as DescribeEnrichmentJob derives that job's status and completion instant from the same record. A dataset no enrichment job names keeps reporting none, so tests/all/dataset_lifecycle.rs:346-352 stays green.
