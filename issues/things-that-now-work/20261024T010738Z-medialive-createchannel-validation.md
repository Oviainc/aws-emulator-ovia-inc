Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 047f25c8e675653717aa743423e962efe8d38250

# CreateChannel never returns the modeled UnprocessableEntityException: an empty/unusable channel configuration is created with 201

_Filed: 2026-10-24T01:07:38Z_
_Operation: CreateChannel | Class: validation_

> _Note: this issue was identified against the code base at commit `047f25c8e675653717aa743423e962efe8d38250`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CreateChannel models `UnprocessableEntityException` (HTTP 422) carrying `ValidationErrors` — "A collection of validation error responses" — because the real service validates the whole channel configuration before minting a channel and reports each problem with the element path it was found at. The emulator's handler builds no 422 at all.

Expected: a CreateChannel request that is not a usable channel configuration is refused 422 UnprocessableEntityException with one ValidationError entry per problem, and no channel is created. In particular (a) a request carrying no `EncoderSettings` (and so no output groups) and no `InputSpecification`/`Destinations`, and (b) a request whose `InputAttachments[i].InputId` names no input of the account, are refused.

Actual: the handler (src/ops/create_channel.rs:178-293) validates only `DryRun`, the attachment-list LENGTH and the store cap, then stores every supplied member verbatim and answers 201 with a channel. `POST /prod/channels {}` mints a channel with no name, no encoder settings, no input specification and no destinations, which then transitions to IDLE and can be STARTed. An `InputAttachments` entry naming a nonexistent input id is likewise stored, so the channel claims an attachment to an input that does not exist (and the input-side derivation `attached_channels` silently matches nothing), where the real service refuses the create.

## Evidence

Spec: smithy_query(operation="CreateChannel") lists `UnprocessableEntityException` (client, httpError 422) among its errors, and `UnprocessableEntityException$ValidationErrors` is a list of `ValidationError{ElementPath, ErrorMessage}` — the per-field channel-configuration refusal channel; `InputAttachment$InputId` is documented as "The ID of the input". Well-established MediaLive behavior: a channel must carry encoder settings with at least one output group plus matching destinations and input attachments, and create is rejected with 422 'channel contains errors' style ValidationErrors otherwise. Handler: src/ops/create_channel.rs — grep shows the only errors it can construct are BadRequestException (dryRun, attachment count), TooManyRequestsException (store cap) and InternalServerErrorException; the 422 variant is never built even though the emulator already knows how to emit ValidationErrors (ops/batch_update_schedule.rs:226-238 does it for the schedule). No test asserts a CreateChannel refusal.

## Steps to Reproduce

1. POST /prod/channels with an empty JSON body {} — expected 422 UnprocessableEntityException with ValidationErrors naming the missing configuration; the emulator answers 201 with a channel whose encoderSettings/inputSpecification/destinations are absent.
2. GET /prod/channels/<id> and wait for state IDLE, then POST /prod/channels/<id>/start — the emulator starts the configuration-less channel.
3. POST /prod/channels with inputAttachments=[{"inputAttachmentName":"a","inputId":"9999999"}] (an id no CreateInput ever minted) — expected 422/400 refusal; the emulator answers 201 and DescribeChannel reports the attachment to the nonexistent input.
