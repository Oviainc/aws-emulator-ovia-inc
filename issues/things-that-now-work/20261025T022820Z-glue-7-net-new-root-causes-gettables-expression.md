Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.glue-tsd-get-tables-expression-star-is-a-wildcard-and-the-rest-is-regex glue.glue-tsd-get-tables-in-a-foreign-catalog-denied glue.gjwr-glue4-job-round-trips-declared-values-and-capacity-defaults glue.gjwr-start-job-run-through-other-region-rejected glue__sqs.glue-crawler-missing-sqs-receive-permission-rejected
Issue-Key: glue.glue-tsd-get-tables-expression-star-is-a-wildcard-and-the-rest-is-regex|run#9|AssertionError|assert wrong == {}, f"Expression selected the wrong tables; pattern -> (got, want): {wrong!r}"
Issue-Key: glue.glue-tsd-get-tables-in-a-foreign-catalog-denied|run#12|AssertionError|assert got == want, (
Issue-Key: glue.gjwr-glue4-job-round-trips-declared-values-and-capacity-defaults|run#14|AssertionError|assert filled == ("G.1X", 10, 10.0, {"MaxConcurrentRuns": 1}), (
Issue-Key: glue.gjwr-start-job-run-through-other-region-rejected|_error#6|AssertionError|raise AssertionError("the call under test SUCCEEDED where real AWS refuses it")
Issue-Key: glue__sqs.glue-crawler-missing-sqs-receive-permission-rejected|_create_crawler_expect_error#16|AssertionError|raise AssertionError("expected CreateCrawler to be rejected, but it succeeded")
Issue-Commit: e9591ea0ffea53e5939624d35a733297d20386ca

# Glue: GetTables Expression is not a regex and a foreign CatalogId is not denied, Glue 4.0 job capacity defaults are never applied, jobs are not region-scoped, and CreateCrawler skips the event-queue permission check

> _Note: this issue was identified against the code base at commit `e9591ea0ffea53e5939624d35a733297d20386ca`._
>
> _This report originally bundled SEVEN root causes. The five below are closed (see "Resolution"). The other two — CloudFormation-declared AWS::Glue resources not existing under their declared names, and a started job run executing no script — are NOT closed: they were split out, with their own replay keys, into `issues/things-that-do-not-work/20261025T022820Z-glue-cfn-declared-resources-and-job-run-execution.md`, so nothing here reports them as working._

## Operation
StartJobRun

## Description
This issue bundles 5 root causes in which the emulator answers 2xx (or the wrong 4xx) for Glue requests whose semantics real AWS implements.

Cause 1 of 5: GetTables ignores/does not implement the Expression filter as a regular expression.
AWS: Smithy GetTablesRequest.Expression (FilterString, length 0..2048) is documented as 'A regular expression pattern. If present, only those tables whose names match the pattern are returned.' Recorded: Expression='ev_.*' selects the tables ev_one and ev_two, and Expression='ev?' selects the table named ev (a bare '*' is additionally accepted as a match-everything wildcard, and an upper-case pattern selects nothing, so the comparison is case-sensitive).
Emulator: every one of those patterns selected NOTHING — got [] where ev_one/ev_two were owed for 'ev_.*' and [] where ev was owed for 'ev?'.
Why: a caller filtering a large catalog server-side silently receives an empty catalog rather than its matching tables — the failure mode is 'the database looks empty', which a client cannot distinguish from a real empty database.
Traces: glue.glue-tsd-get-tables-expression-star-is-a-wildcard-and-the-rest-is-regex.

Cause 2 of 5: a GetTables against another account's CatalogId is answered as not-found instead of denied.
AWS: Smithy GetTables takes CatalogId and models both EntityNotFoundException and (via the catalog resource-policy path) an access-denied refusal. Recorded: naming a CatalogId belonging to a different account, with no resource-based policy granting the caller, is refused with 400 AccessDeniedException whose message is built from the request (principal ARN, action glue:GetTables, the foreign catalog ARN) — the authorization decision happens before any lookup of the database.
Emulator: 400 EntityNotFoundException ('Database <name> not found.'), i.e. the foreign CatalogId was ignored and the lookup performed in the caller's own catalog.
Why: cross-account catalog access was silently reinterpreted as local access. A caller probing whether it may read another account's catalog was told the database does not exist; worse, had the same-named database existed locally the call would have SUCCEEDED and returned the wrong account's tables.
Traces: glue.glue-tsd-get-tables-in-a-foreign-catalog-denied.

