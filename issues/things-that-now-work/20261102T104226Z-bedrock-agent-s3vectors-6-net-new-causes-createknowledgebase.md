Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agent__s3vectors.kbcfn-create-kb-on-a-missing-vector-index-rejected bedrock-agent__s3vectors.kbcfn-create-kb-default-dimensions-on-a-512-index-rejected bedrock-agent__s3vectors.kbcfn-create-kb-with-a-non-embedding-model-rejected bedrock-agent__s3vectors.kbcfn-update-kb-embedding-model-rejected-as-immutable bedrock-agent__s3vectors.kbcfn-update-kb-vector-index-rejected-as-immutable bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store
Issue-Key: bedrock-agent__s3vectors.kbcfn-create-kb-on-a-missing-vector-index-rejected|run#4|AssertionError|assert kb is None, f"a KB on a missing index was accepted: {kb!r}"
Issue-Key: bedrock-agent__s3vectors.kbcfn-create-kb-default-dimensions-on-a-512-index-rejected|run#4|AssertionError|assert kb is None, f"a 1024-d model was accepted on a 512-d index: {kb!r}"
Issue-Key: bedrock-agent__s3vectors.kbcfn-create-kb-with-a-non-embedding-model-rejected|run#5|AssertionError|assert kb is None, f"a text model was accepted as an embedding model: {kb!r}"
Issue-Key: bedrock-agent__s3vectors.kbcfn-update-kb-embedding-model-rejected-as-immutable|run#6|AssertionError|assert (code, msg) == ("ValidationException", want_msg), (
Issue-Key: bedrock-agent__s3vectors.kbcfn-update-kb-vector-index-rejected-as-immutable|run#6|AssertionError|assert (code, msg) == (
Issue-Key: bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store|_run_ingestion_job#16|AssertionError|assert stats["numberOfNewDocumentsIndexed"] == document_count, (
Issue-Commit: fb0a9c1858b32d12ecab08653c5d3b6198801bbb

# bedrock-agent__s3vectors: 6 net-new causes — CreateKnowledgeBase skips every S3-Vectors binding pre-condition (missing index, dimension mismatch, non-embedding model), both UpdateKnowledgeBase immutability refusals are re-worded, and ingestion reports numberOfNewDocumentsIndexed=0

