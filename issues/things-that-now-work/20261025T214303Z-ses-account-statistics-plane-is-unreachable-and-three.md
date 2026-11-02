Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ses.account-sending-enabled-toggle ses.get-send-statistics-datapoint-shape sesv2.account-dedicated-ip-auto-warmup-togglable ses.get-send-quota-reflects-state ses.send-unverified-identity-rejected-quota-unchanged sesv2.suppressed-destination-sandbox-refusal-effect-absent sesv2.dedicated-ip-pool-paginator-walks-every-pool-once sesv2.list-dedicated-ip-pools-forged-next-token-rejected ses.iam-from-address-condition-gates-a-real-signed-send ses.send-email-missing-destination-rejected ses.list-identities-invalid-identity-type-rejected
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: ses|GetAccountSendingEnabled|InvalidAction
Issue-Key: ses|GetSendStatistics|InvalidAction
Issue-Key: ses|GetAccount|NotImplemented
Issue-Key: ses|CreateDedicatedIpPool|NotImplemented
Issue-Key: ses.iam-from-address-condition-gates-a-real-signed-send|run#20|AssertionError|assert allowed_err.get("Message") == expected_allowed_msg, (
Issue-Key: ses.send-email-missing-destination-rejected|run#6|AssertionError|assert err == (
Issue-Key: ses.list-identities-invalid-identity-type-rejected|run#9|AssertionError|assert err == expected, (
Issue-Commit: ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8

# SES account/statistics plane is unreachable and three refusal messages diverge: 7 net-new root causes (GetAccountSendingEnabled and GetSendStatistics are unknown actions, GetAccount and CreateDedicatedIpPool are stubs, MessageRejected omits the region, empty Destination mis-coded MalformedInput, enum-validation text wrong)

