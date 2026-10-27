Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.create-domain-name-access-association-without-vpce-fails apigateway.concurrent-domain-name-access-association-duplicate apigateway.reject-domain-name-access-association-removes-pending apigateway.reject-domain-name-access-association-wrong-state-fails
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.create-domain-name-access-association-without-vpce-fails|_error#11|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: apigateway.concurrent-domain-name-access-association-duplicate|run#22|AssertionError|assert failures[0]["code"] == "BadRequestException", (
Issue-Key: apigateway.reject-domain-name-access-association-removes-pending|run#16|AssertionError|assert code == "BadRequestException", code
Issue-Key: apigateway.reject-domain-name-access-association-wrong-state-fails|run#8|AssertionError|assert code == "BadRequestException", (
Issue-Commit: 05654cdc2683a6b0073d07f9b85293d688309380

# apigateway: the domain-name-access-association surface diverges on 3 net-new causes — a non-existent VPC endpoint is accepted (201), a duplicate pair is refused as ConflictException instead of BadRequestException, and RejectDomainNameAccessAssociation answers 501 NotImplemented

> _Note: this issue was identified against the code base at commit `05654cdc2683a6b0073d07f9b85293d688309380`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainNameAccessAssociation

## Description
Three net-new root causes, grounded in four failing behavioral checks whose ids lead with `apigateway.`. None of the four is marked ALREADY FILED or OWNED in the replay report. Cause count: 3 — each is numbered below with the failing check ids it must be verified against before this issue is submitted.

RELATED OPEN ISSUE (it does NOT cover any of these three causes): 20261026T090242Z-apigateway-every-private-custom-domain-domain-name-access.md lists these same four check ids in its Issue-Trace-Ids, but its one cause is the EC2 interface VPC endpoint never leaving `pending`, and all of its replay keys are `…|Timeout|time.sleep(...)`. In this run the gate is open — the four checks get past the endpoint wait and now fail INSIDE the apigateway access-association calls with concrete wrong answers, which is a different cause in a different crate. Also related but not covering these: 20261023T151100Z-apigateway-api-gateway-putrestapi-overwrite-imports-paths-but-no.md (PutRestApi(overwrite) and ACM wiring on the custom-domain surface).

