Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.get-reputation-entity-unknown-rejected sesv2.update-reputation-entity-policy-concurrent-writes-land-one-of-two sesv2.tenant-create-duplicate-name-rejected sesv2.configuration-set-event-destination-lifecycle sesv2.tenant-resource-association-delete-unknown-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.get-reputation-entity-unknown-rejected|run#8|AssertionError|assert err.get("Code") == "BadRequestException", f"expected BadRequestException, got {err!r}"
Issue-Key: sesv2.update-reputation-entity-policy-concurrent-writes-land-one-of-two|run#10|KeyError|assert baseline["ReputationManagementPolicy"] == standard_arn, (
Issue-Key: sesv2.tenant-create-duplicate-name-rejected|run#10|AssertionError|assert msg == expected_msg, f"expected message {expected_msg!r}, got {msg!r}"
Issue-Key: sesv2.configuration-set-event-destination-lifecycle|run#19|AssertionError|assert err == expected, f'expected {expected}, got {err}'
Issue-Key: sesv2.tenant-resource-association-delete-unknown-rejected|run#14|AssertionError|assert code == "NotFoundException", f"expected NotFoundException, got {code!r}"
Issue-Commit: 6e8bba18a5a906dda1323a55e22368d1363d6826

# sesv2: 5 net-new behavioral root causes in the reputation-entity, tenant and event-destination planes — GetReputationEntity refuses with the wrong error class and never reports a reputation-management policy, CreateTenant's duplicate message is reworded, the CloudWatch dimension-source rejection leaks a generic constraint message, and DeleteTenantResourceAssociation is still 501

> _Note: this issue was identified against the code base at commit `6e8bba18a5a906dda1323a55e22368d1363d6826`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetReputationEntity

## Description
Five distinct root causes, all in the SESv2 emulator, each grounded in its own failing behavioral check. Smithy model consulted: services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json.

=== Cause 1 of 5: GetReputationEntity rejects an unresolvable entity reference with NotFoundException (404) where AWS uses BadRequestException (400), and rewords the message ===
Failing check: sesv2.get-reputation-entity-unknown-rejected
AWS: Smithy GetReputationEntity (GET /v2/email/reputation/entities/{ReputationEntityType}/{ReputationEntityReference}) models both BadRequestException (httpError 400) and NotFoundException (httpError 404). Recorded behavior: when ReputationEntityReference is a well-formed ARN of the same shape as a real entity but does not resolve to a resource the account owns, AWS refuses with HTTP 400 BadRequestException and the message `ReputationEntity <REFERENCE> does not exist` — the reference itself is treated as invalid, because reputation tracking only exists for real, owned resources. Message template: the fixed text is `ReputationEntity <` ... `> does not exist` (no trailing period); the only variable part is REFERENCE, the ReputationEntityReference value taken verbatim from the request, wrapped in angle brackets.
Emulator: answers HTTP 404 `NotFoundException` with the message `Reputation entity <REFERENCE> does not exist.` — wrong error class, wrong status, and a message that is reworded (`Reputation entity` instead of `ReputationEntity`, a bare reference with no angle brackets, and a trailing period AWS does not emit).
Why: a client that distinguishes "bad input, do not retry with this reference" (400 BadRequestException) from "resource gone" (404 NotFoundException) takes the wrong branch — retry/backoff and create-on-404 logic both misfire, and any client matching the message text to extract the offending reference fails because the angle-bracket delimiters are absent. Note this is the same angle-bracket omission class already filed for configuration sets in 20261026T065823Z-sesv2-the-notfoundexception-message-for-a-missing-configuration.md; that issue covers GetConfigurationSet only and does not cover GetReputationEntity or the error-class swap.
Fix: the GetReputationEntity handler's reference-resolution path in the sesv2 crate's ops module — it must classify an unresolvable reference as BadRequestException and build the message from the AWS template, not fall through to the generic not-found helper.
Traces: sesv2.get-reputation-entity-unknown-rejected

