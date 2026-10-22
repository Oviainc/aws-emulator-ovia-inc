Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: codeartifact.describe-domain-returns-404-after-delete codeartifact.domain-lifecycle-create-active-delete codeartifact.repository-lifecycle-create-active-delete codeartifact.domain-asset-size-bytes-counts-each-asset-once-across-repositories codeartifact.create-domain-duplicate-name-rejected codeartifact.delete-domain-with-repositories-rejected codeartifact.domain-delete-with-repositories-rejected codeartifact.create-domain-invalid-name-format-rejected
Issue-Shape-Id: com.amazonaws.codeartifact#CodeArtifactControlPlaneService
Issue-Key: codeartifact.describe-domain-returns-404-after-delete|run#14|AssertionError|assert last == expected, f"expected {expected!r}, got {last!r}"
Issue-Key: codeartifact.domain-lifecycle-create-active-delete|run#15|AssertionError|assert gone == expected_gone, (
Issue-Key: codeartifact.repository-lifecycle-create-active-delete|run#17|AssertionError|assert message == not_found, (
Issue-Key: codeartifact.domain-asset-size-bytes-counts-each-asset-once-across-repositories|run#25|AssertionError|assert gone == expected, (
Issue-Key: codeartifact.create-domain-duplicate-name-rejected|run#7|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: codeartifact.delete-domain-with-repositories-rejected|run#13|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Key: codeartifact.domain-delete-with-repositories-rejected|run#13|AssertionError|assert message == "Domain must contain 0 repositories before deletion", (
Issue-Key: codeartifact.create-domain-invalid-name-format-rejected|run#5|AssertionError|assert refusal == expected, f"expected {expected!r}, got {refusal!r}"
Issue-Commit: 0c5145d1821481e15376cbd37da6da62f6188e28

# CodeArtifact control-plane error messages diverge from AWS (not-found, duplicate-domain, non-empty-domain delete, pattern-validation)

> _Note: this issue was identified against the code base at commit `0c5145d1821481e15376cbd37da6da62f6188e28`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDomain

## Description
Four distinct root causes in how the CodeArtifact emulator RENDERS its modeled error payloads. In every case the HTTP status and the error code are correct; the `message` field is built by a different rule than AWS uses. Each of these messages has a KNOWN rule — it is composed from request inputs (domain name, repository name, caller account) or from the member's modeled constraint — so the composition rule is a requirement, not an incidental recorded string.

Cause 1 of 4: ResourceNotFoundException message for domain/repository reads.
AWS: ResourceNotFoundException (HTTP 404, per Smithy: DescribeDomain/DescribeRepository errors -> ResourceNotFoundException httpError 404). The message is composed from the request plus the owning account, in the form `Domain not found. Domain '<domain>' owned by account '<accountId>' does not exist.` for a domain, and `Repository not found. Repository '<repository>' in domain '<domain>' owned by account '<accountId>' does not exist.` for a repository — a leading `<Resource> not found.` sentence followed by a sentence naming the resource, its parent (for repositories) and the owning account.
Emulator: emits a single short sentence with no quoting, no parent domain phrasing and no owning account — `Domain <domain> not found.` (services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/describe_domain.rs:162 hardcodes `format!("Domain {} not found.", input.domain)`), and `Repository <repository> not found in domain <domain>.` for the repository case.
Why: a client or test that parses the not-found message to recover which resource and which owning account was missed (a common pattern for cross-account domain access, where `domainOwner` is the interesting part) gets a message that never contains the account id, so it cannot distinguish "missing in my account" from "missing in the owner account".
Fix: the message composition in the ResourceNotFoundException construction of the domain/repository read handlers and whatever shared helper they should share.
Traces: codeartifact.describe-domain-returns-404-after-delete, codeartifact.domain-lifecycle-create-active-delete, codeartifact.repository-lifecycle-create-active-delete, codeartifact.domain-asset-size-bytes-counts-each-asset-once-across-repositories.

Cause 2 of 4: ConflictException message for CreateDomain on an existing name.
AWS: ConflictException (HTTP 409, per Smithy: CreateDomain errors -> ConflictException httpError 409), message composed from the requested name and the owning account as `Domain '<domain>' owned by '<accountId>' already exists.`
Emulator: `Domain <domain> already exists.` — unquoted and with no owner clause.
Why: the same cross-account ambiguity as cause 1 — a caller cannot tell from the conflict whether the colliding domain is one it owns.
Fix: the ConflictException construction in the CreateDomain handler.
Traces: codeartifact.create-domain-duplicate-name-rejected.

