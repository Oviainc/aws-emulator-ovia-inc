Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.access-denied-list-domain-names appsync.list-domain-names-access-denied
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.access-denied-list-domain-names|_error#4|AssertionError|raise AssertionError(f"expected a rejection, but the call succeeded and returned {result!r}")
Issue-Key: appsync.list-domain-names-access-denied|_error#2|AssertionError|raise AssertionError(f"expected a rejection, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: 9cff6bb637b3e82ef8d39ccec86044fadd540cad

# AppSync ListDomainNames is not gated by identity-based IAM: both an explicit Deny and a no-policy principal are served 200 with domainNameConfigs instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `9cff6bb637b3e82ef8d39ccec86044fadd540cad`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDomainNames

## Description
1 cause in this issue; it must be closed before submitting.

=== Cause 1 of 1: ListDomainNames performs no identity-based authorization, so every principal is served the account's domain-name inventory ===

AWS: Smithy (appsync) ListDomainNames — GET /v1/domainnames, input nextToken (PaginationToken, @length 1-65536, @pattern ^[\S]+$, httpQuery) and maxResults (@range 0-25, httpQuery), output domainNameConfigs + nextToken, modeled errors AccessDeniedException|403, BadRequestException|400, InternalFailureException|500. The operation is authorized by the IAM action appsync:ListDomainNames on the account-wide resource arn:aws:appsync:<region>:<account>:*, and the recorded behavior shows the authorization decision is taken BEFORE the list is assembled, in both IAM denial shapes:
  * a principal whose identity policy carries an explicit Deny on appsync:ListDomainNames is refused 403 AccessDeniedException with the IAM explicit-deny sentence. Fixed text: 'User: ', ' is not authorized to perform: ', ' on resource: ', ' with an explicit deny in an identity-based policy'. Variable parts, taken from the request and from state: the caller's session ARN, the action name (appsync:ListDomainNames) and the account/region-derived resource ARN arn:aws:appsync:<region>:<account>:*.
  * a principal with NO identity-based policy at all (implicit default-deny) is refused 403 AccessDeniedException with the no-policy sentence. Fixed text: 'User: ', ' is not authorized to perform: ', ' on resource: ', ' because no identity-based policy allows the ', ' action'. Variable parts are the same caller ARN, action and resource ARN.
In both recordings the SAME call made with a permissioned principal succeeds and returns a domainNameConfigs list, so the refusal discriminates by principal and not by the operation being unavailable.

Emulator: ListDomainNames answers HTTP 200 with body {"domainNameConfigs": []} for BOTH denied principals. No AccessDeniedException is produced at any point: the explicit-deny check polls the denied principal and the call simply keeps succeeding, and the no-policy check's very first call succeeds. The operation itself is now served (it no longer answers the NotImplemented scaffold error), but it is served to everyone — authorization is not consulted at all.

Why: ListDomainNames is the account's custom-domain inventory. A client, policy test or least-privilege gate run against the emulator sees a principal that the account has explicitly forbidden — or has granted nothing — successfully enumerate that inventory and observe an empty-vs-populated answer it is not allowed to see. Every such test passes against the emulator and fails against the real deployment, and the two IAM denial shapes (explicit deny vs. no allow) cannot be distinguished by a caller because neither is ever emitted.

Fix: the request-authorization layer must evaluate identity-based policies for appsync:ListDomainNames before the ListDomainNames handler (services/service-com-amazonaws-appsync-awsdeepdishcontrolplaneservice/src/ops/list_domain_names.rs) assembles any list, and refuse with 403 AccessDeniedException carrying the explicit-deny sentence when a Deny statement matches and the no-identity-based-policy sentence when nothing allows the action.

Related but not covering: the open issue 20261029T174359Z-appsync-no-iam-deny-gate-on-flushapicache-listdomainnames.md describes this class in its Cause 1 and Cause 2, but it pinned ListDomainNames at the NotImplemented stub and its replay key for appsync.access-denied-list-domain-names is the old poll TIMEOUT key, so neither of the two keys below is carried by it and the no-policy (implicit-deny) variant in appsync.list-domain-names-access-denied is not described there at all. The open issues 20261027T222245Z-appsync-tagging-operations-are-not-gated-by-identity.md, 20261027T010308Z-appsync-listapis-and-listgraphqlapis-are-not-gated-by.md and 20261025T022306Z-appsync-graphql-api-control-plane-operations-are-not.md are the same class on other operations.

Traces: appsync.access-denied-list-domain-names (explicit Deny), appsync.list-domain-names-access-denied (no attached policy).

## Evidence
appsync.access-denied-list-domain-names — expected ('AccessDeniedException', 'User: <session arn> is not authorized to perform: appsync:ListDomainNames on resource: arn:aws:appsync:<region>:<account>:* with an explicit deny in an identity-based policy'). Actual: no rejection; 'AssertionError: expected a rejection, but the call succeeded and returned {..."HTTPStatusCode": 200..., "domainNameConfigs": []}' (check_appsync_access_denied_list_domain_names.py line 66, raised from the _poll_until probe at line 33).

appsync.list-domain-names-access-denied — expected code 'AccessDeniedException' and message 'User: <caller arn> is not authorized to perform: appsync:ListDomainNames on resource: arn:aws:appsync:<region>:<account>:* because no identity-based policy allows the appsync:ListDomainNames action'. Actual: 'AssertionError: expected a rejection, but the call SUCCEEDED and returned {..."HTTPStatusCode": 200..., "domainNameConfigs": []}' (check_appsync_list_domain_names_access_denied.py line 52, raised from run() line 21).

In both: the fixed text of the expected message is the IAM sentence above; the ARNs, action name, region and account id are values derived from the request's caller and the emulator's account/region state, not literals from one run. The positive control in each check (the same call with the runner's permissioned credentials) is expected to return a domainNameConfigs list — that part is already satisfied, which is exactly why the denied result is indistinguishable from the allowed one.

## Steps to Reproduce
1. CreateRole/assume a session whose identity policy carries an explicit Deny on action appsync:ListDomainNames (resource '*'); call appsync ListDomainNames() as that principal. Expected: 403 AccessDeniedException, message 'User: <session arn> is not authorized to perform: appsync:ListDomainNames on resource: arn:aws:appsync:<region>:<account>:* with an explicit deny in an identity-based policy'. Observed: 200 {"domainNameConfigs": []}.
2. Remove the Deny statement and repeat: the same principal must now succeed and return a domainNameConfigs list.
3. Assume a role/session with NO identity-based policy attached and call appsync ListDomainNames(). Expected: 403 AccessDeniedException, message 'User: <caller arn> is not authorized to perform: appsync:ListDomainNames on resource: arn:aws:appsync:<region>:<account>:* because no identity-based policy allows the appsync:ListDomainNames action'. Observed: 200 {"domainNameConfigs": []}.
4. Call appsync ListDomainNames() with permissioned credentials: must return 200 with a domainNameConfigs list (positive control, already passing).
