Issue-Source: trace-sweep-divergence
Issue-Trace: amplify-create-domain-association-invalid-domain-pattern-rejected
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplify-create-domain-association-invalid-domain-pattern-rejected
Issue-Shape-Id: com.amazonaws.amplify#Amplify
Issue-Key: amplify|CreateDomainAssociation|2xx->2xx|$.domainAssociation.statusReason
Issue-Key: amplify|CreateDomainAssociation|2xx->2xx|$.domainAssociation.subDomains[*].dnsRecord
Issue-Key: amplify|GetDomainAssociation|2xx->2xx|$
Issue-Key: amplify|GetDomainAssociation|2xx->2xx|$.domainAssociation.certificate.certificateVerificationDNSRecord
Issue-Key: amplify|GetDomainAssociation|2xx->2xx|$.domainAssociation.certificateVerificationDNSRecord
Issue-Key: amplify|GetDomainAssociation|2xx->2xx|$.domainAssociation.domainStatus
Issue-Key: amplify|GetDomainAssociation|2xx->2xx|$.domainAssociation.statusReason
Issue-Key: amplify|GetDomainAssociation|2xx->2xx|$.domainAssociation.subDomains[*].subDomainSetting.prefix
Issue-Commit: 89c27cbcf326b5c1a1abaeca904759cc614d9072

# Amplify domain associations never reach a terminal status and misproject DomainAssociation members

> _Note: this issue was identified against the code base at commit `89c27cbcf326b5c1a1abaeca904759cc614d9072`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomainAssociation

## Description
Two net-new root causes in the amplify service's custom-domain surface, both grounded in replayed wire fixture steps. They share one fault: the emulator's domain-association model has no certificate/DNS lifecycle, so it both fills members AWS leaves unresolved at creation time and never advances the association out of its initial state.

Cause 1 of 2: CreateDomainAssociation returns a resolved subdomain DNS record and an empty-string statusReason instead of AWS's unresolved placeholders.
  AWS: Smithy (services/service-com-amazonaws-amplify-amplify/smithy-model/com-amazonaws-amplify-amplify.json) models CreateDomainAssociation -> POST /apps/{appId}/domains returning DomainAssociation, whose `statusReason` (StatusReason, @required, @length 0..1000) is documented "Additional information that describes why the domain association is in the current state" and whose `subDomains` members are SubDomain with `dnsRecord` (DNSRecord, @required, @length 0..1000) documented "The DNS record for the subdomain". Recorded behavior: the CreateDomainAssociation response carries NO `statusReason` member at all (the member is absent, not empty), and each subDomain's `dnsRecord` is the unresolved placeholder form `<prefix> CNAME <pending>` — fixed text ` CNAME <pending>` with the subdomain prefix as the variable part (`*` for the empty/root prefix in the recording). At creation the certificate and the CNAME target do not exist yet, so AWS has no record to publish and no reason text to give.
  Emulator: returns `statusReason` as an empty string `""` (type_mismatch: expected member absent, got ''), and returns a fully resolved dnsRecord of the form `<domainName> CNAME <branchName>.<appId>.amplifyapp.com` instead of the ` CNAME <pending>` placeholder.
  Why: a client that tests `'statusReason' in domainAssociation` to decide whether the service has said anything about the association's state sees a present-but-meaningless member on every create. A client that reads `dnsRecord` to copy the record it must publish at its DNS provider is handed a record AWS has not issued and that the (never-provisioned) certificate does not correspond to, instead of the `<pending>` marker that tells it to poll until the real record exists.
  Fix: the CreateDomainAssociation handler's response projection — omit `statusReason` when there is no reason, and emit the pending-placeholder dnsRecord until a verification record actually exists in state.
  Traces: amplify-create-domain-association-invalid-domain-pattern-rejected#task#5 (keys $.domainAssociation.statusReason, $.domainAssociation.subDomains[0].dnsRecord). Generated app ids, branch names, domain names and the amplifyapp.com host in the recording are per-run values and are not part of the requirement.

