Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.create-duplicate-domain-fails cognito-idp.op-contract-createUserPoolDomain cognito-idp.create-duplicate-group-fails cognito-idp.create-group-duplicate-name-rejected cognito-idp.op-contract-create-group-duplicate-rejected cognito-idp.create-duplicate-identity-provider-rejected cognito-idp.create-identity-provider-duplicate-identifier-rejected cognito-idp.concurrent-delete-user-pool-client-conflict cognito-idp.create-terms-foreign-client-rejected cognito-idp.delete-user-pool-client-removes-it cognito-idp.describe-client-from-other-pool-fails cognito-idp.describe-terms-by-client-unknown-client-rejected cognito-idp.op-contract-delete-user-pool-client cognito-idp.op-contract-describe-user-pool-client cognito-idp.update-user-pool-client-concurrent-modification-conflict cognito-idp.create-client-for-nonexistent-pool-fails cognito-idp.op-contract-set-log-delivery-configuration cognito-idp.op-contract-update-user-pool cognito-idp.op-contract-delete-user-pool-domain cognito-idp.op-contract-update-user-pool-domain cognito-idp.managed-login-branding-duplicate-create-rejected cognito-idp.op-contract-sign-up cognito-idp.deletion-protection-blocks-delete cognito-idp.e2e-delete-user-pool-deletion-protection-rejected cognito-idp.limit-exceeded-user-import-job
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.create-duplicate-domain-fails|run#10|AssertionError|assert message == "Domain already exists.", (
Issue-Key: cognito-idp.op-contract-createUserPoolDomain|run#14|AssertionError|assert err.get("Message") == want, f"expected message {want!r}, got {err.get('Message')!r}"
Issue-Key: cognito-idp.create-duplicate-group-fails|run#9|AssertionError|assert message == "A group with the name already exists.", (
Issue-Key: cognito-idp.create-group-duplicate-name-rejected|run#9|AssertionError|assert message == "A group with the name already exists.", (
Issue-Key: cognito-idp.op-contract-create-group-duplicate-rejected|run#14|AssertionError|assert message == "A group with the name already exists.", (
Issue-Key: cognito-idp.create-duplicate-identity-provider-rejected|run#12|AssertionError|assert message == f"{provider_name} already exists for tenant {pool_id}.", (
Issue-Key: cognito-idp.create-identity-provider-duplicate-identifier-rejected|run#9|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-identity-provider|DescribeUserPoolClient|ResourceNotFoundException
Issue-Key: cognito-idp.create-terms-foreign-client-rejected|run#14|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.delete-user-pool-client-removes-it|run#9|AssertionError|assert message == "User pool client does not exist.", f"DescribeUserPoolClient: message {message!r}"
Issue-Key: cognito-idp.describe-client-from-other-pool-fails|run#13|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.describe-terms-by-client-unknown-client-rejected|run#6|AssertionError|assert message == expected, (
Issue-Key: cognito-idp.op-contract-delete-user-pool-client|run#10|AssertionError|assert message == "User pool client does not exist.", f"DescribeUserPoolClient: message {message!r}"
Issue-Key: cognito-idp.op-contract-describe-user-pool-client|run#15|AssertionError|assert err.get("Message") == want, f"expected message {want!r}, got {err.get('Message')!r}"
Issue-Key: cognito-idp.update-user-pool-client-concurrent-modification-conflict|run#18|AssertionError|assert message == expected_by_code[code], (
Issue-Key: cognito-idp.create-client-for-nonexistent-pool-fails|run#14|AssertionError|assert message == "", f"expected an empty message for the bogus pool id, got {message!r}"
Issue-Key: cognito-idp.op-contract-set-log-delivery-configuration|run#24|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.op-contract-update-user-pool|run#20|AssertionError|assert message == "", f"expected an empty message, got {message!r}"
Issue-Key: cognito-idp.op-contract-delete-user-pool-domain|run#14|AssertionError|assert message == "The domain is not configured for this user pool.", (
Issue-Key: cognito-idp.op-contract-update-user-pool-domain|run#20|AssertionError|assert code == "InvalidParameterException", (
Issue-Key: cognito-idp.managed-login-branding-duplicate-create-rejected|run#12|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.op-contract-sign-up|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: cognito-idp.deletion-protection-blocks-delete|run#13|AssertionError|assert message == expected_message, f"expected the message {expected_message!r}, got {message!r}"
Issue-Key: cognito-idp.e2e-delete-user-pool-deletion-protection-rejected|run#7|AssertionError|assert message == expected, (
Issue-Key: cognito-idp.limit-exceeded-user-import-job|run#15|AssertionError|assert limit_message == expected_limit_message, (
Issue-Commit: eaa7cb6623904c9173b712c8794ba33cc0f5c9ec

# cognito-idp: not-found and duplicate-resource error messages are emulator-invented prose, not AWS's wording (10 causes)