> _Note: this issue was identified against the code base at commit `fb0a9c1858b32d12ecab08653c5d3b6198801bbb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateKnowledgeBase

## Description
SIX net-new root causes of service `bedrock-agent__s3vectors` (the service heading the suite report lists these checks under), each grounded in its own failing behavioral check that the replay report left unmarked (neither ALREADY FILED nor OWNED). A fixer must close ALL SIX before submitting.

=== Cause 1 of 6: CreateKnowledgeBase does not verify that the S3 Vectors index named in storageConfiguration exists ===
AWS: Smithy — CreateKnowledgeBase (PUT /knowledgebases/, success 202) takes a required `knowledgeBaseConfiguration` and an optional `storageConfiguration`, and models `ValidationException` (errorKind client, httpError 400, doc "Input validation failed. Check your request parameters and retry the request."). The model does not express the cross-service binding rule, so the recording supplies it: Bedrock resolves the named vector index at create time and FORWARDS S3 Vectors' own refusal. A request whose `s3VectorsConfiguration.indexName` is not an index of the named vector bucket is refused HTTP 400, Error.Code `ValidationException`, message `The knowledge base storage configuration provided is invalid... The specified index could not be found (Service: S3Vectors, Status Code: 404, Request ID: <request-id>) (SDK Attempt Count: 1)` — fixed text everywhere except `<request-id>`, which is the per-call forwarded S3 Vectors request id (per-run, not part of the requirement); no knowledge base is created, so ListKnowledgeBases does not show it. The identical request naming a real index is accepted.
Emulator: the create is ACCEPTED (202) and the knowledge base goes ACTIVE with `storageConfiguration.s3VectorsConfiguration.indexName` pointing at an index that does not exist. services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/create_knowledge_base.rs validates only the configuration-discriminator shape (`knowledge_base_configuration_problem`) and the account quota; it never resolves the vector bucket or index.
Why: a client relying on AWS's behavior learns at create time that its vector store is mis-wired and fails its deployment there. Against the emulator the mistake is accepted, the knowledge base reports ACTIVE, and every ingestion and retrieval against it later fails or silently stores nothing — the defect surfaces far from its cause, and no error-handling path keyed on ValidationException is taken.
Fix: the CreateKnowledgeBase handler's validation in the bedrock-agent crate — resolve the S3_VECTORS binding (bucket ARN + indexName) against the s3vectors store before any record is written, and emit the forwarded ValidationException sentence above; leave no knowledge base behind.
Traces: bedrock-agent__s3vectors.kbcfn-create-kb-on-a-missing-vector-index-rejected.

=== Cause 2 of 6: CreateKnowledgeBase does not check the embedding model's vector dimension against the bound index's dimension ===
AWS: Smithy — as Cause 1, plus `vectorKnowledgeBaseConfiguration.embeddingModelConfiguration.bedrockEmbeddingModelConfiguration.dimensions` as the optional override. Recorded behavior: Bedrock validates the binding with a probe query against the bound index and forwards that index's refusal, so a Titan-v2 knowledge base with NO dimensions override (1024-d by default) bound to a 512-d index is refused HTTP 400 `ValidationException` with message `The knowledge base storage configuration provided is invalid... Query vector contains invalid values or is invalid for this index (Service: S3Vectors, Status Code: 400, Request ID: <request-id>) (SDK Attempt Count: <n>)` — all fixed text except the forwarded `<request-id>` (a UUID) and `<n>` (the SDK attempt count), both per-run. No record is left behind. The control — the same request declaring dimensions=512 — is accepted, reaches ACTIVE and reads the override back as `{"bedrockEmbeddingModelConfiguration": {"dimensions": 512}}`.
Emulator: the mismatched create is ACCEPTED and the knowledge base reports status ACTIVE on a 512-d index with a 1024-d model.
Why: dimension agreement is the one invariant that makes a vector store usable; AWS refuses the binding up front. Against the emulator the knowledge base is reported healthy and the mismatch can only be discovered when embeddings are written or queried, so a deployment that can never work is recorded as successful.
Fix: the same CreateKnowledgeBase validation stage — compare the effective embedding dimension (the model's default, or the declared `dimensions` override) with the bound index's dimension and emit the forwarded refusal above.
Traces: bedrock-agent__s3vectors.kbcfn-create-kb-default-dimensions-on-a-512-index-rejected.

=== Cause 3 of 6: CreateKnowledgeBase accepts a text-generation model ARN as the embedding model ===
AWS: Smithy — `vectorKnowledgeBaseConfiguration.embeddingModelArn` is the embedding model, and CreateKnowledgeBase models ValidationException (400); the model does not enumerate which foundation models qualify, so the recording supplies the rule. Recorded behavior: an `embeddingModelArn` naming a text-generation model (e.g. a nova-lite foundation-model ARN) is refused HTTP 400 `ValidationException` with message `The specified embedding model <embeddingModelArn> is not supported.` — fixed text `The specified embedding model ` and ` is not supported.`, with `<embeddingModelArn>` the ARN copied from the request; and the refused knowledge base never appears in ListKnowledgeBases. The identical request with an embedding model (amazon.titan-embed-text-v2:0) is accepted and reaches ACTIVE.
Emulator: the create is ACCEPTED (202), the knowledge base goes ACTIVE, and GetKnowledgeBase reports the text-generation ARN as its embeddingModelArn.
Why: the embedding-model allow-list is the pre-condition that keeps a knowledge base's vectors meaningful; a client relying on AWS's refusal catches a mis-pasted model ARN at create. Against the emulator the knowledge base is created and claims to be healthy while nothing it ever embeds can be correct.
Fix: the same CreateKnowledgeBase validation stage (and the matching UpdateKnowledgeBase path) — refuse an `embeddingModelArn` that is not an embedding model with AWS's sentence before any record is written.
Traces: bedrock-agent__s3vectors.kbcfn-create-kb-with-a-non-embedding-model-rejected.

=== Cause 4 of 6: UpdateKnowledgeBase's knowledgeBaseConfiguration-immutability refusal uses the emulator's own wording ===
AWS: Smithy — UpdateKnowledgeBase (PUT /knowledgebases/{knowledgeBaseId}) requires `knowledgeBaseConfiguration` to be re-sent and models ValidationException (400); its own doc states the configuration cannot be changed. Recorded behavior: an update that changes `vectorKnowledgeBaseConfiguration.embeddingModelArn` — and, separately, one that merely ADDS an `embeddingModelConfiguration.bedrockEmbeddingModelConfiguration.dimensions` — is each refused HTTP 400 `ValidationException` with the message `You cannot modify the Vector knowledge base configuration once it is created.` That sentence is entirely FIXED text: nothing in it comes from the request or from state. The stored vector configuration is unchanged afterwards, and a control update that re-sends the identical configuration while changing only `description` succeeds and leaves the knowledge base ACTIVE.
Emulator: the code is right but the message is `knowledgeBaseConfiguration cannot be updated once created.` — a member-path sentence the service never emits, produced by the shared immutability formatter reached from services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/src/ops/update_knowledge_base.rs (crate::state::immutable_member_verdict, member name "knowledgeBaseConfiguration").
Why: within a generic ValidationException the message is the only machine-usable discriminator, so a client that branches on AWS's sentence ("recreate the knowledge base instead of updating it") does not recognise the refusal and cannot tell an immutability refusal from a malformed-configuration one.
Fix: the immutability refusal messages in the UpdateKnowledgeBase handler / the shared `immutable_member_verdict` formatter — emit AWS's fixed sentence for the knowledgeBaseConfiguration member instead of a generic '<member> cannot be updated once created.' template.
Traces: bedrock-agent__s3vectors.kbcfn-update-kb-embedding-model-rejected-as-immutable.

=== Cause 5 of 6: UpdateKnowledgeBase's storageConfiguration-immutability refusal uses the emulator's own wording ===
AWS: Smithy — as Cause 4, for the `storageConfiguration` member. Recorded behavior: an update that re-points `s3VectorsConfiguration.indexName` at a second index is refused HTTP 400 `ValidationException` with the message `You cannot modify the storage configuration of the Vector knowledge base once created.` — again entirely FIXED text, with nothing interpolated; the knowledge base keeps its original indexName, and re-sending the identical storage configuration succeeds.
Emulator: the code is right, the message is `storageConfiguration cannot be updated once created.` — the same generic member-path template, so this is a second distinct AWS sentence that must be produced; it is NOT satisfied by fixing Cause 4 alone.
Why: as Cause 4 — a client keying on AWS's storage-configuration sentence to decide "create a new knowledge base against the new index" never matches the emulator's text.
Fix: the same formatter — a distinct fixed sentence per immutable member, matching AWS's wording for storageConfiguration.
Traces: bedrock-agent__s3vectors.kbcfn-update-kb-vector-index-rejected-as-immutable.

=== Cause 6 of 6: an ingestion job over an S3 data source holding documents completes reporting numberOfNewDocumentsIndexed = 0 ===
AWS: Smithy — IngestionJobStatistics members are PrimitiveLong with default 0; `numberOfNewDocumentsIndexed` is documented as "The number of new source documents in the data source that were successfully indexed.", alongside numberOfDocumentsScanned ("The total number of source documents that were scanned. Includes new, updated, and unchanged documents."), numberOfModifiedDocumentsIndexed, numberOfDocumentsDeleted, numberOfDocumentsFailed and numberOfDocumentsSkipped; the structure is returned on the job by StartIngestionJob / GetIngestionJob / ListIngestionJobs. The counters are the service's own accounting, so a job that reaches COMPLETE over a data source holding N previously-unseen source documents reports numberOfNewDocumentsIndexed == N with failed and skipped at 0, and the vectors are present in the bound S3 Vectors index (readable through s3vectors ListVectors).
Emulator: the job is accepted and reaches COMPLETE, but its statistics come back with numberOfNewDocumentsIndexed = 0 for a data source holding 2 documents, with nothing reported failed or skipped either — the counters are the Smithy zero defaults, so no source enumeration or vector write is happening.
Why: numberOfNewDocumentsIndexed is the signal every knowledge-base sync pipeline polls to decide "retry the sync", "alert on an empty/mis-prefixed bucket" or "proceed to query the KB". Against the emulator a successful ingestion is indistinguishable from one that silently indexed nothing, and every downstream behavior that needs vectors to exist (here, the dataDeletionPolicy RETAIN/DELETE semantics at the vector store) is unreachable.
Fix: the ingestion-job execution and statistics accounting in the bedrock-agent crate — StartIngestionJob's advanced job state, read back by GetIngestionJob, must enumerate the data source's source objects, write them into the bound S3 Vectors index and populate IngestionJobStatistics from that measured work instead of leaving the modeled defaults. NOTE: open issue 20261029T195812Z-bedrock-agent-s3vectors-bedrock-agent-ingestion-into-an-s3-vectors-store.md records this same accounting gap for this service token and this check id, and 20261030T000542Z-bedrock-agent-5-net-new-causes-vector-knowledge.md records it (as numberOfDocumentsScanned=0) under the `bedrock-agent` token; the replay report marked this check neither ALREADY FILED nor OWNED, so it is re-filed here with its current replay key and should close together with those issues if their fix lands first.
Traces: bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store.

## Evidence
Cause 1 — bedrock-agent__s3vectors.kbcfn-create-kb-on-a-missing-vector-index-rejected [mismatch]:
  AssertionError: a KB on a missing index was accepted: {'knowledgeBaseId': 'V6DJOjOYLJ', ..., 'storageConfiguration': {'type': 'S3_VECTORS', 's3VectorsConfiguration': {'vectorBucketArn': 'arn:aws:s3vectors:…:bucket/<prefix>-vec', 'indexName': 'idx-missing'}}, 'status': 'ACTIVE', …}
  (check line 29) Expected ValidationException 400 with 'The knowledge base storage configuration provided is invalid... The specified index could not be found (Service: S3Vectors, Status Code: 404, Request ID: <request-id>) (SDK Attempt Count: 1)' and no KB created; actual 202 + ACTIVE. The quoted ids/ARNs are this run's generated values.

Cause 2 — bedrock-agent__s3vectors.kbcfn-create-kb-default-dimensions-on-a-512-index-rejected [mismatch]:
  AssertionError: a 1024-d model was accepted on a 512-d index: {'knowledgeBaseId': '5ryAJ9Yc1Z', …, 's3VectorsConfiguration': {…, 'indexName': 'idx-512'}, 'status': 'ACTIVE', …}
  (check line 28) Expected ValidationException 400 with 'The knowledge base storage configuration provided is invalid... Query vector contains invalid values or is invalid for this index (Service: S3Vectors, Status Code: 400, Request ID: <uuid>) (SDK Attempt Count: <n>)'; actual the create succeeded.

Cause 3 — bedrock-agent__s3vectors.kbcfn-create-kb-with-a-non-embedding-model-rejected [mismatch]:
  AssertionError: a text model was accepted as an embedding model: {'knowledgeBaseId': '7P6NmVLDxN', …, 'vectorKnowledgeBaseConfiguration': {'embeddingModelArn': 'arn:aws:bedrock:us-west-2::foundation-model/amazon.nova-lite-v1:0'}, 'status': 'ACTIVE', …}
  (check line 29) Expected ValidationException 400 with 'The specified embedding model <that ARN> is not supported.'; actual 202 + ACTIVE.

Cause 4 — bedrock-agent__s3vectors.kbcfn-update-kb-embedding-model-rejected-as-immutable [mismatch]:
  AssertionError: changing the model answered 'ValidationException': 'knowledgeBaseConfiguration cannot be updated once created.'
  (check line 38) Code matches; expected message 'You cannot modify the Vector knowledge base configuration once it is created.' (wholly fixed text). The same divergence applies to the dimensions-added leg of the same check.

Cause 5 — bedrock-agent__s3vectors.kbcfn-update-kb-vector-index-rejected-as-immutable [mismatch]:
  AssertionError: re-pointing the index answered 'ValidationException': 'storageConfiguration cannot be updated once created.'
  (check line 34) Code matches; expected message 'You cannot modify the storage configuration of the Vector knowledge base once created.' (wholly fixed text).

Cause 6 — bedrock-agent__s3vectors.data-deletion-policy-governs-vector-removal-at-the-store [mismatch]:
  AssertionError: RETAIN arm ingestion: expected all 2 documents to be indexed as new, got numberOfNewDocumentsIndexed=0
  (check line 448, from run() line 65 — the FIRST ingestion arm, so the dataDeletionPolicy discrimination the check exists for is never reached.) numberOfDocumentsFailed was also 0, so the 2 documents are unaccounted for across every counter. The document count 2 is the check's setup; the requirement is that a COMPLETE job over N new documents reports N.

## Steps to Reproduce
0. git_clone, then run_behavioral_suite and read the `### bedrock-agent__s3vectors` section of the failing-check list.

