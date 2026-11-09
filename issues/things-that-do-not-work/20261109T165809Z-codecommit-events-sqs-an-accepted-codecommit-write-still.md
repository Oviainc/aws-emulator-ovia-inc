Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event
Issue-Key: codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event|_poll_until#16|AssertionError|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 0cee419aadfb4868f9ccd3aa23bc5045ac08a29b

# codecommit__events__sqs: an accepted CodeCommit write still publishes no 'CodeCommit Repository State Change' event, so the EventBridge→SQS positive control never arrives

> _Note: this issue was identified against the code base at commit `0cee419aadfb4868f9ccd3aa23bc5045ac08a29b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutFile

## Description
One net-new cause, re-grounded on a check that now runs to its assertion instead of timing out.

Cause 1 of 1: a CodeCommit write that moves a reference emits no 'CodeCommit Repository State Change' event to the default EventBridge bus.
AWS: recorded behavior only (CodeCommit eventing is not in the Smithy model). An accepted write that moves a ref — here PutFile on branch 'main', both the branch-creating commit and a later fast-forward commit — publishes an event with source 'aws.codecommit', detail-type 'CodeCommit Repository State Change' and resources [<repository Arn>] to the account's default bus with no opt-in; detail carries event (referenceCreated / referenceUpdated), referenceType 'branch', referenceName as the BARE branch name with the full ref in referenceFullName, and commitId equal to the commitId PutFile returned. A rule matching that source/detail-type with an SQS target therefore receives the envelope as the raw message body within seconds. A write REFUSED for a stale parentCommitId (ParentCommitIdOutdatedException) moves no reference and must publish nothing — that withholding is the property under test, and it is only meaningful against a working positive control.
Emulator: nothing is ever delivered. The check long-polls the SQS target of a rule on {'source': ['aws.codecommit'], 'detail-type': ['CodeCommit Repository State Change']} for 240 s after an accepted PutFile and collects zero records, so the positive control fails: 'the branch-creating commit <commitId> produced a "CodeCommit Repository State Change" event on <queue url> — the positive control for the CodeCommit -> EventBridge -> SQS wiring not met within 240s. Last result: []'. Hypothesis: the codecommit write path publishes no EventBridge events at all.
Why: every event-driven CodeCommit integration is dead — a build triggered by a push, a mirroring consumer, an audit pipeline. Worse for correctness testing, a consumer cannot distinguish 'no event because the write was refused' (the real contract) from 'this service never emits events', so the emulator looks identical whether it enforces parent-commit freshness or not.
Fix: event publication on reference-moving writes in the codecommit service's write path (src/ops/put_file.rs, delete_file.rs, create_commit.rs and the shared ref-update routine), emitted onto the EventBridge default bus only after the write is accepted, with the detail fields above.
Traces: codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event.

Note: open issue 20261022T171422Z-codecommit-events-sqs-codecommit-createbranch-is-a-501-stub-and-no.md describes this same publication gap as its cause 2, but it no longer carries this check's replay key (it was filed when the check timed out at 60 s; the check now reaches the 240 s positive-control assertion), so the cause is unexercised by its replay pointer and is filed here with the current key. The other codecommit__events__sqs failure in this run (pull-request-lifecycle-reaches-eventbridge-scoped-to-one-repository) is owned by 20261026T234121Z-codecommit-2-net-new-behavioral-root-causes-the.md and is not re-filed.

## Evidence
Check codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event (mismatch): 'AssertionError: the branch-creating commit a143949351dda86a37592a2a82d8b55e53e30359 produced a "CodeCommit Repository State Change" event on https://sqs.us-west-2.amazonaws.com/000000000000/sim2real-nd-cc-c682f7d7eb — the positive control for the CodeCommit -> EventBridge -> SQS wiring not met within 240s. Last result: []'. Expected: at least one SQS message whose body is an EventBridge envelope with source 'aws.codecommit', detail-type 'CodeCommit Repository State Change', resources [<repository Arn>] and detail.commitId equal to the commitId PutFile returned. Actual: zero records after 240 s of long polling (the commit id and queue name are per-run values). The later legs — the withheld event for the ParentCommitIdOutdatedException refusal and the released event for the same content committed against the current tip — are never reached.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cc.create_repository(repositoryName=R); create an SQS queue Q.
3. events.put_rule with pattern {'source': ['aws.codecommit'], 'detail-type': ['CodeCommit Repository State Change'], 'resources': [<R's Arn>]} on the default bus and events.put_targets with Q as the target (queue policy allowing events.amazonaws.com).
4. cc.put_file(repositoryName=R, branchName='main', filePath='seed.txt', fileContent=b'seed\\n', commitMessage='seed') and long-poll Q — expect within seconds a message whose body carries detail.event 'referenceCreated', detail.referenceName 'main', detail.referenceFullName 'refs/heads/main' and detail.commitId equal to PutFile's commitId; observe nothing for the whole budget.
5. Repeat with a fast-forward PutFile (parentCommitId = current tip) for 'referenceUpdated'; then with a stale parentCommitId — the refused call must publish nothing while the accepted one does.
6. Re-replay codecommit__events__sqs.a-commit-refused-for-a-stale-parent-moves-no-reference-and-emits-no-state-change-event.
