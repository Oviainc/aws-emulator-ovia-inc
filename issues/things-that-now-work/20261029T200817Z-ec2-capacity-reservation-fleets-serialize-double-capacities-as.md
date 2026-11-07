Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-capacity-reservation-fleet-cancel-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-capacity-reservation-fleet-cancel-lifecycle ec2-capacity-reservation-fleet-lifecycle ec2-describe-capacity-reservation-fleets-contract
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|CreateCapacityReservationFleet|2xx->2xx|$.CreateCapacityReservationFleetResponse.totalFulfilledCapacity
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->2xx|$.DescribeCapacityReservationFleetsResponse.capacityReservationFleetSet.item.instanceTypeSpecificationSet.item.fulfilledCapacity
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->2xx|$.DescribeCapacityReservationFleetsResponse.capacityReservationFleetSet.item.instanceTypeSpecificationSet.item.priority
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->2xx|$.DescribeCapacityReservationFleetsResponse.capacityReservationFleetSet.item.instanceTypeSpecificationSet.item.weight
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->2xx|$.DescribeCapacityReservationFleetsResponse.capacityReservationFleetSet.item.totalFulfilledCapacity
Issue-Key: ec2|CancelCapacityReservationFleets|2xx->2xx|$.CancelCapacityReservationFleetsResponse.successfulFleetCancellationSet.item.previousFleetState
Issue-Key: ec2|DescribeCapacityReservationFleets|2xx->2xx|$.DescribeCapacityReservationFleetsResponse.capacityReservationFleetSet.item.instanceTypeSpecificationSet
Issue-Commit: c42b5bef1562db44010b2579a8522ff39a3564f6

# EC2 capacity reservation fleets serialize double capacities as integers and drop fleet members on cancel

> _Note: this issue was identified against the code base at commit `c42b5bef1562db44010b2579a8522ff39a3564f6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCapacityReservationFleet

## Description
Five net-new causes in Capacity Reservation Fleet, sharing two root faults: double-typed capacity members are serialized as integers, and the fleet's member set and previous state are not tracked.

Cause 1 of 5 - CreateCapacityReservationFleet reports totalFulfilledCapacity with the wrong type and value (fixture ec2-capacity-reservation-fleet-cancel-lifecycle, step task#1). AWS: Smithy CreateCapacityReservationFleet output member TotalFulfilledCapacity -> Double, xmlName totalFulfilledCapacity, so the ec2Query XML carries a decimal; on the create response nothing is fulfilled yet, so the recording shows 0.0. Emulator: emits '1'. Why: a client parsing a double sees a different value AND a non-decimal lexical form. Fix: the CreateCapacityReservationFleet response serializer plus the initial fulfilled-capacity value in state.

Cause 2 of 5 - DescribeCapacityReservationFleets serializes fulfilledCapacity, weight and totalFulfilledCapacity as integers and omits priority (ec2-capacity-reservation-fleet-cancel-lifecycle#task#2). AWS: Smithy FleetCapacityReservation.FulfilledCapacity -> Double, Weight -> DoubleWithConstraints (@range min 0.001 max 99.999), Priority -> IntegerWithConstraints (@range min 0); AWS returns '1.0' for the doubles and the fleet's default priority. Emulator: '1' for each double and no priority element at all. Fix: the DescribeCapacityReservationFleets projection/serializer and the stored instance-type specification.

Cause 3 of 5 - CancelCapacityReservationFleets reports the wrong previousFleetState (ec2-capacity-reservation-fleet-cancel-lifecycle#task#3). AWS: successfulFleetCancellationSet.item.previousFleetState is the state the fleet held immediately before the cancel - submitted in this flow. Emulator: 'active'. Why: a client reconciling fleet history records a transition that never happened. Fix: the cancel handler must read the prior state from the fleet record before mutating it.

Cause 4 of 5 - after cancelling, DescribeCapacityReservationFleets loses the fleet's instance-type specifications and zeroes capacity (ec2-capacity-reservation-fleet-cancel-lifecycle#task#5). AWS: a cancelled fleet still reports its instanceTypeSpecificationSet (availabilityZone, capacityReservationId, instanceType, instancePlatform, fulfilledCapacity, priority, totalInstanceCount, weight, ebsOptimized, createDate) and the fulfilled capacity it had. Emulator: instanceTypeSpecificationSet serialized as empty and totalFulfilledCapacity '0'. Why: the fleet's composition becomes unreadable after cancel, so audit and teardown flows cannot verify what was reserved. Fix: cancel must mark the fleet, not clear its membership, in the fleet state.

Cause 5 of 5 - the same double-as-integer serialization at larger capacity (ec2-capacity-reservation-fleet-lifecycle#task#2): fulfilledCapacity/weight/totalFulfilledCapacity expected '4.0'/'1.0'/'4.0', emulator '4'/'1'/'4'. This cluster also covers task#4 of that fixture.

Fix summary: the ec2Query serializer's handling of Double members for the fleet shapes, and the fleet lifecycle state (previous state, member retention) under services/service-com-amazonaws-ec2-amazonec2/src/ops/.

Traces: representative step plus the four fixture#step bindings in trace_ids.

## Evidence
Replay detail, verbatim (all 200 expected / 200 returned):
1. CreateCapacityReservationFleet: $.CreateCapacityReservationFleetResponse.totalFulfilledCapacity expected '0.0', got '1'.
2. DescribeCapacityReservationFleets: instanceTypeSpecificationSet.item.fulfilledCapacity expected '1.0' got '1'; .priority missing (expected '9999'); .weight expected '1.0' got '1'; totalFulfilledCapacity expected '1.0' got '1'.
3. CancelCapacityReservationFleets: successfulFleetCancellationSet.item.previousFleetState expected 'submitted', got 'active' (cluster also covers ec2-capacity-reservation-fleet-lifecycle#task#6 and ec2-describe-capacity-reservation-fleets-contract#task#5).
4. DescribeCapacityReservationFleets: instanceTypeSpecificationSet type_mismatch (expected the item structure with availabilityZone/capacityReservationId/createDate/ebsOptimized/fulfilledCapacity '1.0'/instancePlatform/instanceType/priority '9999'/totalInstanceCount '1'/weight '1.0', got ''); totalFulfilledCapacity expected '1.0' got '0' (cluster also covers ec2-describe-capacity-reservation-fleets-contract#task#6).
5. DescribeCapacityReservationFleets: fulfilledCapacity expected '4.0' got '4'; weight expected '1.0' got '1'; totalFulfilledCapacity expected '4.0' got '4'.
All five clusters: verdict=mismatch, stub_path empty, owned_by empty.

## Steps to Reproduce
1. Clone, boot the emulator built from the checkout.
2. replay_wire_corpus on simparity-wire-aws-v1.
3. Replay ec2-capacity-reservation-fleet-cancel-lifecycle step task#1 and each fixture#step in trace_ids: create a fleet, describe it, cancel it, describe again; compare totalFulfilledCapacity, instanceTypeSpecificationSet (fulfilledCapacity/weight/priority) and previousFleetState with the recording.
