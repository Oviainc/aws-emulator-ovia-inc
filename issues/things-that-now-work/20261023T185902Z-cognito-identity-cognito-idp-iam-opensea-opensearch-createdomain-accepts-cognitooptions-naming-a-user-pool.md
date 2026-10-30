Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected
Issue-Key: cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected|run#7|AssertionError|assert (code, message) == ("ValidationException", expected), (
Issue-Commit: 144a2fce0f9e3116760062dd946ebab581523079

# opensearch: CreateDomain accepts CognitoOptions naming a user pool that has no hosted domain instead of refusing with ValidationException

> _Note: this issue was identified against the code base at commit `144a2fce0f9e3116760062dd946ebab581523079`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
One cause (Cause 1 of 1).

AWS: Cognito authentication for OpenSearch Dashboards requires the referenced user pool to have a hosted UI domain. CreateDomain with CognitoOptions.Enabled=true pointing at a user pool that has no domain is refused — recorded response: HTTP 400, error code ValidationException, message 'Domain needs to be specified for user pool' (recorded behavior; the OpenSearch model does not state this precondition, so the status and code are the requirement and the message is an example).
Emulator: the call is ACCEPTED — it returns 200 with a DomainStatus (Created=true, Processing=true, CognitoOptions echoed with Enabled=true, UserPoolId, IdentityPoolId and RoleArn), so an unusable domain is provisioned.
Why: a domain whose Dashboards sign-in can never work is reported as healthy, and the precondition clients rely on (create the user-pool domain first, then the OpenSearch domain) is never enforced, so the ordering bug only appears in production.
Fix: the CreateDomain handler's CognitoOptions validation — when CognitoOptions.Enabled is true, look up the named user pool's hosted domain in cognito-idp state and refuse with ValidationException when it has none.
Traces: cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected.

Note: the check id carries the composite service token 'cognito-identity__cognito-idp__iam__opensearch'; the defect is in the opensearch CreateDomain validation path. The companion check in this group is blocked by the already-filed CreateUserPoolDomain scaffold stub and is not re-filed here.

## Evidence
Replay output for cognito-identity__cognito-idp__iam__opensearch.cognito-options-for-a-user-pool-without-a-hosted-domain-rejected:
  AssertionError: expected ValidationException 'Domain needs to be specified for user pool', got 'OK': {'DomainId': '<account>/sim2real-cn-...', 'DomainName': 'sim2real-cn-...', 'ARN': 'arn:aws:es:<region>:<account>:domain/sim2real-cn-...', 'Created': True, 'Deleted': False, 'Endpoint': 'search-sim2real-cn-....<region>.es.amazonaws.com', 'Processing': True, 'ClusterConfig': {...}, 'CognitoOptions': {'Enabled': True, 'UserPoolId': '<region>_eca628261', 'IdentityPoolId': '<region>:94a93b0f-...', 'RoleArn': 'arn:aws:iam::<account>:role/sim2real-cn-...'}, 'DomainProcessingStatus': 'Creating', ...}
Expected: HTTP 400 ValidationException (message example 'Domain needs to be specified for user pool') and no domain created.

## Steps to Reproduce
1. Create a cognito-idp user pool and do NOT create a hosted user-pool domain for it; create an identity pool and an IAM role for OpenSearch/Cognito.
2. Call opensearch CreateDomain with CognitoOptions={'Enabled': True, 'UserPoolId': <pool>, 'IdentityPoolId': <identity pool>, 'RoleArn': <role>}.
3. Expect HTTP 400 ValidationException and no domain; observe HTTP 200 with a DomainStatus whose CognitoOptions are echoed as enabled.
