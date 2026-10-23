Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: evs.associate-eip-to-vlan-environment-not-found evs.disassociate-eip-from-vlan-environment-not-found evs.create-environment-connector-environment-not-found evs.create-environment-host-nonexistent-environment-validation-exception evs.create-environment-host-conflicting-placement-params-rejected evs.create-environment-invalid-vpc-rejected evs.missing-iam-permission-denied
Issue-Shape-Id: com.amazonaws.evs#AmazonElasticVMwareService
Issue-Key: evs.associate-eip-to-vlan-environment-not-found|run#3|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code!r}"
Issue-Key: evs.disassociate-eip-from-vlan-environment-not-found|run#3|AssertionError|assert code == "ResourceNotFoundException", f"expected ResourceNotFoundException, got {code}"
Issue-Key: evs.create-environment-connector-environment-not-found|run#3|AssertionError|assert code == "ResourceNotFoundException", (
Issue-Key: evs.create-environment-host-nonexistent-environment-validation-exception|run#3|AssertionError|assert code == "ValidationException", (
Issue-Key: evs.create-environment-host-conflicting-placement-params-rejected|run#3|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code!r}"
Issue-Key: evs.create-environment-invalid-vpc-rejected|run#7|AssertionError|assert message == expected, f"expected {expected!r}, got {message!r}"
Issue-Key: evs.missing-iam-permission-denied|run#18|AssertionError|assert code == "AccessDeniedException", (
Issue-Commit: a25176d5b2a976d6230e754c8e419e3d957ef7d7

# EVS: 6 behavioral divergences — four not-found/validation paths answer 501 NotImplemented (AssociateEipToVlan, DisassociateEipFromVlan, CreateEnvironmentConnector, CreateEnvironmentHost), CreateEnvironment request validation over-rejects SELF_DEPLOYED fields and never checks vpcId, and an explicit IAM deny is not enforced

