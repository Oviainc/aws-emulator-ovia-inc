Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: identitystore.describe-group-malformed-id-validation-rejected identitystore.describe-group-malformed-identity-store-id-rejected identitystore.invalid-identity-store-id-pattern-validation-rejected identitystore.get-user-id-malformed-attribute-path-rejected identitystore.update-user-invalid-attribute-path-validation-rejected identitystore.username-with-space-rejected identitystore.list-users-invalid-extension-name-rejected identitystore.describe-nonexistent-group-rejected identitystore.describe-nonexistent-user-not-found identitystore.create-group-membership-nonexistent-member-not-found-rejected identitystore.create-membership-nonexistent-member-rejected identitystore.create-group-duplicate-display-name-conflict-rejected identitystore.create-group-duplicate-displayname-rejected identitystore.duplicate-group-display-name-conflict-rejected identitystore.create-user-duplicate-username-rejected identitystore.create-group-membership-duplicate-rejected identitystore.duplicate-group-membership-conflict-rejected
Issue-Shape-Id: com.amazonaws.identitystore#AWSIdentityStore
Issue-Key: identitystore.describe-group-malformed-id-validation-rejected|run#6|AssertionError|assert message == (
Issue-Key: identitystore.describe-group-malformed-identity-store-id-rejected|run#8|AssertionError|assert message == (
Issue-Key: identitystore.invalid-identity-store-id-pattern-validation-rejected|run#7|AssertionError|assert message == (
Issue-Key: identitystore.get-user-id-malformed-attribute-path-rejected|run#8|AssertionError|assert error.get("Message") == (
Issue-Key: identitystore.update-user-invalid-attribute-path-validation-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: identitystore.username-with-space-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: identitystore.list-users-invalid-extension-name-rejected|run#6|AssertionError|assert err == ("ValidationException", expected_message), (
Issue-Key: identitystore.describe-nonexistent-group-rejected|run#6|AssertionError|assert message == "Group not found", (
Issue-Key: identitystore.describe-nonexistent-user-not-found|run#6|AssertionError|assert message == "User not found", (
Issue-Key: identitystore.create-group-membership-nonexistent-member-not-found-rejected|run#8|AssertionError|assert err == ("ResourceNotFoundException", "User with provided member ID not found"), (
Issue-Key: identitystore.create-membership-nonexistent-member-rejected|run#7|AssertionError|assert err == ("ResourceNotFoundException", "User with provided member ID not found"), (
Issue-Key: identitystore.create-group-duplicate-display-name-conflict-rejected|run#12|AssertionError|assert err == ("ConflictException", "Duplicate GroupDisplayName"), (
Issue-Key: identitystore.create-group-duplicate-displayname-rejected|run#7|AssertionError|assert err == ("ConflictException", "Duplicate GroupDisplayName"), (
Issue-Key: identitystore.duplicate-group-display-name-conflict-rejected|run#8|AssertionError|assert message == "Duplicate GroupDisplayName", (
Issue-Key: identitystore.create-user-duplicate-username-rejected|run#7|AssertionError|assert err == ("ConflictException", "Duplicate UserName"), (
Issue-Key: identitystore.create-group-membership-duplicate-rejected|run#15|AssertionError|assert err == ("ConflictException", "Member and Group relationship already exists"), (
Issue-Key: identitystore.duplicate-group-membership-conflict-rejected|run#10|AssertionError|assert message == "Member and Group relationship already exists", (
Issue-Commit: 4cbced68d8f61275ceb5b427ff8478e9aa7bafe5

# identitystore error-message rules diverge: anchored/model-form patterns, sensitive values echoed, and not-found/conflict text rebuilt from internal state (5 causes)

