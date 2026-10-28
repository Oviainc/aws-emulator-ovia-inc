Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam__opensearch.cognito-options-naming-a-nonexistent-user-pool-rejected cognito-identity__cognito-idp__iam__opensearch.cognito-options-without-a-role-arn-rejected
Issue-Key: cognito-identity__cognito-idp__iam__opensearch.cognito-options-naming-a-nonexistent-user-pool-rejected|run#7|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Key: cognito-identity__cognito-idp__iam__opensearch.cognito-options-without-a-role-arn-rejected|run#7|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# opensearch CreateDomain accepts CognitoOptions naming a nonexistent user pool and CognitoOptions.Enabled without a RoleArn (2 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
Two root causes: CreateDomain performs no validation of the CognitoOptions it is handed, so two configurations real AWS refuses outright are accepted and a domain is created that can never serve Cognito-authenticated dashboards. Both checks were marked neither ALREADY FILED nor OWNED.

Cause 1 of 2 — CognitoOptions naming a user pool that does not exist is accepted.
AWS: ValidationException (400) refusing the create, with the message naming the pool, e.g. 'User pool us-west-2_AAAAAAAAA does not exist.' (recorded behavior; the opensearch model declares ValidationException on CreateDomain and the CognitoOptions members are plain strings, so nothing about the envelope is wrong — the rule is that the referenced pool must exist).
Emulator: HTTP 200. DescribeDomain/CreateDomain echoes CognitoOptions {'Enabled': True, 'UserPoolId': 'us-west-2_AAAAAAAAA', 'IdentityPoolId': …, 'RoleArn': …} and reports DomainProcessingStatus 'Creating', Created true.
Why: the referential check that makes a Cognito-backed dashboard configuration meaningful is absent, so a domain reports a working Cognito integration that points at nothing; the failure only appears when a user tries to sign in, with no API error anywhere to key on.
Fix: CognitoOptions validation in the CreateDomain handler (resolve UserPoolId against cognito-idp before accepting), and the ValidationException mapping.
Traces: cognito-identity__cognito-idp__iam__opensearch.cognito-options-naming-a-nonexistent-user-pool-rejected.

Cause 2 of 2 — CognitoOptions with Enabled=true and no RoleArn is accepted.
AWS: ValidationException (400), recorded message 'RoleArn needs to be specified'. CognitoOptions.RoleArn is optional in the model (it is omittable when Enabled is false), so the requirement is the cross-member rule: Enabled=true requires both IdentityPoolId and RoleArn.
Emulator: HTTP 200 with CognitoOptions {'Enabled': True, 'UserPoolId': …, 'IdentityPoolId': …} and no RoleArn at all; the domain is created.
Why: without the role OpenSearch cannot call Cognito on the caller's behalf, so the dashboard sign-in path cannot work; accepting it produces a domain whose own configuration is internally inconsistent while reporting success.
Fix: the cross-member CognitoOptions validation in the CreateDomain handler (Enabled -> IdentityPoolId + RoleArn required), shared with UpdateDomainConfig.
Traces: cognito-identity__cognito-idp__iam__opensearch.cognito-options-without-a-role-arn-rejected.

Related open issues that cover neither of these replay keys: 20261023T185902Z-cognito-identity-cognito-idp-iam-opensea-opensearch-createdomain-accepts-cognitooptions-naming-a-user-pool.md (the user-pool-without-a-hosted-domain case, which owns the third check in this group) and 20261024T055421Z-opensearch-createdomain-skips-documented-validation-describedomain-fabricates-empty.md (other CreateDomain validations).

## Evidence
Cause 1: AssertionError: expected ValidationException 'User pool us-west-2_AAAAAAAAA does not exist.', got 'OK': {'DomainName': 'sim2real-cn-4eb43b8bb7de', 'Created': True, 'CognitoOptions': {'Enabled': True, 'UserPoolId': 'us-west-2_AAAAAAAAA', 'IdentityPoolId': 'us-west-2:1639737c-…', 'RoleArn': 'arn:aws:iam::000000000000:role/sim2real-cn-4eb43b8bb7de'}, 'DomainProcessingStatus': 'Creating', …}
Cause 2: AssertionError: expected ValidationException 'RoleArn needs to be specified', got 'OK': {'DomainName': 'sim2real-cn-9eea5201d429', 'Created': True, 'CognitoOptions': {'Enabled': True, 'UserPoolId': 'us-west-2_4f3766216', 'IdentityPoolId': 'us-west-2:6dd5d343-…'}, …} — note the absent RoleArn.
Expected in both: HTTP 400 ValidationException and NO domain created.

## Steps to Reproduce
Cause 1: CreateDomain(DomainName=…, CognitoOptions={'Enabled': True, 'UserPoolId': 'us-west-2_AAAAAAAAA', 'IdentityPoolId': <a real identity pool>, 'RoleArn': <a real role>}) -> expect ValidationException naming the missing pool, observe 200 with the domain created.
Cause 2: CreateUserPool + CreateIdentityPool, then CreateDomain with CognitoOptions={'Enabled': True, 'UserPoolId': <pool>, 'IdentityPoolId': <identity pool>} and NO RoleArn -> expect ValidationException 'RoleArn needs to be specified', observe 200 with the domain created and CognitoOptions lacking RoleArn.
