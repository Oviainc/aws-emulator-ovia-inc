Issue-Source: trace-sweep-divergence
Issue-Trace: email-email-template-create-render-send-update-delete
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-email-template-create-render-send-update-delete email-identity-resource-policy-and-tagging sesv2-email-identity-create-with-tags sesv2-tag-resource-config-set-initially-empty sesv2-untag-resource-config-set-removes-keys
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2|GetEmailTemplate|2xx->2xx|$.Tags
Issue-Key: sesv2|GetEmailTemplate|2xx->2xx|$headers.content-type
Issue-Key: sesv2|GetEmailTemplate|4xx->2xx|$.message
Issue-Key: sesv2|GetEmailTemplate|4xx->2xx|$headers.content-type
Issue-Key: sesv2|GetEmailTemplate|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: sesv2|GetEmailTemplate|4xx->2xx|status
Issue-Key: sesv2|CreateEmailIdentityPolicy|4xx->4xx|$.message
Issue-Key: sesv2|GetEmailIdentityPolicies|2xx->2xx|$.Policies.sim2real-probe-sesv2-policy-b4235b67
Issue-Key: sesv2|GetEmailIdentityPolicies|2xx->2xx|$headers.content-type
Issue-Key: sesv2|TagResource|4xx->4xx|$.message
Issue-Key: sesv2|ListTagsForResource|2xx->2xx|$headers.content-type
Issue-Key: sesv2|UntagResource|2xx->2xx|$headers.content-type
Issue-Commit: a9006feb831fdd789c02a15722f93d2f2e52852a

# Template, identity-policy and tagging reads diverge: deleted template still resolves