Shared setup for Causes 1-5: create an s3vectors vector bucket and the indexes each cause needs (float32, cosine), and an IAM role trusting bedrock.amazonaws.com with bedrock:InvokeModel and the s3vectors:* read/write actions.

Cause 1: bedrock-agent.create_knowledge_base(name=<unique>, roleArn=<role>, knowledgeBaseConfiguration={'type':'VECTOR','vectorKnowledgeBaseConfiguration':{'embeddingModelArn':'arn:aws:bedrock:<region>::foundation-model/amazon.titan-embed-text-v2:0'}}, storageConfiguration={'type':'S3_VECTORS','s3VectorsConfiguration':{'vectorBucketArn':<bucket arn>,'indexName':'idx-missing'}}) where the bucket holds only 'idx-a'. Observe a 202 and an ACTIVE knowledge base; required: ValidationException 400 forwarding S3 Vectors' 404 'The specified index could not be found', and nothing in ListKnowledgeBases. Control: the same call naming 'idx-a' must be accepted.

Cause 2: create a 512-dimension index 'idx-512' and call create_knowledge_base with the titan-embed-text-v2:0 model and NO embeddingModelConfiguration. Observe acceptance; required: ValidationException 400 forwarding S3 Vectors' 400 'Query vector contains invalid values or is invalid for this index'. Control: the same call with embeddingModelConfiguration.bedrockEmbeddingModelConfiguration.dimensions=512 must be accepted and read the override back.

