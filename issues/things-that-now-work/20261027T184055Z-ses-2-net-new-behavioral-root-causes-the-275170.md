Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.iam-from-address-condition-gates-a-real-signed-send sesv2.list-dedicated-ip-pools-forged-next-token-rejected
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses.iam-from-address-condition-gates-a-real-signed-send|run#23|AssertionError|assert codes == ["AccessDenied", "AccessDenied", "AccessDenied"], (
Issue-Key: ses|ListDedicatedIpPools|BadRequestException
Issue-Commit: 06adb3e2aae322591dd1176b14ac77cd8d29e903

# ses: 2 net-new behavioral root causes — the ses:FromAddress IAM condition key is never evaluated (a non-permitted From is MessageRejected instead of AccessDenied), and the ListDedicatedIpPools forged-NextToken BadRequestException message is emulator-authored

> _Note: this issue was identified against the code base at commit `06adb3e2aae322591dd1176b14ac77cd8d29e903`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendEmail

## Description
Two independent, net-new root causes in the SES surface. Both are grounded in failing behavioral checks that the replay report marked neither ALREADY FILED nor OWNED. Neither is covered by an open issue: 20261025T214303Z-ses-identity-verification-is-auto-completed-and-the.md covers the identity-verification store, and 20261026T010816Z-ses-4-net-new-behavioral-root-causes-putidentitypolicy.md covers PutIdentityPolicy resource-wildcard and oversize refusals — neither touches request authorization of a signed send, nor the dedicated-IP-pool page-cursor refusal message. 20261025T214624Z-sesv2-creatededicatedippool-stub.md is unrelated: CreateDedicatedIpPool answered this check successfully and the divergence is downstream in ListDedicatedIpPools.

=== Cause 1 of 2: SendEmail does not evaluate the caller's IAM policy, so the ses:FromAddress condition key is a no-op — a From address the policy does not name is refused by the identity gate (MessageRejected) instead of by authorization (AccessDenied) ===

AWS: SES is an IAM-authorized service and authorization runs BEFORE any SES-level validation of the message. A caller whose identity policy allows ses:SendEmail only under Condition StringEquals {"ses:FromAddress": [<allowed>]} is refused for any other Source with the SigV4/IAM authorization verdict: HTTP 403, error code AccessDenied, message of the fixed form "User '<caller principal ARN>' is not authorized to perform '<action>' on resource '<resource ARN>'" — fixed text "User '", "' is not authorized to perform '", "' on resource '", "'"; variable parts taken from the request and state: the caller's principal ARN (arn:aws:iam::<account>:user/<user>), the action (ses:SendEmail), and the identity resource ARN derived from the request's Source (arn:aws:ses:<region>:<account>:identity/<Source>). This refusal must be returned on EVERY attempt — it is a deterministic policy evaluation, not a transient. AWS documents the condition key and this exact lever in "Restricting the 'From' address": https://docs.aws.amazon.com/ses/latest/dg/control-user-access.html. Only when authorization passes does the SES identity gate run, which is why a From address the policy DOES name comes back MessageRejected ("Email address is not verified. The following identities failed the check in region <REGION>: <identity>"). Smithy: SendEmail models Source (required, Address) and the client errors MessageRejected / ConfigurationSetDoesNotExistException / AccountSendingPausedException / ConfigurationSetSendingPausedException / MailFromDomainNotVerifiedException, all httpError 400; AccessDenied is not an operation-modeled error because it is produced by the authorization layer ahead of the handler — recorded behavior only for its wire form (403 / AccessDenied / the sentence above).

Emulator: the request is signed with a real, freshly minted IAM user's long-lived access key whose only SES grant is conditioned on ses:FromAddress = [allowed@<domain>]. A SendEmail with Source=denied@<domain> is answered three times in a row with error code MessageRejected and message "Email address is not verified. The following identities failed the check in region US-WEST-2: denied@<domain>". So the caller's identity policy is never consulted at all: the emulator skipped authorization and went straight to the identity gate. Replay assertion: codes == ['AccessDenied','AccessDenied','AccessDenied'] expected, got ['MessageRejected','MessageRejected','MessageRejected'].

Why: a client cannot use SES's primary tenancy control. The whole point of ses:FromAddress is that an application credential scoped to one From address CANNOT send as another; on the emulator that restriction silently does not exist, so every scoped credential is effectively unrestricted and any Source reaches the send path. It also inverts the diagnosis: a caller that correctly receives AccessDenied in production sees MessageRejected here and "fixes" it by verifying the identity, which on real AWS turns a hard authorization failure into a successful send from an address the policy forbids. The check's downstream steps (widening the condition to name both addresses, then expecting the previously denied address to reach MessageRejected) are unobservable for the same reason: widening the policy changes nothing because the policy was never read.

Fix: the SES request-authorization stage that runs ahead of the SendEmail/SendRawEmail handlers — it must evaluate the signing principal's identity policies, including the ses:FromAddress condition key resolved from the request's Source (and the identity resource ARN arn:aws:ses:<region>:<account>:identity/<Source>), and refuse a non-matching Source with 403 AccessDenied and AWS's sentence before any identity-verification check in the handler runs.

=== Cause 2 of 2: ListDedicatedIpPools refuses a forged NextToken with an emulator-authored message ("Token is invalid.") instead of AWS's "Invalid next token." ===