> _Note: this issue was identified against the code base at commit `ca265ed85df5a1b6b7b5ff6c2c813551ab1096d8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountSendingEnabled

## Description
Seven net-new behavioral root causes on the SES account/sending plane and in its refusal text. None of these operations is one of the scaffold-stub clusters the report marked ALREADY FILED, and the open router issue 20261023T120410Z-ses-v1-and-v2-is-entirely-unreachable-the.md no longer carries their replay keys (requests now dispatch).

Cause 1 of 7: the SES v1 awsQuery action table does not know GetAccountSendingEnabled.
AWS: GetAccountSendingEnabled is a modeled SES v1 operation (Smithy com.amazonaws.ses#SimpleEmailService, awsQuery: input Unit, output {Enabled: boolean, default false}, no modeled errors) and answers 200 with the account's current sending state; UpdateAccountSendingEnabled(Enabled=false) then makes it report false.
Emulator: the call is refused 400 InvalidAction with the non-AWS message "unknown action: GetAccountSendingEnabled" — the Action= value is not in the v1 query dispatch table at all, so the account sending toggle cannot be read.
Why: a client cannot read or verify the account-level sending pause, and the error code it gets (InvalidAction, for an action AWS does model) tells it the API does not exist rather than that the emulator lacks it.
Fix: the ses v1 crate's awsQuery action dispatch table plus the operation handler (services/service-com-amazonaws-ses-simpleemailservice/src/ops/get_account_sending_enabled.rs) — register the action and return the stored account sending flag.
Traces: ses.account-sending-enabled-toggle.

Cause 2 of 7: the SES v1 awsQuery action table does not know GetSendStatistics.
AWS: modeled SES v1 operation (Smithy: input Unit, output {SendDataPoints: SendDataPointList}, no modeled errors); answers 200 with a list of datapoints, each carrying Timestamp, DeliveryAttempts, Bounces, Complaints, Rejects (an empty list is legal for a quiet account).
Emulator: refused 400 InvalidAction, message "unknown action: GetSendStatistics".
Why: no client can read sending statistics; the shape of the datapoint (the only thing a caller can rely on) is unobservable.
Fix: the ses v1 awsQuery action dispatch table plus services/service-com-amazonaws-ses-simpleemailservice/src/ops/get_send_statistics.rs.
Traces: ses.get-send-statistics-datapoint-shape.

Cause 3 of 7: SESv2 GetAccount is a scaffold stub.
AWS: GetAccount (Smithy service-com-amazonaws-sesv2-simpleemailservice-v2: GET /v2/email/account, 200, output {DedicatedIpAutoWarmupEnabled, EnforcementStatus, ProductionAccessEnabled, SendQuota, SendingEnabled, SuppressionAttributes, Details, VdmAttributes, PricingAttributes}; modeled errors BadRequestException 400, TooManyRequestsException 429) reports the account's attributes, and PutAccountDedicatedIpWarmupAttributes flips DedicatedIpAutoWarmupEnabled, which GetAccount then reflects.
Emulator: "An error occurred (NotImplemented) when calling the GetAccount operation: operation GetAccount is scaffolded but not implemented in this emulator" — a 501 whose code appears in no SESv2 model.
Why: the account plane (auto-warmup, production access, send quota, suppression attributes) cannot be read at all, so nothing that depends on account state can be configured or asserted.
Fix: the operation's handler, services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/get_account.rs, plus the account record in that crate's state.
Traces: ses.account-dedicated-ip-auto-warmup-togglable.

Cause 4 of 7: SESv2 CreateDedicatedIpPool is a scaffold stub.
AWS: CreateDedicatedIpPool (Smithy: POST /v2/email/dedicated-ip-pools, 200 empty body; input {PoolName required, Tags, ScalingMode}; modeled errors AlreadyExistsException 400, BadRequestException 400, ConcurrentModificationException 500, LimitExceededException 400, TooManyRequestsException 429) creates a pool that ListDedicatedIpPools then pages over.
Emulator: "An error occurred (NotImplemented) when calling the CreateDedicatedIpPool operation: operation CreateDedicatedIpPool is scaffolded but not implemented in this emulator" (501).
Why: no dedicated IP pool can exist, so the ListDedicatedIpPools paginator (PageSize, NextToken, no duplicates, no drops) is unreachable and any client using dedicated IPs fails at setup.
Fix: the operation's handler, services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/create_dedicated_ip_pool.rs, plus the pool collection in that crate's state.
Traces: ses.dedicated-ip-pool-paginator-walks-every-pool-once.

Cause 5 of 7: the MessageRejected text for an unverified sender omits the region clause.
AWS: SendEmail refuses an unverified From identity with MessageRejected (Smithy: SendEmail error MessageRejected, awsQueryError code MessageRejected, httpError 400) and the message names the region the check ran in: fixed text "Email address is not verified. The following identities failed the check in region " + the region in upper case + ": " + the failing identities from the request, e.g. "... in region US-WEST-2: allowed@<domain>". The identity list and the region token are request/endpoint-derived; the rest is fixed text.
Emulator: the code and the leading sentence are right but the region clause is dropped — "Email address is not verified. The following identities failed the check: allowed@<domain>".
Why: the region is the diagnostic half of this message — SES identities are per-region, and a client (or a human reading a log) cannot tell which region's identity list failed. Code that string-matches AWS's message, as the AWS docs and consoles present it, does not match.
Fix: the message formatter on the SendEmail/SendRawEmail identity gate in the ses v1 crate — include the request's region, upper-cased, in the fixed clause.
Traces: ses.iam-from-address-condition-gates-a-real-signed-send.

Cause 6 of 7: SendEmail with an empty Destination is refused with the wrong error code (and the message interpolates the value).
AWS: Destination is a required member (Smithy: SendEmail input Destination required), and structural validation runs before the identity/sandbox gate, so an empty Destination is refused with code ValidationError and the message "1 validation error detected: Value at 'destination' failed to satisfy constraint: Member must not be null" — entirely fixed text apart from the member path 'destination'.
Emulator: code MalformedInput with "1 validation error detected: Value null at 'destination' failed to satisfy constraint: Member must not be null" — the code is not the awsQuery validation code and the message inserts the rejected value ("Value null at") where AWS writes "Value at".
Why: a client that branches on ValidationError (the code every SES v1 structural refusal uses) treats this as an unknown failure class, and the message no longer matches AWS's text.
Fix: the ses v1 request-validation layer / its error serializer (the shared awsQuery structural-validation refusal path), not the SendEmail handler body.
Traces: ses.send-email-missing-destination-rejected.

Cause 7 of 7: the enum-constraint validation message interpolates the rejected value and lists the enum set in the wrong order.
AWS: ListIdentities(IdentityType='Bogus') is refused ValidationError with "1 validation error detected: Value at 'identityType' failed to satisfy constraint: Member must satisfy enum value set: [Domain, EmailAddress]" — fixed text, with the member path and the enum set as AWS renders it ([Domain, EmailAddress]); the rejected value is NOT echoed.
Emulator: same code, different text — "1 validation error detected: Value 'Bogus' at 'identityType' failed to satisfy constraint: Member must satisfy enum value set: [EmailAddress, Domain]": it echoes the value and renders the enum set in the model's declaration order (Smithy com.amazonaws.ses#IdentityType values [EmailAddress, Domain]) rather than AWS's order.
Why: the message is a requirement of the refusal; a caller or test that compares it to AWS's text — the only machine-readable description of which member and which permitted values failed — diverges on every enum rejection, not just this one operation.
Fix: the shared awsQuery enum-constraint message formatter in the ses v1 crate's validation layer: emit "Value at '<member>'" without the rejected value and render the permitted set in AWS's order.
Traces: ses.list-identities-invalid-identity-type-rejected.

## Evidence
Cause 1: expected 200 {Enabled: true/false} from GetAccountSendingEnabled; actual "botocore.exceptions.ClientError: An error occurred (InvalidAction) when calling the GetAccountSendingEnabled operation: unknown action: GetAccountSendingEnabled".
Cause 2: expected 200 {SendDataPoints: [...]} from GetSendStatistics; actual "An error occurred (InvalidAction) when calling the GetSendStatistics operation: unknown action: GetSendStatistics".
Cause 3: expected 200 with the GetAccount attributes (DedicatedIpAutoWarmupEnabled togglable via PutAccountDedicatedIpWarmupAttributes); actual "An error occurred (NotImplemented) when calling the GetAccount operation: operation GetAccount is scaffolded but not implemented in this emulator" (501; modeled errors are only BadRequestException 400 / TooManyRequestsException 429).
Cause 4: expected 200 from CreateDedicatedIpPool and the pool visible through ListDedicatedIpPools; actual "An error occurred (NotImplemented) when calling the CreateDedicatedIpPool operation: operation CreateDedicatedIpPool is scaffolded but not implemented in this emulator" (501).
Cause 5: expected ('MessageRejected', "Email address is not verified. The following identities failed the check in region US-WEST-2: <identity>"); actual ('MessageRejected', "Email address is not verified. The following identities failed the check: <identity>") — replay: "MessageRejected must name the unverified identity that failed the gate; expected '... in region US-WEST-2: allowed@...', got '... failed the check: allowed@...'". The region token is endpoint-derived and the identity comes from the request; the omitted clause " in region <REGION>" is fixed text.
Cause 6: expected ('ValidationError', "1 validation error detected: Value at 'destination' failed to satisfy constraint: Member must not be null"); actual ('MalformedInput', "1 validation error detected: Value null at 'destination' failed to satisfy constraint: Member must not be null").
Cause 7: expected ('ValidationError', "1 validation error detected: Value at 'identityType' failed to satisfy constraint: Member must satisfy enum value set: [Domain, EmailAddress]"); actual ('ValidationError', "1 validation error detected: Value 'Bogus' at 'identityType' failed to satisfy constraint: Member must satisfy enum value set: [EmailAddress, Domain]").

## Steps to Reproduce
1. Boot the emulator and point boto3 at it (clients 'ses' and 'sesv2'; the ses client is signed us-west-2 for cause 5's message).
2. Cause 1: ses.get_account_sending_enabled() -> observe 400 InvalidAction 'unknown action: GetAccountSendingEnabled'.
3. Cause 2: ses.get_send_statistics() -> observe 400 InvalidAction 'unknown action: GetSendStatistics'.
4. Cause 3: sesv2.get_account() -> observe 501 NotImplemented 'operation GetAccount is scaffolded but not implemented in this emulator'.
5. Cause 4: sesv2.create_dedicated_ip_pool(PoolName='p1') -> observe 501 NotImplemented 'operation CreateDedicatedIpPool is scaffolded but not implemented in this emulator'.
6. Cause 5: with an identity that exists but is not verified, ses.send_email(Source=<that identity>, ...) -> observe the MessageRejected message without the ' in region US-WEST-2' clause.
7. Cause 6: ses.send_email(Source=<verified identity>, Destination={}, Message={'Subject':{'Data':'hi'},'Body':{'Text':{'Data':'body'}}}) -> observe ('MalformedInput', "... Value null at 'destination' ...") instead of ('ValidationError', "... Value at 'destination' ...").
8. Cause 7: ses.list_identities(IdentityType='Bogus') -> observe the enum message echoing 'Bogus' and ordering the set [EmailAddress, Domain].
9. Re-replay the checks in trace_ids.