> _Note: this issue was identified against the code base at commit `4cbced68d8f61275ceb5b427ff8478e9aa7bafe5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeGroup

## Description
The identitystore emulator returns the right error CODE and status on every rejection below, but builds the Message by rules that differ from AWS. All five causes are message-construction rules that are known (AWS builds them from the request, or uses a fixed constant that every recorded rejection of that class repeats), so each is a requirement, not an incidental string. 5 distinct causes, all in service `identitystore`.

Cause 1 of 5: ValidationException quotes the Smithy pattern verbatim, anchored.
AWS: a constraint violation is reported as `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`, where <pattern> is the service's runtime pattern WITHOUT the ^ and $ anchors, and with the runtime partition alternation. Recorded: for ResourceId the pattern text is `(arn:aws(-[a-z]{1,5}){0,3}:identitystore:::(user|group|membership)/)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-...`; for IdentityStoreId it is `(arn:aws(-[a-z]{1,5}){0,3}:identitystore::\d{12}:identitystore/)?(d-[0-9a-f]{10}|...)`; for AttributePath it is `(?:\p{L}+:\p{L}+:\p{L}+(?:\.\p{L}+){0,3}|\p{L}+(?:\.\p{L}+){0,2})`. Smithy: ResourceId @pattern `^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?...$`, IdentityStoreId @pattern `^(arn:aws[a-z-]*:identitystore::\d{12}:identitystore/)?...$`; error ValidationException (400).
Emulator: emits the model's @pattern string literally — leading `^`, trailing `$`, and the model-only `arn:aws[a-z-]*` form — e.g. `...Member must satisfy regular expression pattern: ^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?...$`.
Why: clients and test suites that compare or parse the reported constraint (and humans copying the pattern out of the message to validate input locally) get a pattern AWS never emits; the anchors and the partition alternation differ, so the comparison fails even though the request was rejected for the right reason.
Fix: the shared request-validation/error-rendering component that formats ValidationException messages for identitystore — it must render the runtime pattern form (anchors stripped) rather than echoing the model trait.
Traces: identitystore.describe-group-malformed-id-validation-rejected, identitystore.describe-group-malformed-identity-store-id-rejected, identitystore.invalid-identity-store-id-pattern-validation-rejected, identitystore.get-user-id-malformed-attribute-path-rejected, identitystore.update-user-invalid-attribute-path-validation-rejected.

Cause 2 of 5: the offending value of a @sensitive member is echoed back in the ValidationException.
AWS: when the violating member is sensitive, the message omits the value entirely: `1 validation error detected: Value at 'userName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\p{L}\p{M}\p{S}\p{N}\p{P}]+` (recorded for CreateUser with a space in UserName). Smithy: CreateUser.UserName targets UserName, @sensitive, @pattern `^[\p{L}\p{M}\p{S}\p{N}\p{P}]+$`.
Emulator: includes the submitted value — `1 validation error detected: Value 'sim2real-user-... invalid' at 'userName' failed to satisfy constraint: ...` (and anchored, per Cause 1).
Why: the emulator leaks a value the model marks sensitive into an error body that is routinely logged, which is the opposite of the service's redaction behavior; it also makes the message non-deterministic for clients asserting on it.
Fix: the validation-error renderer must suppress the `Value '<v>'` fragment for members whose target shape carries @sensitive, in CreateUser's validation path and the shared renderer.
Traces: identitystore.username-with-space-rejected.

Cause 3 of 5: list-member constraint violations are reported per element with one constraint instead of aggregated on the list member with the full constraint set.
AWS: ListUsers with an invalid Extensions entry is rejected at the list member with every violated/declared constraint listed: `1 validation error detected: Value '[not:a:real:extension]' at 'extensions' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 50, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: aws:identitystore:[a-z]{1,20}, Member must not be null]` (recorded behavior). Smithy: ListUsers.Extensions -> ExtensionNames, @length 1..10, @uniqueItems; error ValidationException (400).
Emulator: `1 validation error detected: Value 'not:a:real:extension' at 'extensions.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^aws:identitystore:[a-z]{1,20}$` — element path, single anchored pattern, value not bracketed.
Why: a client that keys off the reported member path (`extensions` vs `extensions.1.member`) or expects the composite constraint list cannot classify the rejection the way it does against AWS.
Fix: the ListUsers request-validation path plus the shared renderer's handling of list members (aggregate at the list member, emit the bracketed value and the composite constraint list).
Traces: identitystore.list-users-invalid-extension-name-rejected.

