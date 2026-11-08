// SPDX-License-Identifier: CC0-1.0

//! The vector store a knowledge base is bound to, and the embedding model that
//! writes into it.
//!
//! `CreateKnowledgeBase` documents the vector store as something the caller sets
//! up first ("To create a knowledge base, you must first set up your data
//! sources and configure a supported vector store"), so the binding a request
//! carries is resolved against the service that OWNS that store rather than
//! taken on trust: a knowledge base is only as created as the index it writes
//! into. Two things decide it, and both are checked before any record is
//! written:
//!
//! * the index named in `storageConfiguration` must be an index of the named
//!   vector bucket, and
//! * the vectors the embedding model produces must fit that index, which means
//!   the model's effective dimension and the index's dimension must agree.
//!
//! Neither rule is expressed in this service's model, so each refusal is the
//! sentence the vector store answers for it, carried through with the tail AWS
//! appends to a message it forwards from an inner service
//! ([`forwarded_storage_configuration_message`]). The not-found sentence is read
//! off that service's own constant; the probe query's is recorded here, for the
//! reason [`QUERY_VECTOR_REFUSED`] states.
//!
//! The embedding model is checked against the published foundation-model
//! catalog: `vectorKnowledgeBaseConfiguration.embeddingModelArn` is "the model
//! used to create vector embeddings"
//! (`smithy_query(query="shape",shape="VectorKnowledgeBaseConfiguration")`), so
//! a published model whose output is not an embedding cannot be it.
//!
//! LOCK ORDER: every function here that reaches another service's store takes
//! that store's lock and holds no other while it does, and hands back an owned
//! value rather than a guard — so each must be called with NO guard of this
//! service's store held. Keeping the cross-service access one-directional and
//! guard-free is what makes a lock cycle between the stores unreachable.

use super::*;

use crate::ops::types::{S3VectorsConfiguration, VectorKnowledgeBaseConfiguration};

/// The `KnowledgeBaseStorageType` of a knowledge base whose vectors live in
/// Amazon S3 Vectors: the one kind this module resolves
/// (`smithy_query(query="shape",shape="KnowledgeBaseStorageType")`).
pub const S3_VECTORS_STORAGE_TYPE: &str = "S3_VECTORS";

/// The published output modality of a model that produces vector embeddings, as
/// the foundation-model catalog names it.
const EMBEDDING_OUTPUT_MODALITY: &str = "EMBEDDING";

/// The `foundation-model` resource prefix an `embeddingModelArn` carries when it
/// names one of the models the catalog publishes, as the modelled
/// `BedrockEmbeddingModelArn` pattern spells the resource part.
const FOUNDATION_MODEL_RESOURCE: &str = "foundation-model/";

/// How many attempts the tail of a forwarded refusal reports.
///
/// The call behind the refusal is made once — the vector store is resolved in
/// this process, so nothing retries it — and the count is a fact about that call
/// rather than a value to pick.
const SDK_ATTEMPT_COUNT: u32 = 1;

/// The HTTP status S3 Vectors answers for an index it does not hold.
const INDEX_NOT_FOUND_STATUS: u16 = 404;

/// The HTTP status S3 Vectors answers for a query vector an index cannot take.
const QUERY_VECTOR_REFUSED_STATUS: u16 = 400;

/// The name S3 Vectors is reported under in the tail of a forwarded refusal, as
/// its SDK client names the service.
const VECTOR_STORE_SERVICE: &str = "S3Vectors";

/// The sentence S3 Vectors answers the create-time probe query with when the
/// vector does not fit the index it is run against.
///
/// RECORDED, not published: the binding is validated with a probe query against
/// the bound index, and this is the sentence that query comes back refused with,
/// so a client branching on the forwarded text sees what AWS forwards. It is
/// held here rather than read off the S3 Vectors crate because that crate words
/// its own `QueryVectors` refusal differently
/// (`::service_com_amazonaws_s3vectors_s3vectors::state::vector_data_defect`
/// names the length the index holds): which sentence that operation should
/// answer is its own question, and nothing here changes it.
const QUERY_VECTOR_REFUSED: &str =
    "Query vector contains invalid values or is invalid for this index";

/// The default dimension of the vector each published embedding model produces,
/// by the model id the `embeddingModelArn` names.
///
/// Source: each model's own published output-vector length in the Amazon Bedrock
/// model documentation — Titan Text Embeddings V2 produces 1024 values by
/// default (and takes 512 or 256 as an override), Titan Embeddings G1 — Text
/// 1536, Titan Multimodal Embeddings G1 1024, and the Cohere Embed v3 models
/// 1024. A model that is not listed has no default here, so no dimension is
/// assumed for it and the binding is not refused on one.
pub const EMBEDDING_MODEL_DIMENSIONS: [(&str, i32); 8] = [
    ("amazon.titan-embed-text-v2:0", 1024),
    ("amazon.titan-embed-text-v1", 1536),
    ("amazon.titan-embed-text-v1:2:8k", 1536),
    ("amazon.titan-embed-g1-text-02", 1536),
    ("amazon.titan-embed-image-v1", 1024),
    ("amazon.titan-embed-image-v1:0", 1024),
    ("cohere.embed-english-v3", 1024),
    ("cohere.embed-multilingual-v3", 1024),
];

