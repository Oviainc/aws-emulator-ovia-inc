Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d0704f2fe7a69fbeda61cd50377e188330130aec

# StartEngagement records the engagement but never engages the contact's channels (no Page state)

_Filed: 2026-10-24T15:16:48Z_
_Operation: StartEngagement | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d0704f2fe7a69fbeda61cd50377e188330130aec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartEngagement` is documented as "Starts an engagement to a contact or escalation plan. The engagement engages each contact specified in the incident." On the real service, starting an engagement against a contact produces one *page* per contact channel that the contact's engagement `Plan` targets (and, for an `ESCALATION` plan, it engages the contacts the stages name). A page is a first-class record of the service: the model defines `Page` as "Incident Manager engaging a contact's contact channel", and `ListPagesByEngagement` is "Lists the engagements to contact channels that occurred by engaging a contact", with `DescribePage` reporting its `SentTime`/`DeliveryTime`/`ReadTime` and `ListPageReceipts`/`ListPageResolutions` reporting what happened to it.

Expected: a successful `StartEngagement` writes, besides the engagement record, the page(s) the engaged contact's plan resolves to — one per `ChannelTargetInfo` the plan's stages target (resolved through the contact's stored contact channels), each carrying the engagement ARN, the contact ARN, the engaged contact channel, the sender/subject/content the engagement carried, and a sent time — and, for an escalation plan, the per-contact engagement/page records the stages produce.

Actual: the handler inserts exactly one `EngagementRecord` into `state.engagements` and nothing else. The service state (`src/state.rs`) has no page map at all, so engaging a contact has no observable effect on the contact's channels and the engagement half of the data plane is reduced to an opaque message log. `Plan` is never consulted by `StartEngagement`, so a plan with channel targets and a plan with no stages behave identically.

## Evidence

Spec: `StartEngagement` doc ("The engagement engages each contact specified in the incident"), `Page` doc ("Incident Manager engaging a contact's contact channel"), `ListPagesByEngagement` doc ("Lists the engagements to contact channels that occurred by engaging a contact"), and `DescribePage`'s output members (`PageArn`, `EngagementArn`, `ContactArn`, `Sender`, `Subject`, `Content`, `SentTime`, `DeliveryTime`, `ReadTime`) — all from the on-disk Smithy model. Handler: `services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/start_engagement.rs` lines ~251-271 build an `EngagementRecord` and `state.engagements.insert(...)`; `input` members are copied verbatim and the engaged contact's `Plan` is only used to find its alias for the ARN. State: `src/state.rs` declares `contacts`, `contact_channels`, `rotations`, `rotation_overrides`, `engagements` — there is no page/receipt structure, so nothing a page reader could ever resolve exists. Tests: `tests/all/start_engagement.rs` and `tests/all/engagement_lifecycle.rs` assert the engagement record round-trips through `DescribeEngagement`/`ListEngagements`; no test asserts anything about pages, so this is uncovered rather than locked in.

## Steps to Reproduce

1. Onboard the account (active replication set) and CreateContact with Alias="oncall-primary", Type="PERSONAL", Plan={"Stages":[{"DurationInMinutes":0,"Targets":[]}]}.
2. CreateContactChannel under that contact (Type="EMAIL", DeliveryAddress={"SimpleAddress":"ops@example.com"}) and activate it, then UpdateContact so the plan's stage targets that ChannelTargetInfo.
3. StartEngagement with ContactId=<contact ARN>, Sender="ops", Subject="test", Content="page me" — note the returned EngagementArn.
4. Expected: the engagement has engaged the contact channel, so one page exists for it (readable by ListPagesByEngagement/ListPagesByContact/DescribePage, with the engagement's subject/content and a SentTime).
5. Actual: only the engagement record exists; the emulator's state holds no page for the engagement, so no page can ever be reported for it, and the contact's channels are untouched.
