// SPDX-License-Identifier: CC0-1.0

//! The S3 Vectors index a knowledge base is created against.
//!
//! `CreateKnowledgeBase` documents the vector store as something the caller sets
//! up FIRST ("To create a knowledge base, you must first set up your data
//! sources and configure a supported vector store"), and the emulator resolves
//! the `storageConfiguration` binding in the service that owns that store — so
//! every suite whose knowledge bases bind to an S3 Vectors index creates that
//! index before the knowledge base names it, exactly as a caller does against
//! AWS.
//!
//! The index is seeded in the S3 Vectors store on the SAME engine the emulator
//! under test runs on, which is how the two services share state in this
//! process.

use std::sync::Arc;

use service_com_amazonaws_s3vectors_s3vectors::state::{
    bucket_from_arn, service_state, EncryptionRecord, VectorBucketRecord, VectorIndexRecord,
    DEFAULT_INDEX_MODE, DISTANCE_METRIC_COSINE,
};

/// The number of values the vectors of `amazon.titan-embed-text-v2:0` carry by
/// default, which is the embedding model these suites create knowledge bases
/// with — so an index of this dimension is one the model's vectors fit.
pub const EMBEDDING_DIMENSION: i32 = 1024;

/// The `dataType` of an index these suites create: the one value the modelled
/// `DataType` enum admits
/// (`smithy_query(query="shape",shape="DataType",model_path="services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/com-amazonaws-s3vectors-s3vectors.json")`).
const DATA_TYPE: &str = "float32";

/// Create the vector bucket `vector_bucket_arn` names and the index
/// `index_name` inside it, holding vectors of `dimension` values under the
/// cosine distance metric.
///
/// Idempotent in the bucket: a second index of the same bucket is added beside
/// the first rather than replacing it.
pub fn seed_index(
    engine: &Arc<engine::InMemoryEngine>,
    vector_bucket_arn: &str,
    index_name: &str,
    dimension: i32,
) {
    let bucket_name = bucket_from_arn("vectorBucketArn", vector_bucket_arn)
        .expect("the suite's vectorBucketArn is a vector bucket ARN");
    let state = service_state(&**engine).expect("the S3 Vectors store on this engine");
    let mut store = state.lock();
    let bucket = store
        .buckets
        .entry(bucket_name.clone())
        .or_insert_with(|| VectorBucketRecord {
            name: bucket_name.clone(),
            arn: vector_bucket_arn.to_string(),
            creation_time: std::time::SystemTime::now(),
            encryption: EncryptionRecord::default(),
            default_index_mode: DEFAULT_INDEX_MODE.to_string(),
            policy: None,
            indexes: std::collections::BTreeMap::new(),
        });
    let encryption = bucket.encryption.clone();
    let index_mode = bucket.default_index_mode.clone();
    bucket.indexes.insert(
        index_name.to_string(),
        VectorIndexRecord {
            bucket_name: bucket_name.clone(),
            name: index_name.to_string(),
            arn: format!("{vector_bucket_arn}/index/{index_name}"),
            creation_time: std::time::SystemTime::now(),
            data_type: DATA_TYPE.to_string(),
            dimension,
            distance_metric: DISTANCE_METRIC_COSINE.to_string(),
            non_filterable_metadata_keys: None,
            encryption,
            index_mode,
            vectors: std::collections::BTreeMap::new(),
            tombstones: std::collections::VecDeque::new(),
        },
    );
}

/// Delete the index `index_name` of `vector_bucket_arn`, so a knowledge base
/// bound to it is bound to an index the store no longer holds.
pub fn drop_index(
    engine: &Arc<engine::InMemoryEngine>,
    vector_bucket_arn: &str,
    index_name: &str,
) {
    let bucket_name = bucket_from_arn("vectorBucketArn", vector_bucket_arn)
        .expect("the suite's vectorBucketArn is a vector bucket ARN");
    let state = service_state(&**engine).expect("the S3 Vectors store on this engine");
    let mut store = state.lock();
    let removed = store
        .bucket_mut(&bucket_name)
        .expect("the suite's vector bucket")
        .indexes
        .remove(index_name);
    assert!(
        removed.is_some(),
        "the harness must be able to delete the index {index_name:?}"
    );
}

/// The data of the vector the index `index_name` of `vector_bucket_arn` holds
/// under `key`, or `None` when it holds none — what an `s3vectors` `GetVectors`
/// of that key reports.
pub fn indexed_vector(
    engine: &Arc<engine::InMemoryEngine>,
    vector_bucket_arn: &str,
    index_name: &str,
    key: &str,
) -> Option<Vec<f32>> {
    let bucket_name = bucket_from_arn("vectorBucketArn", vector_bucket_arn)
        .expect("the suite's vectorBucketArn is a vector bucket ARN");
    let state = service_state(&**engine).expect("the S3 Vectors store on this engine");
    let store = state.lock();
    let held = store
        .index(&bucket_name, index_name)
        .ok()
        .and_then(|index| index.vectors.get(key))
        .map(|vector| vector.data.clone());
    held
}

/// The keys of the vectors the index `index_name` of `vector_bucket_arn` holds,
/// in key order — what an `s3vectors` `ListVectors` of that index reports.
pub fn index_vector_keys(
    engine: &Arc<engine::InMemoryEngine>,
    vector_bucket_arn: &str,
    index_name: &str,
) -> Vec<String> {
    let bucket_name = bucket_from_arn("vectorBucketArn", vector_bucket_arn)
        .expect("the suite's vectorBucketArn is a vector bucket ARN");
    let state = service_state(&**engine).expect("the S3 Vectors store on this engine");
    let store = state.lock();
    store
        .index(&bucket_name, index_name)
        .map(|index| index.vectors.keys().cloned().collect())
        .unwrap_or_default()
}
