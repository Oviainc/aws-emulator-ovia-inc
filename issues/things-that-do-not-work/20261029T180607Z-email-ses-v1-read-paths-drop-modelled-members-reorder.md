Issue-Source: trace-sweep-divergence
Issue-Trace: email-configuration-set-full-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: email-configuration-set-full-lifecycle email-v1-describe-config-set-omits-event-destinations-without-attribute email-domain-identity-dkim-and-mail-from email-email-identity-verification-and-notifications email-identity-mail-from-domain-pending-transition email-identity-sending-authorization-policy email-receipt-rule-empty-recipients-list-vs-absent-persists email-receipt-rule-set-lifecycle-and-activation email-receipt-ruleset-cloning-and-ip-filters
Issue-Shape-Id: com.amazonaws.ses#SimpleEmailService
Issue-Key: email|DescribeConfigurationSet|2xx->2xx|$.DescribeConfigurationSetResponse.DescribeConfigurationSetResult.EventDestinations.member.MatchingEventTypes.member[*]
Issue-Key: email|GetIdentityDkimAttributes|2xx->2xx|$.GetIdentityDkimAttributesResponse.GetIdentityDkimAttributesResult.DkimAttributes.entry.value.SigningKeyOrigin
Issue-Key: email|SetIdentityNotificationTopic|4xx->2xx|$.ErrorResponse
Issue-Key: email|SetIdentityNotificationTopic|4xx->2xx|status
Issue-Key: email|GetIdentityMailFromDomainAttributes|2xx->2xx|$.GetIdentityMailFromDomainAttributesResponse.GetIdentityMailFromDomainAttributesResult.MailFromDomainAttributes.entry.value.MailFromDomain
Issue-Key: email|GetIdentityPolicies|2xx->2xx|$.GetIdentityPoliciesResponse.GetIdentityPoliciesResult.Policies.entry.value
Issue-Key: email|DescribeReceiptRule|2xx->2xx|$.DescribeReceiptRuleResponse.DescribeReceiptRuleResult.Rule.TlsPolicy
Issue-Key: email|DescribeReceiptRuleSet|2xx->2xx|$.DescribeReceiptRuleSetResponse.DescribeReceiptRuleSetResult.Rules.member[*].TlsPolicy
Issue-Key: email|DescribeActiveReceiptRuleSet|2xx->2xx|$.DescribeActiveReceiptRuleSetResponse.DescribeActiveReceiptRuleSetResult.Rules.member[*].TlsPolicy
Issue-Key: email|DescribeReceiptRuleSet|2xx->2xx|$.DescribeReceiptRuleSetResponse.DescribeReceiptRuleSetResult.Rules.member.TlsPolicy
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# SES v1 read paths drop modelled members, reorder event types and skip SNS-topic validation

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeReceiptRule

## Description
10 net-new wire causes on the SES v1 (awsQuery, endpointPrefix `email`, 2010-12-01) SUCCESS-projection surface, plus one missing refusal. Each is its own replay cluster; they group into five independent defects (event-type ordering, the DKIM attribute projection, the MAIL FROM projection, policy-document round-tripping, the receipt-rule TlsPolicy projection) and one admission gap (SetIdentityNotificationTopic). Severity is HIGH because cause 4 makes the emulator ACCEPT a request AWS refuses.

Cause 1 of 10: DescribeConfigurationSet reorders EventDestination.MatchingEventTypes (two-element case).
AWS:      Smithy services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json — EventDestination.MatchingEventTypes targets EventTypes (a LIST of EventType, required). A Smithy list is ORDERED and carries no @uniqueItems/sorting trait, so the service round-trips the member order the caller supplied at CreateConfigurationSetEventDestination; the recording confirms it (submitted order is read back unchanged). EventType enum values are [send, reject, bounce, complaint, delivery, open, click, renderingFailure].
Emulator: returns the list re-ordered into EventType DECLARATION order rather than submission order — for a destination created with [bounce, send] it answers [send, bounce].
Why:      a client comparing a read-back event destination against what it submitted (drift detection, declarative config reconciliation, idempotent re-apply) sees a difference that is not there and loops or reports spurious drift; ordered-list equality assertions on MatchingEventTypes fail.
Fix:      the DescribeConfigurationSet serializer / the event-destination state representation in the SES crate — store and emit MatchingEventTypes as the ordered list that was submitted instead of normalising it to the enum's order.
Traces:   email-configuration-set-full-lifecycle#task#5 (cluster also covers email-v1-describe-config-set-omits-event-destinations-without-attribute#task#4).