Cause 3 of 5: CreateJob does not apply the Glue 4.0 worker/capacity defaults, so GetJob reports an under-specified job.
AWS: Smithy CreateJob takes WorkerType (enum), NumberOfWorkers and MaxCapacity (all optional) with no modeled default; recorded behavior for a Glue 4.0 glueetl job created with none of them set is that the service fills them in and GetJob reads back WorkerType='G.1X', NumberOfWorkers=10 and MaxCapacity=10.0, alongside ExecutionProperty {'MaxConcurrentRuns': 1}.
Emulator: the same job read back (None, None, None) for (WorkerType, NumberOfWorkers, MaxCapacity); only MaxConcurrentRuns=1 was present.
Why: the job representation is incomplete — a client that reads a job back to size a run, to re-create it from the read-back definition, or to display its capacity gets nulls where AWS always provides concrete values, and a round-trip re-create produces a different job.
Traces: glue.gjwr-glue4-job-round-trips-declared-values-and-capacity-defaults.

Cause 4 of 5: Glue jobs are not scoped to the region they were created in, so a cross-region StartJobRun succeeds.
AWS: recorded behavior — a job created in region A does not exist in region B: StartJobRun in B is refused with 400 EntityNotFoundException (recorded message 'Failed to start job run due to missing metadata.'), GetJobRuns in B is refused with 400 EntityNotFoundException ('Job not found.'), no run is recorded anywhere, and the job remains served in A with an empty run history.
Emulator: StartJobRun through the other region's endpoint SUCCEEDED — the call under test returned 2xx and started a run for a job that region does not own.
Why: region isolation was absent for Glue state, which made every region-scoping test pass incorrectly and let a misconfigured client start billable runs against a region where the job does not exist; it also meant two regions shared one namespace, so a same-named job in two regions collided.
Traces: glue.gjwr-start-job-run-through-other-region-rejected.

Cause 5 of 5: CreateCrawler does not validate the crawler role's SQS access to the S3 target's EventQueueArn.
AWS: Smithy CreateCrawler (Name, Role, Targets required) models InvalidInputException (400). Recorded: for an S3Target that carries an EventQueueArn, Glue checks at CREATE time that the supplied Role can consume that queue; a role with no SQS grant on it is refused with 400 InvalidInputException whose message is 'SQS queue <queue arn> does not exist or the role provided does not have access to it.' and NO crawler is created, while an identically-shaped request whose role does hold the grant is created.
Emulator: CreateCrawler SUCCEEDED for the role with no SQS grant, and the crawler existed.
Why: the negative path of event-mode crawler provisioning inverted — a client was told its wiring is valid, and the failure moved from create time to crawl time (or, in an emulator that never crawls, to nowhere at all). This is a distinct check from the role TRUST validation filed in 20261022T192452Z-glue-createcrawler-skips-role-validation-and-deleteschemaversions-raises.md (Cause 1), where the role is un-assumable; here the role is assumable but lacks sqs:ReceiveMessage/GetQueueAttributes/DeleteMessage on the EventQueueArn.
Traces: glue__sqs.glue-crawler-missing-sqs-receive-permission-rejected.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the checkout this issue was filed on (20/223 checks passed).

