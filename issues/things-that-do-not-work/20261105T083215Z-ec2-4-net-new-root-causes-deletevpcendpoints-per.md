Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2.asvpce-delete-vpc-endpoints-reports-unknown-id-per-item ec2.ec2mr-pub-disassociate-route-table-rejects-main-association ec2.managed-prefix-list-lifecycle ec2.op-contract-modify-vpc-endpoint-notfound-rejected
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2.asvpce-delete-vpc-endpoints-reports-unknown-id-per-item|run#9|AssertionError|assert bad == want, (
Issue-Key: ec2.ec2mr-pub-disassociate-route-table-rejects-main-association|run#19|AssertionError|assert got == expected, (
Issue-Key: ec2.managed-prefix-list-lifecycle|run#21|AssertionError|assert over == "PrefixListMaxEntriesExceeded", (
Issue-Key: ec2.op-contract-modify-vpc-endpoint-notfound-rejected|run#13|AssertionError|assert err == "InvalidVpcEndpointId.NotFound", err
Issue-Commit: 506f51337b02a9b5673b5526bcfdc40d4e5c4821

# ec2: 4 net-new root causes — DeleteVpcEndpoints per-item error, main route-table association refusal, ModifyManagedPrefixList MaxEntries refusal and ModifyVpcEndpoint answering NotImplemented

> _Note: this issue was identified against the code base at commit `506f51337b02a9b5673b5526bcfdc40d4e5c4821`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteVpcEndpoints

## Description
Four net-new root causes in the ec2 service. All are refusal-fidelity defects: the operation answers, but with the wrong error code, the wrong message, or no implementation at all.

Cause 1 of 4: DeleteVpcEndpoints reports unknown ids with the wrong per-item error code and an emulator-authored message.
AWS: DeleteVpcEndpoints is a bulk call — each id that does not resolve gets its own Unsuccessful entry whose Error.Code is InvalidVpcEndpoint.NotFound and whose Error.Message is "user <account-id> does not own a resource <vpce-id>" (fixed text "user " and " does not own a resource "; the variable parts are the caller's account id and the VpcEndpointId from the request), while the real endpoint in the same batch still deletes. Smithy (services/service-com-amazonaws-ec2-amazonec2): DeleteVpcEndpoints declares errors: [] and returns Unsuccessful: UnsuccessfulItemSet — the per-item code/message are wire values, recorded behavior only.
Emulator: both entries carry Error.Code 'InvalidVpcEndpointId.NotFound' and Error.Message "The VpcEndpoint Id '<vpce-id>' does not exist" — wrong code (…EndpointId… for …Endpoint…) and emulator-authored wording.
Why: a client that switches on the per-item Error.Code to tell "not mine / already gone" from a real failure never matches, so bulk cleanup loops abort or retry forever.
Fix: the DeleteVpcEndpoints handler's Unsuccessful-item construction in the ec2 service.
Traces: ec2.asvpce-delete-vpc-endpoints-reports-unknown-id-per-item.

Cause 2 of 4: DisassociateRouteTable does not recognise a VPC's main route-table association, so the main-association guard is unreachable.
AWS: the main association can never be disassociated; DisassociateRouteTable on it is refused HTTP 400 with error code InvalidParameterValue and message "cannot disassociate the main route table association <rtbassoc-id>" (fixed sentence plus the RouteTableAssociationId from the request). The identical call on an explicit association succeeds. Smithy: DisassociateRouteTable declares errors: [] — the code is a wire value, recorded behavior only.
Emulator: the same call answers error code 'InvalidAssociationID.NotFound' with "The association ID '<rtbassoc-id>' does not exist" — it treats the main association id DescribeRouteTables just returned with Main=true as unknown.
Why: clients cannot distinguish "you may not touch the main association" from "that association is gone"; a reconciler reading not-found concludes the association was already removed and proceeds on a still-main-governed subnet.
Fix: the DisassociateRouteTable handler must resolve main associations from route-table state and refuse them with the code and message above.
Traces: ec2.ec2mr-pub-disassociate-route-table-rejects-main-association.

Cause 3 of 4: ModifyManagedPrefixList does not enforce the declared MaxEntries cap with its own error class.
AWS: an AddEntries that would take the list past its declared MaxEntries is refused with error code PrefixListMaxEntriesExceeded and the message "You've reached the maximum number of entries for the prefix list. This modification saves (<resulting-entry-count>) entries and the maximum number of entries for this prefix list is (<MaxEntries>)." — the two parenthesised values come from the request and the list's stored MaxEntries; the surrounding sentence is fixed. The refused modify must not bump Version or change the entry set. Smithy: ModifyManagedPrefixList declares errors: [] and models MaxEntries on the input; the code is recorded behavior only.
Emulator: the over-cap modify is refused with error code InvalidParameterValue instead of PrefixListMaxEntriesExceeded.
Why: a client that catches PrefixListMaxEntriesExceeded to raise MaxEntries and retry instead sees a generic InvalidParameterValue it must treat as a malformed request; the cap becomes indistinguishable from a bad CIDR.
Fix: the ModifyManagedPrefixList handler's entry-count validation in the ec2 service.
Traces: ec2.managed-prefix-list-lifecycle.

Cause 4 of 4: ModifyVpcEndpoint is a scaffold stub, so its not-found boundary answers NotImplemented.
AWS: ModifyVpcEndpoint replaces a gateway endpoint's PolicyDocument (DescribeVpcEndpoints then returns it); a syntactically valid but nonexistent VpcEndpointId is refused with error code InvalidVpcEndpointId.NotFound. Smithy: ModifyVpcEndpoint declares errors: [] with required VpcEndpointId and returns Return: Boolean; the code is recorded behavior only.
Emulator: the call answers error code NotImplemented, so neither the policy replacement nor the not-found refusal exists.
Why: endpoint-policy changes are silently impossible and the client sees an unmodeled error it cannot classify.
Fix: implement the ModifyVpcEndpoint handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/modify_vpc_endpoint.rs). A stub issue naming ModifyVpcEndpoint and that file is already filed from the checkout (cited by ec2.ec2-iot-data-endpoint-rejects-non-full-access-endpoint-policy; open issue 20261101T172217Z-ec2-4-more-operations-are-unimplemented-scaffold-stubs.md covers neighbouring stubs) — this check was NOT marked as covered, so it is listed here and closes with that same fix.
Traces: ec2.op-contract-modify-vpc-endpoint-notfound-rejected.

