Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: customer-profiles.cp-create-domain-rejects-a-duplicate-domain-name customer-profiles.cp-create-profile-rejects-a-nonexistent-domain customer-profiles.cp-create-profile-rejects-a-partytype-outside-the-enum customer-profiles.cp-domain-stats-profilecount-converges-to-the-profiles-created customer-profiles.cp-tag-resource-after-create-adds-to-the-tags-set-at-create customer-profiles.cp-domain-tags-at-create-round-trip-through-get-domain-and-list-tags customer-profiles.cp-domain-unset-optional-fields-stay-absent customer-profiles.cp-predefined-keys-normalise-fullname-and-phone-but-not-account customer-profiles.cp-search-profiles-rejects-a-raw-field-name-as-keyname customer-profiles.cp-search-profiles-rejects-more-than-one-value-per-key customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected
Issue-Shape-Id: com.amazonaws.customerprofiles#CustomerProfiles_20200815
Issue-Key: customer-profiles.cp-create-domain-rejects-a-duplicate-domain-name|run#6|AssertionError|assert got == ("BadRequestException", expected_message), (
Issue-Key: customer-profiles.cp-create-profile-rejects-a-nonexistent-domain|run#11|AssertionError|assert (code, message) == ("ResourceNotFoundException", expected_message), (
Issue-Key: customer-profiles.cp-create-profile-rejects-a-partytype-outside-the-enum|run#11|AssertionError|assert got == ("BadRequestException", expected_message), (
Issue-Key: customer-profiles.cp-domain-stats-profilecount-converges-to-the-profiles-created|run#13|AssertionError|assert stats["ObjectCount"] == 2, (
Issue-Key: customer-profiles|TagResource|BadRequestException
Issue-Key: customer-profiles|ListTagsForResource|BadRequestException
Issue-Key: customer-profiles.cp-domain-unset-optional-fields-stay-absent|run#5|KeyError|assert got["Tags"] == {}, (
Issue-Key: customer-profiles.cp-predefined-keys-normalise-fullname-and-phone-but-not-account|run#9|AssertionError|assert by_name[0]["FoundByItems"] == [{"KeyName": "_fullName", "Values": ["grace hopper"]}], (
Issue-Key: customer-profiles.cp-search-profiles-rejects-a-raw-field-name-as-keyname|_error#8|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it, so there is no error")
Issue-Key: customer-profiles.cp-search-profiles-rejects-more-than-one-value-per-key|_client_error#10|AssertionError|raise AssertionError(f"the call succeeded and returned {result!r}; real AWS rejects it")
Issue-Key: customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected|run#8|AssertionError|assert message == "Can't specify fields or keys together with a templateId", message
Issue-Commit: 75d52ea0c257a701244c018d7c3f550d51e1d46f

# customer-profiles: 10 net-new behavioral causes — refusal messages are emulator-authored, SearchProfiles accepts input AWS refuses, the Tag ARN gate refuses valid domain ARNs, and GetDomain omits Tags/ObjectCount

