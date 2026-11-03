Issue-Source: trace-sweep-divergence
Issue-Trace: email-identity-resource-policy-and-tagging
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-identity-resource-policy-and-tagging
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|GetEmailIdentityPolicies|2xx->2xx|$.Policies.sim2real-probe-sesv2-policy-b4235b67
Issue-Commit: b1a2c7dc19d8b5d372d995b2ce39acbf503a99fb

# UpdateEmailIdentityPolicy does not replace the stored identity sending-authorization policy

> _Note: this issue was identified against the code base at commit `b1a2c7dc19d8b5d372d995b2ce39acbf503a99fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEmailIdentityPolicies

## Description
One net-new cause in the SESv2 identity sending-authorization (resource policy) plane.

Cause 1 of 1: an UpdateEmailIdentityPolicy that rewrites an existing policy document is not persisted, so GetEmailIdentityPolicies keeps returning the document the policy was CREATED with.

AWS: Smithy: UpdateEmailIdentityPolicy PUT /v2/email/identities/{EmailIdentity}/policies/{PolicyName}, required members EmailIdentity (httpLabel), PolicyName (httpLabel, @length 1..64) and Policy (@length min 1); errors BadRequestException(400)/NotFoundException(404)/TooManyRequestsException(429). Its documentation states it "Updates the specified sending authorization policy for the given identity (an email address or a domain). This API returns successfully even if a policy with the specified name does not exist." — i.e. the call REPLACES whatever document is stored under that policy name (and stores it even when the name was not previously present). GetEmailIdentityPolicies (GET /v2/email/identities/{EmailIdentity}/policies) then returns Policies as a map of PolicyName -> the policy document currently in effect, so after a successful UpdateEmailIdentityPolicy the map value for that name is byte-for-byte the document from the UPDATE request, not the one from the earlier CreateEmailIdentityPolicy.

Emulator: after CreateEmailIdentityPolicy stores a document and UpdateEmailIdentityPolicy is then called for the SAME identity and policy name with a different document, GetEmailIdentityPolicies returns the ORIGINAL created document. In the replayed step the update broadened the statement (Sid changed, and the single Action string became a two-element Action array), and the read still showed the pre-update statement: the Sid and the Action shape are the ones from the create call. The update itself answered successfully, so nothing tells the caller the write was dropped.

Why: a client relying on AWS's behavior uses UpdateEmailIdentityPolicy as the normal way to widen or narrow a sending-authorization policy and reads it back to confirm. Against the emulator the read-back silently reports the stale, narrower policy, so (a) code that asserts the new document on read-back fails or loops, and (b) any authorization logic driven by the returned document evaluates against a policy the caller already replaced — a permission grant that AWS would consider in effect appears absent (or, symmetrically, a revocation appears not to have happened). Because the update returns 200, the divergence is invisible until the read.

Fix: the UpdateEmailIdentityPolicy handler / the email-identity state that holds the per-identity policy map — the update must overwrite the map entry for PolicyName with the request's Policy document (inserting it when absent), and GetEmailIdentityPolicies must serialize the stored document rather than a value captured at create time.

Traces: email-identity-resource-policy-and-tagging#task#6 (GetEmailIdentityPolicies, 200/200).

No open issue covers this cause: 20261029T171854Z-sesv2-email-identity-dkim-and-mail-from-attributes-are.md touches the same fixture but only for CreateEmailIdentity's DKIM/MAIL FROM attributes, not for the identity policy plane.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture email-identity-resource-policy-and-tagging, step task#6, operation GetEmailIdentityPolicies, expected status 200, actual status 200.

$.Policies.<policy-name>: value_mismatch.
Expected (the document installed by the preceding UpdateEmailIdentityPolicy): {"Version":"2012-10-17","Statement":[{"Sid":"AllowSendAndRawSend","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":["ses:SendEmail","ses:SendRawEmail"],"Resource":"arn:aws:ses:<region>:<account>:identity/<domain>"}]}
Actual (the document installed earlier by CreateEmailIdentityPolicy): {"Version":"2012-10-17","Statement":[{"Sid":"AllowSend","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::<account>:root"},"Action":"ses:SendEmail","Resource":"arn:aws:ses:<region>:<account>:identity/<domain>"}]}

The requirement is structural, not literal: the policy name, account id, region and domain are values taken from the request and the emulator's state, while the invariant is that the returned document equals the document most recently supplied to UpdateEmailIdentityPolicy for that identity/policy-name pair. Here that document differs from the create-time one in its Sid and in Action being a list of two actions instead of a single string.

## Steps to Reproduce
1. Replay fixture email-identity-resource-policy-and-tagging and inspect step task#6.
2. CreateEmailIdentity for a domain identity.
3. CreateEmailIdentityPolicy on that identity with policy name P and a policy document granting a single action (Action: "ses:SendEmail", Sid: "AllowSend").
4. UpdateEmailIdentityPolicy (PUT /v2/email/identities/{identity}/policies/P) with a DIFFERENT document for the same name P — e.g. Sid "AllowSendAndRawSend" and Action ["ses:SendEmail","ses:SendRawEmail"]. It answers 200.
5. GetEmailIdentityPolicies on that identity and compare Policies[P] with the document sent in step 4: the emulator returns the step-3 document instead.
