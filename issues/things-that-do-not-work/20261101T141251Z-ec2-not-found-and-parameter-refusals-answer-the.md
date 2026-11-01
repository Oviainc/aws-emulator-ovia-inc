Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-allocate-hosts-invalid-zone-error
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-allocate-hosts-invalid-zone-error ec2-coip-pool-mistaken-usage ec2-hardware-and-byoip-not-found-error-catalog ec2-interruptible-capacity-reservation-allocation ec2-managed-prefix-list-entries-and-associations ec2-network-acl-create-configure-entries-delete ec2-network-acl-lifecycle
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|AllocateHosts|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteCoipCidr|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|DeleteCoipCidr|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|UpdateInterruptibleCapacityReservationAllocation|4xx->4xx|$.Response.Errors.Error.Code
Issue-Key: ec2|UpdateInterruptibleCapacityReservationAllocation|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|GetManagedPrefixListEntries|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|DeleteNetworkAclEntry|4xx->4xx|$.Response.Errors.Error.Message
Issue-Key: ec2|CreateNetworkAclEntry|4xx->4xx|$.Response.Errors.Error.Message
Issue-Related: 20261029T200817Z-ec2-parameter-limit-refusals-report-emulator-authored-limits.md
Issue-Commit: 030c70f3b5611e115edea7c769d1aba1b20ff8ad

# EC2 not-found and parameter refusals answer the wrong code and reworded text

