Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearch.e2e-package-association-enables-index-analyzer opensearch.e2e-package-association-search-flow opensearch.update-vpc-endpoint-concurrent-modification-conflict
Issue-Shape-Id: com.amazonaws.opensearch#AmazonOpenSearchService
Issue-Key: opensearch.e2e-package-association-enables-index-analyzer|_analyze#16|Timeout|time.sleep(delay_s)
Issue-Key: opensearch.e2e-package-association-search-flow|_wait_domain_package_status#22|Timeout|time.sleep(interval_s)
Issue-Key: opensearch.update-vpc-endpoint-concurrent-modification-conflict|run#20|AssertionError|assert len(oks) == 1, f"expected exactly one winner among concurrent updates, got {results}"
Issue-Commit: d5b444aa1a2c0b78901c8918371b89e5e61348ae

# opensearch: three checks never produce a response at all — the package-association e2e flows need the domain REST data plane, and the concurrent-update check's readiness poll never observes its target status (2 causes)

> _Split out of `issues/things-that-now-work/20261026T025630Z-opensearch-updatevpcendpoint-and-dissociatepackages-answer-501-notimplemented-so.md`, whose other two causes (UpdateVpcEndpoint answering 501 so in-flight modifications never serialized, and DissociatePackages answering 501) are closed. What remains of that report is recorded here, with the replay keys that are still failing, so the closed causes are not filed as open and these are not filed as closed._

## Operation
AssociatePackage

## Description
Two distinct root causes. Neither is a divergence in a RESPONSE: in both, the emulator answers nothing the check can assert on, because a bounded poll never observes its terminal condition and the check is killed at the suite's 60s budget.

Cause 1 of 2: the two end-to-end package-association flows never complete, because the domain's OpenSearch REST data plane is not served.
AWS: a domain publishes `DomainStatus.Endpoint` and SigV4-signed requests to that host are answered by the engine — a signed `GET /` returns the cluster banner, an associated TXT-DICTIONARY package's file is mounted at `analyzers/<PackageID>` and genuinely drives an analyzer, an index created with `synonyms_path`/`stopwords_path` pointing there expands terms, and `_analyze` reports the tokens that analyzer produces.
Emulator: every signed request to the published endpoint is answered `404 {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}` — the opensearch service claims only its own modelled `/2021-01-01/…` control-plane path space, so no service answers on the host the control plane advertised. `opensearch.e2e-package-association-enables-index-analyzer` therefore spins in `_analyze` and `opensearch.e2e-package-association-search-flow` spends its budget on the data-plane readiness wait that precedes its package-status poll; neither check reports a status, code or message at all.
Why: a client that waits for a documented terminal state (the endpoint serving, the analyzer answering) hangs instead of either proceeding or receiving an error — no success and no failure to handle. The whole custom-package-drives-search capability is unreachable.
Fix: the domain data plane. This is the SAME capability the open issue `issues/things-that-do-not-work/20261024T055421Z-opensearch-domain-data-plane-is-not-served-and.md` records as its cause 1 (with eight traces of its own): serve the published domain endpoint host as an OpenSearch engine, backed by a real in-process index/search/analysis engine, with the associated package's content driving the analyzer. The control-plane half of the original report's cause 3 is already in place: `src/state.rs` settles packages and associations on the clock, and `ListPackagesForDomain` / `ListDomainsForPackage` settle before they report, which is why the `DomainPackageStatus == ACTIVE` poll of `opensearch.concurrent-associate-dissociate-packages-race-consistency` is satisfied and that check passes.
Traces: opensearch.e2e-package-association-enables-index-analyzer, opensearch.e2e-package-association-search-flow.

Cause 2 of 2: `opensearch.update-vpc-endpoint-concurrent-modification-conflict` no longer diverges on its graded assertion, but still never finishes: its `_wait_endpoint_status` poll never observes the status it waits for.
AWS: `UpdateVpcEndpoint` answers 200 with the endpoint in `UPDATING` and the endpoint settles `ACTIVE` carrying exactly the winner's `VpcOptions`; a second writer that lands while the first is in flight is refused `409 ConflictException`.
Emulator: that pair is now served — `UpdateVpcEndpoint` is implemented, and the sibling check `opensearch.concurrency-update-vpc-endpoint-optimistic-lock`, which grades the same "exactly one winner (200) and exactly one loser (409)" requirement, PASSES. This check instead ends as `Check timed out after 60.0s` with its last frame in `_wait_endpoint_status` (`time.sleep(interval_s)`), so the awaited status is never seen. The statuses this surface holds transiently — `CREATING` after a create and `UPDATING` after an update — rest for the emulator's sub-second transition span and then settle `ACTIVE`, and a released endpoint leaves the store at once rather than resting in `DELETING`; a poll sized for AWS's minutes can therefore sample past a transient value entirely, or wait for one no record ever carries.
Why: a client that waits for the transition it was told about never returns, and the check cannot grade the final state (the winner's `SecurityGroupIds`) that follows the wait.
Fix: identify which status the poll waits for and make it observable without breaking the two rules the emulator holds here — a clock-owed transition lands in the sub-second span (`src/state.rs` `PROVISIONING`), and `DeleteVpcEndpoint` currently removes the record in the same call (pinned by `tests/all/vpc_endpoints.rs`, which asserts that a released id names no endpoint immediately). Resting a released endpoint in `DELETING` until the clock removes it — the pattern `PackageAssociation::removal_due` already uses for a dissociated package — is the candidate worth checking first, and it changes what that existing test asserts.
Traces: opensearch.update-vpc-endpoint-concurrent-modification-conflict.

## Evidence
Cause 1: `opensearch.e2e-package-association-enables-index-analyzer` — 'Error: Check timed out after 60.0s', last frame `_analyze` at `time.sleep(delay_s)`. `opensearch.e2e-package-association-search-flow` — 'Error: Check timed out after 60.0s', last frame `_wait_domain_package_status` at `time.sleep(interval_s)`. The published endpoint answers `404 {'__type': 'UnknownService', 'message': "no emulated service is registered for 'es'"}`.
Cause 2: `opensearch.update-vpc-endpoint-concurrent-modification-conflict` — 'Error: Check timed out after 60.0s', last frame `_wait_endpoint_status` at `time.sleep(interval_s)`. Its graded assertion (`run#20`, one winner among concurrent updates) is the one its sibling check now passes; implementing `ListVpcEndpointsForDomain` left this verdict byte-identical, so the poll does not read that listing.

## Steps to Reproduce
Cause 1: run the flow of `opensearch.e2e-package-association-search-flow` (create a domain with AccessPolicies, wait Active, wait for the data plane on `DomainStatus['Endpoint']`, create a TXT-DICTIONARY package from S3, wait AVAILABLE, associate_package, wait `DomainPackageStatus=ACTIVE`, list_packages_for_domain, index a document and search through an analyzer whose `stopwords_path` is `analyzers/<PackageID>`) and the flow of `opensearch.e2e-package-association-enables-index-analyzer` (same up to the association, then create_index with `synonyms_path=analyzers/<PackageID>`, get_index, `_analyze`) -> neither reaches its next step; a signed request to the domain endpoint is `404 UnknownService 'es'`.
Cause 2: create a VPC-provisioned domain, wait Active; create_vpc_endpoint, wait ACTIVE; call update_vpc_endpoint twice back-to-back and again from two threads released by a barrier -> one call answers 200 with the endpoint `UPDATING` and the other `409 ConflictException`, then the check's `_wait_endpoint_status` poll never returns.

Replay the check ids listed in trace_ids.
