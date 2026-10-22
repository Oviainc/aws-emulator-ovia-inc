Issue-Severity: CRITICAL
Issue-Gap-Class: stub
Issue-Commit: 01973b2fa0798bae2c2d50e81b185c07c4a43584

# Bootstrap the EC2 emulator

_Filed: 2026-10-21T21:57:57Z_
_Operation: service-com-amazonaws-ec2-amazonec2 | Class: stub_

> _Note: this issue was identified against the code base at commit `01973b2fa0798bae2c2d50e81b185c07c4a43584`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every handler under `services/service-com-amazonaws-ec2-amazonec2/src/ops/` is an unmodified scaffold: it carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` header and returns `Err(crate::scaffolded::Scaffolded::not_implemented(..))` => HTTP 501, and the crate holds no state. Implement a FIRST SMALL SLICE: the EC2 instance lifecycle, statefully, so a realistic client script works end to end.

### Happy path to implement (in order)

1. **RunInstances** - input: `MinCount` (required), `MaxCount` (required), `ImageId`, `InstanceType`, `KeyName`, `SubnetId`, `SecurityGroupIds`, `UserData`, `TagSpecifications`, `ClientToken`. Must MINT `MaxCount`-many (at least `MinCount`) new instances, each with a fresh unique `i-` id, store them in a reservation with a fresh `r-` `ReservationId`, and return output `ReservationId`, `OwnerId`, `Instances` (list<Instance>) where each `Instance` reports the id, the requested `ImageId`/`InstanceType`/`KeyName`/`SubnetId`, a `State` of `pending` (code 0), a `PrivateIpAddress`, a `LaunchTime` and the tags from `TagSpecifications`. No canned instance id, no echoed-only body.
2. **DescribeInstances** - input: `InstanceIds`, `Filters`, `MaxResults`, `NextToken`; output: `Reservations` (list<Reservation>), `NextToken`. Must return the reservations/instances actually created by RunInstances, filtered by `InstanceIds` when given (unknown id => `InvalidInstanceID.NotFound`), and support at minimum the `instance-state-name`, `instance-id` and `tag:<key>` filters plus `MaxResults`/`NextToken` paging over the stored set. On the first describe after launch, an instance may transition `pending` -> `running`.
3. **CreateTags** - input: `Resources` (required), `Tags` (required); no output members. Must attach the tags to the stored instance records so a subsequent DescribeInstances shows them (and `tag:` filters match). Unknown resource id => `InvalidID`/`InvalidInstanceID.NotFound`.
4. **StopInstances** - input: `InstanceIds` (required); output: `StoppingInstances` (list<InstanceStateChange>). Must mutate stored state to `stopping`/`stopped` and report `PreviousState` = the state actually held before the call and `CurrentState` = the new one.
5. **StartInstances** - input: `InstanceIds` (required); output: `StartingInstances` (list<InstanceStateChange>). Only valid from `stopped`; sets state to `pending`/`running` with truthful `PreviousState`/`CurrentState`.
6. **TerminateInstances** - input: `InstanceIds` (required); output: `TerminatingInstances` (list<InstanceStateChange>). Must set stored state to `shutting-down`/`terminated`; after this, DescribeInstances still reports the instance as `terminated` (it is not removed immediately), and Start/Stop on it must fail.

Every one of these must compute its response from persisted state held in the shared engine reached via `crate::state::service_state(&*self.engine)` - no hardcoded ids, no echoed-input-only responses, no random id without storage. The `DryRun` flag, when true, must not mutate state (respond with the modeled dry-run error path rather than performing the action).

## State Model

In `src/state.rs` (`ComAmazonawsEc2Amazonec2State`):

- `reservations: HashMap<String /* r-xxxx ReservationId */, Reservation>` where `Reservation { reservation_id, owner_id, requester_id, group_ids: Vec<String>, instance_ids: Vec<String> }`
- `instances: HashMap<String /* i-xxxx InstanceId */, Instance>` where `Instance { instance_id, reservation_id, image_id, instance_type, key_name: Option<String>, subnet_id: Option<String>, vpc_id: Option<String>, security_group_ids: Vec<String>, private_ip: String, public_ip: Option<String>, state: InstanceStateName /* pending|running|stopping|stopped|shutting-down|terminated */, state_transition_reason: Option<String>, launch_time: SystemTime, user_data: Option<String>, placement_az: String, monitoring_state: String, architecture: String, root_device_type: String, tags: Vec<(String, String)>, disable_api_termination: bool, client_token: Option<String> }`
- `tags: ` either the per-instance `tags` field above or a shared `HashMap<String /* resource id */, BTreeMap<String, String>>` used by CreateTags/DescribeTags
- `id_counters` for minting monotonic unique `i-`/`r-` ids, and `client_tokens: HashMap<String, String /* ReservationId */>` so a repeated RunInstances with the same `ClientToken` returns the SAME reservation instead of launching again

Operations outside this slice (VPC/subnet/volume/snapshot/security-group/key-pair/launch-template and the ~700 remaining ops) stay 501 and are tracked as individual gap issues later.

## Evidence

`services/service-com-amazonaws-ec2-amazonec2/OPERATIONS.md` defines the contracts cited above: RunInstances (line 4880: `MinCount`/`MaxCount` required; output `ReservationId`, `OwnerId`, `Instances`), DescribeInstances (line 2164: @paginated inputToken=NextToken/outputToken=NextToken/items=Reservations; output `Reservations`, `NextToken`), CreateTags (line 866: `Resources` and `Tags` required, no output members), StartInstances (line 4925: output `StartingInstances` list<InstanceStateChange>), StopInstances (line 4949: output `StoppingInstances`), TerminateInstances (line 4961: output `TerminatingInstances`). Proof of the all-stub state: `src/ops/run_instances.rs` line 4 `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 NotImplemented` and line 329 `Err(crate::scaffolded::Scaffolded::not_implemented("RunInstances"))`; the same marker is present in every file under `src/ops/`. Instance state names and the PreviousState/CurrentState semantics of InstanceStateChange are well-established EC2 behavior.

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/CLI EC2 client at it.
2. `aws ec2 run-instances --image-id ami-12345678 --instance-type t3.micro --min-count 1 --max-count 2 --tag-specifications 'ResourceType=instance,Tags=[{Key=Name,Value=demo}]'` -> expect 200 with a fresh ReservationId and 2 Instances, each with a unique i- id and State=pending. Today: HTTP 501 NotImplemented.
3. `aws ec2 describe-instances` -> expect the reservation and both instances just launched, with the Name tag present and State running/pending.
4. `aws ec2 describe-instances --filters Name=tag:Name,Values=demo` -> expect only the tagged instances.
5. `aws ec2 create-tags --resources <i-id> --tags Key=env,Value=test`, then `aws ec2 describe-instances --instance-ids <i-id>` -> expect both Name and env tags returned from stored state.
6. `aws ec2 stop-instances --instance-ids <i-id>` -> expect StoppingInstances with PreviousState=running, CurrentState=stopping; a following describe-instances shows stopped.
7. `aws ec2 start-instances --instance-ids <i-id>` -> expect StartingInstances with PreviousState=stopped, CurrentState=pending; describe shows running again.
8. `aws ec2 terminate-instances --instance-ids <i-id>` -> expect TerminatingInstances with PreviousState=running, CurrentState=shutting-down; a following describe-instances reports the instance as terminated.
9. `aws ec2 describe-instances --instance-ids i-00000000000000000` -> expect error InvalidInstanceID.NotFound, not an empty success.