Cause 3 of 4: ConflictException message for DeleteDomain while repositories remain.
AWS: ConflictException (HTTP 409, per Smithy: DeleteDomain errors -> ConflictException httpError 409) with the FIXED, request-independent message `Domain must contain 0 repositories before deletion` — it states the precondition and interpolates nothing.
Emulator: interpolates the domain name and a live repository count instead — `Domain <domain> cannot be deleted because it still contains 1 repositories.`
Why: the emulator leaks a count AWS does not publish and omits the precondition wording, so a client matching on AWS's documented sentence does not recognise the refusal and may retry forever.
Fix: the ConflictException construction in the DeleteDomain handler.
Traces: codeartifact.delete-domain-with-repositories-rejected, codeartifact.domain-delete-with-repositories-rejected.

Cause 4 of 4: ValidationException pattern message echoes the Smithy regex anchors.
AWS: ValidationException (HTTP 400, per Smithy: CreateDomain errors -> ValidationException httpError 400) with the standard framework message `1 validation error detected: Value '<value>' at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`, where the pattern is rendered UNANCHORED: `[a-z][a-z0-9\-]{0,48}[a-z0-9]`.
Emulator: renders the modeled pattern verbatim, including the `^`/`$` anchors (Smithy: CreateDomain.domain -> DomainName pattern `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`), producing `... pattern: ^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`.
Why: the validation message is the documented, request-derived contract clients assert on; a caller that compiles or displays the quoted pattern sees a regex AWS never emits, and anchor-sensitive comparisons break.
Fix: the shared constraint-validation message renderer that formats @pattern failures (strip the anchors when rendering, not in the model).
Traces: codeartifact.create-domain-invalid-name-format-rejected.</description>

## Evidence
Cause 1 — codeartifact.describe-domain-returns-404-after-delete:
  expected ('ResourceNotFoundException', "Domain not found. Domain 'sim2real-ca-afac456fa2890' owned by account '000000000000' does not exist.")
  got      ('ResourceNotFoundException', 'Domain sim2real-ca-afac456fa2890 not found.')
  codeartifact.repository-lifecycle-create-active-delete:
  expected "Repository not found. Repository '<repo>' in domain '<domain>' owned by account '000000000000' does not exist."
  got      'Repository <repo> not found in domain <domain>.'
  Source: services/service-com-amazonaws-codeartifact-codeartifactcontrolplaneservice/src/ops/describe_domain.rs:160-167.

Cause 2 — codeartifact.create-domain-duplicate-name-rejected:
  expected ('ConflictException', "Domain 'sim2real-ca-8151249a313c0' owned by '000000000000' already exists.")
  got      ('ConflictException', 'Domain sim2real-ca-8151249a313c0 already exists.')

Cause 3 — codeartifact.domain-delete-with-repositories-rejected:
  expected ('ConflictException', 'Domain must contain 0 repositories before deletion')
  got      ('ConflictException', 'Domain sim2real-behav-dom-ea6b245e0c2e cannot be deleted because it still contains 1 repositories.')

Cause 4 — codeartifact.create-domain-invalid-name-format-rejected:
  expected ('ValidationException', "1 validation error detected: Value 'Invalid_Domain_Name' at 'domain' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-z][a-z0-9\\-]{0,48}[a-z0-9]")
  got      ('ValidationException', "... pattern: ^[a-z][a-z0-9\\-]{0,48}[a-z0-9]$")

In all four the status code and error code already match; only the message composition diverges.

## Steps to Reproduce
1. Boot the emulator and point a codeartifact client at it.
2. Cause 1: create_domain(domain=D); delete_domain(domain=D); describe_domain(domain=D) -> inspect the ResourceNotFoundException message. Likewise create_repository(domain=D, repository=R), delete_repository(...), describe_repository(...).
3. Cause 2: create_domain(domain=D) twice -> inspect the ConflictException message of the second call.
4. Cause 3: create_domain(domain=D); create_repository(domain=D, repository=R); delete_domain(domain=D) -> inspect the ConflictException message.
5. Cause 4: create_domain(domain='Invalid_Domain_Name') -> inspect the ValidationException message's quoted pattern.
