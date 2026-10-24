Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7cd33b8d616838a49131331fab4e1704473d18dc

# Stopping an engagement leaves its not-yet-sent pages live and acceptable

_Filed: 2026-10-24T19:49:18Z_
_Operation: StopEngagement | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7cd33b8d616838a49131331fab4e1704473d18dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

StartEngagement mints EVERY page of the plan up front, stamping each with a future `SentTime` computed from the stages ahead of it (src/ops/start_engagement.rs:288-320). StopEngagement only stamps `stop_time`/`stop_reason` on the engagement record (src/ops/stop_engagement.rs:194-200) and touches no page.

Expected: a stop prevents the stages that have not been reached from engaging anyone — 'Stops an engagement before it finishes the final stage of the escalation plan or engagement plan. Further contacts aren't engaged.' After a stop, pages for stages the engagement never reached should not be reportable as sent engagements of a contact channel (and a stopped engagement's outstanding pages should not go on accepting acknowledgements). Actual: every page of the whole plan already exists the instant the engagement starts and survives the stop unchanged, so DescribePage reports a page with a `SentTime` in the future for an engagement that was stopped, and AcceptPage happily records a DELIVERED/READ receipt and stamps `DeliveryTime`/`ReadTime` on a page for a later stage that was never engaged.

## Evidence

Model/docs: StopEngagement — 'Stops an engagement before it finishes the final stage of the escalation plan or engagement plan. Further contacts aren't engaged.'; `Page.SentTime` is 'the time that Incident Manager engaged the contact channel'. Handler: src/ops/stop_engagement.rs:183-202 mutates only the engagement record; src/ops/start_engagement.rs:304-319 pre-creates pages with `sent_time = start + stage offset`. Test coverage: tests/all/engagement_lifecycle.rs covers the stop instant/reason and leaves pages entirely untested (no occurrence of 'Page' in that file), and tests/all/page_acknowledgement.rs never calls StopEngagement — so no test pins the current behavior.

## Steps to Reproduce

1. Onboard the account, create a PERSONAL contact with three contact channels and a plan whose stages have non-zero DurationInMinutes (e.g. 5, 10, 0).
2. StartEngagement against that contact and note the page ARNs minted immediately (the later stages carry SentTime values minutes in the future).
3. StopEngagement for that engagement.
4. DescribeEngagement shows StopTime set, but DescribePage for a later-stage page still reports it with a future SentTime.
5. AcceptPage that later-stage page with its accept code: it is accepted (200) and stamps DeliveryTime/ReadTime, although that stage was never engaged and the engagement was stopped.
