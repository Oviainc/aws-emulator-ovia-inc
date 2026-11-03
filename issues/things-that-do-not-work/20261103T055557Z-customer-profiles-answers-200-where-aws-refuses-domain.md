Issue-Source: trace-sweep-divergence
Issue-Trace: customer-profiles-ccp-traj-wrong-region
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: customer-profiles-ccp-traj-wrong-region customer-profiles-ccp-traj-wrong-search-key customer-profiles-ccp-traj-search-request-mistakes customer-profiles-create-calculated-attribute-definition-roundtrip
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles|SearchProfiles|4xx->2xx|$.Message
Issue-Key: customer-profiles|SearchProfiles|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: customer-profiles|SearchProfiles|4xx->2xx|status
Issue-Key: customer-profiles|GetDomain|4xx->2xx|$.Message
Issue-Key: customer-profiles|GetDomain|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: customer-profiles|GetDomain|4xx->2xx|status
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|4xx->2xx|$.Message
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: customer-profiles|CreateCalculatedAttributeDefinition|4xx->2xx|status
Issue-Commit: 39390cfa3000e408bc0d1db242197466d1e66569

# customer-profiles answers 200 where AWS refuses: domain scope and request gates are absent (4 causes)

> _Note: this issue was identified against the code base at commit `39390cfa3000e408bc0d1db242197466d1e66569`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchProfiles

## Description
Four net-new wire causes sharing ONE fault: the emulator has no gate where AWS has one, so a request AWS REFUSES is answered 200 with a plausible success body. These are the most dangerous class in the sweep because the client cannot detect them - there is no error to handle.

