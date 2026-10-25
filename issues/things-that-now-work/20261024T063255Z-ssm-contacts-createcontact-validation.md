Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# CreateContact never resolves the ARNs inside Plan, so a contact can be created with an engagement plan that points at nothing

_Filed: 2026-10-24T06:32:55Z_
_Operation: CreateContact | Class: validation_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Plan` is stored verbatim: the handler copies `input.plan` into the record without resolving a single reference inside it. Real AWS resolves every ARN a request carries (the recorded refusal wording for this service is "Invalid value provided - Invalid resource Arn <arn>") and refuses a plan it cannot resolve with the modeled `ValidationException` (HTTP 400) — note `CreateContact` models NO `ResourceNotFoundException`, so 400 `ValidationException` is the only refusal class available to it.

Cases AWS refuses and the emulator admits with 200:
- `Plan.RotationIds` naming a rotation ARN that does not exist (typical for `Type: ONCALL_SCHEDULE`).
- `Plan.Stages[].Targets[].ContactTargetInfo` whose `ContactId` names no stored contact, or which carries no `ContactId` at all (only the required `IsEssential`), so the target names nothing to engage.
- `Plan.Stages[].Targets[].ChannelTargetInfo.ContactChannelId` naming no stored contact channel (or a channel belonging to a different contact).
- A `Target` carrying neither `ChannelTargetInfo` nor `ContactTargetInfo`.

Expected: HTTP 400 `ValidationException`, nothing persisted, and the alias still free.

Actual: HTTP 200 with a `ContactArn`; `GetContact` reports the dangling plan back, and a later `StartEngagement` on that contact silently engages nothing.

## Evidence

Spec: `Plan` = `{Stages: list<Stage>, RotationIds: list<SsmContactsArn>}`; `Stage.Targets` is `@required`; `Target` = `{ChannelTargetInfo, ContactTargetInfo}`; `ChannelTargetInfo.ContactChannelId` is `@required` and `ContactTargetInfo.ContactId` is an `SsmContactsArn` (`smithy_query(query="shape", shape="Plan,Stage,Target,ChannelTargetInfo,ContactTargetInfo")`). `CreateContact`'s modeled errors contain `ValidationException` (400) but no 404, so an unresolvable reference must be a 400. The sibling `CreateRotation` handler in this same crate DOES resolve its `ContactIds` against the store, so the inconsistency is internal as well as against AWS.

Handler: `services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_contact.rs` — only `check_enum("type", ...)`, the alias-uniqueness check and the store ceiling run; `plan: input.plan.clone()` is written with no reference resolution.

Tests: no test asserts the refusal; `tests/all/contact_lifecycle.rs` `CONTACT_CATALOG` deliberately encodes the loose behaviour (a `ContactTargetInfo` with only `IsEssential`, and a `RotationIds` entry the comment describes as "stores it verbatim and resolves no rotation"), and `core/router/tests/all/ssm_contacts_replication_set.rs` asserts 200 for an ONCALL_SCHEDULE plan naming a never-created rotation — those expectations encode the gap and would need updating with the fix.

## Steps to Reproduce

1. Boot the emulator with an onboarded account (ACTIVE replication set in the ssm-contacts store).
2. Call CreateContact with Alias="weekend-rotation", Type="ONCALL_SCHEDULE", Plan={"RotationIds": ["arn:aws:ssm-contacts:us-east-1:000000000000:rotation/never-created"]} — observe HTTP 200 with a ContactArn; expected HTTP 400 ValidationException naming the unresolvable rotation ARN.
3. Call CreateContact with Alias="escalation-tier-1", Type="ESCALATION", Plan={"Stages":[{"DurationInMinutes":20,"Targets":[{"ContactTargetInfo":{"IsEssential":true}}]}]} — observe 200; expected 400 (the target names no contact).
4. Call CreateContact with a PERSONAL contact whose plan stage targets {"ChannelTargetInfo":{"ContactChannelId":"arn:aws:ssm-contacts:us-east-1:000000000000:contact-channel/never/00000000-0000-0000-0000-000000000000","RetryIntervalInMinutes":1}} — observe 200; expected 400.
5. GetContact on any of the returned ARNs and observe the dangling plan reported back verbatim.
