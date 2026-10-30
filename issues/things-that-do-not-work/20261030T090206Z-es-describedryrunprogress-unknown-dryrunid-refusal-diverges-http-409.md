Issue-Source: trace-sweep-divergence
Issue-Trace: es-domain-mutation-not-found-errors
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-domain-mutation-not-found-errors opensearch-domain-lifecycle-and-config-inspection
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeDryRunProgress|4xx->4xx|status
Issue-Key: es|DescribeDryRunProgress|4xx->4xx|$.message
Issue-Commit: 60b375bfe55c4c11c191fcd8cd6edcaa1ac7cf89

# es DescribeDryRunProgress unknown-DryRunId refusal diverges: HTTP 409 instead of 404 and emulator-worded message

> _Note: this issue was identified against the code base at commit `60b375bfe55c4c11c191fcd8cd6edcaa1ac7cf89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDryRunProgress

## Description
2 net-new root causes on the es service token, both in the DescribeDryRunProgress not-found refusal. Both must be closed before this issue is resubmitted.

Cause 1 of 2: the not-found refusal of DescribeDryRunProgress is serialized with HTTP status 409 instead of the 404 the wire carries.
AWS: Smithy (com.amazonaws.opensearch) DescribeDryRunProgress = GET /2021-01-01/opensearch/domain/{DomainName}/dryRun with DryRunId in the dryRunId query string; its modelled errors are BaseException, DisabledOperationException, InternalException, ResourceNotFoundException and ValidationException. The model declares ResourceNotFoundException with @httpError(409), and the recordings contradict it: every recorded not-found refusal on this wire answers HTTP 404 with x-amzn-errortype: ResourceNotFoundException and a JSON body carrying __type and message. Recorded behavior is the requirement here; the error code and message are unaffected by this cause, only the status line.
Emulator: the same refusal is answered 409 (the modelled httpError) for a DryRunId that does not exist, and for a dry-run read against a domain that does not exist.
Why: a client branches on the HTTP status - 404 means the dry run is absent, 409 means a conflicting state that is worth retrying or aborting on. With 409 every absent dry run is misread as a conflict, so wait-until-gone and poll-until-absent loops never terminate and conflict-retry loops retry a permanent condition.
Fix: the service's error-to-HTTP-status mapping for ResourceNotFoundException on the DescribeDryRunProgress path (the layer that serializes the error shape into a response), not the handler - the handler already raises the right error shape. Related open issue, which does NOT carry these replay keys: 20261029T190009Z-es-resourcenotfoundexception-answered-as-http-409-instead-of.md files the same mapping fault for 28 OTHER operations and lists no DescribeDryRunProgress key, so DescribeDryRunProgress is left unfixed and unverified by it.
Traces: es-domain-mutation-not-found-errors#task#3 (status only) and opensearch-domain-lifecycle-and-config-inspection#task#11 (status plus Cause 2).

Cause 2 of 2: the unknown-DryRunId ResourceNotFoundException message is emulator-authored and leaks the domain name.
AWS: the refusal message for a DryRunId that does not exist is 'No Dry Run found for the provided DryRunId: <DryRunId>'. Fixed text: 'No Dry Run found for the provided DryRunId: '. The only variable part is the DryRunId, taken verbatim from the request's dryRunId query parameter; the domain name is NOT part of the message (recorded behavior only - the model constrains the shape, not the text).
Emulator: the error code is correct (ResourceNotFoundException) but the message reads 'Dry run not found for domain <DomainName>: <DryRunId>' - a different sentence, built from the request's DomainName plus the DryRunId.
Why: a client that classifies, logs or passes through the service's not-found text sees a sentence AWS never emits, and the one value it would parse (the DryRunId) sits in a different position; the domain name appears where AWS puts nothing.
Fix: the not-found refusal in the DescribeDryRunProgress handler - compose the message from the fixed AWS text plus the request's DryRunId only. Related open issue, which does NOT carry this replay key (it is a behavioral-corpus issue keyed on opensearch.dry-run-progress-reporting): 20261030T073207Z-opensearch-3-net-new-root-causes-a-verbose.md, Cause 2.
Traces: opensearch-domain-lifecycle-and-config-inspection#task#11.

## Evidence
Cause 1, es-domain-mutation-not-found-errors#task#3: replay detail "status: expected 404, got 409" (divergence_key service=es|DescribeDryRunProgress|status=404->409|keys=status). Expected (recorded AWS): HTTP 404, x-amzn-errortype: ResourceNotFoundException. Actual (emulator): HTTP 409 with the same error type. Smithy cited: com.amazonaws.opensearch#ResourceNotFoundException httpError=409, contradicted by the recording's 404.
Causes 1+2, opensearch-domain-lifecycle-and-config-inspection#task#11: replay detail "status: expected 404, got 409" and "$.message: value_mismatch (expected 'No Dry Run found for the provided DryRunId: 00000000-0000-0000-0000-000000000000', got 'Dry run not found for domain sim2real-domain-7d358745: 00000000-0000-0000-0000-000000000000')" (divergence_key service=es|DescribeDryRunProgress|status=404->409|keys=$.message,status). In that message the fixed text is 'No Dry Run found for the provided DryRunId: '; the trailing GUID is the request's DryRunId and the domain name the emulator interpolates ('sim2real-domain-7d358745' in this run) is a per-run value that does not belong in the message at all.

## Steps to Reproduce
1. Replay fixture es-domain-mutation-not-found-errors, step task#3: GET /2021-01-01/opensearch/domain/<absent-or-live-domain>/dryRun?dryRunId=<guid-that-does-not-exist>. Observe HTTP 409 where the recording is HTTP 404.
2. Replay fixture opensearch-domain-lifecycle-and-config-inspection, step task#11: after creating a domain, call DescribeDryRunProgress on it with DryRunId 00000000-0000-0000-0000-000000000000. Observe HTTP 409 (recording: 404) and body message 'Dry run not found for domain <domain>: <id>' where the recording is 'No Dry Run found for the provided DryRunId: <id>'.