=== Cause 2 of 5: GetReputationEntity omits ReputationManagementPolicy, so a reputation entity has no default policy and UpdateReputationEntityPolicy can never be observed ===
Failing check: sesv2.update-reputation-entity-policy-concurrent-writes-land-one-of-two
AWS: Smithy shape ReputationEntity carries member `ReputationManagementPolicy` (target AmazonResourceName, @length min 1), documented as "The Amazon Resource Name (ARN) of the reputation management policy applied to this entity. This is an Amazon Web Services Amazon SES-managed policy." Recorded behavior: a freshly created tenant's resource-type reputation entity is returned with ReputationManagementPolicy already populated with the SES-managed standard policy ARN — template `arn:aws:ses:REGION:aws:reputation-policy/standard`, whose only variable part is REGION (the request's region); `aws` is the fixed partition-owner account segment and `standard` is the fixed default policy name. After UpdateReputationEntityPolicy (PUT .../policy, modeled errors BadRequestException 400, ConflictException 409) the member reads back as the written ARN.
Emulator: GetReputationEntity returns a ReputationEntity object with no `ReputationManagementPolicy` key at all (the check dies with `KeyError: 'ReputationManagementPolicy'` on the baseline read, before any update is attempted). The entity is therefore policy-less and no write to the policy can be read back.
Why: the whole reputation-management-policy surface is unobservable. A client cannot read the policy in force, cannot verify a policy change it just made, and cannot implement the documented concurrency contract (a losing racer must be refused ConflictException while at least one write lands and the final value is one of the raced ARNs) because the final value is never readable. Code generated against the model will KeyError or null-deref on a member AWS always populates.
Fix: the sesv2 reputation-entity state/serializer — a reputation entity must be materialised with the SES-managed default policy ARN at creation of its underlying resource, GetReputationEntity's serializer must emit the member, and UpdateReputationEntityPolicy must persist into it with the modeled ConflictException on a lost race.
Traces: sesv2.update-reputation-entity-policy-concurrent-writes-land-one-of-two

=== Cause 3 of 5: CreateTenant's duplicate-name AlreadyExistsException message is reworded and drops the account id ===
Failing check: sesv2.tenant-create-duplicate-name-rejected
AWS: Smithy CreateTenant (POST /v2/email/tenants) models AlreadyExistsException with httpError 400. Recorded behavior: creating a tenant whose name is already taken is refused AlreadyExistsException with the message `Tenant with name NAME already exists in account ACCOUNT`. Message template: the fixed text is `Tenant with name ` + ` already exists in account ` (no colon after "name", no trailing period); the variable parts are NAME, the TenantName from the request, and ACCOUNT, the caller's 12-digit account id from emulator state.
Emulator: returns the correct code AlreadyExistsException but the message `Tenant with name: NAME already exists.` — it inserts a colon after `name`, appends a period, and omits the entire ` in account ACCOUNT` clause.
Why: the refusal is correctly classified but a client or operator parsing the message cannot recover which account the name is taken in — the distinguishing fact in a multi-account or cross-account tooling context — and any assertion or log-scraper written against AWS's wording does not match.
Fix: the CreateTenant handler's duplicate-name branch in the sesv2 crate's ops module — build the message from the AWS template, interpolating the request's TenantName and the caller's account id.
Traces: sesv2.tenant-create-duplicate-name-rejected

=== Cause 4 of 5: CreateConfigurationSetEventDestination leaks a generic Smithy constraint-violation message instead of AWS's CloudWatch dimension-source refusal ===
Failing check: sesv2.configuration-set-event-destination-lifecycle
AWS: Smithy CreateConfigurationSetEventDestination models BadRequestException (httpError 400). CloudWatchDimensionConfiguration.DimensionValueSource is @required and targets enum DimensionValueSource with values [MESSAGE_TAG, EMAIL_HEADER, LINK_TAG]. Recorded behavior: a DimensionValueSource whose casing does not match the enum (e.g. the wire-doc spelling `messageTag`) is refused BadRequestException with the message `invalid CloudWatch dimension source` — fixed text with no variable part, i.e. AWS does not echo the offending value or a member path.
Emulator: returns the right code (BadRequestException) but a generic framework-level constraint message of the form `1 validation error detected: Value 'VALUE' at 'eventDestination.cloudWatchDestination.dimensionConfigurations.N.member.dimensionValueSource' failed to satisfy constraint: Member must satisfy enum value set: [MESSAGE_TAG, EMAIL_HEADER, LINK_TAG]` — a different error-message class entirely, echoing the rejected value and a synthetic `...N.member....` member path AWS never emits for this input.
Why: the emulator substitutes a generic deserialization-layer constraint report for a service-specific semantic refusal. A client matching AWS's wording to distinguish "the dimension source is not a legal source" from the many other BadRequestException causes on this operation cannot do so, and the leaked member path (including the `.member.` segment and the 1-based index) is not a stable AWS surface, so tests written against it diverge from real AWS in the other direction. The earlier calls in the lifecycle (create, get, delete of a valid CloudWatch event destination) now succeed, so this is the only remaining divergence in the check — the unimplemented-stub issue 20261026T044445Z-sesv2-createconfigurationseteventdestination-stub.md no longer covers it.
Fix: the CreateConfigurationSetEventDestination handler's validation of EventDestination.CloudWatchDestination.DimensionConfigurations in the sesv2 crate — reject an unmatched DimensionValueSource at the operation's own validation layer with AWS's fixed message rather than letting the shape-level enum constraint reporter answer.
Traces: sesv2.configuration-set-event-destination-lifecycle