Cause 2 of 10: the same reordering with three event types, confirming it is a sort and not a two-element swap.
AWS:      as cause 1 — submission order is preserved; for [bounce, complaint, send] the read-back is [bounce, complaint, send].
Emulator: answers [send, bounce, complaint] — exactly EventType declaration order.
Why:      as cause 1; with three or more types the reordering is guaranteed, so every multi-type event destination read-back diverges.
Fix:      as cause 1 (same serializer/state path).
Traces:   email-configuration-set-full-lifecycle#task#8.

Cause 3 of 10: GetIdentityDkimAttributes omits SigningKeyOrigin from each DkimAttributes entry.
AWS:      recorded behavior only — the on-disk v1 Smithy IdentityDkimAttributes structure models only DkimEnabled, DkimVerificationStatus and DkimTokens, but the recorded AWS response additionally carries a SigningKeyOrigin element inside each DkimAttributes entry value, reporting who owns the signing key for that identity (the value for an SES-managed, Easy-DKIM identity is AWS_SES; BYODKIM identities report the external origin). It is present on every recorded entry, so a client sees it unconditionally.
Emulator: the entry value carries DkimEnabled / DkimVerificationStatus / DkimTokens only — SigningKeyOrigin is absent.
Why:      a client that reads SigningKeyOrigin to decide whether an identity uses Easy DKIM or a customer-provided key gets nothing, and tooling that migrates identities between signing modes cannot tell the current mode apart on the emulator.
Fix:      the GetIdentityDkimAttributes response projection in the SES crate (the DkimAttributes entry writer) plus the identity state that must record the signing-key origin.
Traces:   email-domain-identity-dkim-and-mail-from#task#4.

Cause 4 of 10: SetIdentityNotificationTopic accepts an SnsTopic the account does not hold — the refusal never happens.
AWS:      Smithy: SetIdentityNotificationTopic takes Identity (required), NotificationType (required enum) and SnsTopic, and models no errors, so the refusal is recorded behavior only: supplying an SnsTopic ARN that does not resolve to an existing SNS topic is refused with HTTP 400 and `<Error><Type>Sender</Type><Code>InvalidParameterValue</Code><Message>SNS topic ARN is invalid.</Message></Error>`. The message's fixed text is `SNS topic ` + ` is invalid.` and its single variable part is the SnsTopic ARN taken verbatim from the request.
Emulator: answers HTTP 200 with an empty SetIdentityNotificationTopicResponse and no error envelope at all — the topic is never resolved, so the notification topic is silently bound to a non-existent destination.
Why:      this is the one admission gate protecting SES bounce/complaint/delivery notification wiring. On the emulator a typo'd or cross-account topic ARN is accepted, so a client believes notifications are configured while nothing can ever be delivered; any negative test for the invalid-topic path is unreachable, and the misconfiguration surfaces only as silently lost notifications.
Fix:      the SetIdentityNotificationTopic handler's admission validation in the SES crate — resolve the supplied SnsTopic against SNS state before persisting and refuse with 400 InvalidParameterValue, message 'SNS topic <the request's SnsTopic> is invalid.', when it does not exist.
Traces:   email-email-identity-verification-and-notifications#task#3.

Cause 5 of 10: GetIdentityMailFromDomainAttributes reports an empty-string MailFromDomain where AWS omits the member.
AWS:      Smithy: IdentityMailFromDomainAttributes members MailFromDomain, MailFromDomainStatus and BehaviorOnMXFailure are all required; the recorded response for an identity whose custom MAIL FROM is not (yet) in effect carries NO MailFromDomain element inside the entry value at all — absence is how AWS expresses 'no custom MAIL FROM domain is reportable for this identity', and recorded behavior is the only statement of that.
Emulator: emits `MailFromDomain` present with the empty string.
Why:      a client distinguishes 'no custom MAIL FROM' from 'a custom MAIL FROM is set' by the member's presence; against the emulator the member is always present, so the empty string is read as a configured domain, SDKs that model it as optional hand back Some("") and templating or DNS-record generation driven off it emits records for an empty domain.
Fix:      the GetIdentityMailFromDomainAttributes response projection in the SES crate — omit MailFromDomain instead of serialising an empty placeholder when the identity has no reportable custom MAIL FROM domain.
Traces:   email-identity-mail-from-domain-pending-transition#task#7.

