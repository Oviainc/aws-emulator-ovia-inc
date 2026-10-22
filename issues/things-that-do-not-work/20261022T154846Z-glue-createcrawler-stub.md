Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr__glue__s3.cli-glue-crawler-create-run-check-catalog-workflow emr__glue__s3.cli-glue-crawler-missing-role-permission-error-negative emr__glue__s3.eventbridge-glue-crawler-completion-triggers-downstream-glue-job-positive emr__glue__s3.glue-crawler-assumes-role-trusting-glue-service-positive emr__glue__s3.glue-crawler-detects-s3-partitions-positive emr__glue__s3.glue-crawler-missing-s3-list-permission-rejected
Issue-Key: emr__glue__s3.cli-glue-crawler-create-run-check-catalog-workflow|_run_shell#15|AssertionError|raise AssertionError(
Issue-Key: emr__glue__s3.cli-glue-crawler-missing-role-permission-error-negative|_run_shell#13|AssertionError|raise AssertionError(
Issue-Key: glue|CreateCrawler|NotImplemented
Issue-Commit: db1fbaa8559703996072fdf323a0d84f8f863f33

# Implement glue CreateCrawler: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `db1fbaa8559703996072fdf323a0d84f8f863f33`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCrawler

## Description
`CreateCrawler` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `CreateCrawler` call, so each one unblocks only when `CreateCrawler` works: `emr__glue__s3.cli-glue-crawler-create-run-check-catalog-workflow`, `emr__glue__s3.cli-glue-crawler-missing-role-permission-error-negative`, `emr__glue__s3.eventbridge-glue-crawler-completion-triggers-downstream-glue-job-positive`, `emr__glue__s3.glue-crawler-assumes-role-trusting-glue-service-positive`, `emr__glue__s3.glue-crawler-detects-s3-partitions-positive`, `emr__glue__s3.glue-crawler-missing-s3-list-permission-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/create_crawler.rs`. Replaying the corpus, the emulator refused the `CreateCrawler` call: File "check_emr__glue__s3_cli_glue_crawler_create_run_check_catalog_workflow.py", line 111, in _run_aws
    proc = _run_shell(
        ctx,
    ...<3 lines>...
        check=check,
    )
  File "check_emr__glue__s3_cli_glue_crawler_create_run_check_catalog_workflow.py", line 103, in _run_shell
    raise AssertionError(
    ...<2 lines>...
    )
AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'glue', 'create-crawler', '--name', 'sim2real-crawler-83b8ff1a6f82', '--role', 'arn:aws:iam::000000000000:role/sim2real-crawler-role-6f1ea1593419', '--database-name', 'sim2real_crawler_db_d930253e5259', '--targets', '{"S3Targets": [{"Path": "s3://sim2real-glue-crawler-963ca131fb15/data/"}]}']
  stderr: 
aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateCrawler operation: operation CreateCrawler is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the glue emulator
Call CreateCrawler and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
