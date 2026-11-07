Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time|run#24|AssertionError|assert message == (
Issue-Commit: 2d6e0284801305ae3aa8ad9d3ca49cea550d31a4

# config: PutDeliveryChannel's out-of-enum deliveryFrequency ValidationException lists the allowed enum value set in the wrong order, so the refusal message is not AWS's

> _Note: this issue was identified against the code base at commit `2d6e0284801305ae3aa8ad9d3ca49cea550d31a4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliveryChannel

## Description
ONE root cause. ONE CAUSE must be closed.

Cause 1 of 1: the enum-constraint ValidationException message for deliveryChannel.configSnapshotDeliveryProperties.deliveryFrequency renders the allowed value set in Smithy declaration order instead of the order AWS Config emits, so the message text diverges.

AWS: recorded behavior only for the ordering (the model constrains the value set, not its rendering). PutDeliveryChannel with configSnapshotDeliveryProperties.deliveryFrequency set to a value outside the enum is refused with HTTP 400, error code ValidationException and the message
  "1 validation error detected: Value '<value>' at 'deliveryChannel.configSnapshotDeliveryProperties.deliveryFrequency' failed to satisfy constraint: Member must satisfy enum value set: [TwentyFour_Hours, Twelve_Hours, Three_Hours, Six_Hours, One_Hour]"
Fixed text: "1 validation error detected: Value ", " at ", " failed to satisfy constraint: Member must satisfy enum value set: " and the bracketed set "[TwentyFour_Hours, Twelve_Hours, Three_Hours, Six_Hours, One_Hour]" — the set and its order are fixed for this member, not per-request. Variable parts: the quoted offending value, taken verbatim from the request, and the dotted member path of the member that failed. Smithy (com-amazonaws-configservice-starlingdoveservice.json): ConfigSnapshotDeliveryProperties.deliveryFrequency targets enum MaximumExecutionFrequency with values One_Hour, Three_Hours, Six_Hours, Twelve_Hours, TwentyFour_Hours — the same five values, in declaration order, which is NOT the order AWS prints.

Emulator: the status (400) and the error code (ValidationException) are right, and the sentence frame and member path are right, but the bracketed set is emitted in model declaration order:
  "1 validation error detected: Value 'One_Minute' at 'deliveryChannel.configSnapshotDeliveryProperties.deliveryFrequency' failed to satisfy constraint: Member must satisfy enum value set: [One_Hour, Three_Hours, Six_Hours, Twelve_Hours, TwentyFour_Hours]"
against AWS's "...: [TwentyFour_Hours, Twelve_Hours, Three_Hours, Six_Hours, One_Hour]". ('One_Minute' is this run's request value and fills the template; it is not part of the requirement.)

Why: a client or test that asserts on, parses, or snapshots Config's validation message — the common pattern for surfacing "which values are allowed" back to a user or for golden-file comparisons in CI — gets a string that never matches what AWS returns, so a refusal that is semantically correct still fails verification and callers cannot rely on the message as a stable contract.

Fix: the enum-constraint message renderer used by this service's request-validation stage for PutDeliveryChannel input (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_delivery_channel.rs and the shared constraint-validation path it is driven through) must emit the allowed-value list for MaximumExecutionFrequency in AWS's order rather than the model's declaration order.

Related open issues, named for overlap and NOT relied on as cover (neither carries this check id): 20261030T093351Z-config-6-net-new-behavioral-root-causes-putconfigrule.md (other ValidationException renderings) and 20261026T044854Z-config-3-net-new-behavioral-root-causes-associateresourcetypes.md (AssociateResourceTypes' ValidationException wording).

Traces: config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator serving, bootstrap_signal false; 117/177 checks passed).

[mismatch] config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time
  File "check_config_delivery_channel_snapshot_frequency_bounds_the_next_delivery_time.py", line 622, in run
    assert message == (
  AssertionError: unexpected message: "1 validation error detected: Value 'One_Minute' at 'deliveryChannel.configSnapshotDeliveryProperties.deliveryFrequency' failed to satisfy constraint: Member must satisfy enum value set: [One_Hour, Three_Hours, Six_Hours, Twelve_Hours, TwentyFour_Hours]"

The replayed check asserts, after an established channel at deliveryFrequency=One_Hour:
    code, message = _error(lambda: config.put_delivery_channel(DeliveryChannel={
        "name": channel, "s3BucketName": bucket,
        "configSnapshotDeliveryProperties": {"deliveryFrequency": "One_Minute"}}))
    assert code == "ValidationException"          # PASSES on the emulator
    assert message == ("1 validation error detected: Value 'One_Minute' at "
        "'deliveryChannel.configSnapshotDeliveryProperties.deliveryFrequency' failed to satisfy "
        "constraint: Member must satisfy enum value set: "
        "[TwentyFour_Hours, Twelve_Hours, Three_Hours, Six_Hours, One_Hour]")   # FAILS

Expected vs actual, the only difference being the order of the bracketed set:
  expected: ... enum value set: [TwentyFour_Hours, Twelve_Hours, Three_Hours, Six_Hours, One_Hour]
  actual:   ... enum value set: [One_Hour, Three_Hours, Six_Hours, Twelve_Hours, TwentyFour_Hours]

Model grounding (smithy_query shape=MaximumExecutionFrequency, services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): values ["One_Hour","Three_Hours","Six_Hours","Twelve_Hours","TwentyFour_Hours"] — exactly the order the emulator prints, confirming the message is rendered straight from declaration order while AWS's recorded order is the reverse-ish set above.

The rest of the check (the stored frequency surviving the refusal, the in-place frequency update, the One_Hour / TwentyFour_Hours nextDeliveryTime horizons) is not implicated by this assertion.

## Steps to Reproduce
1. Clone the repo, build and boot the emulator.
2. Replay simparity-behavioral-aws-v1 and re-replay config.delivery-channel-snapshot-frequency-bounds-the-next-delivery-time.
3. Manually: create a delivery bucket with the Config delivery bucket policy, put a configuration recorder and a delivery channel at that bucket with configSnapshotDeliveryProperties.deliveryFrequency=One_Hour.
4. config put-delivery-channel --delivery-channel 'name=<channel>,s3BucketName=<bucket>,configSnapshotDeliveryProperties={deliveryFrequency=One_Minute}'.
5. Observe the 400 ValidationException message: the allowed-value set is printed as [One_Hour, Three_Hours, Six_Hours, Twelve_Hours, TwentyFour_Hours]; AWS prints [TwentyFour_Hours, Twelve_Hours, Three_Hours, Six_Hours, One_Hour] with the rest of the sentence identical.
