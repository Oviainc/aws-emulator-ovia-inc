Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.create-computer-authentication-failed-rejected
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.create-computer-authentication-failed-rejected|run#18|AssertionError|assert attr_values.get("Description") == marker_value, (
Issue-Commit: a4702f9d21375d5a7473607a9aa2a567041fbdb6

# CreateComputer echoes ComputerAttributes verbatim instead of the directory's canonical AD attribute set (1 cause)

> _Note: this issue was identified against the code base at commit `a4702f9d21375d5a7473607a9aa2a567041fbdb6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateComputer

## Description
One net-new root cause in the Directory Service (ds) emulator, found by the behavioral suite simparity-behavioral-ga-prod-v1. The replay report marked this check neither ALREADY FILED nor OWNED by an open issue, so it is filed here. The open issue 20261027T012551Z-ds-4-net-new-behavioral-root-causes-createcomputer.md describes a similar CreateComputer attribute-echo defect, but it does not carry this run's replay key for the check, so it does not cover it; it is named here because closing it in the same place would resolve this cause too.

Cause 1 of 1: CreateComputer returns the request's ComputerAttributes list unchanged, so the response is a verbatim replay of the request rather than the created computer account's own attributes.

AWS: Smithy (services/service-com-amazonaws-directoryservice-directoryservice-20150416/smithy-model/com-amazonaws-directoryservice-directoryservice-20150416.json) — CreateComputer input {DirectoryId @required @pattern ^d-[0-9a-f]{10}$, ComputerName @required @length 1..15, Password @required @sensitive @length 8..64, OrganizationalUnitDistinguishedName, ComputerAttributes -> Attributes (list of Attribute{Name,Value})}; output Computer{ComputerId, ComputerName, ComputerAttributes}; modeled errors AuthenticationFailedException, ClientException, DirectoryUnavailableException, EntityAlreadyExistsException, EntityDoesNotExistException, InvalidParameterException, UnsupportedOperationException (400) and ServiceException (500). The model does not say how the attributes are reported back, so for that part this is recorded behavior only: a CreateComputer that sets one attribute under the lowercase LDAP name 'description' is answered with Computer.ComputerAttributes keyed by the canonical, display-cased AD attribute names — the supplied value appears under 'Description', and the attributes the domain itself derives for the new machine account (the SAM name and the distinguished name) are present as well. The general rule is: a recognized LDAP attribute name is canonicalized to the casing AWS reports it under, and the response describes the created account, not the request body.

Emulator: the handler stores input.ComputerAttributes as given and serializes it straight back. The reply carries exactly one attribute whose Name is the request's lowercase spelling — attrs={'description': '<the marker value from the request>'} — and no derived account attributes at all. ComputerName and ComputerId are correct, and the surrounding legs of the same check pass (CreateComputer on a non-Active directory is refused DirectoryUnavailableException; a repeated ComputerName is refused EntityAlreadyExistsException).

Why: attribute-name case is observable contract here — the request's spelling and the response's spelling legitimately differ on AWS. A client that indexes Computer.ComputerAttributes by 'Description' to confirm its attribute was applied gets a key miss and concludes the write was dropped; a client that reads the account's SAM name or distinguished name out of the response to drive a domain join or to build an LDAP query against the new object has nothing to read.

Fix: the CreateComputer handler and the computer state record it reads back — services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_computer.rs together with the ComputerRecord -> Computer conversion in crate::state. Canonicalize recognized LDAP attribute names to AWS's display casing on the way out, and derive the account's own attributes from the computer name and the directory's domain instead of returning only the request's list.

Traces: ds.create-computer-authentication-failed-rejected (mismatch, assertion at check_ds_create_computer_authentication_failed_rejected.py:66).

## Evidence
Suite simparity-behavioral-ga-prod-v1 — total 14/66 passed (21.2%); ds 12/51 (24%), 39 failed.

[mismatch] ds.create-computer-authentication-failed-rejected — check_ds_create_computer_authentication_failed_rejected.py:66
  Call: create_computer(DirectoryId=<an Active d-...>, ComputerName='sim2real<rand>'[:15], Password=<one-time>, ComputerAttributes=[{'Name': 'description', 'Value': '<marker>'}])
  Expected (real AWS): Computer.ComputerAttributes contains the canonical display-cased names, with the supplied value under 'Description', plus the domain-derived SAM name and distinguished name.
  Actual (emulator): attrs={'description': '<marker>'} — the request list echoed verbatim, nothing added, nothing re-cased.
  AssertionError: expected Description attribute 'sim2real-marker-777e432fb40c', got attrs={'description': 'sim2real-marker-777e432fb40c'}
  (The id 'sim2real-marker-777e432fb40c' is a per-run generated value; the requirement is the KEY 'Description' carrying whatever value the request supplied, not that literal string.)
  Passing legs of the same check, for contrast: CreateComputer before the directory is Active -> DirectoryUnavailableException; duplicate ComputerName -> EntityAlreadyExistsException; ComputerName and ComputerId echo correctly.

Model corroboration (smithy_query query=operation, CreateComputer): output shape CreateComputerResult -> Computer{ComputerId, ComputerName, ComputerAttributes}; ComputerAttributes targets Attributes (list of Attribute{Name,Value}); no trait constrains the response spelling, hence 'recorded behavior only' above.

## Steps to Reproduce
Boot the emulator on fresh state; point a botocore client at the ds endpoint (and ec2 for the VPC fixture).
1. ec2:CreateVpc with a /23 CIDR and two subnets in two distinct AZs.
2. ds:CreateMicrosoftAD(Name='<unique>.example.com', Password=<strong password>, Edition='Standard', VpcSettings={'VpcId': <vpc>, 'SubnetIds': [<two subnets>]}).
3. Poll ds:DescribeDirectories until DirectoryDescriptions[0].Stage == 'Active'.
4. ds:CreateComputer(DirectoryId=<id>, ComputerName='sim2real12345', Password='Sim2Real-Computer-Pw1!', ComputerAttributes=[{'Name': 'description', 'Value': 'marker-1'}]).
5. Read resp['Computer']['ComputerAttributes'] as {a['Name']: a['Value']}. Expect a 'Description' key carrying 'marker-1', plus the account's derived SAM name and distinguished name; observe only {'description': 'marker-1'}.

Re-replay to confirm the fix: ds.create-computer-authentication-failed-rejected
