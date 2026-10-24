Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# GetContactChannel skips the account-level replication-set gate, answering 404 where AWS answers 400 ValidationException

_Filed: 2026-10-24T06:32:55Z_
_Operation: GetContactChannel | Class: error-response_

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every other resource-addressed read in this crate consults the account-level Incident Manager precondition before resolving its ARN (`GetRotation`, `DescribeEngagement`, `GetContactPolicy`, `ListTagsForResource`, `ListRotations` all call `require_active_replication_set*`). `GetContactChannel` does not: it goes straight to `state.contact_channels`.

On an account with no ACTIVE replication set — the normal state of an emulated account, and the state the recorded AWS traces for this service were captured in — AWS refuses a well-formed `ContactChannelId` with HTTP 400 `ValidationException` (message example "Invalid value provided - Account not found for the request") BEFORE the channel is looked up. The emulator instead reports the channel missing.

Expected: HTTP 400, error code `ValidationException`, nothing resolved.

Actual: HTTP 404, error code `ResourceNotFoundException`. A client branches on this: `ValidationException` means "this account is not onboarded", `ResourceNotFoundException` means "the account is fine, create the channel" — against the emulator the client takes the create branch.

`ListContactChannels` (`src/ops/list_contact_channels.rs`) has the same hole: it resolves `ContactId` with no gate and answers 404 on a non-onboarded account; both handlers should consult the same gate the rest of the data plane does.

## Evidence

Grounding: the same precondition is trace-recorded for this service's sibling reads — see the open issue `20261022T233012Z-ssm-contacts-replication-set-account-precondition-never-consulted` (trace `ssm-contacts.get-contact-without-replication-set-rejected`: GetContact without a replication set answers HTTP 400 `ValidationException`, explicitly NOT `ResourceNotFoundException`) and the fixed issue `20261022T233012Z-ssm-contacts-10-data-plane-operations-are-unimplemented` (ten operations, including the read-only `ListRotations`, `GetContactPolicy` and `DescribeEngagement`, recorded as 400 `ValidationException`). `GetContactChannel` models `ValidationException` (client, httpError 400) as well as `ResourceNotFoundException` (404), so the 400 refusal class is available. Those issues name GetContact/DeleteContact/CreateContact/CreateContactChannel; GetContactChannel and ListContactChannels are not covered by any existing issue.

Handler: `services/service-com-amazonaws-ssmcontacts-ssmcontacts/src/ops/get_contact_channel.rs` — `handle` locks the state and immediately does `state.contact_channels.get(&input.contact_channel_id)`; there is no `require_active_replication_set` call, unlike `get_rotation.rs` (which calls `require_active_replication_set_for(&input.rotation_id)`).

Tests: `tests/all/data_plane_precondition.rs` `GATED_OPERATIONS` lists ten operations and deliberately omits GetContactChannel/ListContactChannels; `tests/all/contact_lifecycle.rs` `an_arn_naming_no_stored_resource_answers_404` asserts the 404 only on an ONBOARDED account, so no test covers the non-onboarded case for these two operations.

## Steps to Reproduce

1. Boot the emulator fresh (no replication set installed in the ssm-contacts store — the default state).
2. Control: list_contacts(AliasPrefix=<unique>) returns {"Contacts": []} (precondition-exempt), confirming the service is mounted.
3. Call GetContactChannel with a well-formed, never-created ContactChannelId, e.g. arn:aws:ssm-contacts:us-east-1:000000000000:contact-channel/never-created/00000000-0000-0000-0000-000000000000.
4. Observe HTTP 404 with code ResourceNotFoundException; expected HTTP 400 with code ValidationException (account-level replication-set precondition, message naming the account or the supplied ARN).
5. Call ListContactChannels with a well-formed, never-created ContactId on the same fresh emulator — observe 404 ResourceNotFoundException; expected the same 400 ValidationException.
