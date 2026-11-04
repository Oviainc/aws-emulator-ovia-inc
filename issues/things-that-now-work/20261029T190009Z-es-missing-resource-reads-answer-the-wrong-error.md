Issue-Source: trace-sweep-divergence
Issue-Trace: es-domain-read-not-found-errors
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: es-domain-read-not-found-errors es-domain-mutation-not-found-errors opensearch-domain-maintenance-history es-index-and-domain-datasource-errors es-application-extension-errors opensearch-start-migration-invalid-source-error opensearch-describe-insight-details-unknown-domain opensearch-describe-insight-details-not-found
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: es|DescribeDomainNodes|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|DescribeDomainNodes|4xx->4xx|status
Issue-Key: es|DescribeDomainChangeProgress|4xx->4xx|$.message
Issue-Key: es|DescribeDomainChangeProgress|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|DescribeDomainChangeProgress|4xx->4xx|status
Issue-Key: es|GetDomainMaintenanceStatus|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|GetDomainMaintenanceStatus|4xx->4xx|status
Issue-Key: es|ListDomainMaintenances|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|ListDomainMaintenances|4xx->4xx|status
Issue-Key: es|StartDomainMaintenance|4xx->4xx|$.message
Issue-Key: es|StartDomainMaintenance|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|StartDomainMaintenance|4xx->4xx|status
Issue-Key: es|CreateIndex|4xx->4xx|$.message
Issue-Key: es|CreateIndex|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|CreateIndex|4xx->4xx|status
Issue-Key: es|UpdateIndex|4xx->4xx|$.message
Issue-Key: es|UpdateIndex|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: es|UpdateIndex|4xx->4xx|status
Issue-Key: es|GetMigration|4xx->4xx|$.message
Issue-Key: es|GetMigration|4xx->4xx|status
Issue-Key: es|DescribeInsightDetails|4xx->4xx|$.message
Issue-Key: es|DescribeInsightDetails|4xx->4xx|status
Issue-Related: 20261029T190009Z-es-ten-requests-real-aws-refuses-are-accepted.md
Issue-Commit: 92c3b140342b1bc1fab2a232b8cee3f25ea78ff5

# es: missing-resource reads answer the wrong error class and the wrong validation precedence

> _Note: this issue was identified against the code base at commit `92c3b140342b1bc1fab2a232b8cee3f25ea78ff5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomainChangeProgress

## Description
10 net-new root causes. These are NOT the generic 404-vs-409 status mapping (filed separately): on these operations AWS does not raise ResourceNotFoundException at all, or raises it only after a different check, and the emulator picks the wrong error class, the wrong message, or answers 200.

Cause 1 of 10 - DescribeDomainNodes on an absent domain. AWS: 400 with x-amzn-errortype BaseException (Smithy: DescribeDomainNodes errors include BaseException, @httpError 400 defaulted). Emulator: 409 ResourceNotFoundException. Trace: es-domain-read-not-found-errors#task#4.

Cause 2 of 10 - DescribeDomainChangeProgress on an absent domain. AWS: 400 BaseException with the fixed message "No progress information found" - it reports the absence of progress, not of the domain. Emulator: 409 ResourceNotFoundException 'Domain not found: <domainName>'. Trace: es-domain-read-not-found-errors#task#5.

Cause 3 of 10 - GetDomainMaintenanceStatus on an absent domain. AWS: 400 BaseException. Emulator: 409 ResourceNotFoundException. Trace: es-domain-mutation-not-found-errors#task#9.

Cause 4 of 10 - ListDomainMaintenances on an absent domain. AWS: 400 BaseException. Emulator: 409 ResourceNotFoundException. Trace: opensearch-domain-maintenance-history#task#1.

Cause 5 of 10 - StartDomainMaintenance checks its own required field before the domain. AWS: 404 ResourceNotFoundException "Domain not found: <the DomainName from the request>" - resource resolution precedes the NodeId requirement. Emulator: 400 ValidationException 'NodeId is required to start domain maintenance'. Trace: es-domain-mutation-not-found-errors#task#8.

Cause 6 of 10 - CreateIndex checks the body schema before the domain. AWS: 404 ResourceNotFoundException "Domain not found: <the domain from the request>". Emulator: 400 ValidationException 'IndexSchema must be a JSON object of index schema sections.'. Trace: es-index-and-domain-datasource-errors#task#1.

Cause 7 of 10 - UpdateIndex, same precedence inversion as cause 6. Trace: es-index-and-domain-datasource-errors#task#3.

Cause 8 of 10 - GetMigration's not-found message names the wrong resource. AWS: "Application <the application id> not found". Emulator: 'Migration not found: <the migrationId from the request>'. Trace: es-application-extension-errors#task#9.

Cause 9 of 10 - DescribeInsightDetails on an unknown domain. AWS: 404 ResourceNotFoundException with the fixed message "Resource not found" - it never names the domain. Emulator: 409 with 'Domain not found: <domainName>'. Trace: opensearch-describe-insight-details-unknown-domain#task#1.

Cause 10 of 10 - DescribeInsightDetails for an insight that does not exist. AWS: 404 ResourceNotFoundException "Resource not found". Emulator: 200 with an insight-details body. Trace: opensearch-describe-insight-details-not-found#task#1.

Why: the error class decides how a client reacts. A client that catches ResourceNotFoundException to mean "create it" will try to create against operations where AWS reports a 400 BaseException instead (causes 1-4), and a client that catches ValidationException to mean "my request is malformed, do not retry" will permanently reject a request whose only problem is a missing domain (causes 5-7). Causes 9 and 10 additionally leak a resource name AWS deliberately withholds, and cause 10 hands back fabricated insight data for an insight that does not exist.

Fix: the error selection in each named operation's handler - the order in which it resolves the resource versus validating the body, and which modeled error shape it raises - plus the message text for causes 2, 5, 6, 7, 8 and 9.

## Evidence
Cause 1: expected 400 + x-amzn-errortype BaseException; actual 409 + ResourceNotFoundException.
Cause 2: expected 400 + BaseException + 'No progress information found'; actual 409 + ResourceNotFoundException + 'Domain not found: sim2real-probe-absent'.
Cause 3/4: expected 400 + BaseException; actual 409 + ResourceNotFoundException.
Cause 5: expected 404 + ResourceNotFoundException + 'Domain not found: <domainName>'; actual 400 + ValidationException + 'NodeId is required to start domain maintenance'.
Cause 6/7: expected 404 + ResourceNotFoundException + 'Domain not found: <domainName>'; actual 400 + ValidationException + 'IndexSchema must be a JSON object of index schema sections.'.
Cause 8: expected 'Application <id> not found'; actual 'Migration not found: <id>'.
Cause 9: expected 404 + 'Resource not found'; actual 409 + 'Domain not found: <domainName>'.
Cause 10: expected 404 + ResourceNotFoundException + 'Resource not found'; actual 200.

## Steps to Reproduce
1. Replay es-domain-read-not-found-errors steps task#4 and task#5 against an absent domain: both must be 400 BaseException, not 409 ResourceNotFoundException.
2. Replay es-domain-mutation-not-found-errors step task#8 and es-index-and-domain-datasource-errors steps task#1/#task#3 for the precedence inversion (the domain must be resolved before the body is validated).
3. Replay es-application-extension-errors step task#9, opensearch-describe-insight-details-unknown-domain step task#1 and opensearch-describe-insight-details-not-found step task#1 for the message and 200-instead-of-404 causes.