> _Note: this issue was identified against the code base at commit `eaa7cb6623904c9173b712c8794ba33cc0f5c9ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateUserPoolDomain

## Description
Ten distinct refusal paths in cognito-idp answer with the right error CODE but a message the emulator composed itself. AWS's message text for each of these classes is fixed (with named variable slots) and clients/log scrapers match on it. None of these are covered by an open issue; the closest open items (20261024T114943Z-cognito-idp-user-pool-and-app-client-configuration.md, 20261026T235252Z-cognito-idp-five-causes-declared-custom-attribute-constraints.md) carry different replay keys.

Cause 1 of 10 — CreateUserPoolDomain, duplicate domain.
AWS: Smithy CreateUserPoolDomain errors include InvalidParameterException (httpError 400). AWS answers InvalidParameterException with the fixed text 'Domain already exists.' — no variable parts at all.
Emulator: InvalidParameterException with 'Domain <domain> is already associated with user pool <poolId>.' (recorded: 'Domain sim2real-behav-cip-domain-4035b208bb58 is already associated with user pool us-west-2_53e00324b.').
Why: a client that branches on the exact duplicate-domain text never recognises the collision.
Fix: the CreateUserPoolDomain handler's duplicate-domain refusal path.
Traces: cognito-idp.create-duplicate-domain-fails, cognito-idp.op-contract-createUserPoolDomain.

Cause 2 of 10 — CreateGroup, duplicate group name.
AWS: Smithy CreateGroup errors include GroupExistsException (httpError 400); message is the fixed text 'A group with the name already exists.' (no variables — the group name is NOT interpolated).
Emulator: 'A group named <GroupName> already exists.' (recorded: 'A group named sim2real-behav-cip-group-fbb884792dc4 already exists.').
Why: the emulator leaks the group name into a message AWS keeps constant, so exact-match assertions and localisation keyed on the text break.
Fix: the CreateGroup handler's GroupExistsException path.
Traces: cognito-idp.create-duplicate-group-fails, cognito-idp.create-group-duplicate-name-rejected, cognito-idp.op-contract-create-group-duplicate-rejected.

Cause 3 of 10 — CreateIdentityProvider, duplicate provider.
AWS: DuplicateProviderException with '<ProviderName> already exists for tenant <UserPoolId>.' — fixed text 'already exists for tenant', variables are the request's ProviderName and UserPoolId.
Emulator: 'Provider <ProviderName> already exists in user pool <UserPoolId>.' (recorded: 'Provider sim2real-oidc-a-ce2f8a1fd6ec already exists in user pool us-west-2_50dd9bec6.') — adds a 'Provider ' prefix and says 'in user pool' where AWS says 'for tenant'.
Why: same as above; the code is right, the text is not.
Fix: the CreateIdentityProvider handler's DuplicateProviderException path.
Traces: cognito-idp.create-duplicate-identity-provider-rejected, cognito-idp.create-identity-provider-duplicate-identifier-rejected.

Cause 4 of 10 — app-client-not-found message is one global template, but AWS's text differs per operation.
AWS: ResourceNotFoundException (httpError 404). Per operation the text differs: DescribeUserPoolClient/UpdateUserPoolClient answer the fixed 'User pool client does not exist.' (no id interpolated); CreateTerms/DescribeUserPoolClient-cross-pool answer 'User pool client <ClientId> does not exist in user pool <UserPoolId>.'; DescribeTermsByClient answers 'User pool client <ClientId> does not exist.'
Emulator: every one of these answers the single template 'User pool client <ClientId> does not exist.' (recorded e.g. 'User pool client a3b5e12a6c414b358d43c756a4 does not exist.'), so it is wrong in both directions — it interpolates an id where AWS does not, and omits the ' in user pool <UserPoolId>' clause where AWS has it.
Why: callers cannot distinguish 'no such client in this pool' from 'no such client at all'.
Fix: the per-operation ResourceNotFoundException construction for user-pool-client lookups (shared helper in the client-lookup path must take the operation's message form).
Traces: cognito-idp.concurrent-delete-user-pool-client-conflict, cognito-idp.create-terms-foreign-client-rejected, cognito-idp.delete-user-pool-client-removes-it, cognito-idp.describe-client-from-other-pool-fails, cognito-idp.describe-terms-by-client-unknown-client-rejected, cognito-idp.op-contract-delete-user-pool-client, cognito-idp.op-contract-describe-user-pool-client, cognito-idp.update-user-pool-client-concurrent-modification-conflict.

Cause 5 of 10 — user-pool-not-found message.
AWS: ResourceNotFoundException. SetLogDeliveryConfiguration uses 'UserPool with ID: <UserPoolId>does not exist.' (that is AWS's exact text, with no space before 'does'); CreateUserPoolClient and UpdateUserPool answer with an EMPTY message for a nonexistent pool.
Emulator: always 'User pool <UserPoolId> does not exist.' (recorded 'User pool us-east-1_nonexistent does not exist.', 'User pool us-west-2_ZZZZZZZZZ does not exist.').
Why: callers keying on the operation's documented text mis-classify the failure.
Fix: the pool-lookup refusal path of each named operation.
Traces: cognito-idp.create-client-for-nonexistent-pool-fails, cognito-idp.op-contract-set-log-delivery-configuration, cognito-idp.op-contract-update-user-pool.

Cause 6 of 10 — cross-pool domain operations.
AWS: DeleteUserPoolDomain for a domain owned by another pool answers a different message than the emulator's, and UpdateUserPoolDomain for a domain owned by another pool is InvalidParameterException, not ResourceNotFoundException.
Emulator: DeleteUserPoolDomain answers 'No such domain or user pool exists.'; UpdateUserPoolDomain answers ResourceNotFoundException.
Why: the error class itself is wrong on UpdateUserPoolDomain (404 vs 400), so retry/branch logic diverges.
Fix: the DeleteUserPoolDomain / UpdateUserPoolDomain ownership-check paths.
Traces: cognito-idp.op-contract-delete-user-pool-domain, cognito-idp.op-contract-update-user-pool-domain.

Cause 7 of 10 — ManagedLoginBranding duplicate.
AWS: 'A ManagedLoginBranding already exists for client <ClientId>' — fixed text, variable is the request's ClientId, no trailing period.
Emulator: 'Managed login branding already exists for Client <ClientId>.' (recorded for client 36bd4d27c456449da73ab57308).
Why/Fix: CreateManagedLoginBranding duplicate path.
Traces: cognito-idp.managed-login-branding-duplicate-create-rejected.

Cause 8 of 10 — SignUp for an existing username.
AWS: UsernameExistsException with the fixed text 'User already exists' (no username interpolated).
Emulator: 'User account already exists for <Username>' (recorded 'User account already exists for sim2real-signup-d6af8758801d').
Fix: the SignUp duplicate-username path.
Traces: cognito-idp.op-contract-sign-up.

Cause 9 of 10 — deletion protection.
AWS: InvalidParameterException with the fixed text 'The user pool cannot be deleted because deletion protection is activated. Deletion protection must be inactivated first.' (no pool id).
Emulator: 'User pool <UserPoolId> cannot be deleted while deletion protection is active.'
Fix: the DeleteUserPool deletion-protection guard.
Traces: cognito-idp.deletion-protection-blocks-delete, cognito-idp.e2e-delete-user-pool-deletion-protection-rejected.

Cause 10 of 10 — user-import-job limit.
AWS: LimitExceededException 'The limit for user pool import jobs is: 1000.' — the limit is 1000 per pool and is part of the fixed text.
Emulator: 'User pool <UserPoolId> holds at most 50 user import jobs.' — both the wording and the enforced ceiling (50) are wrong.
Why: callers that legitimately create more than 50 import jobs are refused by the emulator.
Fix: the CreateUserImportJob limit check and its message.
Traces: cognito-idp.limit-exceeded-user-import-job.

## Evidence
Expected vs actual (verbatim from the replay):
- 'Domain already exists.' vs 'Domain sim2real-behav-cip-domain-4035b208bb58 is already associated with user pool us-west-2_53e00324b.'
- 'A group with the name already exists.' vs 'A group named sim2real-behav-cip-group-fbb884792dc4 already exists.'
- 'sim2real-oidc-a-ce2f8a1fd6ec already exists for tenant us-west-2_50dd9bec6.' vs 'Provider sim2real-oidc-a-ce2f8a1fd6ec already exists in user pool us-west-2_50dd9bec6.'
- 'User pool client a3b5e12a6c414b358d43c756a4 does not exist in user pool us-west-2_100d6efc7.' vs 'User pool client a3b5e12a6c414b358d43c756a4 does not exist.'
- 'User pool client 00000000000000000000000000 does not exist.' vs 'User pool client 00000000000000000000000000 does not exist in user pool us-west-2_0002cfbc1.' (the same template is wrong in the opposite direction on DescribeTermsByClient)
- 'UserPool with ID: us-east-1_nonexistentdoes not exist.' vs 'User pool us-east-1_nonexistent does not exist.'
- '' (empty) vs 'User pool us-west-2_ZZZZZZZZZ does not exist.' on UpdateUserPool
- 'A ManagedLoginBranding already exists for client 36bd4d27c456449da73ab57308' vs 'Managed login branding already exists for Client 36bd4d27c456449da73ab57308.'
- 'User already exists' vs 'User account already exists for sim2real-signup-d6af8758801d'
- 'The user pool cannot be deleted because deletion protection is activated. Deletion protection must be inactivated first.' vs 'User pool us-west-2_d1b4d8f3d cannot be deleted while deletion protection is active.'
- 'The limit for user pool import jobs is: 1000.' vs 'User pool us-west-2_0be985cf8 holds at most 50 user import jobs.'
- UpdateUserPoolDomain cross-pool: expected InvalidParameterException, got ResourceNotFoundException.
The ids/names above are per-run values filling the template; the fixed text is the requirement.

## Steps to Reproduce
1. Boot the emulator and replay the behavioral suite.
2. Or by hand: CreateUserPool, then CreateUserPoolDomain twice with the same Domain -> inspect Error.Message; CreateGroup twice with the same GroupName -> inspect Error.Message; DescribeUserPoolClient with a ClientId from another pool -> inspect Error.Message; DeleteUserPool on a pool with DeletionProtection=ACTIVE -> inspect Error.Message.
3. Compare each message against AWS's fixed text listed above.