Cause 6 of 10: GetIdentityPolicies does not round-trip the policy document byte-for-byte.
AWS:      Smithy: GetIdentityPolicies output Policies is a required PolicyMap whose values are the policy documents as STORED. AWS stores and returns the exact bytes supplied to PutIdentityPolicy; the recording shows the compact document the caller submitted coming back unchanged, with no whitespace inserted after ':' or ','.
Emulator: parses the submitted policy into its own JSON value and RE-SERIALIZES it on read, inserting a space after every ':' and ',' — the same logical policy, different bytes.
Why:      a policy document is an opaque blob to its callers: clients diff the read-back against the submitted text to detect drift, and some hash or sign it. Re-serialising makes every read-back look modified, breaks byte-equality and digest comparisons, and would invalidate any signature over the document.
Fix:      the identity-policy state in the SES crate — persist the policy document as the opaque string that was submitted and emit it verbatim from GetIdentityPolicies, instead of storing a parsed value and re-rendering it.
Traces:   email-identity-sending-authorization-policy#task#4 (cluster also covers #task#6).

Cause 7 of 10: DescribeReceiptRule omits Rule.TlsPolicy.
AWS:      Smithy: ReceiptRule.TlsPolicy targets the TlsPolicy enum (values Require, Optional) and its documentation states 'The default is Optional.' A rule created without TlsPolicy therefore reads back with TlsPolicy=Optional, and the recording confirms the element is present on every described rule.
Emulator: the described rule carries no TlsPolicy element at all.
Why:      TlsPolicy is the member that tells a caller whether inbound mail must arrive over TLS. Absent, an SDK hands back None/null, so a client auditing or re-applying receipt rules cannot see the effective policy and a round-trip of a described rule back through CreateReceiptRule silently loses it.
Fix:      the receipt-rule state/serializer in the SES crate — materialise the documented TlsPolicy default (Optional) when the create/update request omits it, and always project the member on the describe paths.
Traces:   email-receipt-rule-empty-recipients-list-vs-absent-persists#task#4 (cluster also covers that fixture's #task#5 and email-receipt-rule-set-lifecycle-and-activation#task#4 and #task#10).

Cause 8 of 10: DescribeReceiptRuleSet omits TlsPolicy on every rule it returns (multi-rule set).
AWS:      as cause 7 — each member of DescribeReceiptRuleSetResult.Rules is a ReceiptRule and carries TlsPolicy (Optional when never set).
Emulator: TlsPolicy missing on Rules.member[0] and Rules.member[1].
Why:      as cause 7, and here it affects the bulk read a client uses to audit a whole rule set, so no rule in the set exposes its TLS requirement.
Fix:      as cause 7 (same projection, reached from the rule-set describe path).
Traces:   email-receipt-rule-set-lifecycle-and-activation#task#5 (cluster also covers #task#8).

Cause 9 of 10: DescribeActiveReceiptRuleSet omits TlsPolicy on every rule it returns.
AWS:      as cause 7, for the active-rule-set read.
Emulator: TlsPolicy missing on Rules.member[0] and Rules.member[1].
Why:      as cause 7; this is the read that describes the rule set actually governing inbound mail, so the effective TLS requirement is invisible exactly where it matters.
Fix:      as cause 7 (same projection, reached from the active-rule-set describe path).
Traces:   email-receipt-rule-set-lifecycle-and-activation#task#12.

