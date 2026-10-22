Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.create-directory-rejects-noncompliant-password ds.create-directory-member-validation-is-scoped ds.create-alias-invalid-alias-rejected ds.delete-nonexistent-directory-rejected ds.describe-directories-absent-id-rejected ds.describe-directories-invalid-next-token ds.list-tags-for-nonexistent-directory ds.update-domain-controllers-limit-enforced
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: ds.create-directory-rejects-noncompliant-password|_create_refused#5|AssertionError|raise AssertionError(
Issue-Key: ds.create-directory-member-validation-is-scoped|_create_refused#10|AssertionError|raise AssertionError(
Issue-Key: ds.create-alias-invalid-alias-rejected|run#5|AssertionError|assert bad_code == "ValidationException", f"expected ValidationException, got {bad_code!r}"
Issue-Key: ds.delete-nonexistent-directory-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ds.describe-directories-absent-id-rejected|run#11|AssertionError|assert single_message == expected_single, (
Issue-Key: ds.describe-directories-invalid-next-token|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: ds.list-tags-for-nonexistent-directory|run#6|AssertionError|assert message == expected, (
Issue-Key: ds.update-domain-controllers-limit-enforced|_discover_domain_controller_limit#3|AssertionError|raise AssertionError(
Issue-Commit: 242e2a822ca6eb3fec2aad3ba89160f0a6f40669

# ds: directory creation skips password and VPC validation, CreateAlias validates after resolution, error envelopes carry no RequestId and no ds Service Quotas are published (5 causes)

> _Note: this issue was identified against the code base at commit `242e2a822ca6eb3fec2aad3ba89160f0a6f40669`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDirectory, CreateMicrosoftAD, CreateAlias, DescribeDirectories, ListTagsForResource, DeleteDirectory

## Description
Five independent root causes in the Directory Service surface, all reached by requests the emulator does answer (no stub involved). Five causes; each lists its own failing check ids.

Cause 1 of 5: CreateDirectory / CreateMicrosoftAD do not enforce the modeled Password constraint.
AWS: Smithy: CreateDirectoryRequest.Password is @required, @sensitive and carries @pattern ^(?=^.{8,64}$)((?=.*\d)(?=.*[A-Z])(?=.*[a-z])|(?=.*\d)(?=.*[^A-Za-z0-9\s])(?=.*[a-z])|(?=.*[^A-Za-z0-9\s])(?=.*[A-Z])(?=.*[a-z])|(?=.*\d)(?=.*[A-Z])(?=.*[^A-Za-z0-9\s]))^ - i.e. 8-64 characters and at least three of the four classes {digit, upper, lower, symbol}; CreateMicrosoftAD carries the same Password shape. A violation is refused at the validation layer with HTTP 400 ValidationException; because the member is @sensitive the value is elided (recorded example: "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: ..."). The message rule is derivable from the request (member name + the modeled pattern, value redacted); the recorded text is an example of it.
Emulator: a 7-character password ("Sim2#Re") is ACCEPTED and a directory is created (DirectoryId d-003ba5fe5a returned).
Why: a client that relies on the service to reject a non-compliant admin password gets a directory it cannot administer, and the whole negative class (too short, too few character classes) is silently permitted, so misconfigurations that AWS blocks reach state.
Fix: input validation for the Password member shared by the CreateDirectory and CreateMicrosoftAD handlers (pattern enforcement must run before any state mutation), plus the validation-error serializer that must elide a @sensitive value.
Traces: ds.create-directory-rejects-noncompliant-password.

Cause 2 of 5: CreateDirectory / CreateMicrosoftAD never resolve VpcSettings, so a request naming a VPC and subnets that do not exist succeeds.
AWS: Smithy: CreateDirectory errors ClientException, DirectoryLimitExceededException, InvalidParameterException (client, HTTP 400). After input validation the request is resolved against EC2 and a non-existent VPC is refused with ClientException (recorded example message from CreateMicrosoftAD: "Invalid VPC ID."); nothing is created.
Emulator: CreateDirectory with VpcSettings={VpcId: vpc-0123456789abcdef0, SubnetIds: [subnet-0123456789abcdef0, subnet-0fedcba9876543210]} - ids that match their patterns but name nothing - returns success and creates a directory.
Why: resource resolution is the step that proves a directory is actually placed in a network; skipping it means every well-shaped request "succeeds", so a caller cannot detect a wrong/absent VPC and later network-dependent calls (DNS addresses, domain controllers, ENIs) have nothing real behind them. It also makes the refusal/acceptance boundary untestable: both checks below use the absent-VPC refusal as their post-validation control.
Additionally observed in this run: two different CreateDirectory calls, with different Name values, both returned DirectoryId 'd-003ba5fe5a'. AWS issues a distinct id per directory (output DirectoryId @pattern ^d-[0-9a-f]{10}$). Hypothesis: the id is derived from a fixed seed/counter rather than per-resource.
Fix: the CreateDirectory/CreateMicrosoftAD handlers' resource-resolution step (look up VpcId/SubnetIds in the emulator's EC2 state and refuse with ClientException when absent) and the id allocation in the directory state store.
Traces: ds.create-directory-member-validation-is-scoped, ds.create-directory-rejects-noncompliant-password.

