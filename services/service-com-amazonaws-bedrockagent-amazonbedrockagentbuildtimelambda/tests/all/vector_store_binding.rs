// SPDX-License-Identifier: CC0-1.0

//! The pre-conditions `CreateKnowledgeBase` resolves before it creates
//! anything, end to end over HTTP.
//!
//! A knowledge base writes its vectors with an embedding model into a vector
//! index, and `CreateKnowledgeBase` documents both as things the caller sets up
//! first ("To create a knowledge base, you must first set up your data sources
//! and configure a supported vector store"). Two rules follow, and this suite
//! pins each over a TABLE rather than one example:
//!
//! * the `embeddingModelArn` must name a model whose published output is an
//!   embedding — the rows span the published output modalities, both ARN forms
//!   the modelled pattern admits, and a model nothing publishes;
//! * the index named in `storageConfiguration` must be an index of the named
//!   vector bucket whose dimension the model's vectors fit — the rows span a
//!   missing index, a missing bucket, both forms `S3VectorsConfiguration`
//!   carries the index in, several index dimensions and the `dimensions`
//!   override.
//!
//! A refused create leaves NOTHING behind: every row asserts the store holds no
//! more knowledge bases after the refusal than before it.
//!
//! Every assertion here goes through the router, so it exercises the real wire
//! path (route matching, model constraint validation, serialization) rather than
//! calling a handler directly.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::state::service_state;
use service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use crate::vector_store_fixture::{seed_index, EMBEDDING_DIMENSION};

/// A SigV4 `Authorization` header scoped to the `bedrock` signing name: the
/// routing key this service claims on.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-1/bedrock/aws4_request, \
     SignedHeaders=host;x-amz-date, Signature=0123456789abcdef";

/// A role ARN matching the modelled `KnowledgeBaseRoleArn` pattern.
const KB_ROLE: &str =
    "arn:aws:iam::000000000000:role/service-role/AmazonBedrockExecutionRoleForKnowledgeBase";

/// The vector bucket every row below binds to, bar the rows that name one
/// nothing holds.
const VECTOR_BUCKET: &str = "arn:aws:s3vectors:::bucket/binding-vectors";

/// The index of [`VECTOR_BUCKET`] whose dimension the suite's default embedding
/// model fits.
const INDEX: &str = "binding-index";

/// A second index of [`VECTOR_BUCKET`], holding vectors of 512 values — half
/// what `amazon.titan-embed-text-v2:0` produces by default, and one of the
/// overrides that model publishes.
const SMALL_INDEX: &str = "binding-index-512";

/// The dimension of [`SMALL_INDEX`].
const SMALL_DIMENSION: i32 = 512;

/// A third index, holding the 1536 values `amazon.titan-embed-text-v1`
/// produces.
const WIDE_INDEX: &str = "binding-index-1536";

/// The dimension of [`WIDE_INDEX`].
const WIDE_DIMENSION: i32 = 1536;

/// The sentence this service opens a refusal it carries through from the vector
/// store with, as the recorded response words it.
const FORWARDED_OPENING: &str = "The knowledge base storage configuration provided is invalid...";

/// The embedding model ARN of a row that is not about the model.
const EMBEDDING_MODEL: &str =
    "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v2:0";

/// One HTTP answer from the emulator, parsed.
#[derive(Debug)]
struct Answer {
    status: u16,
    /// The restJson1 error shape name, from `x-amzn-errortype` or the body's
    /// `__type`; `None` on a success.
    error_type: Option<String>,
    body: Value,
}

impl Answer {
    fn member(&self, path: &[&str]) -> &Value {
        let mut at = &self.body;
        for key in path {
            at = &at[*key];
        }
        at
    }

    fn message(&self) -> String {
        self.member(&["message"])
            .as_str()
            .unwrap_or_default()
            .to_string()
    }
}

