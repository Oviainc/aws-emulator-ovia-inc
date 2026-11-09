Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-domain-lifecycle
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-domain-lifecycle
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|CreateDomain|4xx->2xx|$.portalUrl
Issue-Key: datazone|ListDomains|2xx->2xx|$.items[*].portalUrl
Issue-Related: 20261029T172524Z-datazone-domain-lifecycle-createdomain-skips-required-member-and.md
Issue-Commit: dd81faee0e8912cda86320bb65fd8f2654901268

# datazone domain responses return a portalUrl on the real AWS portal host

> _Note: this issue was identified against the code base at commit `dd81faee0e8912cda86320bb65fd8f2654901268`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
TWO causes, one fault: every datazone response that carries a domain's data-portal URL advertises a host the emulator does not serve, so the URL it hands clients resolves off the emulator to the public AWS DataZone portal domain. Fix both before submitting. The open issue 20261029T172524Z-datazone-domain-lifecycle-createdomain-skips-required-member-and.md covers the other keys of these same two steps (CreateDomain's missing required-member rejection on task#1 and ListDomains' omitted empty list on task#6) and does not carry the portalUrl keys below; they are net-new and are what this issue is about.

Cause 1 of 2: CreateDomain's portalUrl names the public AWS portal host.
AWS: Smithy CreateDomain POST /v2/domains, success code 201; CreateDomainOutput.portalUrl is a String documented as the data portal URL for the domain. Recorded behavior only for the host: the URL AWS returns is served by the endpoint the caller is talking to, so a client can follow it. An emulator must therefore mint this URL on the address it is reached on, deriving the per-domain part from the created domain id.
Emulator: returns a portalUrl on a host it does not own and does not serve — the public AWS form https://<domainId>.datazone.<region>.on.aws (the recording's dzd_* id and us-west-2 region are just this run's values) — which the replay flags host_not_own.
Why: a client or test that opens, probes or asserts the portal URL is sent off the emulator to the internet; in a sandboxed environment that is an egress failure, and in a connected one it reaches a real AWS endpoint that knows nothing about the emulated domain. Nothing the emulator created is reachable at the address it advertised.
Fix: CreateDomain's response serializer / the portal-URL construction in domain state — build portalUrl from the emulator's own announced endpoint rather than a hard-coded AWS host template.
Traces: datazone-domain-lifecycle#task#1.

Cause 2 of 2: ListDomains repeats the same foreign host in each DomainSummary.
AWS: Smithy ListDomains GET /v2/domains, 200; ListDomainsOutput.items targets DomainSummaries, and DomainSummary.portalUrl is the same data-portal URL member. It must match what CreateDomain/GetDomain reported for that domain, i.e. an address the serving endpoint answers on.
Emulator: items[*].portalUrl carries the same public https://<domainId>.datazone.<region>.on.aws value, flagged host_not_own.
Why: the defect is served on the list path too, so a client that discovers domains and then follows their portal URLs is sent off-emulator for every domain, not just the one it created.
Fix: the shared domain-summary projection/serializer that fills portalUrl — it must use the emulator's own endpoint, so fixing the construction once covers both paths.
Traces: datazone-domain-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture datazone-domain-lifecycle (expected -> actual):
1. CreateDomain task#1: $.portalUrl — verdict host_not_own, expected no foreign host, got 'https://dzd_mw9kiksm1.datazone.us-west-2.on.aws' (a host the emulator does not serve). The same step's status/message/x-amzn-errortype divergences belong to the already-open domain-lifecycle issue and are not re-filed here.
2. ListDomains task#6, status 200 on both sides: $.items[0].portalUrl — verdict host_not_own, got 'https://dzd_mw9kiksm1.datazone.us-west-2.on.aws'. The same step's $.domains omission belongs to the already-open domain-lifecycle issue.
The domain id and region in those URLs are this run's values; the requirement is that the host be the emulator's own serving address, not that exact string.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1, fixture datazone-domain-lifecycle:
1. task#1 — CreateDomain: inspect portalUrl in the response. Expect a URL on the endpoint the request was sent to; the emulator returns https://<domainId>.datazone.<region>.on.aws, a host it does not serve.
2. task#6 — ListDomains: inspect items[*].portalUrl. Expect the same emulator-served URL for the listed domain; the emulator repeats the public AWS portal host.
3. Attempt to fetch either URL from the replay environment: it never reaches the emulator.