Cause 3 of 5: CreateAlias resolves the directory before validating the Alias member.
AWS: Smithy: CreateAliasRequest.Alias targets AliasName with @length 1-62 and @pattern ^(?!D-|d-)([\da-zA-Z]+)([-]*[\da-zA-Z])*$; a pattern violation is a validation failure (HTTP 400 ValidationException) and is produced BEFORE the directory lookup, so a malformed alias against a never-created directory id still yields ValidationException rather than EntityDoesNotExistException (which the same operation models for the well-formed-alias case).
Emulator: ds.create_alias(DirectoryId='d-<10 hex>', Alias='SIM2REAL_Invalid_Alias') (underscores violate the pattern) returns EntityDoesNotExistException.
Why: the validate-then-resolve ordering is how clients tell "your input is wrong" from "the resource is gone"; inverting it makes a caller retry/recreate a directory in response to what is really a malformed-alias bug, and the two error families become indistinguishable.
Fix: the CreateAlias handler - run member validation (length + pattern) ahead of directory resolution.
Traces: ds.create-alias-invalid-alias-rejected.

Cause 4 of 5: ds error envelopes carry no RequestId, and the not-found / invalid-token diagnostics are built from a different rule than AWS's.
AWS: ds error responses always carry a request id (surfaced by botocore as ResponseMetadata.RequestId) and ds appends it to the Error.Message as " : RequestId: <id>". The message bodies are request-derived and operation-family specific - recorded examples: DeleteDirectory on an absent id -> EntityDoesNotExistException "Directory: 'd-0123456789' does not exist. : RequestId: <id>"; DescribeDirectories with an absent id in DirectoryIds -> "Invalid directory ID(s): d-37e08017a6. : RequestId: <id>"; DescribeDirectories with a bad NextToken -> "Invalid value passed for the NextToken parameter. : RequestId: <id>"; ListTagsForResource on an absent directory -> "Invalid resource ID. : RequestId: <id>". The hard requirements here are the presence of a request id in the envelope and the message following the request-derived rule; the exact recorded strings are examples.
Emulator: every one of these refusals comes back with ResponseMetadata.RequestId absent (the checks interpolate "RequestId: None") and with a single generic message shape instead of the per-family one - "Directory d-0123456789 does not exist" (no quoting, no period, no RequestId suffix), "Directory d-37e08017a6 does not exist" where AWS says "Invalid directory ID(s): ...", "Directory d-7730a88050 does not exist" where the tag API says "Invalid resource ID.", and "The NextToken value is not valid." for the pagination-token refusal.
Why: a missing request id breaks the correlation every AWS client, SDK retry log and support workflow relies on, and a single generic not-found diagnostic collapses distinct AWS error families (invalid directory id list vs. invalid resource id vs. absent directory) that callers and log scrapers key on.
Fix: the ds error serializer/envelope (emit the request id in the response and in the message suffix) and the per-operation not-found/invalid-token message construction in the DescribeDirectories, DeleteDirectory and ListTagsForResource handlers.
Traces: ds.delete-nonexistent-directory-rejected, ds.describe-directories-absent-id-rejected, ds.describe-directories-invalid-next-token, ds.list-tags-for-nonexistent-directory.