> _Note: this issue was identified against the code base at commit `030c70f3b5611e115edea7c769d1aba1b20ff8ad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AllocateHosts

## Description
Seven net-new refusal families in the ec2 handler answer an error code and/or an error message that is not the one AWS returns. The ec2Query protocol model carries no modeled error shapes for any of these operations (smithy_query query=operation on AllocateHosts, DeleteCoipCidr, GetManagedPrefixListEntries, CreateNetworkAclEntry returns "errors": []), so the requirement for code and text is the recorded AWS response; what is quoted below is each error's fixed text plus the variable parts that come from the request or from emulator state. All seven are HTTP 400 on both sides — only the Response.Errors.Error.Code and .Message differ. Related open issues cover other members of the same families but do NOT carry these replay keys: 20261029T200817Z-ec2-error-paths-choose-the-wrong-error-class.md, 20261029T200817Z-ec2-id-validation-verdict-is-inverted-malformed-ids.md, 20261031T181451Z-ec2-refusals-answer-an-emulator-authored-error-code.md.

Cause 1 of 7: AllocateHosts with an AvailabilityZone that does not exist in the region.
AWS: recorded behavior only — HTTP 400, Code 'InvalidRequest', Message "Invalid value '<AvailabilityZone>' for AvailabilityZone." The fixed text is "Invalid value '" ... "' for AvailabilityZone."; the only variable is the AvailabilityZone value echoed from the request, and the member name is spelled with AWS's public parameter casing (AvailabilityZone), not the wire member name.
Emulator: Code 'InvalidParameterValue', Message "Invalid value '<AvailabilityZone>' for availabilityZone. Region '<region>' has no such Availability Zone" — wrong error code, lower-cased member name, and an extra emulator-authored clause naming the region.
Why: a client that branches on InvalidRequest (or matches the documented sentence) never sees the AZ rejection as such; it sees a generic parameter error, so AZ-fallback logic does not fire.
Fix: the AllocateHosts handler's availability-zone validation (error classification plus message template).
Traces: ec2-allocate-hosts-invalid-zone-error#task#1.

Cause 2 of 7: DeleteCoipCidr naming a customer-owned pool id that does not exist.
AWS: recorded behavior only — HTTP 400, Code 'InvalidPoolID.NotFound'.
Emulator: Code 'InvalidParameterValue'.
Why: the pool-missing condition is indistinguishable from a bad-argument condition, so retry/create-then-delete flows cannot tell a deleted pool from a malformed request.
Fix: the DeleteCoipCidr handler's pool lookup — a missing pool must raise the NotFound class, not the parameter class.
Traces: ec2-coip-pool-mistaken-usage#task#4.

Cause 3 of 7: DeleteCoipCidr given a syntactically wrong coip-pool id.
AWS: recorded behavior only — HTTP 400, Code 'InvalidIpv4PoolCoipId.Malformed', Message "The coip-pool ID '<CoipPoolId>' is malformed". Fixed text: "The coip-pool ID '" ... "' is malformed"; the variable is the id echoed from the request. Model context: DeleteCoipCidr.CoipPoolId targets Ipv4PoolCoipId, which is why AWS's code is keyed to that shape.
Emulator: Code 'InvalidPoolID.Malformed', Message "Invalid id: \"<CoipPoolId>\". A customer-owned address pool id is 'ipv4pool-coip-' followed by 8 or 17 hexadecimal digits" — both the code and the whole sentence are emulator-authored.
Why: callers keyed on InvalidIpv4PoolCoipId.Malformed (and log scrapers keyed on the sentence) do not match, and the code the emulator emits collides with the pool-not-found family.
Fix: the DeleteCoipCidr handler's id-shape validation (error code selection plus message template).
Traces: ec2-hardware-and-byoip-not-found-error-catalog#task#3.

Cause 4 of 7: UpdateInterruptibleCapacityReservationAllocation naming a well-formed capacity-reservation id that does not exist.
AWS: recorded behavior only — HTTP 400, Code 'InvalidCapacityReservationId.NotFound', Message "The capacity-reservation ID '<CapacityReservationId>' does not exist". Fixed text: "The capacity-reservation ID '" ... "' does not exist"; the variable is the id from the request.
Emulator: Code 'InvalidCapacityReservationId.Malformed', Message "Capacity Reservation ID '<id>' is malformed" — the verdict is inverted: a well-formed unknown id is reported as a syntax error.
Why: a caller cannot distinguish "you sent garbage" from "the reservation is gone", so it retries a request that can never succeed instead of re-creating the reservation.
Fix: the UpdateInterruptibleCapacityReservationAllocation handler's id validation/lookup ordering — shape check first, then existence, each with its own class.
Traces: ec2-interruptible-capacity-reservation-allocation#task#5.

Cause 5 of 7: GetManagedPrefixListEntries for a prefix list id that does not exist.
AWS: recorded behavior only — HTTP 400, Message "The prefix list (<PrefixListId>) does not exist." Fixed text: "The prefix list (" ... ") does not exist." — parenthesised id, trailing full stop; the variable is the PrefixListId from the request.
Emulator: Message "The prefix list ID '<PrefixListId>' does not exist" — reworded (quoted id instead of parenthesised, no full stop).
Why: message-matching clients and human operators comparing emulator output with AWS output see a different sentence for the same condition.
Fix: the GetManagedPrefixListEntries handler's not-found message template.
Traces: ec2-managed-prefix-list-entries-and-associations#task#4.

Cause 6 of 7: DeleteNetworkAclEntry for a rule number that is not present in the ACL's chosen direction.
AWS: recorded behavior only — HTTP 400, Message "no ingress entry with number <RuleNumber> in network ACL <NetworkAclId>". Fixed text: "no " <direction, 'ingress' or 'egress', derived from the request's Egress flag> " entry with number " <RuleNumber from the request> " in network ACL " <NetworkAclId from the request>. AWS names both the direction and the ACL.
Emulator: Message "The network acl entry identified by <RuleNumber> does not exist." — emulator-authored prose that drops the direction and the ACL id.
Why: the caller loses the two facts it needs to recover (which ACL, which direction), and the sentence does not match AWS.
Fix: the DeleteNetworkAclEntry handler's entry-lookup error message.
Traces: ec2-network-acl-create-configure-entries-delete#task#10.

Cause 7 of 7: CreateNetworkAclEntry for a port-based protocol without PortRange.
AWS: recorded behavior only — HTTP 400, Message "The request must contain the parameter portRange.from". AWS names the specific missing leaf member (portRange.from); the whole sentence is fixed apart from that member path.
Emulator: Message "The request must contain the parameter portRange for protocol 17" — names the parent member and appends an emulator-authored protocol number.
Why: a client correcting the request from the message is told to supply 'portRange' rather than the leaf AWS actually requires, and the protocol-number suffix is not part of AWS's text. Model context: CreateNetworkAclEntry.PortRange targets PortRange (members From/To) and is NOT marked required in the model, so the requirement is exactly this recorded conditional refusal.
Fix: the CreateNetworkAclEntry handler's PortRange precondition message.
Traces: ec2-network-acl-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (285 fixtures, 2159 steps, score 53.3%). All seven clusters are verdict=mismatch, expected_status 400, actual_status 400, service ec2, stub_path empty, owned_by empty.

1. ec2-allocate-hosts-invalid-zone-error#task#1 AllocateHosts — $.Response.Errors.Error.Code: expected 'InvalidRequest', got 'InvalidParameterValue'; $.Response.Errors.Error.Message: expected "Invalid value 'not-a-real-zone-1z' for AvailabilityZone.", got "Invalid value 'not-a-real-zone-1z' for availabilityZone. Region 'us-west-2' has no such Availability Zone".
2. ec2-coip-pool-mistaken-usage#task#4 DeleteCoipCidr — Code: expected 'InvalidPoolID.NotFound', got 'InvalidParameterValue'.
3. ec2-hardware-and-byoip-not-found-error-catalog#task#3 DeleteCoipCidr — Code: expected 'InvalidIpv4PoolCoipId.Malformed', got 'InvalidPoolID.Malformed'; Message: expected "The coip-pool ID 'coip-pool-00000000000000000' is malformed", got 'Invalid id: "coip-pool-00000000000000000". A customer-owned address pool id is \'ipv4pool-coip-\' followed by 8 or 17 hexadecimal digits'.
4. ec2-interruptible-capacity-reservation-allocation#task#5 UpdateInterruptibleCapacityReservationAllocation — Code: expected 'InvalidCapacityReservationId.NotFound', got 'InvalidCapacityReservationId.Malformed'; Message: expected "The capacity-reservation ID 'cr-00000000000000000' does not exist", got "Capacity Reservation ID 'cr-00000000000000000' is malformed".
5. ec2-managed-prefix-list-entries-and-associations#task#4 GetManagedPrefixListEntries — Message: expected 'The prefix list (pl-00000000000000000) does not exist.', got "The prefix list ID 'pl-00000000000000000' does not exist".
6. ec2-network-acl-create-configure-entries-delete#task#10 DeleteNetworkAclEntry — Message: expected 'no ingress entry with number 100 in network ACL acl-0000000000000002b', got 'The network acl entry identified by 100 does not exist.'.
7. ec2-network-acl-lifecycle#task#6 CreateNetworkAclEntry — Message: expected 'The request must contain the parameter portRange.from', got 'The request must contain the parameter portRange for protocol 17'.

(The recorded ids, zone name, rule number and protocol number above are this run's values filling the template, not requirements.)

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay ec2-allocate-hosts-invalid-zone-error step task#1 — AllocateHosts with an AvailabilityZone absent from the region; compare Response.Errors.Error.Code/.Message.
3. Cause 2: replay ec2-coip-pool-mistaken-usage step task#4 — DeleteCoipCidr against a nonexistent coip pool.
4. Cause 3: replay ec2-hardware-and-byoip-not-found-error-catalog step task#3 — DeleteCoipCidr with a CoipPoolId that is not of the ipv4pool-coip- form.
5. Cause 4: replay ec2-interruptible-capacity-reservation-allocation step task#5 — UpdateInterruptibleCapacityReservationAllocation with a well-formed but unknown cr- id.
6. Cause 5: replay ec2-managed-prefix-list-entries-and-associations step task#4 — GetManagedPrefixListEntries with an unknown pl- id.
7. Cause 6: replay ec2-network-acl-create-configure-entries-delete step task#10 — DeleteNetworkAclEntry for an absent ingress rule number.
8. Cause 7: replay ec2-network-acl-lifecycle step task#6 — CreateNetworkAclEntry for a port-based protocol with no PortRange.
Each step must match the recorded code and message exactly, with only the request/state-derived values substituted.
