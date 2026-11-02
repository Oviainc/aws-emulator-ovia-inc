Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-signals.concurrent-create-slo-same-name-exactly-one-wins
Issue-Shape-Id: com.amazonaws.applicationsignals#ApplicationSignals
Issue-Key: application-signals.concurrent-create-slo-same-name-exactly-one-wins|run#14|AssertionError|assert all(message == expected for _, _c, message in failures), (
Issue-Commit: 798fb32aa8eb1e75369deb4b48c94746eb1ca248

# application-signals CreateServiceLevelObjective duplicate-Name ConflictException under concurrency words an emulator-invented ARN-echoing message instead of AWS's generic conflict sentence

> _Note: this issue was identified against the code base at commit `798fb32aa8eb1e75369deb4b48c94746eb1ca248`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateServiceLevelObjective

## Description
Related open issue: 20261029T192739Z-application-signals-words-seven-refusal-messages-differently-from.md bundles seven refusal-message divergences for this service, and its Cause 4 describes the SAME duplicate-Name conflict wording reached through the SEQUENTIAL path (check application-signals.slo-duplicate-name-conflict-rejected). That issue does NOT carry this check's replay key, so this failure is not covered by it and is filed here; a fixer closing that issue should re-replay the check listed below as well, because this is the concurrent/racing entry into the same uniqueness guard.

Cause 1 of 1: CreateServiceLevelObjective duplicate-Name ConflictException message, on the concurrent-race path.

AWS: Smithy (services/service-com-amazonaws-applicationsignals-applicationsignals/smithy-model/com-amazonaws-applicationsignals-applicationsignals.json): CreateServiceLevelObjective POST /slo, success code 200; input member Name is required, target ServiceLevelObjectiveName with pattern ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$; modeled errors AccessDeniedException(403), ConflictException(409), ServiceQuotaExceededException(402), ThrottlingException(429), ValidationException(400). ConflictException is a client error with httpError 409, a single required member Message, documented "This operation attempted to create a resource that already exists." The model does not state the message text, so: recorded behavior only for the wording — when N callers race CreateServiceLevelObjective on the same Name, AWS admits exactly one and refuses every other racer 409 ConflictException with Message `A resource with the same identifier already exists`. That whole sentence is FIXED text, with NO trailing period and NO variable parts: it does not name the SLO, its Name, or its ARN. The uniqueness decision itself is atomic — exactly one winner regardless of how many racers arrive together.

Emulator: the atomicity and the error code are correct — exactly one of 8 concurrent creates succeeded, the other 7 were each refused ConflictException. What diverges is the Message: all 7 losers answered `Slo with arn 'arn:aws:application-signals:<region>:<account>:slo/<Name>' already exists.`, i.e. the crate's generic already-exists template, which interpolates the computed ARN of the winning SLO and appends a period, in place of AWS's generic fixed sentence. (The ARN in the replay output is one run's value; the defect is the template, not the value.)

Why: a client racing creates (an idempotent provisioner, a Terraform/CDK-style converger, a leader-election-on-create pattern) distinguishes "someone else won the race" from other 409s by the message, and logs/alerts it verbatim; it never matches AWS's "same identifier" phrasing and so misclassifies the loser outcome. The emulator additionally discloses the ARN of a resource the losing caller never named, in a field AWS deliberately keeps generic and identifier-free — so a loser learns the winner's ARN from AWS's own contract, which real AWS does not reveal here.

Fix: the duplicate-Name branch of the CreateServiceLevelObjective handler / the SLO store's Name-uniqueness check — the path that loses the atomic insert must word this operation's own conflict sentence rather than delegating to the crate's shared ARN-echoing already-exists message helper. The same helper is the root of the sequential case in the related issue above, so one correction in that branch closes both entries; note that the sequential and concurrent paths must produce the identical fixed sentence.

Traces: application-signals.concurrent-create-slo-same-name-exactly-one-wins

## Evidence
Check application-signals.concurrent-create-slo-same-name-exactly-one-wins, [mismatch], assertion at check_application_signals_concurrent_create_slo_same_name_exactly_one_wins.py line 52:

  expected every loser's message to be 'A resource with the same identifier already exists'
  got (all 7 losers, identical):
    "Slo with arn 'arn:aws:application-signals:us-west-2:000000000000:slo/sim2real-slo-race-<suffix>' already exists."

Assertions that PASSED immediately before, and which therefore bound the defect to the message alone:
  - line 215: exactly one winner out of 8 concurrent CreateServiceLevelObjective calls on the same Name (atomic uniqueness is correct)
  - line 216: exactly 7 losers
  - line 219: every loser's Error.Code == "ConflictException" (code and 409 classification are correct)

Divergence quoted exactly:
  status/code: 409 ConflictException — CORRECT, not part of this defect
  message expected: `A resource with the same identifier already exists`  (entirely fixed text; no trailing period; no variable parts)
  message actual:   `Slo with arn '<arn>' already exists.`  (fixed text `Slo with arn '` ... `' already exists.` with the winner's computed ARN interpolated as the variable part, plus a trailing period AWS does not send)

The region, account id `000000000000` and the `sim2real-slo-race-<suffix>` name in the actual message are per-run values of the recording, not part of the requirement; the requirement is that no ARN appears in the message at all.

Model citation: CreateServiceLevelObjective POST /slo; ConflictException errorKind client, httpError 409, required member Message, doc "This operation attempted to create a resource that already exists."

## Steps to Reproduce
Boot the emulator and point a botocore `application-signals` client at it.

1. Pick a fresh SLO Name N matching ^[0-9A-Za-z][-._0-9A-Za-z ]{0,126}[0-9A-Za-z]$.
2. From a thread pool of 8 workers, submit 8 concurrent CreateServiceLevelObjective calls, each with Name=N and the same valid RequestBasedSliConfig (RequestBasedSliMetricConfig with a TotalRequestCountMetric and a MonitoredRequestCountMetric.BadCountMetric, Period 300, Stat Sum), plus any Tags.
3. Collect the results. Exactly 1 call succeeds and 7 raise ClientError — both correct.
4. Inspect each loser's response["Error"]["Code"]: all are "ConflictException" — correct.
5. Inspect each loser's response["Error"]["Message"]: each is "Slo with arn '<the winner's ARN>' already exists." instead of the expected "A resource with the same identifier already exists" (no trailing period, no ARN).
6. For the same root cause on the sequential path: CreateServiceLevelObjective(Name=N, RequestBasedSliConfig=...), then call it again with the same Name=N; the single 409 ConflictException carries the same wrong ARN-echoing message.

Cleanup: DeleteServiceLevelObjective(Id=<winner's Arn>).