> _Note: this issue was identified against the code base at commit `75d52ea0c257a701244c018d7c3f550d51e1d46f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SearchProfiles

## Description
Ten net-new root causes in the customer-profiles crate, all reached through operations the emulator DOES serve (every verdict below is a [mismatch] or an emulator-emitted 400 — none is a scaffold stub). They fall into three families: refusal MESSAGES/verdicts the emulator authors itself, validation gates that are absent so the emulator answers 200 where AWS refuses, and GetDomain projection members that are never populated.

Cause 1 of 10: CreateDomain's duplicate-name refusal message carries an extra clause.
AWS: Smithy: CreateDomain models BadRequestException (httpError 400) and DomainName is `name` (@length 1..64, @pattern ^[a-zA-Z0-9_-]+$). Recorded behavior: re-creating an existing domain is refused with HTTP 400, error code `BadRequestException`, message `Domain <DomainName> already exists`. The fixed text of that message is `Domain ` + ` already exists`; the only variable part is the DomainName taken from the request. There is no trailing advice clause.
Emulator: same code, but the message is `Domain <DomainName> already exists; use UpdateDomain to change it` — the emulator appends `; use UpdateDomain to change it`, which AWS never emits.
Why: clients and test harnesses that assert on or parse the duplicate-domain sentence (a very common idempotency branch: treat 'already exists' as success) see a message that does not match AWS's, so exact-match handling breaks against the emulator while working against AWS.
Fix: belongs in the CreateDomain handler's duplicate-name refusal (src/ops/create_domain.rs) — emit only the fixed sentence with the request's DomainName interpolated.
Traces: customer-profiles.cp-create-domain-rejects-a-duplicate-domain-name.

Cause 2 of 10: CreateProfile's domain-not-found message is the emulator's own wording and never names the account.
AWS: Smithy: CreateProfile (POST /domains/{DomainName}/profiles) models ResourceNotFoundException with httpError 404. Recorded behavior: a CreateProfile naming a domain that does not exist is refused with error code `ResourceNotFoundException` and message `DomainName <DomainName> is not found in account <accountId>`. Fixed text: `DomainName ` ... ` is not found in account `; variable parts: the DomainName from the request path and the 12-digit account id of the calling credentials, from the emulator's own request scope.
Emulator: `The domain <DomainName> does not exist` — different sentence shape, and the account id is dropped entirely.
Why: the account id in AWS's sentence is what tells an operator the call landed in the wrong account rather than against a missing domain; a client logging or matching the AWS sentence loses that discrimination, and message-matching error handling diverges.
Fix: belongs in the shared domain-lookup refusal used by CreateProfile (src/ops/create_profile.rs and the crate's domain-not-found helper) — build the message from the requested DomainName plus the resolved account id.
Traces: customer-profiles.cp-create-profile-rejects-a-nonexistent-domain.

Cause 3 of 10: the PartyType enum-validation message echoes the sensitive value and re-sorts the enum set.
AWS: Smithy: CreateProfileRequest$PartyType targets `PartyType`, an enum marked @sensitive (and @deprecated) whose values are INDIVIDUAL, BUSINESS, OTHER. Recorded behavior: an off-enum PartyType is refused server-side with HTTP 400, code `BadRequestException`, message `1 validation error detected: Value at 'partyType' failed to satisfy constraint: Member must satisfy enum value set: [BUSINESS, OTHER, INDIVIDUAL]`. Two things are requirements here: the offending value is NOT echoed (the member is @sensitive, so AWS prints `Value at '<member>'`, not `Value '<x>' at '<member>'`), and the enum set is rendered in the order AWS uses, `[BUSINESS, OTHER, INDIVIDUAL]`. Everything in that sentence is fixed text apart from the member path `partyType`.
Emulator: `1 validation error detected: Value 'ROBOT' at 'partyType' failed to satisfy constraint: Member must satisfy enum value set: [INDIVIDUAL, BUSINESS, OTHER]` — it interpolates the rejected value (leaking a value from a @sensitive member into an error string) and prints the enum in model-declaration order.
Why: the echoed value is a redaction defect — any caller that forwards emulator errors to logs now records data AWS deliberately withholds for sensitive members; and clients asserting on the validation sentence see both a surplus value and a different enum ordering.
Fix: belongs in the generated/validation layer that renders constraint violations for this crate (the CreateProfile input validation, src/ops/create_profile.rs, and the shared constraint-message builder): suppress the value for @sensitive members and use AWS's enum ordering for the set.
Traces: customer-profiles.cp-create-profile-rejects-a-partytype-outside-the-enum.

Cause 4 of 10: GetDomain's Stats.ObjectCount is never incremented by profile writes.
AWS: Smithy: DomainStats members ProfileCount, MeteringProfileCount, ObjectCount, TotalSize are all `long` with default 0; ObjectCount is documented as 'The total number of objects in domain' and TotalSize as 'The total size, in bytes, of all objects in the domain'. Recorded behavior: after two CreateProfile calls into a fresh domain, GetDomain's Stats converges to ProfileCount 2, MeteringProfileCount 2, ObjectCount 2 (each profile is one object) and TotalSize > 0.
Emulator: ProfileCount and MeteringProfileCount do converge to 2, but Stats.ObjectCount stays 0 (the assertion that failed), so TotalSize is never reached either — the object/size accounting is not wired to the profile store at all.
Why: ObjectCount and TotalSize are the usage/metering view of a domain; a client that sizes ingestion, bills, or decides whether to shard a domain reads a permanent zero and concludes the domain is empty while it holds profiles.
Fix: belongs in the domain Stats computation behind GetDomain (src/ops/get_domain.rs and the crate's domain state) — derive ObjectCount and TotalSize from the stored profiles/objects rather than leaving them at their defaults.
Traces: customer-profiles.cp-domain-stats-profilecount-converges-to-the-profiles-created.

Cause 5 of 10: TagResource and ListTagsForResource refuse a well-formed Customer Profiles domain ARN.
AWS: Smithy: TagResource (POST /tags/{resourceArn}) and ListTagsForResource (GET /tags/{resourceArn}) take resourceArn as a @httpLabel targeting `TagArn` (@length 0..256, pattern `^arn:[a-z0-9]{1,10}:profile$` as carried in this repo's model). Recorded behavior: AWS ACCEPTS the domain ARN `arn:aws:profile:<region>:<accountId>:domains/<DomainName>` on both operations — tagging a Customer Profiles domain is exactly what these operations exist for — and answers 200, with ListTagsForResource returning the tags set at CreateDomain. The modelled pattern is therefore not applied as an anchored full-string match by the service (as written it would match no real ARN at all, since every real value continues past `:profile`).
Emulator: both operations answer HTTP 400 `BadRequestException` with `1 validation error detected: Value '<arn>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[a-z0-9]{1,10}:profile$` — the model pattern is enforced as an anchored match, so EVERY valid domain ARN is rejected and the whole tagging surface is unreachable.
Why: no caller can tag or read tags on a domain: tag-based cost allocation, tag-driven cleanup sweeps and `ListTagsForResource` round-trips all fail with a 400 that AWS never returns, and the request looks malformed when it is correct.
Fix: belongs in the resourceArn constraint check shared by the tagging handlers (src/ops/tag_resource.rs, src/ops/list_tags_for_resource.rs, src/ops/untag_resource.rs) — validate the ARN as a Customer Profiles resource ARN (prefix `arn:<partition>:profile:...`) instead of requiring the model pattern to match the whole value.
Traces: customer-profiles.cp-tag-resource-after-create-adds-to-the-tags-set-at-create, customer-profiles.cp-domain-tags-at-create-round-trip-through-get-domain-and-list-tags.