Cause 4 of 5: ResourceNotFoundException messages are rebuilt from internal state instead of the service's fixed strings.
AWS: the message is a constant per resource class, independent of the request — recorded across every not-found check in this suite: DescribeGroup -> `Group not found`; DescribeUser -> `User not found`; CreateGroupMembership with an unknown MemberId.UserId -> `User with provided member ID not found`; GetGroupId -> `GROUP not found.`; GetUserId -> `USER not found.`. Status/code: 404 ResourceNotFoundException. Smithy: ResourceNotFoundException @httpError 404, modeled on DescribeGroup, DescribeUser and CreateGroupMembership.
Emulator: generates prose containing the looked-up id — `group <uuid> not found in this identity store`, `user <uuid> not found in this identity store`.
Why: the message varies per request where AWS's is fixed, and it discloses which id was probed; clients that branch on the not-found message (common in Identity Center provisioning scripts) take the wrong branch.
Fix: the not-found error construction in the DescribeGroup, DescribeUser and CreateGroupMembership handlers (and the shared identitystore error helper they call) must use the service's fixed message per resource class.
Traces: identitystore.describe-nonexistent-group-rejected, identitystore.describe-nonexistent-user-not-found, identitystore.create-group-membership-nonexistent-member-not-found-rejected, identitystore.create-membership-nonexistent-member-rejected.

Cause 5 of 5: ConflictException messages are rebuilt from internal state instead of the service's fixed strings.
AWS: duplicate-create conflicts carry a constant message — recorded consistently: CreateGroup with an existing DisplayName -> `Duplicate GroupDisplayName`; CreateUser with an existing UserName -> `Duplicate UserName`; CreateGroupMembership for an existing (group, member) pair -> `Member and Group relationship already exists`. Status/code: 409 ConflictException. Smithy: ConflictException @httpError 409, modeled on CreateGroup, CreateUser and CreateGroupMembership.
Emulator: emits prose that leaks the winning resource's identifiers — `a group with this DisplayName already exists in identity store d-xxxxxxxxxx (GroupId <uuid>)`, `a user with this UserName already exists in identity store d-xxxxxxxxxx (UserId <uuid>)`, `this member is already a member of group <uuid> (MembershipId <uuid>)`.
Why: the conflict message is request-dependent and id-bearing where AWS's is a fixed token; a caller that distinguishes a display-name collision from other conflicts by message cannot, and the response discloses ids of resources the caller did not ask about.
Fix: the conflict detection/error construction in the CreateGroup, CreateUser and CreateGroupMembership handlers and the shared identitystore error helper.
Traces: identitystore.create-group-duplicate-display-name-conflict-rejected, identitystore.create-group-duplicate-displayname-rejected, identitystore.duplicate-group-display-name-conflict-rejected, identitystore.create-user-duplicate-username-rejected, identitystore.create-group-membership-duplicate-rejected, identitystore.duplicate-group-membership-conflict-rejected.

## Evidence
Cause 1 — expected (AWS, recorded): "1 validation error detected: Value 'not-a-valid-resource-id!!' at 'groupId' failed to satisfy constraint: Member must satisfy regular expression pattern: (arn:aws(-[a-z]{1,5}){0,3}:identitystore:::(user|group|membership)/)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}"; actual (emulator): same text but pattern rendered as "^(arn:aws[a-z-]*:identitystore:::(user|group|membership)/)?([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-...{12}$". Same divergence for 'identityStoreId' and for 'alternateIdentifier.uniqueAttribute.attributePath' / 'operations.1.member.attributePath' (expected "(?:\\p{L}+:...){0,2})", got "^(?:\\p{L}+:...){0,2})$").

