Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a

# Return shipping label is created for a job whose device has never shipped (JobState New)

_Filed: 2026-10-24T15:57:00Z_
_Operation: CreateReturnShippingLabel | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `7b95de6c4fc8fc0be292d7fb35e4aa81be419d0a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`CreateReturnShippingLabel` only refuses a job whose state is `Cancelled`, `WithAWSSortingFacility`, `WithAWS`, `InProgress` or `Complete` (`JobState::has_returnable_device`). Every other state — including `New`, `PreparingAppliance` and `PreparingShipment`, where no device has been handed to the customer and (per this emulator's own `JobMetadata` rendering) no `SnowballId` is even associated with the job — is treated as having a returnable device.

Expected: a return shipping label "will be used to return the Snow device to Amazon Web Services", so it presupposes a device on the customer's side of the journey. For a job that was just created (`JobState=New`) AWS refuses the call with the declared `InvalidJobStateException` ("the job's current state doesn't allow that action to be performed") and records no label.

Actual: the call answers 200 with `Status=InProgress`, persists a label record, and a follow-up `DescribeReturnShippingLabel` then reports `Succeeded` plus an `ExpirationDate` and a `ReturnShippingLabelURI` for a device that was never shipped. A second create is additionally refused with `ReturnShippingLabelAlreadyExistsException`, so the fabricated label also blocks the legitimate call later in the job's life.

## Evidence

Spec/model: `CreateReturnShippingLabel` — "Creates a shipping label that will be used to return the Snow device to Amazon Web Services" — declares `InvalidJobStateException` (HTTP 400, "The action can't be performed because the job's current state doesn't allow that action to be performed"). The same crate already encodes that a `New` job has no device: src/state.rs `to_job_metadata` (~lines 494-535) leaves `SnowballId` absent because a device is associated "once an appliance has been prepared for it, i.e. once it has left `New`" — yet `JobState::has_returnable_device` (src/state.rs ~line 331) returns true for `New`. Handler: src/ops/create_return_shipping_label.rs (~lines 214-273) gates only on `has_returnable_device` and then calls `store.start_return_shipping_label`. Tests: tests/all/return_shipping_labels.rs asserts the refusals for a main export job and a cancelled job (`a_job_with_no_device_to_return_has_no_return_shipping_label`) and asserts success for a freshly created IMPORT job, i.e. the current (divergent) behavior is what is pinned — nothing asserts the pre-shipment refusal.

## Steps to Reproduce

1. CreateAddress with a deliverable address; keep the `AddressId`.
2. CreateJob with `{"JobType": "IMPORT", "SnowballType": "EDGE", "AddressId": "<id>", "ShippingOption": "STANDARD", "Resources": {"S3Resources": [{"BucketArn": "arn:aws:s3:::some-bucket"}]}}`; keep the `JobId`.
3. DescribeJob: `JobMetadata.JobState` is `New` and no `SnowballId` is reported (no device associated).
4. CreateReturnShippingLabel with that `JobId`.
5. Observe 200 with `Status=InProgress` instead of a 400 `InvalidJobStateException`; after ~1s, DescribeReturnShippingLabel reports `Succeeded` with an `ExpirationDate` and a label URI.