Cause 1 — "AssertionError: Expression selected the wrong tables; pattern -> (got, want): {'ev_.*': ([], ['ev_one', 'ev_two']), 'ev?': ([], ['ev'])}".
Cause 2 — "AssertionError: GetTables against a foreign CatalogId returned ('EntityNotFoundException', 'Database <db> not found.'), expected ('AccessDeniedException', 'User: arn:aws:iam::<acct>:user/... is not authorized to perform: glue:GetTables on resource: arn:aws:glue:<region>:<other-acct>:catalog because no resource-based policy allows the glue:GetTables action')" — the requirement is the 400 AccessDeniedException code; the message text is the recorded example of the request-derived template.
Cause 3 — "AssertionError: defaults for an omitted worker config came back as (None, None, None, {'MaxConcurrentRuns': 1}), expected ('G.1X', 10, 10.0, {'MaxConcurrentRuns': 1})".
Cause 4 — "AssertionError: the call under test SUCCEEDED where real AWS refuses it" at `got = _error(lambda: g_other.start_job_run(JobName=name))`; AWS answers EntityNotFoundException for both StartJobRun and GetJobRuns in the other region.
Cause 5 — "AssertionError: expected CreateCrawler to be rejected, but it succeeded" for CreateCrawler(Role=<role without an SQS grant>, Targets={'S3Targets': [{'Path': 's3://<bucket>/bad/', 'EventQueueArn': <queue arn>}]}, RecrawlPolicy={'RecrawlBehavior': 'CRAWL_EVENT_MODE'}); expected 400 InvalidInputException.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay simparity-behavioral-ga-prod-v1, re-running the check ids in trace_ids.
2. Cause 1: create tables ev, ev_one, ev_two in a database, then glue:GetTables(DatabaseName=..., Expression='ev_.*') — expect ev_one and ev_two; and Expression='ev?' — expect ev.
3. Cause 2: glue:GetTables(CatalogId=<another account id>, DatabaseName=<any>) — expect 400 AccessDeniedException naming glue:GetTables on the foreign catalog ARN, not EntityNotFoundException.
4. Cause 3: glue:CreateJob with GlueVersion='4.0', Command {'Name': 'glueetl'} and no WorkerType/NumberOfWorkers/MaxCapacity, then glue:GetJob — expect WorkerType='G.1X', NumberOfWorkers=10, MaxCapacity=10.0.
5. Cause 4: CreateJob in region A, then StartJobRun and GetJobRuns through region B's endpoint — expect 400 EntityNotFoundException from both, no run anywhere, and the job still served in A with an empty run history.
6. Cause 5: create a queue and a Glue-assumable role with NO sqs permissions on it, then glue:CreateCrawler with an S3Target carrying that EventQueueArn — expect 400 InvalidInputException and no crawler; repeat with a role that holds the SQS grant — expect success.

## Resolution
All five causes above are closed, each by the rule it is an instance of rather than by its recorded values:

1. The Data Catalog's table-listing pattern language is a regular expression in which `*` additionally stands for any run of characters, matched against the WHOLE name, case-sensitively, compiled once through the linear-time `regex` crate under a compiled-size ceiling (`glue::state::compile_name_pattern`). Both listings of these tables use it — `GetTables$Expression` and Athena's `ListTableMetadata$Expression` — so the two can never disagree, and a value that is no pattern is refused with each operation's own modelled client error.
2. A request addressing another account's Data Catalog is an authorization decision taken BEFORE the entity lookup (`glue::state::foreign_catalog_refusal`): with no resource policy granting the caller it is refused with AccessDeniedException (400) naming the principal, the `glue:<Operation>` action and the catalog ARN. Applied to every implemented operation that addresses an entity under a `CatalogId`.
3. A job definition naming no capacity is stored with the figure the service documents for its command — ten DPUs for a Spark batch/streaming job, 0.0625 for a Python shell job — expressed from Glue 2.0 on as the worker configuration that version takes (ten G.1X workers). Shared by CreateJob and UpdateJob and inherited by a run.
4. Job definitions and their runs are keyed by Region: CreateJob, GetJob, UpdateJob, StartJobRun, GetJobRun, GetJobRuns, BatchGetJobs, BatchStopJobRun and ListJobs resolve a name only in the endpoint's Region, and StartJobRun/GetJobRuns answer the recorded wording for a job that Region does not hold. GetJobRuns (a 501 stub the region check reaches) is implemented.
5. CreateCrawler evaluates the crawler role's policies in force — read from the account's own IAM records through the shared IAM evaluator — against each event queue its S3 targets name, refusing with InvalidInputException (400) and storing no crawler when the role cannot consume one. GetCrawlers (the 501 stub the check reaches next) is implemented.
