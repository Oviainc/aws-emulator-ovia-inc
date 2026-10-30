Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.package-association-ops-on-missing-domain
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch|ListPackagesForDomain|ResourceNotFoundException
Issue-Commit: 1a0d537a6f45d8cc06f79b3c3e7692a297108dcd

# REOPENS 20261025T080923Z-opensearch-listpackagesfordomain-stub.md: ListPackagesForDomain now refuses an unknown domain with ResourceNotFoundException instead of answering 200 with an empty association list (1 cause)

> _Note: this issue was identified against the code base at commit `1a0d537a6f45d8cc06f79b3c3e7692a297108dcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListPackagesForDomain

## Description
REOPENS 20261025T080923Z-opensearch-listpackagesfordomain-stub.md: its fix left the call refused.

This issue carries 1 cause, in the opensearch service.

Cause 1 of 1: ListPackagesForDomain refuses a domain that does not exist, where AWS serves it.
AWS: Smithy: ListPackagesForDomain(DomainName @pattern "^[a-z][a-z0-9\\-]+$", @length 3..28) binds BaseException (400), ValidationException (400), ResourceNotFoundException (409) and DisabledOperationException (409), and its output is DomainPackageDetailsList plus NextToken. Recorded behavior: the read side of the package-association surface is deliberately PERMISSIVE — for a syntactically valid DomainName that does not exist, ListPackagesForDomain answers HTTP 200 with an EMPTY DomainPackageDetailsList, while the mutating siblings AssociatePackage/DissociatePackage refuse with ValidationException and the message "The domain <domainName> does not exist." (fixed text, domain name from the request).
Emulator: ListPackagesForDomain raises ResourceNotFoundException with `Domain not found: <domainName>` — a refusal where the recording returns 200. The operation's previous scaffold-stub issue was marked fixed, but the implementation it gained over-refuses, so the check still cannot get past its first call.
Why: a client that enumerates package associations across a set of domain names (some of which may have been deleted) gets an exception instead of an empty list, so a listing loop aborts rather than skipping. It also loses the asymmetry AWS deliberately exposes between the permissive read and the refusing writes, which is what tells a caller whether a domain name is the problem.
Fix: the domain-lookup guard in the ListPackagesForDomain handler must not refuse — return an empty DomainPackageDetailsList for an unknown domain, leaving the ValidationException "The domain <domainName> does not exist." to AssociatePackage/DissociatePackage.
Traces: opensearch.package-association-ops-on-missing-domain.

## Evidence
Expected: HTTP 200 with `DomainPackageDetailsList == []` for a syntactically valid but nonexistent DomainName (the check asserts the permissive read before probing the mutating siblings).
Actual: `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the ListPackagesForDomain operation: Domain not found: sim2real-behav-os-missing-91`. The message's fixed text is `Domain not found: ` and the variable part is the requested domain name; AWS emits no error at all on this path.

## Steps to Reproduce
1. Pick a DomainName that matches ^[a-z][a-z0-9\-]+$ (3..28 chars) and was never created, e.g. 'sim2real-behav-os-missing-91'.
2. Call ListPackagesForDomain(DomainName=<that name>) — the emulator raises ResourceNotFoundException 'Domain not found: <name>'; AWS returns HTTP 200 with DomainPackageDetailsList=[].
3. For contrast, call AssociatePackage(PackageID='pkg-<16 hex>', DomainName=<same name>) — AWS refuses ValidationException 'The domain <name> does not exist.'
