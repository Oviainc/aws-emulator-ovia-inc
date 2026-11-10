Issue-Source: trace-sweep-divergence
Issue-Trace: healthlake-fhir-import-job-loads-ndjson-patient
Issue-Step: task#9
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: healthlake-fhir-import-job-loads-ndjson-patient
Issue-Shape-Id: com.amazonaws.healthlake#HealthLake
Issue-Key: healthlake|DescribeFHIRImportJob|2xx->2xx|$.ImportJobProperties.EndTime
Issue-Key: healthlake|DescribeFHIRImportJob|2xx->2xx|$.ImportJobProperties.JobOutputDataConfig.S3Configuration.S3Uri
Issue-Key: healthlake|DescribeFHIRImportJob|2xx->2xx|$.ImportJobProperties.JobProgressReport.Throughput
Issue-Key: healthlake|DescribeFHIRImportJob|2xx->2xx|$.ImportJobProperties.SubmitTime
Issue-Commit: d1403c3a93a205df9bcd3cf9f9f3f511e57967f4

# healthlake: DescribeFHIRImportJob misreports the job's output location and omits JobProgressReport.Throughput

> _Note: this issue was identified against the code base at commit `d1403c3a93a205df9bcd3cf9f9f3f511e57967f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeFHIRImportJob

## Description
Two distinct root causes in the healthlake service's FHIR import-job read path, found by replaying the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout (7 fixtures, 50 steps, 46 passed, 1 mismatched cluster). Both surface in the SAME response body — DescribeFHIRImportJob's ImportJobProperties for a settled import job — so they share one service and one representative fixture step and are bundled here. A fixer must close BOTH before submitting. They are reported by DescribeFHIRImportJob and by ListFHIRImportJobs alike, because both read the one ImportJob record through ImportJob::properties().

=== Cause 1 of 2: JobOutputDataConfig.S3Configuration.S3Uri is echoed back verbatim instead of the job-scoped output prefix the service writes its results under ===
AWS: Smithy (services/service-com-amazonaws-healthlake-healthlake/smithy-model/com-amazonaws-healthlake-healthlake.json): DescribeFHIRImportJob -> ImportJobProperties (required), whose member JobOutputDataConfig targets the union OutputDataConfig { S3Configuration: S3Configuration { S3Uri (required, @length max 1024, @pattern ^s3://[a-z0-9][\.\-a-z0-9]{1,61}[a-z0-9](/.*)?$), KmsKeyId (required) } }; StartFHIRImportJob takes the same shape as its required JobOutputDataConfig and the model documents it only as "The output data configuration supplied when the export job was created" — it states no text for the value HealthLake reports back, so: recorded behavior only. The recording shows that the S3Uri a settled import job REPORTS is not the bare location the request supplied: it is that location extended by one path segment naming the job, of the form <supplied S3Uri>/<32-hex id>-FHIR_IMPORT-<32-hex id>/ (the recorded pair is the job's DatastoreId followed by its JobId, matching AWS's documented naming of the per-job output folder <datastore-id>-FHIR_IMPORT-<job-id>, with a trailing slash). The fixed parts of that template are the literal "-FHIR_IMPORT-" infix and the trailing "/"; the variable parts are the bucket/prefix taken from the request's JobOutputDataConfig and the two ids taken from the job record (its DatastoreId and its own JobId). The concrete bucket name, prefix and hex ids in any one recording are that run's values and are NOT part of the requirement.
Emulator: the job record stores "The `JobOutputDataConfig` the request supplied, reported back verbatim" (services/service-com-amazonaws-healthlake-healthlake/src/state.rs, ImportJob.output_data_config and ImportJob::properties()), so DescribeFHIRImportJob answers HTTP 200 with $.ImportJobProperties.JobOutputDataConfig.S3Configuration.S3Uri equal to exactly the request's value — in this replay "s3://<bucket>/output/" where the recording has "s3://<bucket>/output/<datastoreId>-FHIR_IMPORT-<jobId>/". The per-job segment is never synthesized anywhere in the crate.
Why: the reported S3Uri is how a client finds what the import produced — the job's manifest/failure files are written under the per-job folder, and a client that lists or reads the reported prefix on the emulator is pointed at the whole user-supplied output location instead of this job's own folder. Two concurrent import jobs into the same output bucket become indistinguishable, and any code that parses the DatastoreId/JobId back out of the reported URI (or that asserts the job wrote under its own prefix) fails against the emulator while passing against AWS.
Fix: the projection of ImportJobProperties.JobOutputDataConfig — ImportJob::properties() in the healthlake crate's state (services/service-com-amazonaws-healthlake-healthlake/src/state.rs), or the place StartFHIRImportJob stores output_data_config (src/ops/start_fhir_import_job.rs) — must derive the reported S3Configuration.S3Uri from the supplied one plus the job-scoped segment (<DatastoreId>-FHIR_IMPORT-<JobId>/, single-slash-joined, trailing slash preserved) rather than cloning the request value; KmsKeyId stays as supplied. ListFHIRImportJobs reads the same projection and must agree.
Traces: healthlake-fhir-import-job-loads-ndjson-patient#task#9 ($.ImportJobProperties.JobOutputDataConfig.S3Configuration.S3Uri).

=== Cause 2 of 2: JobProgressReport.Throughput is omitted from the progress report AWS always populates ===
AWS: Smithy: ImportJobProperties.JobProgressReport targets JobProgressReport, documented as "Displays the progress of the import job, including total resources scanned, total resources imported, and total size of data imported"; its member Throughput targets GenericDouble (a double) and is documented as "The transaction rate the import job is processed at". The model marks no member required, and documents no value, so: recorded behavior only. The recording shows a settled import job's JobProgressReport carrying Throughput PRESENT as a number (0.0 for this small single-file NDJSON import) alongside the counts — i.e. Throughput is part of the report AWS serializes, reported as a rate (0.0 when the measured rate rounds to zero), not elided.
Emulator: progress_report() in services/service-com-amazonaws-healthlake-healthlake/src/state/import_run.rs sets every count but deliberately leaves Throughput absent ("a member nothing measured is left ABSENT rather than reported as zero — `Throughput` is a rate nothing timed"), so the serialized JobProgressReport has no Throughput key at all: the replay reports $.ImportJobProperties.JobProgressReport.Throughput missing (expected 0.0, got None) while the sibling counts match.
Why: a client that reads JobProgressReport.Throughput to display or chart import progress gets a null/absent member where AWS always gives it a number; strongly-typed consumers that treat the modelled double as reliably present (or that compute a progress summary over the whole report) see an absent field only on the emulator, so progress-monitoring code cannot be exercised against it.
Fix: the JobProgressReport construction in the healthlake crate's import run (progress_report() in src/state/import_run.rs) must populate Throughput for every reported run — the run's measured rate, 0.0 when nothing was timed or the rate rounds to zero — so the member is present whenever JobProgressReport is. Only Throughput is at issue here: TotalFilesConverted / TotalResourcesGenerated are documented as populated only for the transformation-phase flow and the recording does not assert them.
Traces: healthlake-fhir-import-job-loads-ndjson-patient#task#9 ($.ImportJobProperties.JobProgressReport.Throughput).

Not filed from this cluster: the same step also flags $.ImportJobProperties.SubmitTime and $.ImportJobProperties.EndTime, whose expected values are '<shaped-not-asserted>' and whose actual values are this run's epoch-second clock readings (SubmitTime ~0.5s before EndTime, the job's chosen settle delay). Those are per-run clock values, not a requirement, and no change is asked for there.

No open issue covers either cause: the open healthlake issues concern the data-transformation profile/job surface, TransformData's reachability and DescribeFHIRDatastore's published DatastoreEndpoint (20261109T135225Z-healthlake-3-net-new-causes-datastoreendpoint-is-published.md), none of them the import job's reported output location or progress report, and the replay reports this cluster as neither stub-blocked nor owned.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout: emulator_up=true (/__admin/health 200), bootstrap_signal=false, 7 fixtures replayed, 50 steps (46 passed, 1 mismatched, 0 errored, 3 blocked), score 92.0%, divergence_count=1.

Divergence cluster (verdict=mismatch, stub_path empty, owned_by empty):
  fixture_id: healthlake-fhir-import-job-loads-ndjson-patient
  step_key:   task#9
  service:    healthlake   operation: DescribeFHIRImportJob
  expected_status 200 -> actual_status 200 (the status and the error-free shape are correct; the body's values diverge)
  divergence_key: service=healthlake|DescribeFHIRImportJob|status=200->200|keys=$.ImportJobProperties.EndTime,$.ImportJobProperties.JobOutputDataConfig.S3Configuration.S3Uri,$.ImportJobProperties.JobProgressReport.Throughput,$.ImportJobProperties.SubmitTime
  affected (1): healthlake-fhir-import-job-loads-ndjson-patient#task#9

Per-key detail as replayed:
  $.ImportJobProperties.JobOutputDataConfig.S3Configuration.S3Uri: value_mismatch
      expected 's3://sim2real-probe-healthlake-f99fc061/output/072dd39eb02bdd97681b77b2a90b833b-FHIR_IMPORT-5cb5fab1a5b7383379eeebbdec82b92f/'
      got      's3://sim2real-probe-healthlake-f99fc061/output/'
      (the bucket name and the two 32-hex ids are this run's values; the requirement is the <supplied S3Uri>/<DatastoreId>-FHIR_IMPORT-<JobId>/ shape, whose fixed text is the '-FHIR_IMPORT-' infix and the trailing '/')
  $.ImportJobProperties.JobProgressReport.Throughput: missing (expected 0.0, got None)
      (the requirement is that the member is serialized; 0.0 is the recorded rate for this one-file import)
  $.ImportJobProperties.SubmitTime: value_mismatch (expected '<shaped-not-asserted>', got 1794293362.3382323)
  $.ImportJobProperties.EndTime:   value_mismatch (expected '<shaped-not-asserted>', got 1794293362.838335)
      — per-run clock readings produced by this replay's own run; NOT filed as a defect.

Emulator source backing the two filed causes:
  services/service-com-amazonaws-healthlake-healthlake/src/state.rs — ImportJob.output_data_config: "The `JobOutputDataConfig` the request supplied, reported back verbatim"; ImportJob::properties() clones it into ImportJobProperties.job_output_data_config.
  services/service-com-amazonaws-healthlake-healthlake/src/state/import_run.rs — progress_report(): "a member nothing measured is left ABSENT rather than reported as zero — `Throughput` is a rate nothing timed"; the constructed JobProgressReport sets no throughput field.

Smithy citations: DescribeFHIRImportJob(DatastoreId required, JobId required) -> ImportJobProperties (required); ImportJobProperties.JobOutputDataConfig -> OutputDataConfig union -> S3Configuration{S3Uri required @length max 1024 @pattern ^s3://..., KmsKeyId required}; ImportJobProperties.JobProgressReport -> JobProgressReport.Throughput: GenericDouble, "The transaction rate the import job is processed at"; errors InternalServerException 500 / ResourceNotFoundException 404 / ThrottlingException 429 / ValidationException 400 (none involved here).

## Steps to Reproduce
Boot the emulator from this checkout and point a healthlake client (awsJson1_0) at it, then re-replay fixture healthlake-fhir-import-job-loads-ndjson-patient, step task#9.

1. Create an S3 bucket in the emulator and PutObject one newline-delimited FHIR file (one Patient resource per line) under an input prefix, e.g. s3://<bucket>/input/patient.ndjson.
2. CreateFHIRDatastore(DatastoreTypeVersion="R4") and poll DescribeFHIRDatastore until DatastoreStatus == ACTIVE; note the DatastoreId.
3. StartFHIRImportJob(DatastoreId=<id>, InputDataConfig={"S3Uri":"s3://<bucket>/input/"}, JobOutputDataConfig={"S3Configuration":{"S3Uri":"s3://<bucket>/output/","KmsKeyId":"<key>"}}, DataAccessRoleArn=<role arn>, JobName=<name>); note the JobId.
4. Poll DescribeFHIRImportJob(DatastoreId=<id>, JobId=<jobId>) until JobStatus is terminal (COMPLETED), then read the response body.
   Cause 1 — ImportJobProperties.JobOutputDataConfig.S3Configuration.S3Uri:
     Expect: the supplied output location extended by this job's own folder — "s3://<bucket>/output/<DatastoreId>-FHIR_IMPORT-<JobId>/".
     Observe: "s3://<bucket>/output/" — the request value echoed verbatim.
   Cause 2 — ImportJobProperties.JobProgressReport.Throughput:
     Expect: the member present as a number (0.0 for this one-file import), alongside the TotalNumberOf* counts.
     Observe: the key is absent from JobProgressReport entirely.
5. Call ListFHIRImportJobs(DatastoreId=<id>) and check the same two members on the job's entry: they are projected from the same record and must agree with step 4's expectation.
(SubmitTime/EndTime in the same body are per-run clock values and are not under test.)
