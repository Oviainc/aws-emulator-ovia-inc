Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# UpdateContact replaces Plan without resolving the ARNs it names

_Filed: 2026-10-24T06:32:55Z_
_Operation: UpdateContact | Class: validation_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`UpdateContact` is the call a client uses to attach an engagement plan AFTER the contact's channels exist (channels can only be created once the contact exists), so it is the main path on which plan references matter. The handler assigns `contact.plan = plan.clone()` with no validation.

Real AWS resolves the plan it is given and refuses one it cannot: a `ChannelTargetInfo.ContactChannelId` that names no channel (or a channel belonging to another contact), a `ContactTargetInfo` whose `ContactId` names no contact or is absent entirely, a `RotationIds` entry that names no rotation, or a `Target` carrying neither target info — each is refused with the modeled `ValidationException` (HTTP 400) and the stored plan is left alone.

Expected: HTTP 400 `ValidationException`, the contact's previous plan unchanged.

Actual: HTTP 200 and the dangling plan is persisted; `GetContact` reports it, and a subsequent `StartEngagement` engages nothing. A request that names neither `DisplayName` nor `Plan` is likewise accepted as a 200 no-op.

## Evidence

Spec: `UpdateContact` input is `ContactId` (required), `DisplayName`, `Plan`; `ValidationException` (client, 400) is modeled. `ChannelTargetInfo.ContactChannelId` is `@required` within the target and is an `SsmContactsArn`; the service's own recorded refusal wording for an unresolvable ARN is "Invalid value provided - Invalid resource Arn <arn>" (see the existing replication-set issues' recorded `DescribeEngagement` message), i.e. AWS does resolve request ARNs.

Handler: `services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/update_contact.rs` — after the account gate and the contact lookup, the only logic is `if let Some(plan) = &input.plan { contact.plan = plan.clone(); }`; no member of the plan is resolved against `state.contacts`, `state.contact_channels` or `state.rotations`.

Tests: `tests/all/contact_updates.rs` (`an_update_replaces_the_members_it_names_and_leaves_the_rest`, `updates_accumulate_and_the_last_write_wins`, `members_outside_their_modelled_constraint_are_refused`) only exercises well-formed plans and boundary constraints; nothing asserts a plan whose references do not resolve.

## Steps to Reproduce

1. Boot the emulator with an onboarded account; CreateContact (PERSONAL) and note the ContactArn.
2. Call UpdateContact with ContactId=<arn> and Plan={"Stages":[{"DurationInMinutes":5,"Targets":[{"ChannelTargetInfo":{"ContactChannelId":"arn:aws:ssm-contacts:us-east-1:000000000000:contact-channel/other/00000000-0000-0000-0000-000000000000","RetryIntervalInMinutes":1}}]}]} — observe HTTP 200; expected HTTP 400 ValidationException because the channel does not exist.
3. Call GetContact and observe the dangling plan stored.
4. Call UpdateContact with Plan={"RotationIds":["arn:aws:ssm-contacts:us-east-1:000000000000:rotation/never-created"]} — observe 200; expected 400.
5. Call UpdateContact with only ContactId (no DisplayName, no Plan) — observe a 200 no-op.