Cause 6 of 10: GetDomain omits the Tags member for an untagged domain instead of returning it present and empty.
AWS: Smithy: GetDomainResponse$Tags targets TagMap (map, @length 1..50) and is optional; the other optional members (DefaultEncryptionKey, DeadLetterQueueUrl, Matching, RuleBasedMatching) are omitted when unset. Recorded behavior only: on a domain created with no Tags, GetDomain returns `Tags` PRESENT and empty (`{}`) — it is not absent and not null — while the genuinely unset members above are absent from the response entirely.
Emulator: the response has no `Tags` key at all for an untagged domain, so reading it raises KeyError in the client; the sparse-vs-present distinction AWS draws between Tags and the other optional members is lost.
Why: SDK callers that index `response['Tags']` (safe against AWS) crash against the emulator, and code that distinguishes 'domain has no tags' from 'tags not reported' cannot tell the two apart.
Fix: belongs in the GetDomain response projection (src/ops/get_domain.rs) — always serialize Tags, empty when the domain carries none, while continuing to omit the unset optional members.
Traces: customer-profiles.cp-domain-unset-optional-fields-stay-absent.

Cause 7 of 10: predefined search keys are indexed verbatim, so FoundByItems reports the value as written instead of the normalized form.
AWS: Smithy: SearchProfiles returns Items[].FoundByItems as FoundByKeyValue{KeyName, Values}, documented as 'the KeyName and Values list that were used to find a profile'. Recorded behavior only: the predefined `_fullName` index is lowercased and the predefined `_phone` index has punctuation stripped, and FoundByItems names the STORED form — a profile created with FirstName 'Grace'/LastName 'Hopper' searched by `_fullName` 'Grace Hopper' comes back with FoundByItems `[{KeyName: _fullName, Values: ['grace hopper']}]`, and one created with PhoneNumber '+15550100' searched by `_phone` comes back with `Values: ['15550100']` (no '+'), while `_account` is NOT normalized and matches only the exact value (no prefix match).
Emulator: the search succeeds but FoundByItems echoes the value as written — `[{KeyName: _fullName, Values: ['Grace Hopper']}]` — so the per-key normalization that distinguishes _fullName/_phone from _account is not applied to the stored index.
Why: FoundByItems is how a client learns WHICH stored key matched, e.g. to deduplicate profiles or to display the canonical indexed value; echoing the request value makes every search look like an exact hit and hides the case/punctuation folding callers depend on when they themselves normalize before comparing.
Fix: belongs in the profile key-indexing state and the SearchProfiles FoundByItems projection (src/ops/create_profile.rs key indexing, src/ops/search_profiles.rs) — lowercase _fullName, strip punctuation for _phone, leave _account exact, and report the stored form.
Traces: customer-profiles.cp-predefined-keys-normalise-fullname-and-phone-but-not-account.