/// The foundation-model id `arn` names, or `None` when it names no published
/// model.
///
/// `BedrockEmbeddingModelArn` admits a bare model id as well as a full ARN, and
/// an ARN of another kind of model resource (a custom model, an inference
/// profile, a SageMaker endpoint) names no published model — so only the two
/// forms that do are read here.
pub fn embedding_model_id(arn: &str) -> Option<&str> {
    if arn.starts_with("arn:") {
        // A full ARN: everything after the account field is the resource, and
        // only a `foundation-model/<id>` resource names a published model. The
        // id itself may carry colons (`amazon.titan-embed-text-v2:0`), so the
        // resource is taken from the ARN's fields rather than by splitting on
        // the last colon.
        let resource = arn.splitn(6, ':').nth(5)?;
        return resource.strip_prefix(FOUNDATION_MODEL_RESOURCE);
    }
    // A bare model id, which is what the pattern admits beside an ARN.
    Some(arn)
}

/// Whether `configuration` names an embedding model that the published catalog
/// says is NOT one, answering the ARN the request carried when it does.
///
/// The catalog is the service's own published data, so the rule is read off it
/// rather than from a list kept here: a model the catalog publishes whose output
/// modality is not an embedding cannot create the knowledge base's vectors. A
/// model the catalog does not publish is not judged — nothing published says
/// what it produces — so it is not refused here.
pub fn unsupported_embedding_model(
    configuration: &VectorKnowledgeBaseConfiguration,
) -> Option<&str> {
    let arn = configuration.embedding_model_arn.as_str();
    let model_id = embedding_model_id(arn)?;
    let model =
        ::service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::state::catalogued_model(
            model_id,
        )?;
    (!model
        .output_modalities
        .contains(&EMBEDDING_OUTPUT_MODALITY))
    .then_some(arn)
}

/// The `ValidationException` message a knowledge-base operation answers when the
/// `embeddingModelArn` it was given is not an embedding model: the ARN the
/// request carried fills the one variable part.
pub fn unsupported_embedding_model_message(embedding_model_arn: &str) -> String {
    format!("The specified embedding model {embedding_model_arn} is not supported.")
}

/// The number of values the vectors of this configuration's embedding model
/// carry: the `dimensions` override when the request declares one, else the
/// model's own default.
///
/// `bedrockEmbeddingModelConfiguration.dimensions` is "the dimensions details
/// for the vector configuration used on the Bedrock embeddings model"
/// (`smithy_query(query="shape",shape="BedrockEmbeddingModelConfiguration")`),
/// so a declared value is what the model is run with and the default applies
/// only where none is declared. `None` when neither is known, which is what
/// keeps a model nothing publishes a dimension for from being refused on one.
pub fn effective_embedding_dimension(
    configuration: &VectorKnowledgeBaseConfiguration,
) -> Option<i32> {
    if let Some(declared) = configuration
        .embedding_model_configuration
        .as_ref()
        .and_then(|model| model.bedrock_embedding_model_configuration.as_ref())
        .and_then(|bedrock| bedrock.dimensions)
    {
        return Some(declared);
    }
    let model_id = embedding_model_id(&configuration.embedding_model_arn)?;
    EMBEDDING_MODEL_DIMENSIONS
        .iter()
        .find(|(id, _)| *id == model_id)
        .map(|(_, dimension)| *dimension)
}

/// The vector bucket and index a knowledge base's `storageConfiguration` binds
/// it to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct S3VectorsBinding {
    /// The vector bucket's name, as the S3 Vectors store keys it.
    pub vector_bucket: String,
    /// The index's name within that bucket.
    pub index: String,
}