Cause 2 — expected: "1 validation error detected: Value at 'userName' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\p{L}\\p{M}\\p{S}\\p{N}\\p{P}]+"; actual: "1 validation error detected: Value 'sim2real-user-d21534fcc449 invalid' at 'userName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[\\p{L}\\p{M}\\p{S}\\p{N}\\p{P}]+$".

Cause 3 — expected: ValidationException "1 validation error detected: Value '[not:a:real:extension]' at 'extensions' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 50, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: aws:identitystore:[a-z]{1,20}, Member must not be null]"; actual: ValidationException "1 validation error detected: Value 'not:a:real:extension' at 'extensions.1.member' failed to satisfy constraint: Member must satisfy regular expression pattern: ^aws:identitystore:[a-z]{1,20}$".

Cause 4 — expected ('ResourceNotFoundException', 'Group not found') / ('ResourceNotFoundException', 'User not found') / ('ResourceNotFoundException', 'User with provided member ID not found'); actual ('ResourceNotFoundException', 'group ea288040-fa7e-4fe4-bde8-5db2b7fe17e9 not found in this identity store'), ('ResourceNotFoundException', 'user 7a37f37b-c1aa-463b-9007-4237bad23fb7 not found in this identity store'), ('ResourceNotFoundException', 'user 11111111-2222-3333-4444-555555555555 not found in this identity store').

Cause 5 — expected ('ConflictException', 'Duplicate GroupDisplayName') / ('ConflictException', 'Duplicate UserName') / ('ConflictException', 'Member and Group relationship already exists'); actual ('ConflictException', 'a group with this DisplayName already exists in identity store d-9067f3a1c2 (GroupId 445c530c-422c-4282-ba24-2c2313005608)'), ('ConflictException', 'a user with this UserName already exists in identity store d-9067f3a1c2 (UserId 4957f2cf-a430-434a-a57d-8171b94a538b)'), ('ConflictException', 'this member is already a member of group b65a9410-80f5-4ae2-b577-31deb9b7a7ec (MembershipId 3078bf4f-9eb4-4c95-a289-492e4e1a2361)'). HTTP status and error code matched in all cases; only the Message rule diverges.

## Steps to Reproduce
1. Resolve an identity store id (sso-admin ListInstances) and point an identitystore client at the emulator.
2. Cause 1: identitystore DescribeGroup(IdentityStoreId=<store>, GroupId='not-a-valid-resource-id!!') -> read Error.Message; repeat with DescribeGroup(IdentityStoreId='not-a-valid-identity-store-id', GroupId=<valid>), CreateUser(IdentityStoreId='not-a-valid-identity-store-id', ...), GetUserId(AlternateIdentifier={'UniqueAttribute': {'AttributePath': 'userName123', 'AttributeValue': 'x'}}) and UpdateUser(Operations=[{'AttributePath': '1bad', 'AttributeValue': 'x'}]). The reported pattern is anchored with ^...$ and uses the model's arn:aws[a-z-]* form.
3. Cause 2: CreateUser(UserName='<name> invalid') -> the Message echoes the submitted (sensitive) UserName.
4. Cause 3: ListUsers(IdentityStoreId=<store>, Extensions=['not:a:real:extension']) -> Message reports path 'extensions.1.member' with a single anchored pattern.
5. Cause 4: DescribeGroup / DescribeUser with a well-formed but never-created id, and CreateGroupMembership with MemberId={'UserId': '11111111-2222-3333-4444-555555555555'} -> Message embeds the probed id instead of the fixed string.
6. Cause 5: CreateGroup twice with the same DisplayName; CreateUser twice with the same UserName; CreateGroupMembership twice for the same (GroupId, MemberId) -> the ConflictException Message embeds the existing resource's ids instead of the fixed string.
