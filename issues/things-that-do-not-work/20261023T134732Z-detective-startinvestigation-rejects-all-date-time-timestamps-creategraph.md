Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: detective.start-investigation-nonexistent-graph-not-found-rejected detective.start-investigation-malformed-entity-arn-validation-rejected detective.start-investigation-unknown-entity-not-found detective.start-investigation-invalid-time-range-rejected detective.investigation-lifecycle-state-transitions detective.create-graph-idempotent-returns-same-arn detective.create-graph-concurrent-idempotent-same-arn detective.create-members-verification-fails-for-nonexistent-account detective.create-members-batch-processes-all-accounts-independently
Issue-Shape-Id: com.amazonaws.detective#AmazonDetective
Issue-Key: detective.start-investigation-nonexistent-graph-not-found-rejected|run#8|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: detective.start-investigation-malformed-entity-arn-validation-rejected|run#7|AssertionError|assert message == (
Issue-Key: detective|StartInvestigation|ValidationException
Issue-Key: detective.create-graph-idempotent-returns-same-arn|run#4|AssertionError|assert arn1 == arn2, f"CreateGraph is not idempotent: {arn1!r} != {arn2!r}"
Issue-Key: detective.create-graph-concurrent-idempotent-same-arn|run#5|AssertionError|assert len(unique_arns) == 1, f"expected every concurrent CreateGraph to agree on one ARN, got {unique_arns!r}"
Issue-Key: detective.create-members-verification-fails-for-nonexistent-account|run#11|AssertionError|assert final == "VERIFICATION_FAILED", f"expected VERIFICATION_FAILED, got {final!r}"
Issue-Key: detective.create-members-batch-processes-all-accounts-independently|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: a59989a9f63c403b66485a4e93f54ab6df22c2a6

# Detective: StartInvestigation rejects all date-time timestamps; CreateGraph is not account-singleton idempotent; member verification never settles