/// The binding `storage_configuration` names, or `None` when it names no S3
/// Vectors index at all.
///
/// `S3VectorsConfiguration` carries the index either as an `indexArn` — which
/// names the bucket too — or as an `indexName` beside the `vectorBucketArn`
/// (`smithy_query(query="shape",shape="S3VectorsConfiguration")`), so both forms
/// resolve to the same pair.
pub fn s3_vectors_binding(
    storage_configuration: Option<&StorageConfiguration>,
) -> Option<S3VectorsBinding> {
    let configuration = storage_configuration
        .filter(|storage| storage.r#type == S3_VECTORS_STORAGE_TYPE)?
        .s3_vectors_configuration
        .as_ref()?;
    binding_of(configuration)
}

/// The pair one `s3VectorsConfiguration` names, by whichever of its two forms it
/// carries.
fn binding_of(configuration: &S3VectorsConfiguration) -> Option<S3VectorsBinding> {
    use ::service_com_amazonaws_s3vectors_s3vectors::state as vectors;

    if let Some(index_arn) = configuration.index_arn.as_deref() {
        if let Ok((vector_bucket, index)) = vectors::index_from_arn("indexArn", index_arn) {
            return Some(S3VectorsBinding {
                vector_bucket,
                index,
            });
        }
    }
    let index = configuration.index_name.clone()?;
    let vector_bucket =
        vectors::bucket_from_arn("vectorBucketArn", configuration.vector_bucket_arn.as_deref()?)
            .ok()?;
    Some(S3VectorsBinding {
        vector_bucket,
        index,
    })
}

/// Why the S3 Vectors store cannot serve the binding a knowledge base names.
/// Each is a refusal the vector store itself answers, which this service carries
/// through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorBindingProblem {
    /// The named index is not an index of the named vector bucket.
    IndexNotFound,
    /// The index is there, but the embedding model's vectors do not fit it:
    /// the probe query the binding is validated with is refused.
    IndexCannotTakeTheVector,
}

impl VectorBindingProblem {
    /// The forwarded refusal's message: the vector store's own sentence, with
    /// the tail AWS appends to a message it carries through from another
    /// service.
    pub fn message(&self) -> String {
        match self {
            Self::IndexNotFound => forwarded_storage_configuration_message(
                ::service_com_amazonaws_s3vectors_s3vectors::state::INDEX_NOT_FOUND,
                INDEX_NOT_FOUND_STATUS,
            ),
            Self::IndexCannotTakeTheVector => forwarded_storage_configuration_message(
                QUERY_VECTOR_REFUSED,
                QUERY_VECTOR_REFUSED_STATUS,
            ),
        }
    }
}

/// The `ValidationException` message a knowledge-base operation answers when the
/// vector store refuses the storage configuration it was given: this service's
/// own opening, the sentence the vector store answered, and the tail that names
/// the call behind it.
///
/// The request id is drawn afresh per call — it is what correlates one answer
/// with the call behind it — so it is derived from nothing in the request.
pub fn forwarded_storage_configuration_message(inner: &str, status: u16) -> String {
    format!(
        "The knowledge base storage configuration provided is invalid... {inner} \
         (Service: {VECTOR_STORE_SERVICE}, Status Code: {status}, Request ID: {}) \
         (SDK Attempt Count: {SDK_ATTEMPT_COUNT})",
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// Why the vector store cannot serve the knowledge base `knowledge_base_configuration`
/// and `storage_configuration` describe, resolved in the S3 Vectors store
/// itself. `None` when it can, and when the request binds no S3 Vectors index.
///
/// LOCK ORDER: as the module doc says — call with NO guard of this service's
/// store held.
pub fn s3_vectors_binding_problem<Eng: Send + Sync + 'static>(
    engine: &Eng,
    knowledge_base_configuration: &KnowledgeBaseConfiguration,
    storage_configuration: Option<&StorageConfiguration>,
) -> Option<VectorBindingProblem> {
    let binding = s3_vectors_binding(storage_configuration)?;
    let state = ::service_com_amazonaws_s3vectors_s3vectors::state::service_state(engine)?;
    let dimension = {
        let store = state.lock();
        match store.index(&binding.vector_bucket, &binding.index) {
            Ok(index) => index.dimension,
            Err(_) => return Some(VectorBindingProblem::IndexNotFound),
        }
    };

    // The index is there, so the binding is validated the way the service
    // validates it: with a probe query, which the index refuses unless the
    // vectors the embedding model produces have exactly as many values as it
    // holds.
    let effective = knowledge_base_configuration
        .vector_knowledge_base_configuration
        .as_ref()
        .and_then(effective_embedding_dimension)?;
    (effective != dimension).then_some(VectorBindingProblem::IndexCannotTakeTheVector)
}

/// How many values the vectors of the index `binding` names carry, or `None`
/// when the S3 Vectors store holds no such index.
///
/// LOCK ORDER: as the module doc says.
pub fn bound_index_dimension<Eng: Send + Sync + 'static>(
    engine: &Eng,
    binding: &S3VectorsBinding,
) -> Option<i32> {
    let state = ::service_com_amazonaws_s3vectors_s3vectors::state::service_state(engine)?;
    let store = state.lock();
    store
        .index(&binding.vector_bucket, &binding.index)
        .ok()
        .map(|index| index.dimension)
}