Cause 10 of 10: DescribeReceiptRuleSet omits TlsPolicy on a cloned single-rule set.
AWS:      as cause 7 — a rule produced by CloneReceiptRuleSet reads back with TlsPolicy present (Optional).
Emulator: Rules.member carries no TlsPolicy.
Why:      as cause 7, and it confirms the gap survives cloning, so a cloned rule set cannot be compared against its origin on this member.
Fix:      as cause 7.
Traces:   email-receipt-ruleset-cloning-and-ip-filters#task#4.

## Evidence
Replayed corpus simparity-wire-aws-v1 against the booted emulator (emulator_up=true). Nine clusters are 200->200 projection mismatches; one (cause 4) is a missing refusal.

Cause 1 — DescribeConfigurationSet, email-configuration-set-full-lifecycle#task#5 (200 expected, 200 actual):
  $...EventDestinations.member.MatchingEventTypes.member[0]: value_mismatch (expected 'bounce', got 'send')
  $...EventDestinations.member.MatchingEventTypes.member[1]: value_mismatch (expected 'send', got 'bounce')
  affected also: email-v1-describe-config-set-omits-event-destinations-without-attribute#task#4

Cause 2 — DescribeConfigurationSet, email-configuration-set-full-lifecycle#task#8 (200/200):
  member[0]: expected 'bounce', got 'send'
  member[1]: expected 'complaint', got 'bounce'
  member[2]: expected 'send', got 'complaint'
  => the emulator output is EventType declaration order [send, bounce, complaint]; AWS returns submission order.