> _Note: this issue was identified against the code base at commit `a25176d5b2a976d6230e754c8e419e3d957ef7d7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateEipToVlan, DisassociateEipFromVlan, CreateEnvironmentConnector, CreateEnvironmentHost, CreateEnvironment, GetEnvironment

## Description
Six distinct root causes in the bootstrapped EVS emulator (service-com-amazonaws-evs-amazonelasticvmwareservice, sdkId `evs`, awsJson1_0, API version 2023-07-27). All six are grounded in failing checks of the behavioral suite; none is marked ALREADY FILED or OWNED by an existing issue. Each cause must be closed before this issue is submitted.

=== Cause 1 of 6: AssociateEipToVlan answers 501 NotImplemented instead of the modeled ResourceNotFoundException for a nonexistent environmentId ===
AWS: Smithy model (com-amazonaws-evs-amazonelasticvmwareservice.json) binds AssociateEipToVlan with errors ResourceNotFoundException (HTTP 404), ThrottlingException (429), ValidationException (400). environmentId is EnvironmentId, pattern ^(env-[a-zA-Z0-9]{10})$, so `env-0000000000` is well-formed; a well-formed id that names no environment is a resource condition, answered with ResourceNotFoundException. Recorded response message (example only, built from the request id): "Environment env-0000000000 was not found".
Emulator: returns HTTP 501 with error code `NotImplemented` ("operation AssociateEipToVlan is scaffolded but not implemented in this emulator"). The handler is still the generated scaffold — its `handle` returns `Scaffolded::not_implemented("AssociateEipToVlan")`.
Why: a client cannot distinguish "this VLAN/environment does not exist" from "the service is broken"; `NotImplemented` is not in the operation's error contract, so SDK error handling (retry classification, catch of ResourceNotFoundException) never fires and the 501 surfaces as an unexpected server fault.
Fix: the operation's handler — implement environment lookup and return the modeled ResourceNotFoundException (404) when the environmentId is unknown; delete the SCAFFOLDED marker.
Traces: check evs.associate-eip-to-vlan-environment-not-found; handler services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/associate_eip_to_vlan.rs:171.

=== Cause 2 of 6: DisassociateEipFromVlan answers 501 NotImplemented instead of ResourceNotFoundException for a nonexistent environmentId ===
AWS: Smithy: DisassociateEipFromVlan errors = ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400); environmentId pattern ^(env-[a-zA-Z0-9]{10})$. A well-formed but unknown environmentId is refused 404 ResourceNotFoundException. Recorded message example: "Environment env-0000000000 was not found".
Emulator: HTTP 501, error code `NotImplemented`; the handler is the untouched scaffold stub.
Why: same as Cause 1 — the EIP-detach path of the HCX VLAN surface is unusable and the refusal is unclassifiable by a client.
Fix: the operation's handler — implement the lookup and return the modeled 404.
Traces: check evs.disassociate-eip-from-vlan-environment-not-found; handler services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/disassociate_eip_from_vlan.rs (scaffold `not_implemented` in `handle`).

=== Cause 3 of 6: CreateEnvironmentConnector answers 501 NotImplemented instead of ResourceNotFoundException for a nonexistent environmentId ===
AWS: Smithy: CreateEnvironmentConnector errors = ResourceNotFoundException (404), ThrottlingException (429), ValidationException (400). A create against a well-formed but unknown environmentId is refused 404 ResourceNotFoundException and creates no connector. Recorded message example: "Environment env-0000000000 was not found".
Emulator: HTTP 501, error code `NotImplemented` from the scaffold stub, so the parent-existence precondition is never evaluated.
Why: a client provisioning a vCenter connector gets a server fault rather than the modeled not-found refusal, and cannot tell whether a connector was partially created.
Fix: the operation's handler — implement the environment lookup/connector creation and return the modeled 404 for an unknown parent.
Traces: check evs.create-environment-connector-environment-not-found; handler services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment_connector.rs.

=== Cause 4 of 6: CreateEnvironmentHost answers 501 NotImplemented instead of ValidationException ===
AWS: Smithy: CreateEnvironmentHost's error contract is ThrottlingException (429) and ValidationException (400) ONLY — it has no ResourceNotFoundException. Consequently a well-formed but nonexistent environmentId is surfaced as 400 ValidationException (recorded message example, built from the request id: "env-0000000000 was not found"). The same 400 ValidationException class is what the documented mutually-exclusive placement pair (host.dedicatedHostId with host.placementGroupId) is refused with; the recording confirms that for a nonexistent environment the ValidationException carries the not-found text, so both checks exercise this one rejection path.
Emulator: HTTP 501, error code `NotImplemented` for both requests; the handler is the untouched scaffold stub, so neither the environment-existence precondition nor the placement-parameter conflict check exists.
Why: host addition is the core post-create EVS workflow; a client sees a 501 server fault where AWS returns a 400 the caller can act on, and a request AWS refuses (conflicting placement parameters) is never validated.
Fix: the operation's handler — implement host creation, refuse an unknown environmentId with the modeled ValidationException (400), and refuse dedicatedHostId+placementGroupId on the same host object with ValidationException.
Traces: checks evs.create-environment-host-nonexistent-environment-validation-exception and evs.create-environment-host-conflicting-placement-params-rejected; handler services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment_host.rs (SCAFFOLDED marker at line 4).

=== Cause 5 of 6: CreateEnvironment request validation rejects the SELF_DEPLOYED-unsupported members that AWS accepts, and never validates that vpcId exists ===
AWS: Smithy: CreateEnvironment's only modeled error is ValidationException (400); vpcId is VpcId (len 12..21, pattern ^vpc-[a-f0-9]{8}([a-f0-9]{9})?$) and vcfVersion is one of SELF_DEPLOYED | VCF-5.2.1 | VCF-5.2.2, documented as "SELF_DEPLOYED: ... The licenseInfo, hosts, vcfHostnames, siteId, and connectivityInfo parameters are not supported." Recorded behavior for a request carrying all five of those members together with vcfVersion=SELF_DEPLOYED and a well-formed but nonexistent vpcId/serviceAccessSubnetId: ValidationException whose joined message (AWS joins every failed request validation with " | ") contains NO clause about licenseInfo/hosts/vcfHostnames/siteId/connectivityInfo, and DOES contain a clause naming the VPC: "The VPC id vpc-00000000 is not valid or does not exist. Please verify the VPC id exists and is valid." So AWS's rules here are (a) supplying the SELF_DEPLOYED-unsupported members is not by itself a refusal, and (b) a vpcId that names no VPC is refused with a clause naming that vpcId. (The recording's leading "Request validation failure: You must be subscribed to AWS Premium Support" clause is an account-entitlement gate of the recording account and is NOT claimed as a requirement here; the requirement is which request-derived clauses are present.)
Emulator: answers ValidationException (correct code/status) but with a message built from the wrong clause set: 'Request validation failure: licenseInfo is not supported when vcfVersion is SELF_DEPLOYED | hosts is not supported when vcfVersion is SELF_DEPLOYED | vcfHostnames is not supported when vcfVersion is SELF_DEPLOYED | siteId is not supported when vcfVersion is SELF_DEPLOYED | connectivityInfo is not supported when vcfVersion is SELF_DEPLOYED'. It emits a field failure per SELF_DEPLOYED-unsupported member (`vcf_mode_failures`) and has no vpcId-existence precondition at all — the only vpcId check in the handler is `state.vpc_is_in_use`, i.e. reuse by another EVS environment, never existence.
Why: a client that must pass those members to satisfy the shipped request model (botocore marks licenseInfo/hosts/connectivityInfo/vcfHostnames/siteId required on CreateEnvironmentRequest) can never get past the emulator's validation, so no SELF_DEPLOYED environment can be created; and a genuinely bad vpcId/subnetId is accepted as valid, hiding the one misconfiguration this validation exists to catch.
Fix: CreateEnvironment's request validation in the operation's handler — drop (or make non-fatal) the SELF_DEPLOYED "not supported" field failures, and add a vpcId/serviceAccessSubnetId existence precondition that contributes the VPC-invalid clause to the aggregated ValidationException.
Traces: check evs.create-environment-invalid-vpc-rejected; services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/create_environment.rs:281-315 (`vcf_mode_failures`, `vpc_is_in_use`, `aggregate_validation_error`).

=== Cause 6 of 6: an explicit IAM deny of evs:* is not enforced — GetEnvironment answers ResourceNotFoundException instead of AccessDeniedException ===
AWS: recorded behavior only (the EVS model declares no AccessDeniedException; it is the service-common authorization refusal). A caller assuming a role whose inline identity policy contains {"Effect":"Deny","Action":"evs:*","Resource":"*"} is refused at the IAM layer BEFORE the resource is evaluated: error code AccessDeniedException, with the recorded message of the documented explicit-deny form (example): "User: arn:aws:sts::<account>:assumed-role/<role>/<session> is not authorized to perform: evs:GetEnvironment on resource: arn:aws:evs:<region>:<account>:environment/env-0000000000 with an explicit deny in an identity-based policy". The same call with authorized credentials instead reaches EVS's own handling and returns ResourceNotFoundException.
Emulator: the denied caller gets ResourceNotFoundException — exactly what the authorized caller gets. Authorization is not evaluated for EVS requests at all, so the explicit deny has no effect.
Why: this is an overly-permissive divergence: tests and tooling that assert a deny is effective (least-privilege regression checks, policy-change verification) pass against the emulator while the equivalent real call is denied, and a client cannot distinguish "denied" from "absent".
Fix: the authorization step on the EVS request path (the service's auth/authorization middleware consulting the emulator's IAM policy store), ahead of GetEnvironment's handler — refuse with AccessDeniedException when an identity policy explicitly denies the action.
Traces: check evs.missing-iam-permission-denied; services/service-com-amazonaws-evs-amazonelasticvmwareservice/src/ops/get_environment.rs (handler reached despite the deny).

## Evidence
Suite: simparity-behavioral-aws-v1, evs 3/20 passed (15%), 17 failed. Failures clustered here (all other evs failures were marked ALREADY FILED against unimplemented scaffold stubs CreateEntitlement, UpdateEnvironmentConnector, GetAccountSettings, GetVersions and ec2 CreateDhcpOptions, and are skipped).

Cause 1 — evs.associate-eip-to-vlan-environment-not-found:
  expected: code ResourceNotFoundException (HTTP 404), message e.g. "Environment env-0000000000 was not found"
  actual:   AssertionError: expected ResourceNotFoundException, got 'NotImplemented' (HTTP 501)

Cause 2 — evs.disassociate-eip-from-vlan-environment-not-found:
  expected: ResourceNotFoundException (404)
  actual:   AssertionError: expected ResourceNotFoundException, got NotImplemented (501)

Cause 3 — evs.create-environment-connector-environment-not-found:
  expected: ResourceNotFoundException (404) for a nonexistent environmentId, no connector created
  actual:   AssertionError: expected ResourceNotFoundException for a nonexistent environmentId, got 'NotImplemented' (501)

Cause 4 — evs.create-environment-host-nonexistent-environment-validation-exception:
  expected: ValidationException (400) — CreateEnvironmentHost models no ResourceNotFoundException; message e.g. "env-0000000000 was not found"
  actual:   AssertionError: expected ValidationException ..., got 'NotImplemented' (501)
  evs.create-environment-host-conflicting-placement-params-rejected (host.dedicatedHostId + host.placementGroupId):
  expected: ValidationException (400)
  actual:   AssertionError: expected ValidationException, got 'NotImplemented' (501)

Cause 5 — evs.create-environment-invalid-vpc-rejected (vcfVersion=SELF_DEPLOYED, vpcId=vpc-00000000, subnet-00000000, all five SELF_DEPLOYED-unsupported members supplied):
  expected (recorded AWS): ValidationException whose joined message carries the VPC clause "The VPC id vpc-00000000 is not valid or does not exist. Please verify the VPC id exists and is valid." and NO licenseInfo/hosts/vcfHostnames/siteId/connectivityInfo clause
  actual:   ValidationException with 'Request validation failure: licenseInfo is not supported when vcfVersion is SELF_DEPLOYED | hosts is not supported when vcfVersion is SELF_DEPLOYED | vcfHostnames is not supported when vcfVersion is SELF_DEPLOYED | siteId is not supported when vcfVersion is SELF_DEPLOYED | connectivityInfo is not supported when vcfVersion is SELF_DEPLOYED' — no VPC clause at all

Cause 6 — evs.missing-iam-permission-denied (assumed role with inline Deny on evs:*, calling GetEnvironment on env-0000000000):
  expected: AccessDeniedException (explicit deny, identity-based policy)
  actual:   AssertionError: expected AccessDeniedException for a caller with an explicit evs:* deny, got 'ResourceNotFoundException' — identical to the authorized positive control

## Steps to Reproduce
Boot the emulator and point a botocore `evs` client at it (region/credentials as the harness configures).

Cause 1: evs.associate_eip_to_vlan(environmentId="env-0000000000", vlanName="edgeVTep", allocationId="eipalloc-00000000") -> observe 501 NotImplemented instead of 404 ResourceNotFoundException.
Cause 2: evs.disassociate_eip_from_vlan(environmentId="env-0000000000", vlanName="edgeVTep", associationId="eipassoc-00000000") -> 501 NotImplemented instead of 404 ResourceNotFoundException.
Cause 3: evs.create_environment_connector(environmentId="env-0000000000", type="VCENTER", applianceFqdn="vcenter.example.invalid", secretIdentifier="<any>") -> 501 NotImplemented instead of 404 ResourceNotFoundException.
Cause 4: evs.create_environment_host(environmentId="env-0000000000", host={"hostName":"h1","keyName":"k","instanceType":"i4i.metal"}) -> 501 instead of 400 ValidationException; repeat with host also carrying dedicatedHostId="h-01234567" and placementGroupId="pg-01234567" -> 501 instead of 400 ValidationException.
Cause 5: evs.create_environment(vpcId="vpc-00000000", serviceAccessSubnetId="subnet-00000000", vcfVersion="SELF_DEPLOYED", termsAccepted=True, initialVlans=<valid /28 plan>, environmentName=<unique>, plus shape-valid licenseInfo/hosts/vcfHostnames/siteId/connectivityInfo placeholders required by the shipped request model) -> ValidationException whose message lists the five "not supported when vcfVersion is SELF_DEPLOYED" clauses and never mentions the nonexistent vpcId.
Cause 6: with the `iam`/`sts` clients, create a role trusted by the account root, attach an inline policy {"Effect":"Deny","Action":"evs:*","Resource":"*"}, assume it, and call evs.get_environment(environmentId="env-0000000000") with the assumed-role credentials -> ResourceNotFoundException instead of AccessDeniedException (the same call with the default credentials returns the same ResourceNotFoundException, showing the deny is ignored).