> _Note: this issue was identified against the code base at commit `a9006feb831fdd789c02a15722f93d2f2e52852a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetEmailTemplate

## Description
Eight net-new causes across the SESv2 template, identity-resource-policy and tagging surfaces. Smithy: GetEmailTemplate -> {TemplateName (required), TemplateContent (required), Tags}, errors BadRequestException(400)/NotFoundException(404)/TooManyRequestsException(429).

Cause 1 of 8: GetEmailTemplate omits the empty Tags list and uses the wrong content-type. AWS: a template created without tags is read back with Tags as an EMPTY LIST, and the response carries content-type application/x-amz-json-1.1 (recorded behavior only; the model declares restJson1). Emulator: Tags absent, content-type application/json. Why: a client reading response.Tags gets null where AWS guarantees a list. Fix: the GetEmailTemplate serializer plus the service-wide response content-type. Traces: email-email-template-create-render-send-update-delete#task#3 (same cluster also at task#8).

Cause 2 of 8: a DELETED email template is still readable. AWS: after DeleteEmailTemplate, GetEmailTemplate for that name answers 404 NotFoundException with 'Template <TEMPLATE-NAME> does not exist.' — the fixed text is 'Template ' + name + ' does not exist.', the variable part being the TemplateName from the request path. Emulator: answers 200 with the template body, no message and no x-amzn-errortype header. Why: the delete is not durable, so a client cannot tell a live template from a deleted one, cleanup verification passes wrongly, and a re-create path that probes with Get takes the 'already exists' branch forever. Fix: the DeleteEmailTemplate handler must remove the template from state (and GetEmailTemplate must raise NotFoundException for an absent name). Traces: email-email-template-create-render-send-update-delete#task#12.

Cause 3 of 8: CreateEmailIdentityPolicy duplicate refusal message. AWS: 400 AlreadyExistsException with 'Policy <POLICY-NAME> already exists' — the fixed text is 'Policy ' + '<' + policy name + '>' + ' already exists' (no trailing period) and the only variable part is the PolicyName from the request; the identity is NOT named. Emulator: 'Policy <name> already exists for email identity <identity>.', an emulator-authored sentence that drops the brackets, names the identity and adds a period. Why: a client matching AWS's text does not recognise the duplicate-policy refusal. Fix: the CreateEmailIdentityPolicy handler's duplicate refusal message. Traces: email-identity-resource-policy-and-tagging#task#3.

Cause 4 of 8: GetEmailIdentityPolicies re-serializes the stored policy document. AWS: the policy document is returned as the EXACT string that was submitted — AWS stores the document verbatim, so a compact document comes back compact. Emulator: returns a re-serialized document with a space after every ':' and ',' (it parses the JSON and re-emits it with its own formatting), and uses the wrong content-type. Why: a client that compares the policy it submitted with the one it reads back, or that hashes/signs the document, sees a spurious change on every round trip. Fix: the identity-policy state must keep the submitted document string and GetEmailIdentityPolicies must return it unmodified. Traces: email-identity-resource-policy-and-tagging#task#4 (same cluster also at task#6).

Cause 5 of 8: TagResource on a nonexistent identity uses the wrong message. AWS: 404 NotFoundException with 'No EmailIdentity present with name: <IDENTITY>' — the fixed text is 'No EmailIdentity present with name: ' and the single variable part is the identity NAME taken from the resource ARN in the request. Emulator: 'Resource <full-arn> does not exist.', naming the ARN instead. Why: a client matching AWS's text, or extracting the identity name from the message, does not recognise the error. Fix: the TagResource handler's not-found refusal message. Traces: email-identity-resource-policy-and-tagging#task#9. The open issues 20261028T171153Z-ses-2-net-new-refusal-fidelity-root-causes.md and 20261029T065607Z-ses-3-net-new-refusal-message-divergences-the.md concern the reserved-prefix and other tagging sentences, not this one.

Cause 6 of 8: content-type on the successful ListTagsForResource response (200): application/json instead of application/x-amz-json-1.1. Fix: the service's response content-type default. Traces: email-identity-resource-policy-and-tagging#task#10 (same cluster also at task#12, sesv2-email-identity-create-with-tags#task#2, sesv2-tag-resource-config-set-initially-empty#task#2, sesv2-untag-resource-config-set-removes-keys#task#4).

Cause 7 of 8: same content-type defect on the successful UntagResource response (200). Traces: email-identity-resource-policy-and-tagging#task#11 (same cluster also at sesv2-untag-resource-config-set-removes-keys#task#3).

Cause 8 of 8: same content-type defect on the successful GetEmailIdentityPolicies response (200) when no policy remains. Traces: email-identity-resource-policy-and-tagging#task#14.

## Evidence
Replay of trace set simparity-wire-aws-v1.
Cause 1 (task#3, 200/200): $.Tags expected [], got absent; $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.
Cause 2 (task#12): status expected 404, got 200; $.message expected 'Template sim2real-probe-sesv2-template-4d4c2072 does not exist.', got absent; $headers.x-amzn-errortype expected to be present, got absent.
Cause 3 (email-identity-resource-policy-and-tagging#task#3, 400/400): $.message expected 'Policy <sim2real-probe-sesv2-policy-b4235b67> already exists', got 'Policy sim2real-probe-sesv2-policy-b4235b67 already exists for email identity sim2real-probe-sesv2-policydom-b4235b67.com.'
Cause 4 (task#4, 200/200): $.Policies.<name> expected '{"Version":"2012-10-17","Statement":[{"Sid":"AllowSend",...}]}' (compact, as submitted), got the same document re-emitted with ', ' and ': ' separators.
Cause 5 (task#9, 404/404): $.message expected 'No EmailIdentity present with name: nonexistent-sim2real-probe-sesv2-d-b4235b67.com', got 'Resource arn:aws:ses:us-west-2:000000000000:identity/nonexistent-sim2real-probe-sesv2-d-b4235b67.com does not exist.'
Causes 6,7,8 (task#10, task#11, task#14; all 200/200): $headers.content-type expected 'application/x-amz-json-1.1', got 'application/json'.

## Steps to Reproduce
1. Replay fixture email-email-template-create-render-send-update-delete (steps task#3, task#12) and fixture email-identity-resource-policy-and-tagging (steps task#3, task#4, task#9, task#10, task#11, task#14).
2. Cause 1: CreateEmailTemplate without tags, then GetEmailTemplate and look for Tags: [].
3. Cause 2: DeleteEmailTemplate, then GetEmailTemplate for the same name; expect 404 NotFoundException.
4. Cause 3: CreateEmailIdentityPolicy twice with the same PolicyName and compare the 400 message.
5. Cause 4: CreateEmailIdentityPolicy with a compact JSON document, then GetEmailIdentityPolicies and compare the returned string byte for byte.
6. Cause 5: TagResource with an identity ARN that does not exist and compare the 404 message.
7. Causes 6-8: inspect the content-type header on ListTagsForResource, UntagResource and GetEmailIdentityPolicies.
