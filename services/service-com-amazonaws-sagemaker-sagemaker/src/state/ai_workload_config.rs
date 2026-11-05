// SPDX-License-Identifier: CC0-1.0

//! The AI-workload-configuration slice of this service's state: the
//! [`AIWorkloadConfig`] record a benchmark job runs, its bound and the sentences a
//! missing one is refused with.

use super::*;

/// The ARN resource kind an AI workload configuration carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `AIWorkloadConfigArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:ai-workload-config/...`), and named
/// once here so the create and the describe mint and match the same ARN.
pub const AI_WORKLOAD_CONFIG_KIND: &str = "ai-workload-config";

/// See [`MODEL_QUOTA`]. Bounds the AI workload configuration store the same way, and is
/// what `CreateAIWorkloadConfig` answers `ResourceLimitExceeded` past.
pub const AI_WORKLOAD_CONFIG_QUOTA: usize = 1_000;

/// Why a reader could not resolve an AI workload configuration, named once here so every
/// operation that READS one refuses an unknown name with the same sentence: the family's
/// [`ai_entity_not_found`], quoting the identifier the request presented. The delete
/// relays the inner service's rendering of it instead
/// ([`ai_entity_not_found_relayed`]).
pub fn ai_workload_config_not_found(identifier: &str) -> String {
    ai_entity_not_found(AI_WORKLOAD_CONFIG_ENTITY, identifier)
}

/// Why a create that references an AI workload configuration could not resolve it: the
/// family's [`ai_entity_not_found_for_account`], quoting the
/// `AIWorkloadConfigIdentifier` the request presented.
///
/// Named once here so `CreateAIBenchmarkJob` and `CreateAIRecommendationJob` refuse an
/// unresolvable reference with the same sentence.
pub fn ai_workload_config_not_found_for_account(identifier: &str) -> String {
    ai_entity_not_found_for_account(AI_WORKLOAD_CONFIG_ENTITY, identifier)
}

/// An AI workload configuration created by `CreateAIWorkloadConfig`, as
/// `DescribeAIWorkloadConfig` replays it.
///
/// A workload configuration is inert data — a dataset configuration plus a benchmark
/// tool specification — so it carries no status: the service returns it ready, and
/// there is no transition for the clock to owe.
#[derive(Debug, Clone)]
pub struct AIWorkloadConfig {
    pub name: String,
    pub arn: String,
    pub dataset_config: Option<crate::ops::types::AIDatasetConfig>,
    pub workload_configs: Option<crate::ops::types::AIWorkloadConfigs>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another AI workload configuration fits, i.e. the store is below
    /// [`AI_WORKLOAD_CONFIG_QUOTA`]. A second inherent `impl` block, beside this slice's
    /// own types rather than in the one above.
    pub fn ai_workload_configs_below_quota(&self) -> bool {
        self.ai_workload_configs.len() < AI_WORKLOAD_CONFIG_QUOTA
    }

    /// Whether another AI benchmark job fits, i.e. the store is below
    /// [`AI_BENCHMARK_JOB_QUOTA`].
    pub fn ai_benchmark_jobs_below_quota(&self) -> bool {
        self.ai_benchmark_jobs.len() < AI_BENCHMARK_JOB_QUOTA
    }
}
