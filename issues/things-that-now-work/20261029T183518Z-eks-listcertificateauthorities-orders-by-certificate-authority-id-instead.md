Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eks.list-certificate-authorities-newest-first-ordering
Issue-Shape-Id: com.amazonaws.eks#AWSWesleyFrontend
Issue-Key: eks.list-certificate-authorities-newest-first-ordering|run#10|AssertionError|assert full[0]["id"] == ca1_id, f"expected newest CA ({ca1_id}) first, got {full}"
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# eks: ListCertificateAuthorities orders by certificate-authority id instead of newest createdAt first (1 cause)

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificateAuthorities

## Description
Cause 1 of 1: `ListCertificateAuthorities` returns a cluster's certificate authorities in certificate-authority **id** order instead of newest-`createdAt`-first, so the successor CA a caller just created is reported after the outgoing CA it is meant to replace.

AWS: the Smithy model (services/service-com-amazonaws-eks-awswesleyfrontend/smithy-model/com-amazonaws-eks-awswesleyfrontend.json) declares `ListCertificateAuthorities` as GET /clusters/{clusterName}/certificate-authorities -> `certificateAuthorities: CertificateAuthoritySummaryList` + `nextToken`, `@paginated(inputToken: nextToken, outputToken: nextToken, items: certificateAuthorities, pageSize: maxResults)`, and documents it as "Lists the certificate authorities (CAs) for your cluster. A cluster has at most two certificate authorities: the outgoing CA that's currently signing and, during a rotation, one successor CA." The model states no ordering, so this is **recorded behavior only**: the recording shows real EKS listing the CAs newest-`createdAt` first — after `CreateCertificateAuthority` on a cluster that already holds its service-managed CA, element [0] is the just-created successor (`createdBy: CUSTOMER`, `signingStatus: NOT_USED`) and element [1] is the older in-use CA (`createdBy: EKS`, `signingStatus: IN_USE`) — and preserving that same order across pagination, so a `maxResults=1` first page carries the newest CA and the second page the older one. No error is involved: both the real and emulated calls are HTTP 200.

Emulator: the handler walks `cluster.certificate_authorities.values()` — a map keyed by CA id — and explicitly treats that id order as the listing order ("The records are keyed by certificate authority id, so walking the map walks the ids in order"), and the page token resumes on `record.id.as_str() > after`. The listing is therefore sorted lexicographically by the generated CA UUID, which is unrelated to `createdAt`. In the replay the emulator answered the two-element listing as [`3fa3bd89-…` createdAt 18:25:17, `signingStatus: IN_USE`], [`96630f7f-…` createdAt 18:25:37, `signingStatus: NOT_USED`] — i.e. the CA created second is last — and the check failed on `expected newest CA first`. Because the paging comparison is also on id, every page inherits the same wrong order rather than only the unpaginated call.

Why: a client that rotates a cluster CA reads element [0] of the listing to find the successor CA it must distribute/activate (and reads the tail to find the outgoing signer). Against the emulator it gets whichever CA happens to sort first by UUID, so roughly half of all rotations pick the wrong CA — it will try to activate the already-`IN_USE` outgoing CA, or treat the fresh `NOT_USED`/`IN_PROGRESS` successor as the current signer. Ordering being id-derived also makes the result non-deterministic across runs (ids are random), so a client cannot even rely on a stable-but-wrong order.

Fix: belongs in the `ListCertificateAuthorities` handler's page assembly — the listing must materialise the cluster's CA records into a sequence ordered by `createdAt` descending (with a deterministic tie-break, e.g. id, since both CAs can be created within the same clock tick) before applying `maxResults`/`nextToken`, and the resume-after comparison must follow that same order key rather than comparing ids, so a token resumes the newest-first walk. Id-keyed map iteration should remain only as a lookup index, not as the listing order.

Traces: eks.list-certificate-authorities-newest-first-ordering. Handler and the comments that encode the wrong order: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_certificate_authorities.rs lines 6-7, 263-285. Not covered by the open issue 20261029T175608Z-eks-8-net-new-root-causes-in-certificate.md, which bundles certificate-authority/pod-identity REFUSAL defects (wrong error codes, messages, PassRole ordering) and says nothing about the success-path ordering of this listing.

## Evidence
Check eks.list-certificate-authorities-newest-first-ordering (mismatch).

Expected (recorded AWS): after creating a second CA on an ACTIVE cluster, `list_certificate_authorities(clusterName=...)["certificateAuthorities"]` is newest-createdAt-first, so [0].id == the id just returned by CreateCertificateAuthority and [1].id == the pre-existing EKS-managed CA; [0].createdAt >= [1].createdAt; with maxResults=1 page1[0] == full[0], a nextToken is present, page2[0] == full[1], and page2 has no nextToken.

Actual (emulator): HTTP 200 with the list in CA-id order:
  [{'id': '3fa3bd89-20f8-4201-8b63-a474afb0863f', 'createdAt': 2026-10-29 18:25:17.249824, 'createdBy': 'EKS', 'activatedAt': 2026-10-29 18:25:17.249824, 'activatedBy': 'EKS', 'signingStatus': 'IN_USE', 'distributionStatus': 'COMPLETE'},
   {'id': '96630f7f-e654-4e21-b9fc-9e66cc90a0a4', 'createdAt': 2026-10-29 18:25:37.278105, 'createdBy': 'CUSTOMER', 'signingStatus': 'NOT_USED', 'distributionStatus': 'IN_PROGRESS'}]
AssertionError: expected newest CA (96630f7f-e654-4e21-b9fc-9e66cc90a0a4) first.

The specific ids, timestamps and cluster name above are per-run values that fill the template; the requirement is the ORDER (descending createdAt, preserved across pages), not these values. Source of the behavior: services/service-com-amazonaws-eks-awswesleyfrontend/src/ops/list_certificate_authorities.rs:267-285 iterates `certificate_authorities.values()` (id-keyed map) and pages with `record.id.as_str() > after`.

## Steps to Reproduce
1. Create an EKS cluster role (iam:CreateRole + attach arn:aws:iam::aws:policy/AmazonEKSClusterPolicy), a VPC and two subnets, then eks:CreateCluster and wait for status ACTIVE.
2. eks:ListCertificateAuthorities(clusterName=<cluster>) -> exactly one service-managed CA (createdBy EKS, signingStatus IN_USE).
3. eks:CreateCertificateAuthority(clusterName=<cluster>) -> note the returned certificateAuthority.id (the successor; createdAt is later than the CA from step 2).
4. eks:ListCertificateAuthorities(clusterName=<cluster>) -> AWS returns the id from step 3 at index 0 (newest createdAt first); the emulator returns whichever id sorts first lexicographically, which here puts the older IN_USE CA at index 0.
5. eks:ListCertificateAuthorities(clusterName=<cluster>, maxResults=1), then repeat with the returned nextToken -> AWS walks the same newest-first order across the two pages; the emulator walks id order.