Cause 5 of 5: Service Quotas publishes no quotas for ServiceCode "ds", so the adjustable domain-controller limit cannot be discovered.
AWS: recorded behavior only - ListServiceQuotas(ServiceCode='ds') and ListAWSDefaultServiceQuotas(ServiceCode='ds') return the Directory Service quota catalog, including the per-directory domain-controller quota whose Value the caller then enforces against (it is adjustable, so it is published rather than fixed).
Emulator: both paginated calls return successfully with no Quotas entry matching a domain-controller quota for ServiceCode 'ds' (no ClientError - the catalog is simply empty for this service code), so the limit is undiscoverable and the dependent UpdateNumberOfDomainControllers check cannot run.
Why: a client that discovers a quota before sizing a request gets nothing back and must hardcode a guess; any quota-driven workflow for ds silently degrades.
Fix: the Service Quotas quota catalog/state for the 'ds' service code (the data the ListServiceQuotas / ListAWSDefaultServiceQuotas handlers read).
Traces: ds.update-domain-controllers-limit-enforced.

## Evidence
From the behavioral replay (ds 0/145 passed):
- ds.create-directory-rejects-noncompliant-password: "'sim2real-behav-b2536105b174.example.com' was ACCEPTED (DirectoryId='d-003ba5fe5a'); a refusal was expected" - the password leg under test was "Sim2#Re" (7 chars, below the modeled 8-64 minimum); expected HTTP 400 ValidationException with the value elided.
- ds.create-directory-member-validation-is-scoped: "CreateDirectory unexpectedly succeeded for 'sim2real-behav-0b2cf8d5ff21': DirectoryId='d-003ba5fe5a'" - this is the leg whose Name/ShortName/Size are all valid and whose VpcSettings name an absent VPC; AWS answers 400 ClientException (example message "Invalid VPC ID.") and creates nothing. Note the identical DirectoryId returned to a different Name in the previous check.
- ds.create-alias-invalid-alias-rejected: "expected ValidationException, got 'EntityDoesNotExistException'" for Alias='SIM2REAL_Invalid_Alias' against a well-formed, never-created DirectoryId.
- ds.delete-nonexistent-directory-rejected: expected message "Directory: 'd-0123456789' does not exist. : RequestId: <rid>", got 'Directory d-0123456789 does not exist' with ResponseMetadata.RequestId = None.
- ds.describe-directories-absent-id-rejected: expected 'Invalid directory ID(s): d-37e08017a6. : RequestId: <rid>', got 'Directory d-37e08017a6 does not exist' (RequestId None).
- ds.describe-directories-invalid-next-token: expected 'Invalid value passed for the NextToken parameter. : RequestId: <rid>', got 'The NextToken value is not valid.' (RequestId None).
- ds.list-tags-for-nonexistent-directory: ListTagsForResource(ResourceId='d-7730a88050') expected 'Invalid resource ID. : RequestId: <rid>', got 'Directory d-7730a88050 does not exist' (RequestId None).
- ds.update-domain-controllers-limit-enforced: "Service Quotas does not list a 'domain controller' quota entry for the ds service in this account/region" after paginating ListServiceQuotas and ListAWSDefaultServiceQuotas with ServiceCode='ds'.

## Steps to Reproduce
1. Boot the emulator; point botocore `ds` and `service-quotas` clients at it.
2. Password: ds.create_directory(Name='x-y.example.com', ShortName='SIM2REAL', Password='Sim2#Re', Size='Small', VpcSettings={'VpcId':'vpc-0123456789abcdef0','SubnetIds':['subnet-0123456789abcdef0','subnet-0fedcba9876543210']}) -> expect 400 ValidationException with the password value elided; observe success with a DirectoryId. Repeat with ds.create_microsoft_ad(... Edition='Standard' ...).
3. VPC resolution: repeat step 2 with a fully compliant Password ('Sim2Real#Pass99') -> expect 400 ClientException (example message 'Invalid VPC ID.') and no directory; observe success. Run the call twice with different Name values and compare the returned DirectoryId values.
4. CreateAlias ordering: ds.create_alias(DirectoryId='d-0123456789', Alias='SIM2REAL_Invalid_Alias') -> expect 400 ValidationException naming 'alias' and the modeled pattern; observe EntityDoesNotExistException.
5. Error envelope: ds.delete_directory(DirectoryId='d-0123456789'), ds.describe_directories(DirectoryIds=['d-37e08017a6']), ds.describe_directories(NextToken='not-a-token'), ds.list_tags_for_resource(ResourceId='d-7730a88050') -> inspect e.response['ResponseMetadata']['RequestId'] (absent) and Error.Message (generic, no ' : RequestId: ' suffix).
6. Quotas: paginate service-quotas list_service_quotas(ServiceCode='ds') and list_aws_default_service_quotas(ServiceCode='ds') -> no domain-controller quota entry is returned.