AWS: a NextToken this service never issued is refused with BadRequestException. Smithy: ListDedicatedIpPools (GET /v2/email/dedicated-ip-pools, input NextToken httpQuery=NextToken, PageSize httpQuery=PageSize) models errors BadRequestException (httpError 400) and TooManyRequestsException (429). The message for the bad-cursor class is the fixed sentence "Invalid next token." — recorded behavior only; it has no variable part and in particular never echoes the rejected token.

Emulator: the status and error code are correct (400 BadRequestException), but the message is "Token is invalid." — a different sentence from the same emulator-authored family. Replay assertion: expected message 'Invalid next token.', got 'Token is invalid.'. The positive control (the pool is listed when called without a token) was not reached because the message assertion fired first.

Why: the error message is part of the contract. Clients and test suites that branch on, log, or assert the refusal sentence — and SDK/CLI users reading it — get text AWS never sends, so a bad-cursor condition is not recognizable as the same condition it is in production, and a parity test that pins AWS's wording fails against the emulator.

Fix: the ListDedicatedIpPools handler's NextToken validation / its BadRequestException construction in the sesv2 crate (the shared page-cursor refusal helper the dedicated-IP-pool listing uses), so the bad-cursor message is AWS's fixed "Invalid next token."

=== Traces ===
Cause 1: ses.iam-from-address-condition-gates-a-real-signed-send
Cause 2: ses.list-dedicated-ip-pools-forged-next-token-rejected

## Evidence
Cause 1 — check ses.iam-from-address-condition-gates-a-real-signed-send (mismatch):
  AssertionError: a From address the caller's ses:FromAddress condition does not name must be refused with AccessDenied on every attempt, not once; got ['MessageRejected', 'MessageRejected', 'MessageRejected'] (last message 'Email address is not verified. The following identities failed the check in region US-WEST-2: denied@sim2real-behav-fromcond-63a45f35e31f.example.com')
  Expected (AWS): HTTP 403, Code=AccessDenied, Message="User '<arn:aws:iam::<account>:user/<user>>' is not authorized to perform 'ses:SendEmail' on resource '<arn:aws:ses:<region>:<account>:identity/denied@<domain>>'", returned on all three attempts.
  Actual (emulator): Code=MessageRejected, Message="Email address is not verified. The following identities failed the check in region <REGION>: denied@<domain>" on all three attempts — i.e. the identity gate answered and authorization never ran. (The host-specific domain/user/account values above are per-run values that fill the message template; only the fixed text and the request/state-derived slots are the requirement.)
  Note the same check's earlier step passed: Source=allowed@<domain> (the address the policy DOES name) was refused MessageRejected with the correct unverified-identity sentence — confirming the emulator reaches the identity gate for both addresses and distinguishes neither by policy.

Cause 2 — check ses.list-dedicated-ip-pools-forged-next-token-rejected (mismatch):
  AssertionError: expected message 'Invalid next token.', got 'Token is invalid.'
  Expected (AWS): 400 BadRequestException with Message="Invalid next token."
  Actual (emulator): 400 BadRequestException with Message="Token is invalid." (status and code correct, message diverges; the sentence is fully fixed text with no variable part).

## Steps to Reproduce
Cause 1 (ses.iam-from-address-condition-gates-a-real-signed-send):
1. iam.create_user(UserName=<user>); iam.put_user_policy with a document allowing ["ses:SendEmail","ses:SendRawEmail"] on Resource "*" with Condition {"StringEquals": {"ses:FromAddress": ["allowed@<domain>"]}}, where <domain> is a fresh unverified RFC 2606 domain.
2. iam.create_access_key for that user and build an SES v1 client signing with those credentials.
3. As that user, call SendEmail(Source="allowed@<domain>", Destination={ToAddresses:["success@simulator.amazonses.com"]}, Message={...}) — observe MessageRejected "Email address is not verified..." (authorization passed, identity gate refused). This step already behaves correctly.
4. As the same user, call the identical SendEmail three times with Source="denied@<domain>" — an address the condition does NOT name.
   Expected: every call 403 AccessDenied with "User '<user ARN>' is not authorized to perform 'ses:SendEmail' on resource 'arn:aws:ses:<region>:<account>:identity/denied@<domain>'".
   Actual: every call MessageRejected with the unverified-identity sentence.
5. (Blocked downstream by the same cause) Widen the policy's ses:FromAddress to name both addresses, confirm via GetUserPolicy, then re-send from denied@<domain>: the verdict must change from AccessDenied to MessageRejected. On the emulator nothing changes, because the policy is never evaluated.

Cause 2 (ses.list-dedicated-ip-pools-forged-next-token-rejected):
1. sesv2.create_dedicated_ip_pool(PoolName=<pool>, ScalingMode="STANDARD").
2. sesv2.list_dedicated_ip_pools(PageSize=1, NextToken="c2ltMnJlYWwtZm9yZ2VkLW5leHQtdG9rZW4=") — a cursor this service never issued.
   Expected: ClientError 400 BadRequestException, Message "Invalid next token."
   Actual: 400 BadRequestException, Message "Token is invalid."
3. sesv2.list_dedicated_ip_pools() without a token still lists <pool> (positive control).