Cause 8 of 10: SearchProfiles accepts a raw field name as KeyName and answers 200 instead of refusing an undefined key.
AWS: Smithy: SearchProfiles models ResourceNotFoundException with httpError 404; KeyName is `name` (@length 1..64, @pattern ^[a-zA-Z0-9_-]+$) and must name a key DEFINED in the domain (a predefined key such as `_account`, or a key declared by an object type) — the raw profile FIELD name is not one. Recorded behavior: SearchProfiles with KeyName='AccountNumber' is refused with error code `ResourceNotFoundException` and message `KeyName AccountNumber is not defined in the domain`; fixed text `KeyName ` + ` is not defined in the domain`, with the rejected key name taken verbatim from the request.
Emulator: the call SUCCEEDS (HTTP 200) — the check's `_error()` helper reports the call returned instead of raising — so an undefined key silently behaves like a defined one.
Why: 'wrong key' and 'no results' become indistinguishable. A client that searches by the field name instead of the predefined key gets a plausible answer (or a plausible empty page) from the emulator and a 404 from AWS, so the bug is only discovered in production.
Fix: belongs in SearchProfiles' input resolution (src/ops/search_profiles.rs) — resolve KeyName against the domain's predefined and object-type-declared keys and refuse ResourceNotFoundException/404 with the sentence above before any matching is done.
Traces: customer-profiles.cp-search-profiles-rejects-a-raw-field-name-as-keyname.

Cause 9 of 10: SearchProfiles accepts more than one element in Values instead of refusing BadRequestException.
AWS: Smithy: SearchProfilesRequest$Values is a required `requestValueList` (list of string1To255) and SearchProfiles models BadRequestException with httpError 400; the modelled list carries no max, so the one-value-per-key rule is enforced server-side. Recorded behavior: three values for one KeyName is refused with code `BadRequestException` and message `Expected 1 values for key name <KeyName> but got <n>` — fixed text `Expected 1 values for key name ` + ` but got `, with the KeyName from the request and n the ACTUAL length of the submitted Values list (not pinned to any one number).
Emulator: the call succeeds with HTTP 200 and returns the profile matched by the first value (the replay shows a 200 with one Item and FoundByItems naming `_account`), so the surplus values are silently ignored.
Why: a client that batches several values into one key believes it searched for all of them; it gets results for one and no error, so missing matches look like absent data rather than a rejected request — and the same code raises BadRequestException against AWS.
Fix: belongs in SearchProfiles' input validation (src/ops/search_profiles.rs), ahead of matching — refuse BadRequestException/400 with the message above whenever Values holds other than exactly one element for a key name.
Traces: customer-profiles.cp-search-profiles-rejects-more-than-one-value-per-key.

