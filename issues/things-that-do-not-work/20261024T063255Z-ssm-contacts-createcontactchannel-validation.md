Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# CreateContactChannel accepts a DeliveryAddress whose format does not match the channel Type

_Filed: 2026-10-24T06:32:55Z_
_Operation: CreateContactChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`DeliveryAddress.SimpleAddress` is documented in the model as format-bound to the channel `Type`: "SMS - '+' followed by the country code and phone number / VOICE - '+' followed by the country code and phone number / EMAIL - any standard email format". Real AWS refuses a mismatched address with the modeled `ValidationException` (HTTP 400) and creates nothing — e.g. `Type: "EMAIL"` with `SimpleAddress: "+15550100"`, `Type: "SMS"` with `SimpleAddress: "ops@example.com"`, or `Type: "VOICE"` with `SimpleAddress: "not-a-number"`.

Expected: HTTP 400 `ValidationException` naming `deliveryAddress.simpleAddress`, and no channel minted.

Actual: the handler's only check on the address is that it is present and non-empty; every other string is stored verbatim, the call answers 200 with a `ContactChannelArn`, and `GetContactChannel`/`ListContactChannels` then report an EMAIL channel whose address is a phone number (and vice versa). The emulator also delivers an activation code to that nonsensical address, so the channel can be activated.

## Evidence

Spec: `ContactChannelAddress$SimpleAddress` documentation in the Smithy model (`smithy_query(query="doc", shape="ContactChannelAddress", member="SimpleAddress")`) states the per-`ChannelType` expected formats; `ValidationException` (client, httpError 400) is modeled on `CreateContactChannel`, which models no other way to refuse a malformed member.

Handler: `services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/create_contact_channel.rs` — the `delivery_address` match only rejects `None`/empty (`"Value at 'deliveryAddress.simpleAddress' failed to satisfy constraint: Member must not be empty"`); `input.r#type` is checked against `CHANNEL_TYPE_VALUES` but is never correlated with the address.

Tests: no coverage. `tests/all/contact_lifecycle.rs` `CHANNEL_CATALOG` only ever pairs matching type/address (`EMAIL`/`ops@example.com`, `SMS`/`+15550100`, `VOICE`/`+15550101`), and `members_outside_their_modelled_constraint_are_refused` only exercises a `DeliveryAddress` naming no `SimpleAddress` at all. Nothing asserts the mismatched-format case either way.

## Steps to Reproduce

1. Boot the emulator with an onboarded account (an ACTIVE replication set in the ssm-contacts store) and create a contact with CreateContact.
2. Call CreateContactChannel with ContactId=<contact arn>, Name="email", Type="EMAIL", DeliveryAddress={"SimpleAddress": "+15550100"}.
3. Observe HTTP 200 with a ContactChannelArn; expected HTTP 400 ValidationException (a phone number is not a standard email format).
4. Repeat with Type="SMS" and DeliveryAddress={"SimpleAddress": "ops@example.com"} — again 200 instead of 400.
5. Call GetContactChannel on the returned ARN and observe the mismatched address stored and reported.