Cause 1 of 4: SearchProfiles resolves a domain that does not exist in the request's scope and returns 200.
AWS: Smithy: SearchProfiles (POST /domains/{DomainName}/profiles/search) models ResourceNotFoundException with httpError 404; DomainName is a required @httpLabel. A Customer Profiles domain is regional, so a domain created in one region is NOT visible to a request signed for another. Recorded response: 404 with error code ResourceNotFoundException and Message 'DomainName <DomainName> is not found in account <accountId>' (fixed text 'DomainName ' / ' is not found in account '; variable parts are the requested DomainName and the caller's account id).
Emulator: HTTP 200 with a search response body, no error type header, no Message - the domain resolves regardless of the scope the request was made in, and the undefined-key steps clustered here come back 200 as well.
Why: a client that targets the wrong region (or an unprovisioned one) gets an empty-but-successful search instead of a 404; 'no such domain' and 'no matching profiles' become indistinguishable, so misconfiguration ships to production undetected.
Fix: SearchProfiles' domain resolution must scope the domain store by the request's region/account and refuse ResourceNotFoundException/404 with the sentence above before any matching runs.
Traces: customer-profiles-ccp-traj-wrong-region#task#4 (cluster also covers customer-profiles-ccp-traj-wrong-search-key#task#3, #task#4, #task#5, where the expected 404 refusal is likewise replaced by a 200).

Cause 2 of 4: SearchProfiles accepts more than one value for a key name and returns 200.
AWS: Smithy: SearchProfilesRequest$Values is a required `requestValueList`; the model puts no max on the list, so the one-value-per-key rule is enforced server-side, and SearchProfiles models BadRequestException/400. Recorded response: 400, error code BadRequestException, Message 'Expected 1 values for key name <KeyName> but got <n>'. Fixed text: 'Expected 1 values for key name ' + ' but got '; variable parts: the KeyName from the request and n = the ACTUAL length of the submitted Values list (never pinned to one number).
Emulator: HTTP 200 with a result body; the surplus values are silently ignored.
Why: a client that batches several values into one key believes it searched for all of them, gets matches for one, and reads the absent matches as absent data - while the same code raises BadRequestException against AWS.
Fix: SearchProfiles' input validation, ahead of matching - refuse 400 with the sentence above whenever Values holds other than exactly one element for a key name, leaving the index untouched.
Traces: customer-profiles-ccp-traj-search-request-mistakes#task#3 (cluster also covers #task#4 and #task#8 of the same fixture).

Cause 3 of 4: GetDomain resolves a domain outside the request's scope and returns 200.
AWS: Smithy: GetDomain (GET /domains/{DomainName}) models ResourceNotFoundException/404. Recorded response for a domain that exists only in another region: 404 with Message 'Domain <DomainName> is not found' (fixed text 'Domain ' / ' is not found'; variable part the requested DomainName).
Emulator: HTTP 200 with a full domain body and no Message - the domain is served from a scope-blind store.
Why: region isolation is a correctness property clients depend on (blue/green by region, DR drills, 'is this domain provisioned here?' probes). An emulator that answers every region alike makes every such probe pass.
Fix: GetDomain's domain lookup - key the domain store by region (and account) and refuse with the sentence above when the domain is not in the request's scope.
Traces: customer-profiles-ccp-traj-wrong-region#task#3.

Cause 4 of 4: CreateCalculatedAttributeDefinition accepts an AttributeDetails object type that does not exist in the domain.
AWS: Smithy: CreateCalculatedAttributeDefinition (POST /domains/{DomainName}/calculated-attributes/{CalculatedAttributeName}) takes a required AttributeDetails and models BadRequestException with httpError 400; the attribute items name an object type that must already be defined in the domain. Recorded response: 400 with Message 'Invalid object type name: <objectTypeName>' - fixed text 'Invalid object type name: ', with the offending object type name taken from the request's AttributeDetails.
Emulator: HTTP 200, a definition is created for an object type the domain has never seen, no error type header, no Message.
Why: the definition is accepted and then can never compute anything, so the caller discovers the typo only as permanently empty calculated attributes - against AWS the mistake is caught at create time.
Fix: CreateCalculatedAttributeDefinition's input validation - resolve every AttributeDetails item's object type against the domain's defined object types and refuse 400 with the sentence above. (Open stub issue 20261030T101909Z-customer-profiles-createcalculatedattributedefinition-stub.md exists, but the replay shows this operation SERVING and answering 200 - the missing gate is this cause, not the stub.)
Traces: customer-profiles-create-calculated-attribute-definition-roundtrip#task#5.

Related but not covering: open behavioral issue 20261102T122410Z-customer-profiles-10-net-new-behavioral-causes-refusal.md names the SearchProfiles multi-value and undefined-key gates from the BEHAVIORAL suite (its Causes 8 and 9); it carries no wire replay keys, and the region-scope causes (1 and 3) and the object-type gate (4) are not among its causes.

## Evidence
Replay of simparity-wire-aws-v1 (emulator_up=true, bootstrap_signal=false). Each cluster is verdict=mismatch where the expected status is an error and the actual status is 200.

Cause 1 - fixture customer-profiles-ccp-traj-wrong-region step task#4, SearchProfiles:
  status: expected 404, got 200
  $.Message: missing (expected 'DomainName <DomainName> is not found in account <accountId>', got None)
  $headers.x-amzn-errortype: missing (expected a shaped error type, got None). affected_count=4, including customer-profiles-ccp-traj-wrong-search-key#task#3, #task#4, #task#5.

Cause 2 - fixture customer-profiles-ccp-traj-search-request-mistakes step task#3, SearchProfiles:
  status: expected 400, got 200
  $.Message: missing (expected 'Expected 1 values for key name _account but got 2', got None)
  $headers.x-amzn-errortype: missing. affected_count=3 (#task#4, #task#8).

Cause 3 - fixture customer-profiles-ccp-traj-wrong-region step task#3, GetDomain:
  status: expected 404, got 200
  $.Message: missing (expected 'Domain <DomainName> is not found', got None)
  $headers.x-amzn-errortype: missing.

Cause 4 - fixture customer-profiles-create-calculated-attribute-definition-roundtrip step task#5, CreateCalculatedAttributeDefinition:
  status: expected 400, got 200
  $.Message: missing (expected 'Invalid object type name: <objectTypeName>', got None)
  $headers.x-amzn-errortype: missing.

The key name '_account', the count 2 and the recorded domain/object-type names are that run's fill-ins; the templates and which parts are variable are stated in the description.

## Steps to Reproduce
Cause 1: CreateDomain(D, DefaultExpirationDays=366) in region R1; then SearchProfiles(DomainName=D, KeyName='_account', Values=[A]) signed for region R2 -> expect 404 ResourceNotFoundException 'DomainName D is not found in account <account id>'; observe HTTP 200 with a search body.
Cause 2: CreateDomain(D); CreateProfile(D, AccountNumber=A); SearchProfiles(D, KeyName='_account', Values=[A, A2]) -> expect 400 BadRequestException 'Expected 1 values for key name _account but got 2' (n = len(Values)); observe HTTP 200 with the profile matched by the first value.
Cause 3: CreateDomain(D) in region R1; GetDomain(DomainName=D) signed for region R2 -> expect 404 'Domain D is not found'; observe HTTP 200 with the domain body.
Cause 4: CreateDomain(D); CreateCalculatedAttributeDefinition(D, CalculatedAttributeName=C, Statistic=COUNT, AttributeDetails naming an ObjectTypeName never defined in D) -> expect 400 BadRequestException 'Invalid object type name: <that name>'; observe HTTP 200 and a created definition.

Re-replay fixtures customer-profiles-ccp-traj-wrong-region, customer-profiles-ccp-traj-wrong-search-key, customer-profiles-ccp-traj-search-request-mistakes and customer-profiles-create-calculated-attribute-definition-roundtrip to confirm.