Cause 2 of 2: GetDomainAssociation never leaves PENDING_VERIFICATION, so a domain whose certificate cannot be issued never terminates, and the association it reports omits `certificate` while inventing a verification record, a prefix and a reason text.
  AWS: Smithy models GetDomainAssociation -> GET /apps/{appId}/domains/{domainName} returning DomainAssociation with `domainStatus` (@required, enum DomainStatus: PENDING_VERIFICATION, IN_PROGRESS, AVAILABLE, IMPORTING_CUSTOM_CERTIFICATE, PENDING_DEPLOYMENT, AWAITING_APP_CNAME, FAILED, CREATING, REQUESTING_CERTIFICATE, UPDATING) and a `certificate` member documented as describing "the SSL/TLS certificate for the domain association". Recorded behavior: for a domain whose certificate request cannot be satisfied, the association reaches the TERMINAL status `FAILED` within the polling window, with `certificate.type` = `AMPLIFY_MANAGED` and `certificate.certificateVerificationDNSRecord` absent, with `certificateVerificationDNSRecord` absent on the association, with `subDomains[0].subDomainSetting.prefix` absent, `updateStatus` absent, and `statusReason` set to ACM's fixed text "We require additional verification to request a certificate for one or more domain names in this request. The following page describes this in more detail: https://docs.aws.amazon.com/acm/latest/userguide/troubleshooting-failed.html#failed-additional-verification-required" (entirely fixed text; it carries no value from the request or state).
  Emulator: holds `domainStatus` = `PENDING_VERIFICATION` indefinitely — the replay's poll was capped at 60s without ever observing a terminal status — omits the `certificate` member entirely, emits a `certificateVerificationDNSRecord` of the form `_<hex>.<domainName>. CNAME _<hex>.acm-validations.aws.` that no certificate authority issued, reports `subDomains[0].subDomainSetting.prefix` as `""` where AWS omits it, and substitutes its own `statusReason` text "Waiting for the domain's DNS records. This emulator resolves no DNS, so the certificate verification record and the subdomain records are never observed." — emulator-authored prose that is not an AWS message.
  Why: domainStatus is the only signal a client has for a custom-domain workflow. Because no terminal status is ever reached, every poll loop (CLI, CDK, console-equivalent automation) hangs until its own timeout rather than being told the domain failed and why, and the emulator-authored statusReason means a client that matches on AWS's additional-verification text to surface remediation never matches. The invented verification record also leads a client to publish a DNS record that corresponds to no certificate request.
  Fix: the domain-association state machine behind GetDomainAssociation (status progression plus the association projection) — drive the association to a terminal status, project `certificate` and omit `certificateVerificationDNSRecord`/`prefix`/`updateStatus` when unset, and use AWS's fixed reason text for the failure class rather than emulator prose.
  Traces: amplify-create-domain-association-invalid-domain-pattern-rejected#task#6 (keys $, $.domainAssociation.domainStatus, $.domainAssociation.statusReason, $.domainAssociation.certificate.certificateVerificationDNSRecord, $.domainAssociation.certificateVerificationDNSRecord, $.domainAssociation.subDomains[0].subDomainSetting.prefix).

Related but NOT covering these: open issues 20261029T122518Z-amplify-s3-amplify-hosting-edge-never-applies-an-app-s.md (hosting-edge customRules rewrites) and 20261029T185952Z-amplify-manual-deployment-data-plane-the-zipuploadurl-handed.md (deployment upload URL / StartJob message). Neither carries either cause above, and no open issue carries these fixture steps' replay keys.

## Evidence
Cause 1 (amplify-create-domain-association-invalid-domain-pattern-rejected#task#5, CreateDomainAssociation, status 200 expected and 200 returned):
  expected $.domainAssociation.statusReason: member ABSENT. actual: '' (empty string).
  expected $.domainAssociation.subDomains[0].dnsRecord: '<prefix> CNAME <pending>' (recording: '* CNAME <pending>'; fixed text ' CNAME <pending>', variable part = the subdomain prefix). actual: '<domainName> CNAME <branchName>.<appId>.amplifyapp.com' (recording: 'sim2real-dom-<uuid>.example.com CNAME sim2real-conv-branch-<uuid>.d2wso5v4p70pdh.amplifyapp.com').

Cause 2 (amplify-create-domain-association-invalid-domain-pattern-rejected#task#6, GetDomainAssociation, status 200 expected and 200 returned):
  expected: domainStatus 'FAILED'; statusReason = 'We require additional verification to request a certificate for one or more domain names in this request. The following page describes this in more detail: https://docs.aws.amazon.com/acm/latest/userguide/troubleshooting-failed.html#failed-additional-verification-required' (all fixed text); certificate = {type: 'AMPLIFY_MANAGED', customCertificateArn absent, certificateVerificationDNSRecord absent}; association-level certificateVerificationDNSRecord absent; updateStatus absent; subDomains[0].subDomainSetting.prefix absent; subDomains[0].verified false.
  actual: the poll never observed a terminal status (replay detail: '$: poll capped at 60s'); the association returned domainStatus 'PENDING_VERIFICATION', statusReason "Waiting for the domain's DNS records. This emulator resolves no DNS, so the certificate verification record and the subdomain records are never observed.", no `certificate` member, certificateVerificationDNSRecord '_<hex>.<domainName>. CNAME _<hex>.acm-validations.aws.', and subDomains[0].subDomainSetting.prefix ''.
  The hex label values, uuid-bearing domain/branch names and the appId in the recording are per-run and not part of the requirement; the status progression, the member presence/absence and the fixed reason text are.

## Steps to Reproduce
1. amplify.create_app(name=<unique>) -> appId; amplify.create_branch(appId, branchName=<unique>).
2. amplify.create_domain_association(appId=appId, domainName=<unique>.example.com, enableAutoSubDomain=False, subDomainSettings=[{'prefix': '', 'branchName': branchName}]).
   Observed: response contains statusReason '' and subDomains[0].dnsRecord '<domain> CNAME <branch>.<appId>.amplifyapp.com'.
   Expected: statusReason member absent; subDomains[0].dnsRecord the pending placeholder '<prefix-or-*> CNAME <pending>'.
3. Poll amplify.get_domain_association(appId=appId, domainName=<the domain>) until domainAssociation.domainStatus is terminal.
   Observed: domainStatus stays 'PENDING_VERIFICATION' until the 60 s poll cap; no `certificate` member; an invented certificateVerificationDNSRecord; subDomainSetting.prefix ''; emulator-authored statusReason.
   Expected: domainStatus reaches 'FAILED' with the ACM additional-verification statusReason above, certificate.type 'AMPLIFY_MANAGED', and certificateVerificationDNSRecord / prefix / updateStatus absent.