Cause 10 of 10: PutProfileObjectType's TemplateId-conflict refusal message is emulator-authored.
AWS: Smithy: PutProfileObjectType (PUT /domains/{DomainName}/object-types/{ObjectTypeName}) models BadRequestException with httpError 400, and PutProfileObjectTypeRequest$TemplateId's documentation names AllowProfileCreation, SourceLastUpdatedTimestampFormat, Fields and Keys as the attributes that must not accompany it. Recorded behavior: a request carrying TemplateId together with Fields and Keys is refused with code `BadRequestException` and message `Can't specify fields or keys together with a templateId` — that sentence is entirely FIXED text for this conflict class; it interpolates no request value and does not list the offending member names.
Emulator: the code is now correct (`BadRequestException`) but the message is `The following attributes cannot be present when TemplateId is specified: Fields, Keys` — a sentence of the emulator's own composition that enumerates the request's members. Note the earlier FIXED issue 20261029T183204Z-customer-profiles-2-net-new-root-causes-mergeprofiles.md (its Cause 2) stood this refusal up; the verdict it asked for is now reached, so this is NOT a reopened refusal — only the wording remains divergent, and the check still fails on it.
Why: the refusal is the documented way to learn that template-based and explicit object-type configuration are mutually exclusive; a client or test asserting AWS's sentence does not match, and a message that enumerates members invites callers to parse a list AWS never sends.
Fix: belongs in the PutProfileObjectType handler's TemplateId mutual-exclusivity check (src/ops/put_profile_object_type.rs) — emit the fixed sentence above with the BadRequestException/400 it already returns.
Traces: customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, customer-profiles 14/45 passed (31%). All ten causes below are failures the emulator itself produced (served the call and answered wrongly); none is marked ALREADY FILED or OWNED.

Cause 1 — customer-profiles.cp-create-domain-rejects-a-duplicate-domain-name [mismatch]
  expected: ('BadRequestException', 'Domain <DomainName> already exists')
  actual:   ('BadRequestException', 'Domain <DomainName> already exists; use UpdateDomain to change it')
  fixed text = 'Domain ' / ' already exists'; variable part = DomainName from the request.

Cause 2 — customer-profiles.cp-create-profile-rejects-a-nonexistent-domain [mismatch]
  expected: ('ResourceNotFoundException', 'DomainName <DomainName> is not found in account <accountId>')
  actual:   ('ResourceNotFoundException', 'The domain <DomainName> does not exist')
  fixed text = 'DomainName ' / ' is not found in account '; variable parts = requested DomainName, caller account id.

Cause 3 — customer-profiles.cp-create-profile-rejects-a-partytype-outside-the-enum [mismatch]
  expected: ('BadRequestException', "1 validation error detected: Value at 'partyType' failed to satisfy constraint: Member must satisfy enum value set: [BUSINESS, OTHER, INDIVIDUAL]")
  actual:   ('BadRequestException', "1 validation error detected: Value '<submitted value>' at 'partyType' failed to satisfy constraint: Member must satisfy enum value set: [INDIVIDUAL, BUSINESS, OTHER]")
  model: com.amazonaws.customerprofiles#PartyType is @sensitive, values INDIVIDUAL/BUSINESS/OTHER.

Cause 4 — customer-profiles.cp-domain-stats-profilecount-converges-to-the-profiles-created [mismatch]
  assertion: 'Stats.ObjectCount is 0; each profile is one object, so 2 profiles is 2 objects'
  ProfileCount and MeteringProfileCount reached 2 (those assertions passed); ObjectCount stayed 0, so TotalSize > 0 was never evaluated.

Cause 5 — customer-profiles.cp-tag-resource-after-create-adds-to-the-tags-set-at-create [error] and customer-profiles.cp-domain-tags-at-create-round-trip-through-get-domain-and-list-tags [error]
  expected: HTTP 200 for TagResource / ListTagsForResource on 'arn:aws:profile:<region>:<accountId>:domains/<DomainName>'
  actual:   HTTP 400 BadRequestException "1 validation error detected: Value '<arn>' at 'resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:[a-z0-9]{1,10}:profile$" (raised by botocore.errorfactory.BadRequestException on TagResource and on ListTagsForResource).

Cause 6 — customer-profiles.cp-domain-unset-optional-fields-stay-absent [error]
  expected: GetDomain on an untagged domain returns Tags present and empty ({}); DefaultEncryptionKey, DeadLetterQueueUrl, Matching, RuleBasedMatching absent.
  actual:   KeyError: 'Tags' — the member is missing from the GetDomain response.

Cause 7 — customer-profiles.cp-predefined-keys-normalise-fullname-and-phone-but-not-account [mismatch]
  expected: FoundByItems == [{'KeyName': '_fullName', 'Values': ['grace hopper']}] (lowercased stored form); _phone stored without '+'.
  actual:   [{'KeyName': '_fullName', 'Values': ['Grace Hopper']}] — value as written.

Cause 8 — customer-profiles.cp-search-profiles-rejects-a-raw-field-name-as-keyname [mismatch]
  expected: ('ResourceNotFoundException', 'KeyName <KeyName> is not defined in the domain') for KeyName='AccountNumber'.
  actual:   AssertionError 'the call succeeded; real AWS rejects it, so there is no error' — HTTP 200.

Cause 9 — customer-profiles.cp-search-profiles-rejects-more-than-one-value-per-key [mismatch]
  expected: ('BadRequestException', 'Expected 1 values for key name <KeyName> but got <n>') with n = len(Values).
  actual:   HTTP 200 with one Item and FoundByItems naming the first value's key — 'the call succeeded ... real AWS rejects it'.

Cause 10 — customer-profiles.put-profile-object-type-template-and-fields-conflict-rejected [mismatch]
  expected: ('BadRequestException', "Can't specify fields or keys together with a templateId")
  actual:   ('BadRequestException', 'The following attributes cannot be present when TemplateId is specified: Fields, Keys')
  the error CODE is correct here; only the sentence diverges.

## Steps to Reproduce
All steps use a fresh domain name per run.

Cause 1: CreateDomain(DomainName=D, DefaultExpirationDays=366); CreateDomain(DomainName=D, ...) again -> expect 400 BadRequestException 'Domain D already exists'; observe the extra '; use UpdateDomain to change it'.

Cause 2: CreateProfile(DomainName=<a domain that was never created>, AccountNumber=A) -> expect 404 ResourceNotFoundException 'DomainName <name> is not found in account <callers account id>'; observe 'The domain <name> does not exist'.

Cause 3: CreateDomain(D); CreateProfile(DomainName=D, PartyType='ROBOT') -> expect 400 BadRequestException "1 validation error detected: Value at 'partyType' failed to satisfy constraint: Member must satisfy enum value set: [BUSINESS, OTHER, INDIVIDUAL]"; observe the submitted value echoed and the enum re-ordered.

Cause 4: CreateDomain(D); CreateProfile(D, AccountNumber=A1); CreateProfile(D, AccountNumber=A2); poll GetDomain(D)['Stats'] -> ProfileCount and MeteringProfileCount reach 2 but ObjectCount stays 0 (expected 2) and TotalSize stays 0 (expected > 0).

Cause 5: CreateDomain(D, Tags={'k':'v'}); TagResource(resourceArn='arn:aws:profile:<region>:<account>:domains/D', tags={'k2':'v2'}) -> expect 200; observe 400 BadRequestException naming pattern ^arn:[a-z0-9]{1,10}:profile$. Same for ListTagsForResource(resourceArn=<same ARN>), which must return the tags set at CreateDomain.

Cause 6: CreateDomain(D, DefaultExpirationDays=366) with NO Tags; GetDomain(D) -> expect 'Tags' present and == {} (and DefaultEncryptionKey / DeadLetterQueueUrl / Matching / RuleBasedMatching absent); observe 'Tags' missing from the response.

Cause 7: CreateDomain(D); CreateProfile(D, AccountNumber=A, FirstName='Grace', LastName='Hopper', PhoneNumber='+15550100'); SearchProfiles(D, KeyName='_fullName', Values=['Grace Hopper']) -> expect FoundByItems Values ['grace hopper']; SearchProfiles(D, KeyName='_phone', Values=['+15550100']) -> expect Values ['15550100']; SearchProfiles(D, KeyName='_account', Values=[A[:-1]]) -> expect no items (exact, not prefix).

Cause 8: CreateDomain(D); CreateProfile(D, AccountNumber=A); confirm SearchProfiles(D, KeyName='_account', Values=[A]) finds it; then SearchProfiles(D, KeyName='AccountNumber', Values=[A]) -> expect 404 ResourceNotFoundException 'KeyName AccountNumber is not defined in the domain'; observe HTTP 200.

Cause 9: CreateDomain(D); CreateProfile(D, AccountNumber=A); SearchProfiles(D, KeyName='_account', Values=[A, A2, A3]) -> expect 400 BadRequestException 'Expected 1 values for key name _account but got 3'; observe HTTP 200 with the profile matched by the first value. The rejected call must leave the index untouched.

Cause 10: CreateDomain(D); PutProfileObjectType(D, ObjectTypeName='CTR', TemplateId='CTR-NoInferred', Fields={...}, Keys={...}) -> expect 400 BadRequestException "Can't specify fields or keys together with a templateId"; observe the emulator's 'The following attributes cannot be present when TemplateId is specified: Fields, Keys'. Then GetProfileObjectType(D,'CTR') must answer ResourceNotFoundException 'ObjectType CTR not found', and PutProfileObjectType with TemplateId alone must succeed and round-trip TemplateId.
