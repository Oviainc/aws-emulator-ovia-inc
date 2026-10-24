Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: a85c524bdc735756e5abfd799d7dcf6de3bb06dc

# CreateCampaign mints a second campaign for a name the Connect instance already uses; ConflictException is unreachable

_Filed: 2026-10-24T07:51:25Z_
_Operation: CreateCampaign | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `a85c524bdc735756e5abfd799d7dcf6de3bb06dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateCampaign` stores a record for every call, keyed only by the server-minted sequence id: two calls with the same `name` and the same `connectInstanceId` both answer HTTP 200 with two different ids/ARNs, and `ListCampaigns` then reports two campaigns with identical names under one instance. The handler has no code path that ever constructs `ConflictException`, so the modeled 409 for this operation is unreachable.

AWS does not end up with two campaigns of the same name in one Connect instance: the campaign name is the human identifier a Connect instance's campaigns are unique under, the operation is marked `@idempotent` and its own documentation says "This API is idempotent" even though it takes no client token — so the repeat is resolved against what already exists rather than minting a second resource. Expected for the second create: either the modeled `ConflictException` (HTTP 409) or the already-existing campaign's id/arn — exactly one campaign exists afterwards. Actual: two campaigns exist with the same name under the same instance.

## Evidence

Spec: the model declares `ConflictException` (HTTP 409, "The request could not be processed because of conflict in the current state of the resource") for `CreateCampaign` and marks the operation `@idempotent`, with the doc "Creates a campaign for the specified Amazon Connect account. This API is idempotent." (see OPERATIONS.md / smithy_query CreateCampaign). With no client-token member in the input, the only thing a repeated create can be idempotent against is the (connectInstanceId, name) pair.

Behavior corpus: this service's suite contains the check `connectcampaignsv2.concurrent-create-campaign-same-name-one-winner` (listed in Issue-Trace-Ids of 20261023T134440Z-connectcampaignsv2-behavioral-suite-blocked-and-diverging-...), i.e. two same-name creates recorded against AWS leave exactly ONE winner — the emulator produces two winners. That check aborted in setup (Connect SearchAvailablePhoneNumbers 501), so the divergence has not been reported by any existing issue.

Handler: src/ops/create_campaign.rs:188-261 — after the onboarding gate it calls `store.next_campaign_id()` and `insert_new_campaign(...)` unconditionally; the only refusals are `ServiceQuotaExceededException` (store full) and `InternalServerException` (minted id taken). src/state.rs `insert_new_campaign` keys the store on the minted id only; there is no name index.

Test coverage: tests/all/campaign_lifecycle.rs::each_create_mints_a_distinct_id_and_keeps_its_own_record ASSERTS THE DIVERGENT BEHAVIOR — it submits the same body ({"name":"same-name",...}) twice and requires distinct ids and ARNs. That test encodes the gap rather than covering it, and would need updating with the fix.

## Steps to Reproduce

1. Boot the emulator and onboard instance-1 (PUT /v2/connect-instance/instance-1/onboarding, wait for SUCCEEDED).
2. PUT /v2/campaigns with {"name":"same-name","connectInstanceId":"instance-1"} -> HTTP 200, note id A.
3. PUT /v2/campaigns with the IDENTICAL body -> observe HTTP 200 and a different id B; expected either HTTP 409 ConflictException or id A returned again.
4. POST /v2/campaigns-summary with {} -> observe two summaries named "same-name" under connectInstanceId instance-1; expected one.