=== Cause 5 of 5: DeleteTenantResourceAssociation answers 501 NotImplemented, so no tenant-resource association can ever be removed and the not-found precondition is never reached ===
Failing check: sesv2.tenant-resource-association-delete-unknown-rejected
AWS: Smithy DeleteTenantResourceAssociation (POST /v2/email/tenants/resources/delete, inputs TenantName and ResourceArn both @required @length min 1, empty output, code 200) models BadRequestException 400, NotFoundException 404 and TooManyRequestsException 429 — and notably NOT NotImplemented, which is not a modeled error for this operation at all. Recorded behavior: when ResourceArn names a configuration set that does not exist, the call is refused NotFoundException with the message `Configuration set <NAME> does not exist:` — fixed text `Configuration set <` + `> does not exist:` (note the trailing colon AWS emits), the one variable part being NAME, the configuration-set name parsed out of the request's ResourceArn. When the ResourceArn names a resource that DOES exist, the delete is an idempotent HTTP 200 with an empty body, whether or not an association is currently present — so a second delete of an already-removed association must also return 200, not a refusal.
Emulator: every call is refused with code `NotImplemented`, an unmodeled error. The check's first delete attempt gets `NotImplemented` instead of `NotFoundException`, and the two later legs (deleting the real association, then repeating it idempotently) are unreachable.
Why: tenant-resource associations are write-only — a client can create one but can never remove it, so tenant membership is permanently append-only and cleanup is impossible. NotImplemented is not in the operation's modeled error set, so a generated client's error matcher falls through to its unmodeled/unknown branch rather than any handled case. The report marked a different check blocked by this same stub (an auto-filed stub issue names the operation and services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/delete_tenant_resource_association.rs) but did not mark this one; this cause is filed for the refusal semantics that bare stub issue does not state — the NotFoundException class, its exact message template, the parse of the configuration-set name out of ResourceArn, and the idempotent-200 contract when the resource exists.
Fix: implement the DeleteTenantResourceAssociation handler in the sesv2 crate's ops module, with association removal in tenant state, the modeled NotFoundException for a non-existent underlying resource, and an idempotent 200 otherwise.
Traces: sesv2.tenant-resource-association-delete-unknown-rejected

All five causes must be closed before this issue is submitted; re-replay every check id in trace_ids.

## Evidence
Behavioral suite: 27/107 passed (25.2%); sesv2 27/83 with 56 failing. After skipping every check the report marked ALREADY FILED or OWNED, these five sesv2 checks survive, with the following replay output.

Cause 1 — sesv2.get-reputation-entity-unknown-rejected [mismatch]:
  Expected (AWS): Error.Code 'BadRequestException' (HTTP 400), Error.Message 'ReputationEntity <REFERENCE> does not exist'
  Actual (emulator): {'Code': 'NotFoundException', 'Message': 'Reputation entity <REFERENCE> does not exist.'} (HTTP 404)
  Assertion: AssertionError: expected BadRequestException, got {'Message': 'Reputation entity arn:aws:ses:us-west-2:<account>:tenant/<tenant>/<bogus-entity> does not exist.', 'Code': 'NotFoundException'}
  (the ARN, tenant name and bogus entity name are per-run values, not part of the requirement)

Cause 2 — sesv2.update-reputation-entity-policy-concurrent-writes-land-one-of-two [error]:
  Expected (AWS): GetReputationEntity(ReputationEntityType='RESOURCE', ReputationEntityReference=<tenant ARN>).ReputationEntity.ReputationManagementPolicy == 'arn:aws:ses:<region>:aws:reputation-policy/standard'
  Actual (emulator): the ReputationEntity structure has no ReputationManagementPolicy member
  Assertion: KeyError: 'ReputationManagementPolicy' at check line 38 (the baseline read, before any policy write)

Cause 3 — sesv2.tenant-create-duplicate-name-rejected [mismatch]:
  Expected (AWS): AlreadyExistsException, message 'Tenant with name <TenantName> already exists in account <account-id>'
  Actual (emulator): AlreadyExistsException, message 'Tenant with name: <TenantName> already exists.'
  Assertion: AssertionError: expected message 'Tenant with name sim2real-tenant-e08cf24318ba already exists in account 000000000000', got 'Tenant with name: sim2real-tenant-e08cf24318ba already exists.'
  (the generated tenant name and the account id are per-run values filling the template)