Cause 3 — GetIdentityDkimAttributes, email-domain-identity-dkim-and-mail-from#task#4 (200/200):
  $.GetIdentityDkimAttributesResponse...DkimAttributes.entry.value.SigningKeyOrigin: missing (expected 'AWS_SES', got None)
  ('AWS_SES' is the value for an SES-managed signing key; the requirement is that the member is projected and reports the identity's actual signing-key origin.)

Cause 4 — SetIdentityNotificationTopic, email-email-identity-verification-and-notifications#task#3 (expected 400, ACTUAL 200):
  status: expected 400, got 200
  $.ErrorResponse: missing (expected {'Error': {'Type': 'Sender', 'Code': 'InvalidParameterValue',
       'Message': 'SNS topic <the request SnsTopic ARN> is invalid.'}, 'RequestId': <uuid>}, got None)
  Message template: fixed 'SNS topic ' + ' is invalid.'; variable = the SnsTopic ARN from the request.
  The emulator answered an empty 200 SetIdentityNotificationTopicResponse.

Cause 5 — GetIdentityMailFromDomainAttributes, email-identity-mail-from-domain-pending-transition#task#7 (200/200):
  $...MailFromDomainAttributes.entry.value.MailFromDomain: unexpected_present (expected None, got '')

Cause 6 — GetIdentityPolicies, email-identity-sending-authorization-policy#task#4 (200/200), affected also #task#6:
  $...Policies.entry.value: value_mismatch
    expected (as submitted, compact): {"Version":"2012-10-17","Statement":[{"Sid":"AllowSend","Effect":"Allow","Principal":{"AWS":"<root arn>"},"Action":"ses:SendEmail","Resource":"<identity arn>"}]}
    got (re-serialized):            {"Version": "2012-10-17", "Statement": [{"Sid": "AllowSend", "Effect": "Allow", "Principal": {"AWS": "<root arn>"}, "Action": "ses:SendEmail", "Resource": "<identity arn>"}]}
  Same logical policy; the divergence is purely the inserted whitespace after ':' and ','. (The ARNs are per-run values, not part of the requirement.)

Causes 7-10 — ReceiptRule.TlsPolicy missing (all 200/200):
  DescribeReceiptRule           email-receipt-rule-empty-recipients-list-vs-absent-persists#task#4
    $.DescribeReceiptRuleResponse...Rule.TlsPolicy: missing (expected 'Optional', got None)
    affected also: same fixture #task#5, email-receipt-rule-set-lifecycle-and-activation#task#4 and #task#10
  DescribeReceiptRuleSet        email-receipt-rule-set-lifecycle-and-activation#task#5 (affected also #task#8)
    Rules.member[0].TlsPolicy: missing (expected 'Optional', got None)
    Rules.member[1].TlsPolicy: missing (expected 'Optional', got None)
  DescribeActiveReceiptRuleSet  email-receipt-rule-set-lifecycle-and-activation#task#12
    Rules.member[0].TlsPolicy and Rules.member[1].TlsPolicy: missing (expected 'Optional', got None)
  DescribeReceiptRuleSet        email-receipt-ruleset-cloning-and-ip-filters#task#4
    Rules.member.TlsPolicy: missing (expected 'Optional', got None)

Smithy grounding (services/service-com-amazonaws-ses-simpleemailservice/smithy-model/com-amazonaws-ses-simpleemailservice.json):
  ReceiptRule.TlsPolicy -> TlsPolicy enum [Require, Optional], doc: 'The default is Optional.'
  EventDestination.MatchingEventTypes -> EventTypes (list of EventType, required); EventType = [send, reject, bounce, complaint, delivery, open, click, renderingFailure]
  IdentityDkimAttributes -> {DkimEnabled (required), DkimVerificationStatus (required), DkimTokens} — SigningKeyOrigin is NOT modelled here, so cause 3 is recorded behavior only.
  IdentityMailFromDomainAttributes -> {MailFromDomain, MailFromDomainStatus, BehaviorOnMXFailure} all required; the recording nonetheless omits MailFromDomain for a non-effective custom MAIL FROM, so cause 5 is recorded behavior only.
  GetIdentityPolicies output Policies -> PolicyMap (required).
  SetIdentityNotificationTopic -> input {Identity (required), NotificationType (required enum), SnsTopic}, errors [] — so cause 4's refusal is recorded behavior only.

## Steps to Reproduce
Causes 1-2 (event-type ordering):
1. CreateConfigurationSet, then CreateConfigurationSetEventDestination with MatchingEventTypes submitted as ['bounce','send'] (and in a second destination ['bounce','complaint','send']).
2. Call DescribeConfigurationSet with ConfigurationSetAttributeNames=['eventDestinations'].
3. Observe the lists returned as ['send','bounce'] and ['send','bounce','complaint'] — EventType declaration order — instead of the submitted order.

Cause 3 (SigningKeyOrigin):
1. VerifyDomainDkim / create a domain identity, then call GetIdentityDkimAttributes for it.
2. Observe the entry value with DkimEnabled, DkimVerificationStatus and DkimTokens but no SigningKeyOrigin element; AWS also reports SigningKeyOrigin (AWS_SES for an SES-managed key).

Cause 4 (SetIdentityNotificationTopic admission):
1. Verify an email identity.
2. Call SetIdentityNotificationTopic with NotificationType=Bounce and SnsTopic set to an SNS topic ARN that does NOT exist in the account.
3. Observe HTTP 200 and an empty SetIdentityNotificationTopicResponse; the topic is bound.
4. AWS refuses with HTTP 400, Error.Type Sender, Error.Code InvalidParameterValue, Error.Message 'SNS topic <the ARN you sent> is invalid.'
5. Confirm via GetIdentityNotificationAttributes that the emulator persisted the bogus topic.

Cause 5 (MailFromDomain):
1. SetIdentityMailFromDomain on an identity so the custom MAIL FROM is not yet in effect (Pending).
2. Call GetIdentityMailFromDomainAttributes for that identity.
3. Observe MailFromDomain present as the empty string; AWS omits the member entirely in that state.

Cause 6 (policy round-trip):
1. PutIdentityPolicy with a COMPACT policy document (no spaces after ':' or ',').
2. Call GetIdentityPolicies for the same policy name and compare the returned string BYTE-FOR-BYTE with what was submitted.
3. Observe spaces inserted after every ':' and ','.

Causes 7-10 (TlsPolicy):
1. CreateReceiptRuleSet, then CreateReceiptRule WITHOUT a TlsPolicy member (twice, for a two-rule set).
2. Call DescribeReceiptRule, DescribeReceiptRuleSet, SetActiveReceiptRuleSet + DescribeActiveReceiptRuleSet, and CloneReceiptRuleSet + DescribeReceiptRuleSet on the clone.
3. Observe no TlsPolicy element on any returned rule; AWS returns TlsPolicy=Optional (the documented default).

Finally re-replay all ten fixture#step bindings named in this issue (issue_trace/issue_step plus trace_ids).
