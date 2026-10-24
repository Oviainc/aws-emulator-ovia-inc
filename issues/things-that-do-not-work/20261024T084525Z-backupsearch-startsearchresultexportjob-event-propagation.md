Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 1f773e23b3e0bd2b9ce8ba90d57212de869e61a4

# Export job never writes the results object into the destination S3 bucket and never resolves the bucket

_Filed: 2026-10-24T08:45:25Z_
_Operation: StartSearchResultExportJob | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `1f773e23b3e0bd2b9ce8ba90d57212de869e61a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartSearchResultExportJob is documented as "starts a job to export the results of search job to a designated S3 bucket" (Smithy op doc), with the destination given by ExportSpecification.s3ExportSpecification.DestinationBucket (@required) and the optional DestinationPrefix. On AWS the export job assumes the supplied RoleArn, writes a results file (CSV + metadata) under that bucket/prefix, and settles to ExportJobStatus COMPLETED; the object is then observable through S3 (the behavioral recordings for this surface create the destination bucket with s3.create_bucket before starting the export, which is why the bucket is part of the observable contract).

The emulator's handler performs NO S3 interaction at all:
(a) the DestinationBucket is stored on the ExportJob record and echoed back by GetSearchResultExportJob/ListSearchResultExportJobs, but it is never resolved against the S3 emulator, so an export addressed at a bucket that does not exist is accepted and settles to COMPLETED/FAILED exactly as one addressed at a real bucket;
(b) no object is ever created in the destination bucket, so a caller that exports and then does s3:GetObject/ListObjectsV2 under the prefix finds nothing;
(c) worse, the handler deliberately forces the terminal status FAILED whenever the search job has any results to export (src/ops/start_search_result_export_job.rs:370-380), with an emulator-authored StatusMessage ("this emulator cannot transmit the N result(s) of search job '<id>' to the destination bucket") that AWS never emits, and only COMPLETEs the degenerate empty-result case — which writes nothing either.

Expected: the export job resolves the destination bucket through the S3 emulator (refusing/failing the job the way AWS does when it is absent or not writable), writes the results object under DestinationBucket/DestinationPrefix, and settles to COMPLETED with no emulator-authored failure text; a later S3 read of that key returns the exported results.

## Evidence

Spec: Smithy StartSearchResultExportJob doc ("export the results of search job to a designated S3 bucket"), ExportSpecification union -> S3ExportSpecification{DestinationBucket @required, DestinationPrefix}, ExportJobStatus enum COMPLETED|FAILED|RUNNING. Handler: services/service-com-amazonaws-backupsearch-cryobackupsearchservice/src/ops/start_search_result_export_job.rs — the only use of export_specification is to store it (line 288-298, 393); there is no S3 dependency in the crate's Cargo.toml and no bucket lookup anywhere; lines 363-380 choose FAILED for any non-empty result set with a message that names the emulator. Test coverage: tests/all/export_jobs.rs::an_export_that_would_have_to_transmit_items_ends_in_the_modelled_failed_status pins the emulator's FAILED branch (it would have to be updated by the fix); no test asserts anything about the destination bucket or an exported object. Existing issues: 20261023T110504Z (export/stop refusals) covers only the RoleArn error code and refusal message templates; 20261024T024736Z covers search-scope resolution against AWS Backup recovery points — neither mentions the destination bucket or the written object.

## Steps to Reproduce

1. Boot the emulator with the backup-search and s3 services mounted.
2. s3: CreateBucket(Bucket="export-dest").
3. backup-search: StartSearchJob(SearchScope={"BackupResourceTypes":["S3"]}), then StopSearchJob and poll GetSearchJob until Status=STOPPED (a status an export may start from).
4. StartSearchResultExportJob(SearchJobIdentifier=<id>, ExportSpecification={"s3ExportSpecification":{"DestinationBucket":"export-dest","DestinationPrefix":"results/"}}, RoleArn=<role in this account>).
5. Poll GetSearchResultExportJob until the status is terminal, then s3: ListObjectsV2(Bucket="export-dest", Prefix="results/"): the emulator reports COMPLETED (or FAILED with emulator-authored text when the job had results) but the bucket is empty — AWS leaves the exported results object there.
6. Repeat with DestinationBucket="bucket-that-does-not-exist": the emulator accepts and settles the export exactly as before instead of surfacing the destination failure AWS does.