Cause 4 — sesv2.configuration-set-event-destination-lifecycle [mismatch]:
  Request: CreateConfigurationSetEventDestination with EventDestination.CloudWatchDestination.DimensionConfigurations=[{DimensionName:..., DimensionValueSource:'messageTag', DefaultDimensionValue:'none'}]
  Expected (AWS): ('BadRequestException', 'invalid CloudWatch dimension source')
  Actual (emulator): ('BadRequestException', "1 validation error detected: Value 'messageTag' at 'eventDestination.cloudWatchDestination.dimensionConfigurations.1.member.dimensionValueSource' failed to satisfy constraint: Member must satisfy enum value set: [MESSAGE_TAG, EMAIL_HEADER, LINK_TAG]")
  The preceding create/get/delete legs of the same check passed, so the event destination itself is implemented.

Cause 5 — sesv2.tenant-resource-association-delete-unknown-rejected [mismatch]:
  Expected (AWS): NotFoundException, message 'Configuration set <configuration-set-name> does not exist:'
  Actual (emulator): Error.Code 'NotImplemented'
  Assertion: AssertionError: expected NotFoundException, got 'NotImplemented' at check line 52
  NotImplemented is not in the operation's modeled error set (BadRequestException/NotFoundException/TooManyRequestsException).

Model evidence (services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/com-amazonaws-sesv2-simpleemailservice-v2.json):
  GetReputationEntity errors = BadRequestException(400), NotFoundException(404), TooManyRequestsException(429)
  ReputationEntity.ReputationManagementPolicy -> AmazonResourceName, @length min 1, "the ARN of the reputation management policy applied to this entity ... an AWS SES-managed policy"
  CreateTenant errors = AlreadyExistsException(400), BadRequestException(400), LimitExceededException(400), TooManyRequestsException(429)
  CloudWatchDimensionConfiguration.DimensionValueSource @required -> enum DimensionValueSource values [MESSAGE_TAG, EMAIL_HEADER, LINK_TAG]
  DeleteTenantResourceAssociation: POST /v2/email/tenants/resources/delete, code 200, empty output, errors = BadRequestException(400), NotFoundException(404), TooManyRequestsException(429)

## Steps to Reproduce
Boot the emulator and run the behavioral suite; or reproduce each cause directly with a botocore sesv2 client pointed at the emulator.

Cause 1: sesv2.create_tenant(TenantName=<fresh name>); take TenantArn; build a sibling reference by replacing the last ARN path segment with a name that was never created; call sesv2.get_reputation_entity(ReputationEntityType='RESOURCE', ReputationEntityReference=<that reference>). AWS: 400 BadRequestException 'ReputationEntity <reference> does not exist'. Emulator: 404 NotFoundException 'Reputation entity <reference> does not exist.'

Cause 2: sesv2.create_tenant(TenantName=<fresh name>); call sesv2.get_reputation_entity(ReputationEntityType='RESOURCE', ReputationEntityReference=<TenantArn>) and read ['ReputationEntity']['ReputationManagementPolicy']. AWS: 'arn:aws:ses:<region>:aws:reputation-policy/standard'. Emulator: the key is absent. Then race two sesv2.update_reputation_entity_policy calls on the same reference with .../reputation-policy/strict and .../reputation-policy/none from two threads released by a barrier; any loser must be ConflictException, at least one must succeed, and a follow-up get must return exactly one of the two raced ARNs; finally a solo update back to .../standard must read back.

Cause 3: sesv2.create_tenant(TenantName=N); call sesv2.create_tenant(TenantName=N) again. Inspect Error.Code and Error.Message. AWS: AlreadyExistsException, 'Tenant with name N already exists in account <caller account id>'. Emulator: 'Tenant with name: N already exists.'

Cause 4: sesv2.create_configuration_set(ConfigurationSetName=C); call sesv2.create_configuration_set_event_destination(ConfigurationSetName=C, EventDestinationName=D, EventDestination={'Enabled': True, 'MatchingEventTypes': ['SEND'], 'CloudWatchDestination': {'DimensionConfigurations': [{'DimensionName': 'dim', 'DimensionValueSource': 'messageTag', 'DefaultDimensionValue': 'none'}]}}). AWS: BadRequestException 'invalid CloudWatch dimension source'. Emulator: BadRequestException with the generic '1 validation error detected: ...' constraint report. Then confirm D is absent from get_configuration_set_event_destinations(ConfigurationSetName=C).

Cause 5: sesv2.create_tenant(TenantName=T); sesv2.create_configuration_set(ConfigurationSetName=A); associate it with sesv2.create_tenant_resource_association(TenantName=T, ResourceArn=arn:aws:ses:<region>:<account>:configuration-set/A); then call sesv2.delete_tenant_resource_association(TenantName=T, ResourceArn=arn:aws:ses:<region>:<account>:configuration-set/<a configuration set never created>). AWS: NotFoundException 'Configuration set <that name> does not exist:'. Emulator: NotImplemented. Then delete the real association (expect HTTP 200, and the ARN gone from list_tenant_resources) and delete it a second time (expect an idempotent HTTP 200, not a refusal).