Cause 3: create_knowledge_base with embeddingModelArn='arn:aws:bedrock:<region>::foundation-model/amazon.nova-lite-v1:0'. Observe acceptance; required: ValidationException 400 'The specified embedding model <that ARN> is not supported.' and the KB absent from ListKnowledgeBases.

Cause 4: create a valid Titan-v2 knowledge base, wait for ACTIVE, then update_knowledge_base re-sending name/roleArn but with knowledgeBaseConfiguration.vectorKnowledgeBaseConfiguration.embeddingModelArn changed to cohere.embed-english-v3; repeat with the original ARN plus embeddingModelConfiguration.bedrockEmbeddingModelConfiguration.dimensions=512. Each is refused ValidationException, but with 'knowledgeBaseConfiguration cannot be updated once created.' instead of 'You cannot modify the Vector knowledge base configuration once it is created.'

Cause 5: on the same knowledge base, update_knowledge_base with storageConfiguration.s3VectorsConfiguration.indexName changed to a second index. Refused ValidationException with 'storageConfiguration cannot be updated once created.' instead of 'You cannot modify the storage configuration of the Vector knowledge base once created.'

Cause 6: create an S3 bucket holding 2 small text objects under one prefix; create a Titan-v2 / 1024-d S3_VECTORS knowledge base over a matching index; create_data_source for that prefix (dataDeletionPolicy=RETAIN); start_ingestion_job and poll get_ingestion_job until COMPLETE; read `statistics` -> numberOfNewDocumentsIndexed = 0 (failed and skipped also 0) where 2 is required, and s3vectors list_vectors on the index returns none.

Re-replay all six: the check ids listed in trace_ids.
