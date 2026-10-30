Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected athena__glue__s3.cli-glue-crawler-run-then-athena-query-workflow athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive
Issue-Key: athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected|_start_crawler_with_retry#10|Timeout|time.sleep(interval_s)
Issue-Key: athena__glue__s3.cli-glue-crawler-run-then-athena-query-workflow|_start_crawler_with_retry#10|Timeout|time.sleep(interval_s)
Issue-Key: athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive|_start_crawler_with_retry#27|Timeout|time.sleep(10)
Issue-Commit: 2fe9ad840c0110bf567bc4da7d67a08009876174

# Glue StartCrawler refuses every crawl (501 NotImplemented), so no crawler run executes, publishes a table or emits a Glue Crawler State Change event — reached through the athena__glue__s3 checks (1 cause)

> _Note: this issue was identified against the code base at commit `2fe9ad840c0110bf567bc4da7d67a08009876174`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
Cause 1 of 1: Glue StartCrawler is an unimplemented scaffold stub, so no crawler run ever executes and none of the post-start crawler semantics is observable.

Related open issues (named here so a fixer can see the overlap rather than treating this as unrelated work): 20261024T015202Z-glue-startcrawler-stub.md names the unimplemented operation and its handler file, and 20261029T184212Z-glue-crawlers-never-run-and-a-glue-job.md carries the same cause grounded in the emr__glue__s3 checks. Neither carries the three athena__glue__s3 replay keys below, and in this suite these three checks are NOT attributed to the StartCrawler stub cluster because the refusal is swallowed by the checks' own retry wrapper and surfaces as a check timeout instead of a NotImplemented error.

AWS: Smithy — StartCrawler takes Name (required) and answers 200 with an empty output shape; its modeled errors are EntityNotFoundException, CrawlerRunningException and OperationTimeoutException only — NotImplemented is not a modeled outcome and 501 is not a status this operation ever answers. After an accepted start, GetCrawler.Crawler.State leaves READY (RUNNING, then STOPPING) and returns to READY with Crawler.LastCrawl present and LastCrawl.Status == SUCCEEDED; the crawl infers a schema from the bytes of the objects under the S3 target path and publishes a table into the crawler's DatabaseName (GetTables then lists it). Recorded behavior for the two further legs these checks assert: (a) a crawler whose role lacks s3:GetObject/s3:ListBucket on the target bucket completes its crawl but publishes NO table (the S3 binding is a real permission boundary), and granting the permission and re-running the SAME crawler then publishes the table; (b) a crawl that finishes Succeeded emits an EventBridge event with source "aws.glue", detail-type "Glue Crawler State Change" and detail.crawlerName / detail.state == "Succeeded", which a matching rule delivers to its target.
Emulator: StartCrawler answers the scaffold refusal — HTTP 501, error code NotImplemented, message "operation StartCrawler is scaffolded but not implemented in this emulator" (fixed text apart from the operation name, which is taken from the request). The fixed text is "operation <OperationName> is scaffolded but not implemented in this emulator"; AWS has no such message or code for this operation at all. Because every start is refused, `aws glue start-crawler` exits non-zero on all six propagation retries (10s apart) and each check exhausts its budget — the suite reports "Check timed out after 60.0s" for all three. Consequently the crawler never leaves READY, no LastCrawl is ever recorded, no table is published, the permission boundary cannot be exercised, and no "Glue Crawler State Change" event is published.
Why: every client workflow that catalogs S3 data by crawling is unusable — a crawl cannot be started at all, and the downstream steps that depend on it (querying the crawled table, asserting that a role without S3 read publishes no table, and event-driven pipelines keyed on "Glue Crawler State Change") can never run. A caller also sees an unmodeled 501/NotImplemented where its SDK expects either success or one of the three modeled errors, so retry and error-handling logic built against the model mis-branches.
Fix: services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs (the operation's handler) plus the crawler run state in the Glue service state that GetCrawler reads (State transitions and the LastCrawl record), the schema inference/table publication that a crawl performs against the S3 target, the role's S3 read permission check, and the Glue Crawler State Change event publication.
Traces: athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected, athena__glue__s3.cli-glue-crawler-run-then-athena-query-workflow, athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive.

## Evidence
Expected (AWS): StartCrawler(Name) -> 200, empty body; then GetCrawler shows State leaving READY and returning to READY with LastCrawl.Status == "SUCCEEDED", and GetTables on the crawler's database lists the inferred table.
Actual (emulator): StartCrawler -> HTTP 501, Error.Code "NotImplemented", Error.Message "operation StartCrawler is scaffolded but not implemented in this emulator" (same stub refusal the suite reports verbatim for the other StartCrawler-blocked checks in this run, which it summarises as: UNIMPLEMENTED `StartCrawler` (services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs), which blocks 34 check(s) in this suite). Message anatomy: fixed text "operation " + <operation name from the request> + " is scaffolded but not implemented in this emulator"; no variable part comes from Glue state.
Replay details for the three grounding checks (the refusal is retried 6x at 10s by each check's own _start_crawler_with_retry / `aws glue start-crawler` wrapper, which consumes the whole check budget):
  - athena__glue__s3.cli-glue-crawler-missing-s3-permission-rejected — "Error: Check timed out after 60.0s" (blocked in _start_crawler_with_retry before the negative leg's GetTables assertion).
  - athena__glue__s3.cli-glue-crawler-run-then-athena-query-workflow — "Error: Check timed out after 60.0s" (blocked in _start_crawler_with_retry before _wait_crawler_ready).
  - athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive — "Error: Check timed out after 60.0s" (blocked in _start_crawler_with_retry, which calls glue.start_crawler(Name=...) directly, before _wait_for_crawler_state and the archived-event assertions).

## Steps to Reproduce
1. s3.CreateBucket + s3.PutObject(Key="crawler-data/rows.csv", Body="id,name\n1,alice\n2,bob\n").
2. iam.CreateRole trusting glue.amazonaws.com with S3 read on that bucket.
3. glue.CreateDatabase(DatabaseInput={Name: <db>}).
4. glue.CreateCrawler(Name=<crawler>, Role=<role arn>, DatabaseName=<db>, Targets={S3Targets:[{Path: s3://<bucket>/crawler-data/}]}) — accepted.
5. glue.StartCrawler(Name=<crawler>).
   Observed: HTTP 501 NotImplemented "operation StartCrawler is scaffolded but not implemented in this emulator".
   Expected: 200 with an empty body.
6. Poll glue.GetCrawler(Name=<crawler>): State stays READY forever, no LastCrawl member; glue.GetTables(DatabaseName=<db>) stays empty; no "Glue Crawler State Change" event is delivered to an EventBridge rule matching {source:[aws.glue], detail-type:[Glue Crawler State Change], detail:{crawlerName:[<crawler>], state:[Succeeded]}}.