## Evidence
Cause 1 — expected Unsuccessful set {( 'vpce-00000000000000000', 'InvalidVpcEndpoint.NotFound', 'user <account> does not own a resource vpce-00000000000000000'), ('vpce-11111111111111111', 'InvalidVpcEndpoint.NotFound', 'user <account> does not own a resource vpce-11111111111111111')}. Actual: [{'Error': {'Code': 'InvalidVpcEndpointId.NotFound', 'Message': "The VpcEndpoint Id 'vpce-00000000000000000' does not exist"}, 'ResourceId': 'vpce-00000000000000000'}, {'Error': {'Code': 'InvalidVpcEndpointId.NotFound', 'Message': "The VpcEndpoint Id 'vpce-11111111111111111' does not exist"}, 'ResourceId': 'vpce-11111111111111111'}].
Cause 2 — AssertionError: DisassociateRouteTable on the main association 'rtbassoc-0000000000000002b' returned ('InvalidAssociationID.NotFound', "The association ID 'rtbassoc-0000000000000002b' does not exist"), expected ('InvalidParameterValue', 'cannot disassociate the main route table association rtbassoc-0000000000000002b').
Cause 3 — AssertionError: expected PrefixListMaxEntriesExceeded, got InvalidParameterValue (the check then asserts the message "You've reached the maximum number of entries for the prefix list. This modification saves (3) entries and the maximum number of entries for this prefix list is (2)." for a MaxEntries=2 list at cap).
Cause 4 — expected error code InvalidVpcEndpointId.NotFound for ModifyVpcEndpoint(VpcEndpointId='vpce-00000000000000000', PolicyDocument=<deny-all>); actual AssertionError: NotImplemented.

## Steps to Reproduce
Cause 1: create one interface VPC endpoint, then ec2 DeleteVpcEndpoints(VpcEndpointIds=[<real>, 'vpce-'+'0'*17, 'vpce-'+'1'*17]). Expect two Unsuccessful entries with Error.Code InvalidVpcEndpoint.NotFound and message "user <account> does not own a resource <id>"; observed InvalidVpcEndpointId.NotFound / "The VpcEndpoint Id '<id>' does not exist".
Cause 2: create a VPC, read the main route table's association via DescribeRouteTables(Filters=[association.main=true]), then DisassociateRouteTable(AssociationId=<main assoc id>). Expect InvalidParameterValue "cannot disassociate the main route table association <id>"; observed InvalidAssociationID.NotFound.
Cause 3: CreateManagedPrefixList(MaxEntries=2, one entry), wait create-complete, ModifyManagedPrefixList adding a second entry (succeeds), then add a third. Expect PrefixListMaxEntriesExceeded with the "saves (3) entries ... maximum ... is (2)" message and no version/entry change; observed InvalidParameterValue.
Cause 4: create a VPC + route table + Gateway endpoint for com.amazonaws.<region>.s3, wait available, then ModifyVpcEndpoint(VpcEndpointId='vpce-'+'0'*17, PolicyDocument=<deny-all>). Expect InvalidVpcEndpointId.NotFound; observed NotImplemented.
