Issue-Severity: MEDIUM
Issue-Gap-Class: event-propagation
Issue-Commit: 4affd7a378830b3c6d8042007351e66db953803d

# CreateEnvironmentHost launches no EC2 bare metal instance, so ec2InstanceId and networkInterfaces are never reported

_Filed: 2026-10-24T00:04:35Z_
_Operation: CreateEnvironmentHost | Class: event-propagation_

> _Note: this issue was identified against the code base at commit `4affd7a378830b3c6d8042007351e66db953803d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

An Amazon EVS host IS an EC2 bare metal instance: 'An ESX host that runs on an Amazon EC2 bare metal instance' (`Host` doc), with `ec2InstanceId` ('The unique ID of the EC2 instance that represents the host') and `networkInterfaces` ('The elastic network interfaces that are attached to the host', max 2) reported on it, and the host is created from the caller's `keyName` and `instanceType` in the caller's VPC. Expected: once a host reaches CREATED it reports the id of a real EC2 instance in the account (visible to ec2 DescribeInstances) and the ENIs attached to it. Actual: the handler creates only an EVS-side record; no instance or ENI is created in the shared engine's EC2 store, and `StoredHost::to_wire` hard-codes `ec2_instance_id: None` and `network_interfaces: None`. Every host the emulator reports is missing both members for its whole lifetime, and a cross-service workflow (add host -> look the instance up in EC2 -> terminate/inspect it) has nothing to find.

## Evidence

Spec: `Host` shape doc and its `ec2InstanceId` / `networkInterfaces` members (smithy_query shape=Host); `instanceType` doc notes EC2 instances created through Amazon EVS. Handler: src/ops/create_environment_host.rs lines 298-320 (stores a StoredHost only; no EC2 interaction at all — unlike CreateEnvironment, which does reach into `service_com_amazonaws_ec2_amazonec2::state` via src/state/ec2_resources.rs); src/state.rs `StoredHost::to_wire` lines 661-676 set both members to None. Tests: tests/all/hosts.rs asserts hostName/ipAddress/keyName/instanceType/hostState only — no test references ec2InstanceId or networkInterfaces, so nothing pins the omission.

## Steps to Reproduce

1. CreateEnvironment and wait for CREATED.
2. CreateEnvironmentHost with a hostName, keyName and a bare metal instanceType.
3. Wait for the host to reach CREATED and call ListEnvironmentHosts — `ec2InstanceId` and `networkInterfaces` are absent on the Host.
4. Call ec2 DescribeInstances on the same engine — no instance was launched for the host.