/// The emulator under test: one router over one shared engine, exactly as the
/// production router mounts this service.
struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        // The vector store a caller sets up BEFORE naming it: one bucket with
        // three indexes of different dimensions.
        for (index, dimension) in [
            (INDEX, EMBEDDING_DIMENSION),
            (SMALL_INDEX, SMALL_DIMENSION),
            (WIDE_INDEX, WIDE_DIMENSION),
        ] {
            seed_index(&engine, VECTOR_BUCKET, index, dimension);
        }
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    async fn call(&self, method: &str, uri: &str, body: Option<Value>) -> Answer {
        let bytes = body
            .map(|value| Bytes::from(serde_json::to_vec(&value).expect("serialize request body")))
            .unwrap_or_default();
        let request = http::Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "bedrock-agent.eu-west-1.amazonaws.com")
            .header("authorization", AUTHORIZATION)
            .header("content-type", "application/json")
            .body(bytes)
            .expect("build the request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let header_error = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(ToString::to_string);
        let raw = response.into_body();
        let body: Value = if raw.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&raw).unwrap_or_else(|error| {
                panic!(
                    "{method} {uri} -> {status} body is not JSON ({error}): {}",
                    String::from_utf8_lossy(&raw)
                )
            })
        };
        let error_type = header_error.or_else(|| {
            body.get("__type")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
        Answer {
            status,
            error_type,
            body,
        }
    }

    async fn create_knowledge_base(&self, request: Value) -> Answer {
        self.call("PUT", "/knowledgebases/", Some(request)).await
    }

    async fn get_knowledge_base(&self, knowledge_base_id: &str) -> Answer {
        self.call("GET", &format!("/knowledgebases/{knowledge_base_id}"), None)
            .await
    }

    /// How many knowledge bases this service's store holds — what says whether a
    /// refused create left a record behind.
    fn stored_knowledge_bases(&self) -> usize {
        let state = service_state(&*self.engine).expect("this service's store on the engine");
        let held = state.lock().knowledge_bases.len();
        held
    }

    /// One `ListKnowledgeBases` page: the ids it reports, and the token it hands
    /// out.
    async fn list_knowledge_bases(
        &self,
        max_results: Option<i64>,
        next_token: Option<&str>,
    ) -> (Vec<String>, Option<String>) {
        let mut body = json!({});
        if let Some(max_results) = max_results {
            body["maxResults"] = json!(max_results);
        }
        if let Some(next_token) = next_token {
            body["nextToken"] = json!(next_token);
        }
        let listed = self.call("POST", "/knowledgebases/", Some(body)).await;
        assert_eq!(
            listed.status, 200,
            "case setup: expected ListKnowledgeBases -> 200, got {listed:?}"
        );
        let ids = listed
            .member(&["knowledgeBaseSummaries"])
            .as_array()
            .unwrap_or_else(|| panic!("knowledgeBaseSummaries is a list in {:?}", listed.body))
            .iter()
            .map(|summary| {
                summary["knowledgeBaseId"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        let next = listed
            .member(&["nextToken"])
            .as_str()
            .map(ToString::to_string);
        (ids, next)
    }
}

/// A `CreateKnowledgeBase` body over `embedding_model`, with the storage
/// configuration the row carries and whatever `dimensions` override it declares.
fn create_request(
    name: &str,
    embedding_model: &str,
    dimensions: Option<i32>,
    storage: Value,
) -> Value {
    let mut vector = json!({ "embeddingModelArn": embedding_model });
    if let Some(dimensions) = dimensions {
        vector["embeddingModelConfiguration"] = json!({
            "bedrockEmbeddingModelConfiguration": { "dimensions": dimensions },
        });
    }
    json!({
        "name": name,
        "roleArn": KB_ROLE,
        "knowledgeBaseConfiguration": { "type": "VECTOR", "vectorKnowledgeBaseConfiguration": vector },
        "storageConfiguration": storage,
    })
}

/// An S3 Vectors storage configuration naming the index by `vectorBucketArn`
/// and `indexName` — one of the two forms `S3VectorsConfiguration` carries it
/// in.
fn by_name(vector_bucket_arn: &str, index_name: &str) -> Value {
    json!({
        "type": "S3_VECTORS",
        "s3VectorsConfiguration": {
            "vectorBucketArn": vector_bucket_arn,
            "indexName": index_name,
        },
    })
}

/// The same binding by `indexArn`, the other form: the ARN names the bucket as
/// well as the index.
fn by_arn(vector_bucket_arn: &str, index_name: &str) -> Value {
    json!({
        "type": "S3_VECTORS",
        "s3VectorsConfiguration": {
            "indexArn": format!("{vector_bucket_arn}/index/{index_name}"),
        },
    })
}

/// Assert `answer` is the modelled `ValidationException` (HTTP 400) and report
/// its message.
fn validation_message(case: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status, 400,
        "case {case}: expected the refused create to answer HTTP 400, got {answer:?}"
    );
    assert_eq!(
        answer.error_type.as_deref(),
        Some("ValidationException"),
        "case {case}: expected the modelled ValidationException, got {:?}",
        answer.error_type
    );
    answer.message()
}

/// One row of the embedding-model table.
struct ModelRow {
    case: &'static str,
    /// The `embeddingModelArn` the request carries.
    embedding_model: String,
    /// `true` when the create must be served.
    accepted: bool,
}

/// `CreateKnowledgeBase` refuses an `embeddingModelArn` whose published output
/// is not an embedding, and serves one whose is.
///
/// The rows span the published output modalities of the foundation-model
/// catalog — embedding, text and image — both forms the modelled
/// `BedrockEmbeddingModelArn` pattern admits (a full ARN and a bare model id),
/// and an ARN of a model nothing publishes, which says nothing about what it
/// produces and so is not refused on it.
#[tokio::test]
async fn an_embedding_model_that_produces_no_embedding_is_refused() {
    let emulator = Emulator::new();
    let rows = [
        ModelRow {
            case: "the published embedding model the suite creates with",
            embedding_model: EMBEDDING_MODEL.to_string(),
            accepted: true,
        },
        ModelRow {
            case: "another provider's published embedding model",
            embedding_model:
                "arn:aws:bedrock:eu-west-1::foundation-model/cohere.embed-english-v3".to_string(),
            accepted: true,
        },
        ModelRow {
            case: "an embedding model named by its bare model id",
            embedding_model: "amazon.titan-embed-text-v2:0".to_string(),
            accepted: true,
        },
        ModelRow {
            case: "a text-generation model",
            embedding_model: "arn:aws:bedrock:eu-west-1::foundation-model/amazon.nova-lite-v1:0"
                .to_string(),
            accepted: false,
        },
        ModelRow {
            case: "another provider's text-generation model",
            embedding_model:
                "arn:aws:bedrock:eu-west-1::foundation-model/anthropic.claude-3-haiku-20240307-v1:0"
                    .to_string(),
            accepted: false,
        },
        ModelRow {
            case: "an image-generation model",
            embedding_model:
                "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-image-generator-v1"
                    .to_string(),
            accepted: false,
        },
        ModelRow {
            case: "a text-generation model named by its bare model id",
            embedding_model: "amazon.nova-lite-v1:0".to_string(),
            accepted: false,
        },
        ModelRow {
            case: "a model ARN the catalog publishes nothing under",
            embedding_model:
                "arn:aws:sagemaker:eu-west-1:000000000000:endpoint/an-embedding-endpoint"
                    .to_string(),
            accepted: true,
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        let before = emulator.stored_knowledge_bases();
        let answer = emulator
            .create_knowledge_base(create_request(
                &format!("binding-model-kb-{position}"),
                &row.embedding_model,
                None,
                by_name(VECTOR_BUCKET, INDEX),
            ))
            .await;

        if row.accepted {
            assert_eq!(
                answer.status, 202,
                "case {}: expected the create to be served, got {answer:?}",
                row.case
            );
            let knowledge_base_id = answer
                .member(&["knowledgeBase", "knowledgeBaseId"])
                .as_str()
                .unwrap_or_else(|| panic!("case {}: a minted knowledgeBaseId", row.case))
                .to_string();
            // The round trip: the embedding model the create was served with is
            // the one the stored record reports.
            let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
            assert_eq!(
                fetched.member(&[
                    "knowledgeBase",
                    "knowledgeBaseConfiguration",
                    "vectorKnowledgeBaseConfiguration",
                    "embeddingModelArn",
                ]),
                &json!(row.embedding_model),
                "case {}: expected the stored embedding model to read back, got {:?}",
                row.case,
                fetched.body
            );
            assert_eq!(
                emulator.stored_knowledge_bases(),
                before + 1,
                "case {}: a served create stores exactly one knowledge base",
                row.case
            );
            continue;
        }

        let message = validation_message(row.case, &answer);
        assert_eq!(
            message,
            format!(
                "The specified embedding model {} is not supported.",
                row.embedding_model
            ),
            "case {}: expected AWS's unsupported-model sentence, got {message:?}",
            row.case
        );
        assert_eq!(
            emulator.stored_knowledge_bases(),
            before,
            "case {}: a refused create must leave no knowledge base behind",
            row.case
        );
    }
}

/// One row of the storage-binding table.
struct BindingRow {
    case: &'static str,
    /// The storage configuration the request carries.
    storage: Value,
    /// The `dimensions` override the request declares, if any.
    dimensions: Option<i32>,
    /// The embedding model the request names.
    embedding_model: &'static str,
    /// The vector store's own sentence the refusal must carry through, or
    /// `None` when the create must be served.
    forwarded: Option<&'static str>,
    /// The HTTP status the vector store answered, as the forwarded tail reports
    /// it.
    forwarded_status: u16,
}

/// `CreateKnowledgeBase` resolves the S3 Vectors index its storage
/// configuration names and carries that store's own refusal through, and leaves
/// no knowledge base behind when it does.
///
/// The rows span what the binding can be wrong about: an index the named bucket
/// does not hold, a bucket nothing holds, and a model whose vectors do not fit
/// the index's dimension — against the served rows that name a real index, by
/// either of the two forms, at three different dimensions, with and without the
/// `dimensions` override.
#[tokio::test]
async fn a_storage_configuration_the_vector_store_refuses_is_carried_through() {
    let emulator = Emulator::new();
    let rows = vec![
        BindingRow {
            case: "an index the named vector bucket does not hold",
            storage: by_name(VECTOR_BUCKET, "no-such-index"),
            dimensions: None,
            embedding_model: EMBEDDING_MODEL,
            forwarded: Some("The specified index could not be found"),
            forwarded_status: 404,
        },
        BindingRow {
            case: "a vector bucket nothing holds",
            storage: by_name("arn:aws:s3vectors:::bucket/no-such-vector-bucket", INDEX),
            dimensions: None,
            embedding_model: EMBEDDING_MODEL,
            forwarded: Some("The specified index could not be found"),
            forwarded_status: 404,
        },
        BindingRow {
            case: "an indexArn naming an index nothing holds",
            storage: by_arn(VECTOR_BUCKET, "no-such-index"),
            dimensions: None,
            embedding_model: EMBEDDING_MODEL,
            forwarded: Some("The specified index could not be found"),
            forwarded_status: 404,
        },
        BindingRow {
            case: "a 1024-value model on a 512-value index",
            storage: by_name(VECTOR_BUCKET, SMALL_INDEX),
            dimensions: None,
            embedding_model: EMBEDDING_MODEL,
            forwarded: Some("Query vector contains invalid values or is invalid for this index"),
            forwarded_status: 400,
        },
        BindingRow {
            case: "a 1536-value model on a 1024-value index",
            storage: by_name(VECTOR_BUCKET, INDEX),
            dimensions: None,
            embedding_model:
                "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v1",
            forwarded: Some("Query vector contains invalid values or is invalid for this index"),
            forwarded_status: 400,
        },
        BindingRow {
            case: "a dimensions override that does not fit the index either",
            storage: by_name(VECTOR_BUCKET, SMALL_INDEX),
            dimensions: Some(256),
            embedding_model: EMBEDDING_MODEL,
            forwarded: Some("Query vector contains invalid values or is invalid for this index"),
            forwarded_status: 400,
        },
        BindingRow {
            case: "the model's default dimension on the index that holds it",
            storage: by_name(VECTOR_BUCKET, INDEX),
            dimensions: None,
            embedding_model: EMBEDDING_MODEL,
            forwarded: None,
            forwarded_status: 0,
        },
        BindingRow {
            case: "the same index by its indexArn",
            storage: by_arn(VECTOR_BUCKET, INDEX),
            dimensions: None,
            embedding_model: EMBEDDING_MODEL,
            forwarded: None,
            forwarded_status: 0,
        },
        BindingRow {
            case: "a dimensions override that fits the 512-value index",
            storage: by_name(VECTOR_BUCKET, SMALL_INDEX),
            dimensions: Some(SMALL_DIMENSION),
            embedding_model: EMBEDDING_MODEL,
            forwarded: None,
            forwarded_status: 0,
        },
        BindingRow {
            case: "a 1536-value model on the index that holds 1536",
            storage: by_name(VECTOR_BUCKET, WIDE_INDEX),
            dimensions: None,
            embedding_model:
                "arn:aws:bedrock:eu-west-1::foundation-model/amazon.titan-embed-text-v1",
            forwarded: None,
            forwarded_status: 0,
        },
        BindingRow {
            case: "a dimensions override that fits the 1536-value index",
            storage: by_name(VECTOR_BUCKET, WIDE_INDEX),
            dimensions: Some(WIDE_DIMENSION),
            embedding_model: EMBEDDING_MODEL,
            forwarded: None,
            forwarded_status: 0,
        },
    ];

    for (position, row) in rows.iter().enumerate() {
        let before = emulator.stored_knowledge_bases();
        let answer = emulator
            .create_knowledge_base(create_request(
                &format!("binding-storage-kb-{position}"),
                row.embedding_model,
                row.dimensions,
                row.storage.clone(),
            ))
            .await;

        let Some(forwarded) = row.forwarded else {
            assert_eq!(
                answer.status, 202,
                "case {}: expected the create to be served, got {answer:?}",
                row.case
            );
            let knowledge_base_id = answer
                .member(&["knowledgeBase", "knowledgeBaseId"])
                .as_str()
                .unwrap_or_else(|| panic!("case {}: a minted knowledgeBaseId", row.case))
                .to_string();
            // The round trip: the binding the create was served with is the one
            // the stored record reports.
            let fetched = emulator.get_knowledge_base(&knowledge_base_id).await;
            assert_eq!(
                fetched.member(&["knowledgeBase", "storageConfiguration"]),
                &row.storage,
                "case {}: expected the stored storage configuration to read back, got {:?}",
                row.case,
                fetched.body
            );
            assert_eq!(
                emulator.stored_knowledge_bases(),
                before + 1,
                "case {}: a served create stores exactly one knowledge base",
                row.case
            );
            continue;
        };

        let message = validation_message(row.case, &answer);
        let expected_opening = format!("{FORWARDED_OPENING} {forwarded} (Service: S3Vectors, Status Code: {}, Request ID: ", row.forwarded_status);
        assert!(
            message.starts_with(&expected_opening),
            "case {}: expected the refusal to carry the vector store's own sentence \
             ({expected_opening:?}), got {message:?}",
            row.case
        );
        assert!(
            message.ends_with(") (SDK Attempt Count: 1)"),
            "case {}: expected the forwarded tail to close with the attempt count, got \
             {message:?}",
            row.case
        );
        assert_eq!(
            emulator.stored_knowledge_bases(),
            before,
            "case {}: a refused create must leave no knowledge base behind",
            row.case
        );
    }
}

/// A refused create is in NO listing, and every served one is: the listing
/// reports exactly the knowledge bases the store holds, in creation order, and
/// pages by `maxResults`.
#[tokio::test]
async fn list_knowledge_bases_reports_exactly_the_knowledge_bases_the_store_holds() {
    let emulator = Emulator::new();
    let (empty, token) = emulator.list_knowledge_bases(None, None).await;
    assert!(
        empty.is_empty() && token.is_none(),
        "case the empty account: expected no knowledge base and no token, got {empty:?}/{token:?}"
    );

    // Three refused creates, each failing a different pre-condition, and three
    // served ones around them.
    let refused = [
        create_request(
            "binding-listing-missing-index-kb",
            EMBEDDING_MODEL,
            None,
            by_name(VECTOR_BUCKET, "no-such-index"),
        ),
        create_request(
            "binding-listing-mismatched-kb",
            EMBEDDING_MODEL,
            None,
            by_name(VECTOR_BUCKET, SMALL_INDEX),
        ),
        create_request(
            "binding-listing-text-model-kb",
            "arn:aws:bedrock:eu-west-1::foundation-model/amazon.nova-lite-v1:0",
            None,
            by_name(VECTOR_BUCKET, INDEX),
        ),
    ];
    let mut served: Vec<String> = Vec::new();
    for (position, request) in refused.into_iter().enumerate() {
        let accepted = emulator
            .create_knowledge_base(create_request(
                &format!("binding-listing-kb-{position}"),
                EMBEDDING_MODEL,
                None,
                by_name(VECTOR_BUCKET, INDEX),
            ))
            .await;
        assert_eq!(
            accepted.status, 202,
            "case the listing's setup: expected the create to be served, got {accepted:?}"
        );
        served.push(
            accepted
                .member(&["knowledgeBase", "knowledgeBaseId"])
                .as_str()
                .expect("a minted knowledgeBaseId")
                .to_string(),
        );
        let answer = emulator.create_knowledge_base(request).await;
        validation_message("the listing's refused create", &answer);
    }

    let (listed, token) = emulator.list_knowledge_bases(None, None).await;
    assert_eq!(
        listed, served,
        "case the whole listing: expected exactly the served knowledge bases, in creation \
         order, got {listed:?}"
    );
    assert!(
        token.is_none(),
        "case the whole listing: a page that reported everything hands out no token, got \
         {token:?}"
    );

    // And it pages: `maxResults` shortens the page and the token resumes right
    // after it, so the pages together are the whole listing once.
    let mut paged: Vec<String> = Vec::new();
    let mut next = None;
    loop {
        let (page, token) = emulator.list_knowledge_bases(Some(2), next.as_deref()).await;
        assert!(
            page.len() <= 2,
            "case the paged listing: a page carries at most the maxResults asked for, got \
             {page:?}"
        );
        paged.extend(page);
        next = token;
        if next.is_none() {
            break;
        }
    }
    assert_eq!(
        paged, served,
        "case the paged listing: the pages together are the whole listing once, got {paged:?}"
    );
}

/// The request id in a forwarded refusal names the CALL, so two refusals of the
/// same mis-wired binding do not report the same one.
#[tokio::test]
async fn each_forwarded_refusal_names_its_own_call() {
    let emulator = Emulator::new();
    let request = create_request(
        "binding-request-id-kb",
        EMBEDDING_MODEL,
        None,
        by_name(VECTOR_BUCKET, "no-such-index"),
    );
    let first = emulator.create_knowledge_base(request.clone()).await;
    let second = emulator.create_knowledge_base(request).await;

    let (first, second) = (
        validation_message("the first refusal", &first),
        validation_message("the second refusal", &second),
    );
    let request_id = |message: &str| {
        message
            .split_once("Request ID: ")
            .and_then(|(_, tail)| tail.split_once(')'))
            .map(|(id, _)| id.to_string())
            .unwrap_or_else(|| panic!("a forwarded refusal names a request id: {message:?}"))
    };
    let (first, second) = (request_id(&first), request_id(&second));
    assert_eq!(
        first.len(),
        36,
        "case two refusals: expected an 8-4-4-4-12 request id, got {first:?}"
    );
    assert_ne!(
        first, second,
        "case two refusals: each refusal names its own call, got {first:?} twice"
    );
}
