Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: aa34a7732176a121f53f6fa0a60cefb8de4c89bc

# CreateContactChannel never returns the modeled ConflictException for a duplicate channel on a contact

_Filed: 2026-10-24T21:29:27Z_
_Operation: CreateContactChannel | Class: error-response_

> _Note: this issue was identified against the code base at commit `aa34a7732176a121f53f6fa0a60cefb8de4c89bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateContactChannel declares `ConflictException` (HTTP 409) in the model, but no code path in the handler can produce it: the only refusals it can answer are the account-gate/empty-address/enum ValidationException, the ResourceNotFoundException for an unknown ContactId, and a store-ceiling ValidationException. Creating a second contact channel with the SAME Name under the SAME contact therefore succeeds and mints a second, independent ContactChannelArn, leaving the contact with two channels of the same name that ListContactChannels reports side by side.

Expected: a contact channel name is the channel's identifier within its contact, so a second create under a live channel name is refused with ConflictException (HTTP 409) naming the existing channel — the same way the sibling CreateContact refuses a duplicate Alias with ConflictException. Actual: HTTP 200 and a duplicate channel.

## Evidence

Spec: `CreateContactChannel` errors per the model/OPERATIONS.md — `AccessDeniedException, ConflictException (HTTP 409), DataEncryptionException, InternalServerException, ThrottlingException, ValidationException`; the only inconsistent-state condition this create can reach is a channel that already exists under the contact. Well-established behavior: Incident Manager identifies a channel within a contact by Name (the ARN is `contact-channel/<contact alias>/<id>`), and a repeated create is reported as a conflict, exactly as the emulator already does for CreateContact's Alias (`src/ops/create_contact.rs` lines 217-228).

Handler: `src/ops/create_contact_channel.rs` lines 183-301 — no uniqueness check at all; `state.contact_channels.insert(arn, record)` is keyed by a freshly minted ARN derived from `channels_created`, so each call always inserts.

Tests: `tests/all/contact_lifecycle.rs` asserts ConflictException only for duplicate CreateContact aliases (lines 423, 629); no test creates two channels with the same name under one contact.

## Steps to Reproduce

1. Onboard the account and CreateContact {Alias: "oncall-primary", Type: "PERSONAL", Plan: {Stages: []}}.
2. CreateContactChannel {ContactId: <arn>, Name: "email", Type: "EMAIL", DeliveryAddress: {SimpleAddress: "ops@example.com"}} → 200.
3. Repeat the identical CreateContactChannel call (no IdempotencyToken).
4. Observe HTTP 200 with a second, different ContactChannelArn instead of a 409 ConflictException; ListContactChannels for the contact then reports two channels named "email".
