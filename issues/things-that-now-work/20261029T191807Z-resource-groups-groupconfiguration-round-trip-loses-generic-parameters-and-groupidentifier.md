Issue-Source: trace-sweep-divergence
Issue-Trace: resource-groups-error-config-query-conflict
Issue-Step: setup#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: resource-groups-error-config-query-conflict resource-groups-generic-group-configuration
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|CreateGroup|2xx->2xx|$.GroupConfiguration.GroupParameters
Issue-Key: resource-groups|GetGroupConfiguration|2xx->2xx|$.GroupConfiguration.GroupParameters
Issue-Key: resource-groups|GetGroupConfiguration|2xx->2xx|$.GroupIdentifier
Issue-Key: resource-groups|PutGroupConfiguration|2xx->2xx|$
Issue-Commit: 907b6e75f03592bc638357f3d5c791839011739f

# GroupConfiguration round-trip loses Generic Parameters and GroupIdentifier in responses

> _Note: this issue was identified against the code base at commit `907b6e75f03592bc638357f3d5c791839011739f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
This issue bundles 3 causes with one root: the emulator does not retain or serialize the full group service-configuration document. Parameters attached to a configuration item are dropped, and the configuration-returning operations omit members the recordings show AWS returning. All three must be closed before this issue is submitted.

Cause 1 of 3: CreateGroup drops GroupConfigurationItem.Parameters from the echoed GroupConfiguration.
AWS: Smithy: CreateGroup input member Configuration targets GroupConfigurationList (length 0..2); CreateGroupOutput member GroupConfiguration targets GroupConfiguration, whose Configuration member is a list of GroupConfigurationItem{Type (required, pattern ^AWS::[a-zA-Z0-9]+::[a-zA-Z0-9]+$), Parameters: list of GroupConfigurationParameter{Name (required, pattern ^[a-z-]+$, length 1..80), Values}}. Recorded response: HTTP 200 echoing every configuration item submitted, including the AWS::ResourceGroups::Generic item's Parameters entry (a parameter Name with its Values list) exactly as sent.
Emulator: HTTP 200 with the configuration item present but $.GroupConfiguration.GroupParameters absent — the parameters supplied on the request are not reflected back at all (replay reports it as missing/None).
Why: a client that creates a configuration group and reads back the applied parameters (for example to confirm an allowed-resource-types restriction took effect) sees no parameters, so it concludes the restriction was not applied, and any code that round-trips the create response into a later PutGroupConfiguration silently drops the parameters.
Fix: CreateGroup's handler/state write for group configuration, plus the CreateGroupOutput serializer for GroupConfiguration items' Parameters.
Traces: resource-groups-error-config-query-conflict#setup#2, resource-groups-generic-group-configuration#setup#1.

Cause 2 of 3: GetGroupConfiguration omits GroupIdentifier and the configuration item Parameters.
AWS: Smithy: GetGroupConfigurationOutput declares GroupConfiguration (recorded behavior only for the GroupIdentifier member: the on-disk model does not declare it, but the recording shows AWS answering 200 with a top-level GroupIdentifier{GroupName, GroupArn} alongside GroupConfiguration whose Generic item carries its Parameters). Recorded response: HTTP 200, body contains GroupIdentifier with the group's name and its arn:aws:resource-groups:<region>:<account>:group/<name>, and GroupConfiguration.Configuration listing both configuration items with the Generic item's parameters.
Emulator: HTTP 200 (status correct) but the body has no GroupIdentifier and no parameters on the Generic configuration item.
Why: a client reading a group's configuration cannot tell which group it got back (no name/ARN) and cannot see the parameters that constrain membership; both are the reason the call is made.
Fix: GetGroupConfiguration's response serializer (add the GroupIdentifier projection) and the state it reads from, which must retain configuration-item parameters.
Traces: resource-groups-generic-group-configuration#task#2, resource-groups-generic-group-configuration#task#4.

Cause 3 of 3: PutGroupConfiguration answers an empty body where AWS returns the applied configuration.
AWS: Smithy: PutGroupConfiguration http code 202, and the modelled PutGroupConfigurationOutput has no members (recorded behavior only for the body: the recording shows AWS answering 202 with a document containing GroupIdentifier{GroupName, GroupArn} and GroupConfiguration.Configuration listing the applied items, including the Generic item's Parameters). Hypothesis: the service returns the applied configuration document for this operation even though the checked-in model declares the output empty.
Emulator: HTTP 202 (status correct) with an entirely empty JSON body {}.
Why: a client that applies a configuration and uses the response to confirm what is now in effect gets nothing and must issue a second GetGroupConfiguration — which (cause 2) is also incomplete.
Fix: PutGroupConfiguration's response serializer, emitting the group identifier and the configuration now in effect from the state it just wrote.
Traces: resource-groups-generic-group-configuration#task#3.

Values such as the group name, the account id and the region in the recorded ARNs are per-run; the requirement is that the members are present and derived from the request and the emulator's own state.

## Evidence
Replay of simparity-wire-aws-v1 (all mismatches, status matched where noted):
- resource-groups-error-config-query-conflict#setup#2 CreateGroup, 200->200: $.GroupConfiguration.GroupParameters missing (expected [{'Name': 'allowed-resource-types', 'Values': ['AWS::EC2::CapacityReservation']}], got None); also affects resource-groups-generic-group-configuration#setup#1.
- resource-groups-generic-group-configuration#task#2 GetGroupConfiguration, 200->200: $.GroupIdentifier missing (expected {'GroupName': 'sim2real-probe-resource-groups-rg-config-7fc94640', 'GroupArn': 'arn:aws:resource-groups:us-west-2:000000000000:group/sim2real-probe-resource-groups-rg-config-7fc94640'}, got None) and $.GroupConfiguration.GroupParameters missing; also affects task#4.
- resource-groups-generic-group-configuration#task#3 PutGroupConfiguration, 202->202: $ empty_body (expected a document with GroupIdentifier and GroupConfiguration.Configuration = [{'Type': 'AWS::EC2::CapacityReservationPool'}, {'Type': 'AWS::ResourceGroups::Generic', 'Parameters': [...]}], got {}).

## Steps to Reproduce
1. CreateGroup with Configuration = [{Type: AWS::EC2::CapacityReservationPool}, {Type: AWS::ResourceGroups::Generic, Parameters: [{Name: allowed-resource-types, Values: [AWS::EC2::CapacityReservation]}]}] (fixture resource-groups-generic-group-configuration#setup#1 / resource-groups-error-config-query-conflict#setup#2). The 200 response must echo the Generic item's Parameters.
2. Call GetGroupConfiguration on that group (task#2, task#4): the 200 body must carry GroupIdentifier{GroupName, GroupArn} and the Generic item's Parameters.
3. Call PutGroupConfiguration on that group (task#3): the 202 response must carry the group identifier and the configuration now in effect, not an empty body.