=== Cause 1 of 3: CreateDomainNameAccessAssociation never checks that accessAssociationSource names an existing VPC endpoint, so it mints a dangling association with HTTP 201 ===
AWS: Smithy (com.amazonaws.apigateway#BackplaneControlService) CreateDomainNameAccessAssociation — POST /domainnameaccessassociations, success code 201; required members `domainNameArn`, `accessAssociationSourceType` (enum AccessAssociationSourceType, one member: VPCE) and `accessAssociationSource` ("The identifier of the domain name access association source. For a VPCE, the value is the VPC endpoint ID"); modeled errors BadRequestException (400), ConflictException (409), LimitExceededException (429), TooManyRequestsException (429), UnauthorizedException (401). The association is defined to bind a private custom domain to a real interface endpoint, so a source id naming no endpoint is an incomplete/incorrect request: AWS refuses it with 400 BadRequestException. Recorded behavior: the message is the endpoint id taken from the request followed by the fixed text " is not a valid VPC endpoint id" (fixed text = " is not a valid VPC endpoint id"; variable part = the `accessAssociationSource` value the caller sent), and NO association is created for that source — a subsequent GetDomainNameAccessAssociations lists nothing for it.
Emulator: the request is ACCEPTED. services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_domain_name_access_association.rs validates only the string bounds, the `domainNameArn` shape, the domain's existence and its PRIVATE endpoint type, and the duplicate-pair check; `accessAssociationSource` is copied straight into the stored record and never resolved against any EC2 VPC endpoint. The replay shows HTTP 201 and a full DomainNameAccessAssociation body for a source that does not exist (accessAssociationSource reported back verbatim, with a freshly minted domainNameAccessAssociationArn), where a 400 BadRequestException was owed.
Why: the association is the authorization edge of a PRIVATE custom domain. A client that typos or stale-references an endpoint id gets a 201 and a listed association that can never route traffic, instead of being told at create time which id is wrong; infrastructure tools treat the 201 as convergence and ship a domain whose private access binding is inert. It is also the strictly over-permissive direction: the emulator cannot be used to test the error handling of this path at all.
Fix: the validation step of the CreateDomainNameAccessAssociation handler — the `accessAssociationSource` value must be resolved as a VPC endpoint of the caller's account/Region before a record is stored (the same cross-reference the PRIVATE-domain check already performs for the domain), and an unresolvable id must be answered with the modeled BadRequestException carrying the request's id plus the fixed " is not a valid VPC endpoint id" text, with the store left unchanged.
Traces: apigateway.create-domain-name-access-association-without-vpce-fails.

=== Cause 2 of 3: a duplicate (domain name, access association source) pair is refused with 409 ConflictException where AWS answers 400 BadRequestException "...already exists" ===
AWS: Smithy CreateDomainNameAccessAssociation models BOTH BadRequestException (400) and ConflictException (409); the model does not say which applies to a repeated pair. Recorded behavior only: real AWS refuses the second create of a pair that already exists with error code BadRequestException and a message containing the fixed phrase "already exists" (the identifying parts — the source id and the domain name — come from the request and from stored state), and exactly one association remains. Deleting that one makes it gone and a repeat delete is NotFoundException.
Emulator: the same request is refused with error code ConflictException (HTTP 409) and the emulator-authored message "The access association source <source> is already associated with the domain name <domainNameArn>: <existing association arn>" — built in create_domain_name_access_association.rs where the duplicate branch constructs `ConflictException`. The invariant parts are right (exactly one record is stored under one lock hold, so the race produces one winner and one loser) — only the refusal's class and wording diverge. Note the two causes compose: without Cause 1 a real duplicate can only be reached after a valid endpoint exists, and with Cause 1 present a duplicate can also be minted against a bogus source.
Why: callers branch on the error code, not the HTTP status alone. A client retrying an idempotent provisioning step catches BadRequestException for "already exists" and treats ConflictException as a retryable concurrent-modification signal (or vice versa), so a loser of this race is either retried forever or escalated as an unexpected failure; message-matching on "already exists" also fails.
Fix: the duplicate branch of the CreateDomainNameAccessAssociation handler's refusal classification — the existing-pair refusal must be the modeled BadRequestException (400) whose message states that the association for that source and domain already exists, using AWS's "already exists" wording with the source id and domain name taken from the request and stored state, instead of the ConflictException this handler constructs today.
Traces: apigateway.concurrent-domain-name-access-association-duplicate.

=== Cause 3 of 3: RejectDomainNameAccessAssociation answers 501 NotImplemented for a same-account association instead of the modeled 400 BadRequestException ===
AWS: Smithy RejectDomainNameAccessAssociation — POST /rejectdomainnameaccessassociations, success code 202, required query members `domainNameAccessAssociationArn` and `domainNameArn`; modeled errors BadRequestException (400), ConflictException (409), NotFoundException (404), TooManyRequestsException (429), UnauthorizedException (401). Its documentation states the operation rejects an association whose source is in ANOTHER AWS account: "To remove a domain name access association with an access association source in your own account, use the DeleteDomainNameAccessAssociation operation." So rejecting an association the caller owns is not a legal state transition. Recorded behavior: AWS refuses it with 400 BadRequestException and the entirely fixed message "Cannot reject the domain name access association when the private custom domain and domain name access association are in the same account." (no variable parts), and leaves the association intact and still listed; DeleteDomainNameAccessAssociation then removes it.
Emulator: the call reaches the scaffold's refusal. services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/reject_domain_name_access_association.rs reads and resolves both ARNs, then unconditionally returns `crate::scaffolded::Scaffolded::not_implemented("RejectDomainNameAccessAssociation")` for every pair it DOES hold — so the exact case AWS refuses with a modeled 400 comes back as error code `NotImplemented` (the replay reports `AssertionError: NotImplemented` and "expected BadRequestException rejecting a same-account access association …, got 'NotImplemented'"). The malformed-ARN 400 and unknown-resource 404 paths are already implemented; only the resolved-pair branch is the stub. The report does not mark these checks ALREADY FILED, so no unimplemented-operation issue covers this handler.
Why: a client cannot distinguish "this emulator does not implement Reject" from "AWS refuses this Reject". Code that exercises the documented ownership rule — reject only another account's association, delete your own — sees a 501 server-class error, which SDK retry policies and most error handling treat as a transient server fault rather than the permanent client-side refusal it is; the same-account guard is therefore untestable, and so is the invariant that a refused Reject leaves the association listed.
Fix: the resolved-pair branch of the RejectDomainNameAccessAssociation handler — once both ARNs resolve to held records, an association whose access association source belongs to the caller's own account must be refused with the modeled BadRequestException carrying AWS's fixed sentence above, leaving the stored association untouched, instead of the scaffold's NotImplemented. The cross-account success path (202, association removed) remains this operation's remaining work.
Traces: apigateway.reject-domain-name-access-association-removes-pending, apigateway.reject-domain-name-access-association-wrong-state-fails.

## Evidence
Replay report, suite simparity-behavioral-ga-prod-v1 (160/345 passed; service `apigateway` 154/263 with 109 failures). The four checks below are the only `apigateway.` failures in this run that the report marks neither ALREADY FILED nor OWNED.

Cause 1 — [mismatch] apigateway.create-domain-name-access-association-without-vpce-fails
  check_apigateway_create_domain_name_access_association_without_vpce_fails.py line 50 -> _error() line 86:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned
    {'ResponseMetadata': {'HTTPStatusCode': 201, ...},
     'domainNameAccessAssociationArn': 'arn:aws:apigateway:<region>::/domainnameaccessassociations/<id>',
     'domainNameArn': 'arn:aws:apigateway:<region>::/domainnames/<name>+<id>',
     'accessAssociationSourceType': 'VPCE',
     'accessAssociationSource': 'vpce-0123456789abcdef0', 'tags': {}}
  (the generated arns/ids above are per-run values, not requirements). The check first proves a positive control: an association for a REAL endpoint is created and is visible via GetDomainNameAccessAssociations, so this is not "the emulator refuses everything". It then asserts for the bogus id:
    assert code == 'BadRequestException'
    assert message == f'{bogus_vpce} is not a valid VPC endpoint id'   # bogus_vpce = 'vpce-0123456789abcdef0'
  Expected: HTTP 400, code BadRequestException, message = <the accessAssociationSource from the request> + ' is not a valid VPC endpoint id', and no association listed for that source. Actual: HTTP 201 and a stored, listable association.

Cause 2 — [mismatch] apigateway.concurrent-domain-name-access-association-duplicate
  check_apigateway_concurrent_domain_name_access_association_duplicate.py line 92:
  AssertionError: expected BadRequestException for the duplicate association, got ConflictException: The access association source vpce-<id> is already associated with the domain name arn:aws:apigateway:<region>::/domainnames/<name>+<id>: arn:aws:apigateway:<region>::/domainnameaccessassociations/<id>
  The check races two identical CreateDomainNameAccessAssociation calls behind a barrier and asserts:
    assert len(successes) == 1 and len(failures) == 1          # these PASSED
    # 'AWS rejects the racing duplicate with BadRequestException, not ConflictException.'
    assert failures[0]['code'] == 'BadRequestException'        # FAILED: got 'ConflictException'
    assert 'already exists' in failures[0]['message'].lower()  # not reached
  Expected: HTTP 400 / BadRequestException whose message carries the fixed phrase 'already exists'. Actual: HTTP 409 / ConflictException with emulator-authored text ('... is already associated with the domain name ...: <arn>'). The one-record invariant and the later delete/NotFoundException steps were not reached.

Cause 3 — [mismatch] apigateway.reject-domain-name-access-association-removes-pending
  check_apigateway_reject_domain_name_access_association_removes_pending.py line 86: assert code == 'BadRequestException', code -> AssertionError: NotImplemented
         — [mismatch] apigateway.reject-domain-name-access-association-wrong-state-fails
  check_apigateway_reject_domain_name_access_association_wrong_state_fails.py line 59:
  AssertionError: expected BadRequestException rejecting a same-account access association (reject only applies to an association shared from a different AWS account), got 'NotImplemented'
  Both checks create a PRIVATE domain + a real execute-api interface endpoint + an association, confirm it is listed, then call RejectDomainNameAccessAssociation(domainNameAccessAssociationArn=A, domainNameArn=D) and assert:
    assert code == 'BadRequestException'
    assert message == ('Cannot reject the domain name access association when the private custom domain and '
                       'domain name access association are in the same account.')
  Expected: HTTP 400 / BadRequestException with that fixed sentence verbatim, the association still listed, and DeleteDomainNameAccessAssociation then removing it. Actual: error code NotImplemented.

Emulator-side corroboration (checkout under replay):
  services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/create_domain_name_access_association.rs — `bound_input` checks only lengths/tags; `domain_name_key_of` validates the domain ARN; the PRIVATE check and `store.access_association_of(...)` duplicate check follow; `input.access_association_source` is then stored verbatim in `DomainNameAccessAssociationEntry` with no VPC-endpoint resolution anywhere in the file (Cause 1), and the duplicate branch constructs `CreateDomainNameAccessAssociationErrorModeled::ConflictException` with the message quoted above (Cause 2).
  services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/reject_domain_name_access_association.rs — module doc says "PARTLY IMPLEMENTED — REFUSALS ONLY … The SUCCESS path is still the scaffold's 501"; `handle` resolves both ARNs, calls `store.holds(...)` twice, then unconditionally `Err(crate::scaffolded::Scaffolded::not_implemented("RejectDomainNameAccessAssociation"))` — so the same-account case, which AWS refuses with a modeled BadRequestException, falls into the stub (Cause 3).

## Steps to Reproduce
Boot the emulator. Provision the prerequisites once (region = the Region the client signs for):
  P1. ec2 CreateVpc / CreateSubnet / CreateSecurityGroup, then ec2 CreateVpcEndpoint(ServiceName='com.amazonaws.<region>.execute-api', VpcEndpointType='Interface', ...) -> note VpcEndpointId as E; wait until DescribeVpcEndpoints reports State 'available'.
  P2. acm ImportCertificate(a self-signed cert whose CN is <domain>) -> note CertificateArn.
  P3. apigateway CreateDomainName(domainName=<domain>, certificateArn=<arn>, endpointConfiguration={'types':['PRIVATE']}, policy=<execute-api Invoke policy>) -> note domainNameArn as D and domainNameId; poll GetDomainName until domainNameStatus == 'AVAILABLE'.

Cause 1:
  1. apigateway CreateDomainNameAccessAssociation(domainNameArn=D, accessAssociationSourceType='VPCE', accessAssociationSource=E) -> expect 201; GetDomainNameAccessAssociations lists it (positive control).
  2. apigateway CreateDomainNameAccessAssociation(domainNameArn=D, accessAssociationSourceType='VPCE', accessAssociationSource='vpce-0123456789abcdef0')  # an id no endpoint has
     -> OBSERVE HTTP 201 and a stored association echoing that source.
     -> EXPECT HTTP 400, Error.Code 'BadRequestException', Error.Message = '<the accessAssociationSource sent> is not a valid VPC endpoint id', and GetDomainNameAccessAssociations listing nothing for that source.

Cause 2:
  3. Issue step 1's exact call TWICE (sequentially is enough; concurrently from two threads behind a barrier reproduces the racing form).
     -> OBSERVE the second call answers HTTP 409 with Error.Code 'ConflictException' and message 'The access association source <E> is already associated with the domain name <D>: <existing association arn>'.
     -> EXPECT HTTP 400 with Error.Code 'BadRequestException' and a message carrying the fixed phrase 'already exists' (source id and domain name filled from the request/state). Either way exactly one association must remain for the pair — that part already holds.

Cause 3:
  4. With the association from step 1 (arn A) listed by GetDomainNameAccessAssociations(resourceOwner='SELF'), call apigateway RejectDomainNameAccessAssociation(domainNameAccessAssociationArn=A, domainNameArn=D) — both resources are in the caller's own account.
     -> OBSERVE Error.Code 'NotImplemented' (the scaffold refusal).
     -> EXPECT HTTP 400, Error.Code 'BadRequestException', Error.Message exactly 'Cannot reject the domain name access association when the private custom domain and domain name access association are in the same account.' (fully fixed text), with A still listed afterwards.
  5. apigateway DeleteDomainNameAccessAssociation(domainNameAccessAssociationArn=A) -> expect success and A gone from the listing (this already works and is the checks' positive control, so a target that refuses everything cannot pass).

Re-replay after the fix exactly the four check ids in trace_ids.