> _Note: this issue was identified against the code base at commit `a59989a9f63c403b66485a4e93f54ab6df22c2a6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartInvestigation

## Description
Three independent root causes in the emulator's Detective service (services/service-com-amazonaws-detective-amazondetective), each grounded in failing behavioral checks. A fixer must close ALL THREE before submitting.

=== Cause 1 of 3: StartInvestigation deserializes @timestampFormat("date-time") members as epoch integers, so every call is rejected ===
AWS: Smithy model com-amazonaws-detective-amazondetective.json, StartInvestigation (POST /investigations/startInvestigation): ScopeStartTime and ScopeEndTime both target Timestamp, are @required, and carry timestampFormat "date-time"; the protocol is REST-JSON, so these members arrive on the wire as ISO-8601 strings (e.g. "2024-05-01T12:00:00Z"), which is exactly what the AWS SDKs send for a datetime argument. A request with well-formed date-time scope members must be judged on its merits: success (HTTP 200 with a 21-digit InvestigationId per the output's @length 21/@pattern ^[0-9]+$), or one of the modeled errors — ResourceNotFoundException (404) for a graph that does not exist, ValidationException (400) for a malformed EntityArn or an invalid time range. Recorded AWS messages seen in the suite, as examples only: "Graph Not Found. Please check the graphArn provided".
Emulator: EVERY StartInvestigation call, regardless of the rest of the request, fails with ValidationException and message "invalid timestamp: failed to parse int" — the input deserializer parses the date-time members as epoch-second integers and errors on the string form before any graph/entity/range logic runs. Five checks fail on this one defect, three of them while merely setting up or exercising a different assertion.
Why: a client cannot start an investigation at all, and every negative path collapses into one wrong 400: a nonexistent graph reports ValidationException instead of ResourceNotFoundException (404), and a malformed EntityArn reports the timestamp message instead of the ARN validation error. Error-code-driven retry/branching logic is wrong, and the whole investigation surface is unreachable.
Fix: the StartInvestigation input deserializer (ops/start_investigation.rs and the shared timestamp decoding used for @timestampFormat("date-time") members) must accept the ISO-8601 string form; graph existence and input validation must then be evaluated in the handler.
Traces: detective.start-investigation-nonexistent-graph-not-found-rejected, detective.start-investigation-malformed-entity-arn-validation-rejected, detective.start-investigation-unknown-entity-not-found, detective.start-investigation-invalid-time-range-rejected, detective.investigation-lifecycle-state-transitions.

=== Cause 2 of 3: CreateGraph mints a new graph on every call instead of returning the account's single behavior graph ===
AWS: recorded behavior only (the model states CreateGraph takes only Tags, returns GraphArn, and declares ConflictException 409 and ServiceQuotaExceededException 402, but does not state the singleton rule). The recordings show that an account administers at most one behavior graph per Region, and that repeated CreateGraph by that account — both sequentially and from 8 concurrent callers — returns the SAME GraphArn rather than provisioning additional graphs.
Emulator: each CreateGraph allocates a fresh graph and returns a fresh ARN. Two sequential calls returned two different ARNs, and 8 concurrent calls returned 8 distinct ARNs.
Why: clients and tooling that call CreateGraph to obtain "this account's graph" silently accumulate graphs; every subsequent per-graph read/mutation addresses a different graph than the one another caller just created, so membership, tagging and datasource state appear to vanish. Concurrency-safe bootstrap code that relies on CreateGraph converging on one resource breaks.
Fix: CreateGraph handler plus the service's state (ops/create_graph.rs, state.rs) must key the behavior graph per account+region and return the existing graph's ARN on repeat calls, atomically so concurrent calls converge on one ARN.
Traces: detective.create-graph-idempotent-returns-same-arn, detective.create-graph-concurrent-idempotent-same-arn.

=== Cause 3 of 3: invited members never leave INVITED; verification never settles at VERIFICATION_FAILED ===
AWS: recorded behavior only (the model gives CreateMembers/GetMembers shapes and the MemberStatus values but not the transition timing). The recordings show CreateMembers with fabricated AccountId/EmailAddress pairs returns them all in Members with empty UnprocessedAccounts, and that each member then independently transitions to Status VERIFICATION_FAILED (observed via GetMembers), because the account/email pair cannot be verified — one member per account in the batch, none dropped.
Emulator: the member status stays INVITED indefinitely. One check polled GetMembers for 60s and still saw 'INVITED'; the batch check polled for all three fabricated accounts and timed out after the harness limit without any of them leaving INVITED.
Why: a client that waits on member verification (the documented way to learn an invitation can never be accepted) waits forever; code that distinguishes "pending" from "failed verification" can never make progress, and batch callers cannot tell which accounts were rejected.
Fix: the member state machine behind CreateMembers/GetMembers (ops/create_members.rs, ops/get_members.rs and the member records in state.rs) must advance a member whose account/email pair cannot be verified to VERIFICATION_FAILED, independently per account in a batch.
Traces: detective.create-members-verification-fails-for-nonexistent-account, detective.create-members-batch-processes-all-accounts-independently.

## Evidence
Cause 1 — detective.start-investigation-nonexistent-graph-not-found-rejected: expected code ResourceNotFoundException (404), got 'ValidationException'. detective.start-investigation-unknown-entity-not-found: "expected start_investigation to succeed for an unobserved-but-valid entity, got 'ValidationException': 'invalid timestamp: failed to parse int'". detective.start-investigation-malformed-entity-arn-validation-rejected: expected the EntityArn validation message, got 'invalid timestamp: failed to parse int'. detective.investigation-lifecycle-state-transitions and detective.start-investigation-invalid-time-range-rejected: botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartInvestigation operation: invalid timestamp: failed to parse int. Model: ScopeStartTime/ScopeEndTime -> Timestamp, required, timestampFormat "date-time".

Cause 2 — detective.create-graph-idempotent-returns-same-arn: "CreateGraph is not idempotent: '...graph:6332809a2ccd6097de01987327762509' != '...graph:0f8c59bda9543d4e5146895ed8211a79'". detective.create-graph-concurrent-idempotent-same-arn: "expected every concurrent CreateGraph to agree on one ARN", got a set of 8 distinct graph ARNs. Expected: one ARN (the account's single behavior graph) from every call.

Cause 3 — detective.create-members-verification-fails-for-nonexistent-account: "expected VERIFICATION_FAILED, got 'INVITED'". detective.create-members-batch-processes-all-accounts-independently: check timed out after the harness limit while polling GetMembers for all three fabricated accounts to reach VERIFICATION_FAILED.

## Steps to Reproduce
1. Cause 1: detective.create_graph(); then detective.start_investigation(GraphArn=<any well-formed graph arn>, EntityArn="arn:aws:iam::<acct>:role/x", ScopeStartTime=now-4h, ScopeEndTime=now) with any AWS SDK. Observe ValidationException "invalid timestamp: failed to parse int" instead of a 200 or a merits-based error; repeat with a never-created GraphArn and observe ValidationException instead of ResourceNotFoundException.
2. Cause 2: call detective.create_graph() twice in a row (and then 8 times concurrently) from the same account/region; compare the returned GraphArn values — they all differ instead of being identical.
3. Cause 3: detective.create_members(GraphArn=arn, Accounts=[{AccountId:'9xxxxxxxxxxx', EmailAddress:'x@example.com'}], DisableEmailNotification=True), then poll detective.get_members(GraphArn=arn, AccountIds=[...]) — Status stays INVITED and never becomes VERIFICATION_FAILED.
